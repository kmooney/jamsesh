//! Turns one Jev request/response into readable, indented lines (no JSON punctuation) for
//! the brain panel, and keeps Jev's probabilities over time for its confidence charts.

use std::collections::VecDeque;

use serde_json::Value;

/// One request to Jev and what came back.
#[derive(Clone, Debug)]
pub struct Exchange {
    /// E.g. "bar 5.2 · notes".
    pub title: String,
    pub latency: Option<f64>,
    pub state: Value,
    /// Question name and the question as sent.
    pub questions: Vec<(String, Value)>,
    /// Jev's `answers` object.
    pub answers: Option<Value>,
    pub error: Option<String>,
    /// What the robot did with the answers.
    pub decision: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Title,
    Section,
    /// A key with an optional value.
    Field,
    /// Free text, e.g. a question's instructions.
    Text,
    /// A probability bar; `true` marks the option that was chosen.
    Prob(f64, bool),
    Warn,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub indent: u8,
    pub key: String,
    pub value: String,
    pub kind: Kind,
}

fn line(indent: u8, kind: Kind, key: impl Into<String>, value: impl Into<String>) -> Line {
    Line { indent, key: key.into(), value: value.into(), kind }
}

pub fn lines(e: &Exchange) -> Vec<Line> {
    let mut out = Vec::new();
    let latency = e.latency.map(|l| format!("answered in {l:.2} s")).unwrap_or_default();
    out.push(line(0, Kind::Title, &e.title, latency));
    if let Some(err) = &e.error {
        out.push(line(0, Kind::Warn, "no answer", err));
    }

    out.push(line(0, Kind::Section, "RESULT", ""));
    // The default font has no middle dot.
    out.push(line(1, Kind::Text, e.decision.replace('·', "|"), ""));

    if let Some(answers) = &e.answers {
        out.push(line(0, Kind::Section, "RESPONSE", ""));
        for (name, _) in &e.questions {
            if let Some(a) = answers.get(name) {
                answer_lines(&mut out, name, a);
            }
        }
    }

    out.push(line(0, Kind::Section, "REQUEST: STATE", ""));
    value_lines(&mut out, 1, &e.state);

    out.push(line(0, Kind::Section, "REQUEST: QUESTIONS", ""));
    for (name, q) in &e.questions {
        let kind = q.get("type").and_then(Value::as_str).unwrap_or("?");
        out.push(line(1, Kind::Field, pretty(name), format!("({kind})")));
        if let Some(text) = q.get("instructions").and_then(Value::as_str) {
            out.push(line(2, Kind::Text, text, ""));
        }
        match q.get("criteria") {
            Some(Value::Object(m)) => {
                for (k, v) in m {
                    out.push(line(2, Kind::Field, pretty_option(k), scalar(v)));
                }
            }
            Some(Value::Array(levels)) => {
                for (i, v) in levels.iter().enumerate() {
                    out.push(line(2, Kind::Field, format!("level {i}"), scalar(v)));
                }
            }
            _ => {}
        }
    }
    out
}

