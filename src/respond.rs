//! Responding to the human. Jev is asked what the human is doing (how intense, whether they're
//! leaving space, where they're heading); code turns that into how the robot responds: fill
//! their gaps, lay back while they're busy, carry the music when they're silent, and follow
//! their intensity. Jev's own choices (role, chord, beat) are then weighted by that response.
//!
//! Measured before this existed: Jev's "what should you play" answers moved more with the
//! robot's own recent playing than with the human's, and barely with the human's energy.

use serde_json::{Value, json};

use crate::jev::{Answers, Question};
use crate::listener::{Playing, Summary};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Building,
    Steady,
    WindingDown,
    PhraseEnded,
}

impl Direction {
    const ALL: [Direction; 4] = [Direction::Building, Direction::Steady, Direction::WindingDown, Direction::PhraseEnded];

    fn key(self) -> &'static str {
        match self {
            Direction::Building => "building",
            Direction::Steady => "steady",
            Direction::WindingDown => "winding_down",
            Direction::PhraseEnded => "phrase_ended",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Direction::Building => "Getting busier, louder or higher over the last few steps",
            Direction::Steady => "Keeping about the same level of activity",
            Direction::WindingDown => "Getting sparser, softer or lower",
            Direction::PhraseEnded => "Just finished a phrase and paused, as if waiting for an answer",
        }
    }
}

/// What the human is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Perception {
    /// 0 (silent) ..= 4 (very busy and loud).
    pub intensity: f64,
    /// Probability the human is leaving space for the robot right now.
    pub space: f64,
    pub direction: Direction,
    /// Jev answered (false: estimated by code).
    pub from_jev: bool,
}

pub const INTENSITY_LEVELS: [&str; 5] = [
    "Silent or almost silent",
    "Sparse and soft: a few quiet notes",
    "Moderate: steady playing at medium volume",
    "Busy or loud",
    "Very busy and loud, driving hard",
];

/// How the robot responds, decided by code from the perception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    /// The human left a gap or finished a phrase: fill it.
    Answer,
    /// The human is busy: lay back under them.
    Support,
    /// The human is silent: keep the music going.
    Carry,
    /// Somewhere in between: match them.
    Follow,
}

