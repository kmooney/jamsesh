//! Turns one Jev request/response into readable, indented lines (no JSON punctuation) for
//! the brain panel.

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
    out.push(line(1, Kind::Text, &e.decision, ""));

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
            for (level, p) in sorted_probs(a) {
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
            for (option, p) in probs.iter().take(shown) {
                out.push(line(2, Kind::Prob(*p, option == choice), pretty_option(option), ""));
            }
            if probs.len() > shown {
                let rest: f64 = probs[shown..].iter().map(|(_, p)| p).sum();
                out.push(line(2, Kind::Text, format!("+{} more options, {:.0}% together", probs.len() - shown, rest * 100.0), ""));
            }
        }
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
}