fn answer_lines(out: &mut Vec<Line>, name: &str, a: &Value) {
    let confidence = a.get("confidence").and_then(Value::as_f64).map(|c| format!(", confidence {c:.2}")).unwrap_or_default();
    match a.get("type").and_then(Value::as_str) {
        Some("noul") => {
            let p = a.get("noul").and_then(Value::as_f64).unwrap_or(0.0);
            out.push(line(1, Kind::Field, pretty(name), format!("yes {:.0}%", p * 100.0)));
            out.push(line(2, Kind::Prob(p, p >= 0.5), "yes", ""));
        }
        Some("score") => {
            let score = a.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            out.push(line(1, Kind::Field, pretty(name), format!("score {score:.2}{confidence}")));
            let legend = a.get("legend");
            let nearest = score.round() as i64;
            // In level order, so rows stay put from one answer to the next.
            let mut levels = sorted_probs(a);
            levels.sort_by_key(|(level, _)| level.parse::<i64>().unwrap_or(i64::MAX));
            for (level, p) in levels {
                let text = legend.and_then(|l| l.get(&level)).map(scalar).unwrap_or_default();
                let chosen = level.parse::<i64>().ok() == Some(nearest);
                out.push(line(2, Kind::Prob(p, chosen), format!("{level}  {text}"), ""));
            }
        }
        _ => {
            let choice = a.get("choice").and_then(Value::as_str).unwrap_or("?");
            out.push(line(1, Kind::Field, pretty(name), format!("{}{confidence}", pretty_option(choice))));
            let probs = sorted_probs(a);
            let shown = 8;
            // The likeliest few, listed alphabetically so rows stay put from one answer to the next.
            let mut top: Vec<_> = probs.iter().take(shown).collect();
            top.sort_by_key(|(option, _)| pretty_option(option).to_lowercase());
            for (option, p) in top {
                out.push(line(2, Kind::Prob(*p, option == choice), pretty_option(option), ""));
            }
            if probs.len() > shown {
                let rest: f64 = probs[shown..].iter().map(|(_, p)| p).sum();
                out.push(line(2, Kind::Text, format!("+{} more options, {:.0}% together", probs.len() - shown, rest * 100.0), ""));
            }
        }
    }
}

/// Requests kept per chart.
pub const POINTS: usize = 48;
/// Distinct line colors; the panel supplies the actual colors.
pub const PALETTE: usize = 10;
/// Lines drawn per chart, most likely options first.
const MAX_SERIES: usize = 8;

/// Jev's probabilities over time, one chart per question, since the robot's part last changed.
#[derive(Default)]
pub struct History {
    pub charts: Vec<Chart>,
    /// Bumped on every change, so the panel knows when to redraw.
    pub version: u64,
}

pub struct Chart {
    pub name: String,
    /// Each answer's probabilities by option label, oldest first.
    pub points: VecDeque<Vec<(String, f64)>>,
    /// Which option holds each palette color, so a line keeps its color while it's shown.
    slots: [Option<String>; PALETTE],
}

/// One line on a chart.
pub struct Series {
    pub label: String,
    pub color: usize,
    /// Per point; None where the option wasn't offered.
    pub values: Vec<Option<f64>>,
}

impl History {
    pub fn reset(&mut self) {
        self.charts.clear();
        self.version += 1;
    }

    pub fn add(&mut self, e: &Exchange) {
        let Some(answers) = &e.answers else { return };
        for (name, _) in &e.questions {
            let Some(a) = answers.get(name) else { continue };
            let kind = a.get("type").and_then(Value::as_str);
            let probs: Vec<(String, f64)> = match kind {
                // A yes/no answer is one line: the chance of yes.
                Some("noul") => a.get("noul").and_then(Value::as_f64).map(|p| vec![("yes".to_string(), p)]).unwrap_or_default(),
                Some("score") => sorted_probs(a).into_iter().map(|(k, p)| (format!("level {k}"), p)).collect(),
                _ => sorted_probs(a).into_iter().map(|(k, p)| (pretty_option(&k), p)).collect(),
            };
            if probs.is_empty() {
                continue;
            }
            // Notes/chords modes ask one question per beat; they share a chart, beat after beat.
            let group = match name.strip_prefix("beat_") {
                Some(n) if n.parse::<u32>().is_ok() => "each beat".to_string(),
                _ => pretty(name),
            };
            let i = match self.charts.iter().position(|c| c.name == group) {
                Some(i) => i,
                None => {
                    self.charts.push(Chart { name: group, points: VecDeque::new(), slots: Default::default() });
                    self.charts.len() - 1
                }
            };
            let points = &mut self.charts[i].points;
            points.push_back(probs);
            while points.len() > POINTS {
                points.pop_front();
            }
        }
        self.version += 1;
    }
}

