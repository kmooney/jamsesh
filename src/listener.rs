//! Records what the human plays (in beats) and summarizes windows of it.

use crate::theory::{Chord, Key, detect_chord};

struct HeldNote {
    note: u8,
    vel: u8,
    start: f64,
    end: Option<f64>,
}

/// A note heard inside a summary window. `offset` is relative to the window start
/// (negative if it was already sounding when the window began).
#[derive(Clone, Debug)]
pub struct Heard {
    pub note: u8,
    pub vel: u8,
    pub offset: f64,
    pub dur: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playing {
    Silent,
    Melody,
    Chords,
    ChordsAndMelody,
}

impl Playing {
    pub fn describe(self) -> &'static str {
        match self {
            Playing::Silent => "silent",
            Playing::Melody => "single-note melody",
            Playing::Chords => "chords",
            Playing::ChordsAndMelody => "chords and melody",
        }
    }

    pub fn is_chordal(self) -> bool {
        matches!(self, Playing::Chords | Playing::ChordsAndMelody)
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    /// Notes that started inside the window.
    pub onsets: Vec<Heard>,
    /// Duration-weighted pitch-class profile of everything sounding.
    pub hist: [f64; 12],
    pub playing: Playing,
    pub chord: Option<Chord>,
    pub avg_vel: Option<f64>,
    pub mean_pitch: Option<f64>,
}

impl Summary {
    pub fn to_json(&self, key: Key) -> serde_json::Value {
        let mut notes: Vec<String> = self.onsets.iter().take(16).map(|h| key.note_name(h.note)).collect();
        if self.onsets.len() > 16 {
            notes.push("...".into());
        }
        let loudness = self.avg_vel.map(|v| match v as u8 {
            0..=50 => "soft",
            51..=90 => "medium",
            _ => "loud",
        });
        let register = self.mean_pitch.map(|p| match p as u8 {
            0..=52 => "low",
            53..=71 => "middle",
            _ => "high",
        });
        serde_json::json!({
            "playing": self.playing.describe(),
            "chord": self.chord.map(|c| c.name(key)),
            "notes": notes,
            "loudness": loudness,
            "register": register,
        })
    }
}

#[derive(Default)]
pub struct Listener {
    notes: Vec<HeldNote>,
    sustain: bool,
    /// Notes released while the sustain pedal was down; they end when it lifts.
    pedalled: Vec<u8>,
}

impl Listener {
    pub fn note_on(&mut self, note: u8, vel: u8, beat: f64) {
        self.end_note(note, beat);
        self.pedalled.retain(|&n| n != note);
        self.notes.push(HeldNote { note, vel, start: beat, end: None });
    }

    pub fn note_off(&mut self, note: u8, beat: f64) {
        if self.sustain {
            self.pedalled.push(note);
        } else {
            self.end_note(note, beat);
        }
    }

    pub fn sustain(&mut self, down: bool, beat: f64) {
        self.sustain = down;
        if !down {
            for note in std::mem::take(&mut self.pedalled) {
                self.end_note(note, beat);
            }
        }
    }

    fn end_note(&mut self, note: u8, beat: f64) {
        for n in self.notes.iter_mut().filter(|n| n.note == note && n.end.is_none()) {
            n.end = Some(beat);
        }
    }

    /// Drop notes that ended before `beat`.
    pub fn prune(&mut self, beat: f64) {
        self.notes.retain(|n| n.end.is_none_or(|e| e >= beat));
    }

    pub fn summarize(&self, from: f64, to: f64) -> Summary {
        let mut sounding = Vec::new();
        for n in &self.notes {
            let end = n.end.unwrap_or(f64::INFINITY);
            if n.start < to && end > from {
                let s = n.start.max(from);
                let e = end.min(to);
                sounding.push(Heard { note: n.note, vel: n.vel, offset: n.start - from, dur: e - s });
            }
        }
        let onsets: Vec<Heard> = sounding.iter().filter(|h| h.offset >= 0.0).cloned().collect();

        let mut hist = [0.0; 12];
        for h in &sounding {
            hist[(h.note % 12) as usize] += h.dur + if h.offset >= 0.0 { 0.25 } else { 0.0 };
        }

        // Maximum number of simultaneously sounding notes.
        let mut edges: Vec<(f64, i32)> = Vec::new();
        for h in &sounding {
            let s = h.offset.max(0.0);
            edges.push((s, 1));
            edges.push((s + h.dur.max(0.01), -1));
        }
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut cur, mut max_poly) = (0, 0);
        for (_, d) in edges {
            cur += d;
            max_poly = max_poly.max(cur);
        }

        // Onsets that aren't struck together with another note read as melody.
        let lone_onsets = onsets
            .iter()
            .filter(|h| !onsets.iter().any(|o| o.note != h.note && (o.offset - h.offset).abs() < 0.12))
            .count();
        let chordal = max_poly >= 3;
        let playing = match (chordal, lone_onsets >= 2 || (!chordal && !onsets.is_empty())) {
            (false, false) if sounding.is_empty() => Playing::Silent,
            (false, _) => Playing::Melody,
            (true, false) => Playing::Chords,
            (true, true) => Playing::ChordsAndMelody,
        };

        let chord = if chordal {
            let bass = sounding.iter().map(|h| h.note).min().map(|n| n % 12);
            detect_chord(&hist, bass)
        } else {
            None
        };

        let avg = |f: fn(&Heard) -> f64| {
            (!onsets.is_empty()).then(|| onsets.iter().map(f).sum::<f64>() / onsets.len() as f64)
        };
        Summary {
            avg_vel: avg(|h| h.vel as f64),
            mean_pitch: avg(|h| h.note as f64),
            onsets,
            hist,
            playing,
            chord,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::Mode;

    #[test]
    fn summarizes_chord_bar() {
        let mut l = Listener::default();
        for n in [45, 52, 55, 60] {
            l.note_on(n, 70, 0.02);
        }
        for n in [45, 52, 55, 60] {
            l.note_off(n, 3.9);
        }
        let s = l.summarize(0.0, 4.0);
        assert_eq!(s.playing, Playing::Chords);
        let key = Key { tonic: 0, mode: Mode::Major };
        assert_eq!(s.chord.map(|c| c.name(key)).as_deref(), Some("Am7"));
    }

    #[test]
    fn sustain_pedal_extends_notes() {
        let mut l = Listener::default();
        l.sustain(true, 0.0);
        l.note_on(60, 70, 0.0);
        l.note_off(60, 0.5);
        l.note_on(64, 70, 1.0);
        l.note_off(64, 1.5);
        l.note_on(67, 70, 2.0);
        l.note_off(67, 2.5);
        l.sustain(false, 3.5);
        let s = l.summarize(0.0, 4.0);
        assert_eq!(s.playing, Playing::ChordsAndMelody);
        assert_eq!(s.onsets.len(), 3);
    }

    #[test]
    fn melody_and_silence() {
        let mut l = Listener::default();
        assert_eq!(l.summarize(0.0, 4.0).playing, Playing::Silent);
        l.note_on(72, 60, 0.0);
        l.note_off(72, 1.0);
        l.note_on(74, 60, 1.0);
        l.note_off(74, 2.0);
        assert_eq!(l.summarize(0.0, 4.0).playing, Playing::Melody);
    }
}