impl Stance {
    pub fn name(self) -> &'static str {
        match self {
            Stance::Answer => "answer",
            Stance::Support => "support",
            Stance::Carry => "carry",
            Stance::Follow => "follow",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub stance: Stance,
    /// 0.0 ..= 3.0, the robot's activity (see decide::ENERGY_LEVELS).
    pub energy: f64,
}

/// The perception questions, asked alongside the part's own questions in every request.
pub fn questions() -> Vec<(String, Question)> {
    vec![
        (
            "human_intensity".into(),
            Question::Score {
                instructions: "How intensely is the human playing right now (human_this_window_so_far, \
                               the end of human_timeline_oldest_first, and human_activity)? Judge only the human."
                    .into(),
                levels: INTENSITY_LEVELS.iter().map(|s| s.to_string()).collect(),
            },
        ),
        (
            "human_leaving_space".into(),
            Question::Noul {
                instructions: "Is the human leaving space right now that another musician could fill? \
                               Judge only the human."
                    .into(),
                when_true: "A gap: few or no new notes lately, a pause after playing, or long held notes".into(),
                when_false: "Actively playing: new notes keep coming".into(),
            },
        ),
        (
            "human_direction".into(),
            Question::Choice {
                instructions: "Over the last few steps, where is the human's playing heading? Judge only the human.".into(),
                options: Direction::ALL.iter().map(|d| (d.key().to_string(), d.describe().to_string())).collect(),
            },
        ),
    ]
}

/// Facts about the human's activity for Jev's state: notes and loudness bar by bar (the last
/// "bar" may be partial), and how long since their last new note. Per bar even when decisions
/// are per beat, so a held chord doesn't read as a sudden stop.
pub fn activity_json(windows: &[&Summary], step_beats: f64, heard: f64, beats_per_bar: f64) -> Value {
    let per_chunk = ((beats_per_bar / step_beats).round() as usize).max(1);
    let mut chunks: Vec<&[&Summary]> = windows.rchunks(per_chunk).collect();
    chunks.reverse();
    let per_beat: Vec<f64> = chunks
        .iter()
        .map(|c| {
            let notes: usize = c.iter().map(|s| s.onsets.len()).sum();
            (notes as f64 / (c.len() as f64 * step_beats) * 10.0).round() / 10.0
        })
        .collect();
    let loudness: Vec<Value> = chunks
        .iter()
        .map(|c| {
            let v: Vec<f64> = c.iter().filter_map(|s| s.avg_vel).collect();
            if v.is_empty() { Value::Null } else { json!((v.iter().sum::<f64>() / v.len() as f64).round()) }
        })
        .collect();
    json!({
        "new_notes_per_beat_by_bar_oldest_first": per_beat,
        "average_velocity_by_bar_oldest_first": loudness,
        "beats_since_last_new_note": beats_since_last_note(windows, step_beats, heard).map(|b| (b * 2.0).round() / 2.0),
    })
}

/// Beats from the human's last new note to now. `windows` are consecutive and oldest first;
/// the last one is in progress and has been heard for `heard` beats.
fn beats_since_last_note(windows: &[&Summary], step_beats: f64, heard: f64) -> Option<f64> {
    let n = windows.len();
    windows.iter().enumerate().rev().find_map(|(i, s)| {
        let last = s.onsets.iter().map(|h| h.offset).fold(f64::NEG_INFINITY, f64::max);
        last.is_finite().then(|| heard + (n - 1 - i) as f64 * step_beats - last)
    })
}

/// Code's own estimate, for mock mode or when Jev doesn't answer.
pub fn estimate(windows: &[&Summary], step_beats: f64, heard: f64) -> Perception {
    let recent = |k: usize| -> (f64, f64) {
        // (notes per beat, mean velocity) over the last k windows.
        let ws: Vec<_> = windows.iter().rev().take(k).collect();
        let notes: usize = ws.iter().map(|s| s.onsets.len()).sum();
        let vels: Vec<f64> = ws.iter().filter_map(|s| s.avg_vel).collect();
        let vel = if vels.is_empty() { 0.0 } else { vels.iter().sum::<f64>() / vels.len() as f64 };
        (notes as f64 / (ws.len().max(1) as f64 * step_beats), vel)
    };
    let span = ((4.0 / step_beats).ceil() as usize).max(1);
    let (density, vel) = recent(span);
    let held = windows.last().is_some_and(|s| s.playing != Playing::Silent);
    let intensity = if density == 0.0 && !held {
        0.0
    } else {
        (0.8 + density * 1.2 + (vel - 64.0) / 40.0).clamp(0.5, 4.0)
    };
    let since = beats_since_last_note(windows, step_beats, heard);
    let space = match since {
        Some(b) if b >= 2.0 => 0.9,
        Some(b) if b >= 1.0 => 0.6,
        Some(_) => 0.15,
        None => 1.0,
    };
    // Notes per beat in the span before that, to tell which way it's heading.
    let before = {
        let older: Vec<&Summary> = windows.iter().rev().skip(span).take(span).copied().collect();
        let notes: usize = older.iter().map(|s| s.onsets.len()).sum();
        notes as f64 / (older.len().max(1) as f64 * step_beats)
    };
    // A phrase (a few notes in the last three bars) that stopped between a beat and two bars
    // ago is still waiting for an answer.
    let phrase_notes: usize = windows.iter().rev().take(((12.0 / step_beats).ceil() as usize).max(1)).map(|s| s.onsets.len()).sum();
    let direction = if since.is_some_and(|b| (1.0..=8.0).contains(&b)) && phrase_notes >= 3 {
        Direction::PhraseEnded
    } else if density > before * 1.4 + 0.1 {
        Direction::Building
    } else if density < before * 0.6 - 0.05 {
        Direction::WindingDown
    } else {
        Direction::Steady
    };
    Perception { intensity, space, direction, from_jev: false }
}

/// Jev's reading of the human, falling back to code's estimate for anything it didn't answer.
pub fn perceive(answers: Option<&Answers>, estimate: Perception) -> Perception {
    let Some(a) = answers else { return estimate };
    let intensity = a.score("human_intensity");
    let space = a.noul("human_leaving_space");
    let direction = a
        .choice("human_direction")
        .and_then(|(c, _)| Direction::ALL.into_iter().find(|d| d.key() == c));
    Perception {
        intensity: intensity.unwrap_or(estimate.intensity).clamp(0.0, 4.0),
        space: space.unwrap_or(estimate.space).clamp(0.0, 1.0),
        direction: direction.unwrap_or(estimate.direction),
        from_jev: intensity.is_some() || space.is_some() || direction.is_some(),
    }
}

/// Blend a new reading with the previous one, so per-beat decisions don't flip their response
/// every beat: about half-and-half per beat, settling within a bar; bar-long steps take the new
/// reading as is. Direction is categorical and not blended.
pub fn smooth(prior: Option<Perception>, new: Perception, step_beats: f64) -> Perception {
    let Some(old) = prior else { return new };
    let a = (step_beats / 2.0).min(1.0);
    Perception {
        intensity: old.intensity + (new.intensity - old.intensity) * a,
        space: old.space + (new.space - old.space) * a,
        ..new
    }
}

/// How to respond: fill gaps, lay back under a busy human, carry a silent one, follow the rest.
/// Energy tracks the human's intensity and where it's heading.
pub fn plan(p: &Perception) -> Plan {
    let follow = p.intensity * 0.75;
    let lean = match p.direction {
        Direction::Building => 0.4,
        Direction::WindingDown => -0.4,
        _ => 0.0,
    };
    let (stance, energy) = if p.intensity < 0.6 && p.direction != Direction::PhraseEnded {
        (Stance::Carry, 1.2)
    } else if p.direction == Direction::PhraseEnded || p.space > 0.6 {
        // They've stopped, so their intensity now is low; answer with some presence anyway.
        (Stance::Answer, (follow + 0.5).max(1.5))
    } else if p.intensity >= 2.5 {
        (Stance::Support, follow - 0.6)
    } else {
        (Stance::Follow, follow)
    };
    Plan { stance, energy: (energy + lean).clamp(0.0, 3.0) }
}

/// One line for logs and the Jev panel, e.g. "you 2.6 building, space 20% -> answer e2.1".
pub fn describe(p: &Perception, plan: &Plan) -> String {
    format!(
        "you {:.1} {}, space {:.0}%{} -> {} e{:.1}",
        p.intensity,
        p.direction.key().replace('_', " "),
        p.space * 100.0,
        if p.from_jev { "" } else { " (est.)" },
        plan.stance.name(),
        plan.energy
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listener::Listener;

    fn windows(script: &[(f64, u8, u8, f64)], beats: usize) -> Vec<Summary> {
        let mut l = Listener::default();
        for &(start, note, vel, len) in script {
            l.note_on(note, vel, start);
            l.note_off(note, start + len);
        }
        (0..beats).map(|b| l.summarize(b as f64, b as f64 + 1.0)).collect()
    }

    fn estimate_of(script: &[(f64, u8, u8, f64)]) -> Perception {
        let ws = windows(script, 16);
        let refs: Vec<&Summary> = ws.iter().collect();
        estimate(&refs, 1.0, 1.0)
    }

    #[test]
    fn busy_human_gets_support_silent_gets_carried_a_pause_gets_answered() {
        let busy: Vec<_> = (0..32).map(|i| (i as f64 * 0.5, 60 + (i % 5) as u8, 105, 0.4)).collect();
        let p = estimate_of(&busy);
        assert!(p.intensity >= 2.5, "{p:?}");
        assert_eq!(plan(&p).stance, Stance::Support);

        let p = estimate_of(&[]);
        assert_eq!(p.intensity, 0.0);
        assert_eq!(plan(&p).stance, Stance::Carry);

        // A phrase over the first 12 beats, then 4 beats of nothing.
        let phrase: Vec<_> = (0..12).map(|i| (i as f64, 72 + (i % 4) as u8, 80, 0.9)).collect();
        let p = estimate_of(&phrase);
        assert_eq!(p.direction, Direction::PhraseEnded, "{p:?}");
        assert_eq!(plan(&p).stance, Stance::Answer);
    }

    #[test]
    fn per_beat_readings_are_smoothed_bar_readings_are_not() {
        let quiet = Perception { intensity: 0.0, space: 1.0, direction: Direction::Steady, from_jev: true };
        let busy = Perception { intensity: 3.0, space: 0.0, direction: Direction::Building, from_jev: true };
        let beat = smooth(Some(quiet), busy, 1.0);
        assert_eq!((beat.intensity, beat.space, beat.direction), (1.5, 0.5, Direction::Building));
        assert_eq!(smooth(Some(quiet), busy, 4.0), busy);
        assert_eq!(smooth(None, busy, 1.0), busy);
    }

    #[test]
    fn energy_follows_the_human() {
        let soft = Perception { intensity: 1.0, space: 0.2, direction: Direction::Steady, from_jev: true };
        let loud = Perception { intensity: 3.5, ..soft };
        let building = Perception { direction: Direction::Building, ..soft };
        assert!(plan(&loud).energy > plan(&soft).energy);
        assert!(plan(&building).energy > plan(&soft).energy);
    }
}