impl Chart {
    /// The options worth drawing (ever above 5% in view, at most MAX_SERIES by peak), in
    /// alphabetical order, and how many others were left out.
    pub fn series(&mut self) -> (Vec<Series>, usize) {
        let mut peaks: Vec<(String, f64)> = Vec::new();
        for point in &self.points {
            for (label, p) in point {
                match peaks.iter_mut().find(|(l, _)| l == label) {
                    Some(peak) => peak.1 = peak.1.max(*p),
                    None => peaks.push((label.clone(), *p)),
                }
            }
        }
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let hidden = peaks.len().saturating_sub(MAX_SERIES) + peaks.iter().take(MAX_SERIES).filter(|(_, p)| *p < 0.05).count();
        let mut shown: Vec<String> = peaks.into_iter().take(MAX_SERIES).filter(|(_, p)| *p >= 0.05).map(|(l, _)| l).collect();
        // Alphabetical, so the legend doesn't reshuffle as probabilities move.
        shown.sort_by_key(|l| l.to_lowercase());

        // Free the colors of lines no longer shown, then give new lines a free one.
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|l| !shown.contains(l)) {
                *slot = None;
            }
        }
        let series = shown
            .into_iter()
            .map(|label| {
                let color = match self.slots.iter().position(|s| s.as_ref() == Some(&label)) {
                    Some(c) => c,
                    None => {
                        let c = self.slots.iter().position(Option::is_none).unwrap_or(0);
                        self.slots[c] = Some(label.clone());
                        c
                    }
                };
                let values = self.points.iter().map(|pt| pt.iter().find(|(l, _)| *l == label).map(|(_, p)| *p)).collect();
                Series { label, color, values }
            })
            .collect();
        (series, hidden)
    }
}

fn sorted_probs(a: &Value) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = match a.get("probabilities") {
        Some(Value::Object(m)) => m.iter().filter_map(|(k, p)| p.as_f64().map(|p| (k.clone(), p))).collect(),
        _ => Vec::new(),
    };
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

/// Objects become "key: value" lines, nested objects indent, lists of plain values join
/// with commas, lists of objects are numbered.
fn value_lines(out: &mut Vec<Line>, indent: u8, v: &Value) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                field(out, indent, &pretty(k), val);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                field(out, indent, &format!("{}.", i + 1), item);
            }
        }
        other => out.push(line(indent, Kind::Text, scalar(other), "")),
    }
}

fn field(out: &mut Vec<Line>, indent: u8, key: &str, val: &Value) {
    match val {
        Value::Object(m) if m.is_empty() => out.push(line(indent, Kind::Field, key, "none")),
        Value::Array(a) if a.is_empty() => out.push(line(indent, Kind::Field, key, "none")),
        Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()) => {
            out.push(line(indent, Kind::Field, key, a.iter().map(scalar).collect::<Vec<_>>().join(", ")));
        }
        // Small flat objects read best on one line: "chord: Am7 | notes: B4 | playing: chords".
        Value::Object(m) if m.len() <= 6 && m.values().all(is_flat) => {
            let parts: Vec<String> = m
                .iter()
                .filter(|(_, v)| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                .map(|(k, v)| match v {
                    Value::Array(a) => format!("{}: {}", pretty(k), a.iter().map(scalar).collect::<Vec<_>>().join(" ")),
                    v => format!("{}: {}", pretty(k), scalar(v)),
                })
                .collect();
            out.push(line(indent, Kind::Field, key, if parts.is_empty() { "nothing".into() } else { parts.join("  |  ") }));
        }
        Value::Object(_) | Value::Array(_) => {
            out.push(line(indent, Kind::Field, key, ""));
            value_lines(out, indent + 1, val);
        }
        other => out.push(line(indent, Kind::Field, key, scalar(other))),
    }
}

/// A plain value, or a list of plain values.
fn is_flat(v: &Value) -> bool {
    match v {
        Value::Object(_) => false,
        Value::Array(a) => a.iter().all(|x| !x.is_object() && !x.is_array()),
        _ => true,
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "none".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
            Some(f) => format!("{f:.2}"),
            None => n.to_string(),
        },
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        other => other.to_string(),
    }
}

fn pretty(key: &str) -> String {
    key.replace('_', " ")
}

/// "note_E4" -> "E4 (note)", "chord_Am7" -> "Am7 (chord)", "root_fifth" -> "root fifth".
fn pretty_option(key: &str) -> String {
    for (prefix, kind) in [("note_", "note"), ("chord_", "chord")] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return format!("{rest} ({kind})");
        }
    }
    pretty(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_without_json_punctuation() {
        let e = Exchange {
            title: "bar 5.2 · notes".into(),
            latency: Some(0.18),
            state: json!({
                "key": "C major",
                "timeline_oldest_first": [
                    { "human": { "chord": "Am7", "notes": ["B4"] }, "you": { "played": "notes ~", "notes": [] } }
                ],
            }),
            questions: vec![(
                "beat_1".into(),
                json!({ "type": "choice", "instructions": "Pick one", "criteria": { "note_E4": "E4", "hold": "keep" } }),
            )],
            answers: Some(json!({
                "beat_1": { "type": "choice", "choice": "hold", "confidence": 0.3,
                            "probabilities": { "hold": 0.6, "note_E4": 0.4 } }
            })),
            error: None,
            decision: "notes ~".into(),
        };
        let ls = lines(&e);
        for l in &ls {
            for bad in ['{', '}', '[', ']', '"'] {
                assert!(!l.key.contains(bad) && !l.value.contains(bad), "punctuation in {l:?}");
            }
        }
        let text: Vec<String> = ls.iter().map(|l| format!("{}{} {}", "  ".repeat(l.indent as usize), l.key, l.value)).collect();
        let joined = text.join("\n");
        assert!(joined.contains("key C major"));
        assert!(joined.contains("timeline oldest first"));
        assert!(joined.contains("chord: Am7"));
        assert!(joined.contains("notes: B4"));
        assert!(joined.contains("E4 (note)"));
        assert!(ls.iter().any(|l| l.kind == Kind::Prob(0.6, true) && l.key == "hold"));
    }

    fn answered(probs: &[(&str, f64)]) -> Exchange {
        let p: serde_json::Map<String, Value> = probs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        Exchange {
            title: String::new(),
            latency: None,
            state: json!({}),
            questions: vec![("beat_1".into(), json!({})), ("energy".into(), json!({}))],
            answers: Some(json!({
                "beat_1": { "type": "choice", "choice": probs[0].0, "probabilities": p },
                "energy": { "type": "score", "score": 1.2, "probabilities": { "0": 0.2, "1": 0.8 } },
            })),
            error: None,
            decision: String::new(),
        }
    }

    #[test]
    fn history_keeps_one_line_per_option_with_stable_colors() {
        let mut h = History::default();
        h.add(&answered(&[("chord_Am7", 0.7), ("hold", 0.3)]));
        h.add(&answered(&[("chord_Fmaj7", 0.6), ("chord_Am7", 0.4)]));
        assert_eq!(h.charts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["each beat", "energy"]);

        let (series, hidden) = h.charts[0].series();
        assert_eq!(hidden, 0);
        let am = series.iter().find(|s| s.label == "Am7 (chord)").unwrap();
        assert_eq!(am.values, [Some(0.7), Some(0.4)]);
        let hold = series.iter().find(|s| s.label == "hold").unwrap();
        assert_eq!(hold.values, [Some(0.3), None], "not offered the second time");
        let am_color = am.color;

        // A new leader doesn't take an existing line's color.
        h.add(&answered(&[("chord_G7", 0.9), ("chord_Am7", 0.1)]));
        let (series, _) = h.charts[0].series();
        assert_eq!(series.iter().find(|s| s.label == "Am7 (chord)").unwrap().color, am_color);
        let colors: std::collections::HashSet<_> = series.iter().map(|s| s.color).collect();
        assert_eq!(colors.len(), series.len());

        let labels: Vec<String> = series.iter().map(|s| s.label.clone()).collect();
        assert_eq!(labels, ["Am7 (chord)", "Fmaj7 (chord)", "G7 (chord)", "hold"], "alphabetical");
        let levels: Vec<String> = h.charts[1].series().0.into_iter().map(|s| s.label).collect();
        assert_eq!(levels, ["level 0", "level 1"]);
        h.reset();
        assert!(h.charts.is_empty());
    }
}
