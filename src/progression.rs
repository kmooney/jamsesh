//! The human's chord progression over the last several bars, beat by beat, and what it
//! predicts: if the changes repeat in a cycle, which chord comes next and when.
//!
//! Decisions are made a step ahead, so without this the robot can only follow a chord change
//! after hearing it. With it, Jev (and code, for parts that simply follow the human's chords)
//! can arrive on the next chord together with the human.

use serde_json::{Value, json};

use crate::theory::{Chord, Key};

/// Changes kept; enough for a few passes of an 8-chord cycle.
const KEEP: usize = 32;
/// Longest cycle looked for, in chord changes.
const MAX_CYCLE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Change {
    chord: Chord,
    /// Beats: the change's first beat, and the end of its last heard beat.
    start: f64,
    end: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    /// The repeating chords, oldest first, ending with the current one.
    pub cycle: Vec<Chord>,
    /// How many times the cycle has been heard, counting the pass in progress (e.g. 1.5).
    pub times: f64,
    pub current: Chord,
    /// How long the current chord lasted last time round, in beats.
    pub usual: f64,
    pub next: Chord,
    /// Beats from the asked-about moment to the expected change (≤ 0: due now).
    pub change_in: f64,
}

#[derive(Default, Clone)]
pub struct Progression {
    changes: Vec<Change>,
}

impl Progression {
    /// One finished beat (`beat` .. `beat` + 1) and the chord heard in it, if any. Beats with
    /// no chord (silence, melody alone) don't break the chord that was sounding, and neither
    /// do melody notes that recolour it (Cmaj7 heard as Cmaj9 stays one change, named as first
    /// heard). A short detour that comes back to the same chord is taken as a misreading and
    /// merged away: any single beat, or up to a bar when the detour holds every note of the
    /// chord around it (F, Dm9, F: the held F with a melody D over it).
    pub fn hear(&mut self, beat: f64, chord: Option<Chord>) {
        let Some(chord) = chord else { return };
        let n = self.changes.len();
        let misread = |detour: &Change, around: Chord| {
            let beats = detour.end - detour.start;
            beats <= 1.0 || (beats <= 4.0 && around.pcs().iter().all(|pc| detour.chord.pcs().contains(pc)))
        };
        if n > 0 && self.changes[n - 1].chord.same_family(chord) {
            self.changes[n - 1].end = beat + 1.0;
        } else if n >= 2 && self.changes[n - 2].chord.same_family(chord) && misread(&self.changes[n - 1], chord) {
            self.changes.pop();
            self.changes[n - 2].end = beat + 1.0;
        } else {
            self.changes.push(Change { chord, start: beat, end: beat + 1.0 });
        }
        if self.changes.len() > KEEP {
            self.changes.remove(0);
        }
    }

    /// The shortest cycle the latest changes repeat, and what it predicts at beat `at`.
    /// Needs the cycle heard twice in a row, the current chord included.
    pub fn predict(&self, at: f64) -> Option<Prediction> {
        let c = &self.changes;
        let n = c.len();
        let period = (2..=MAX_CYCLE).filter(|p| n >= 2 * p).find(|&p| (0..p).all(|k| c[n - 1 - k].chord.same_family(c[n - 1 - k - p].chord)))?;
        // Extend back as far as the pattern holds: `matched` changes equal the one a cycle later.
        let matched = (0..n - period).rev().take_while(|&i| c[i].chord.same_family(c[i + period].chord)).count();
        let previous = c[n - 1 - period];
        let usual = (previous.end - previous.start).round().max(1.0);
        let current = c[n - 1];
        Some(Prediction {
            cycle: c[n - period..].iter().map(|x| x.chord).collect(),
            times: (matched + period) as f64 / period as f64,
            current: current.chord,
            usual,
            next: c[n - period].chord,
            change_in: current.start + usual - at,
        })
    }

    /// For Jev's state: the recent changes with their lengths, and the pattern if there is one.
    pub fn to_json(&self, key: Key, at: f64) -> Value {
        let recent: Vec<String> = self
            .changes
            .iter()
            .rev()
            .take(16)
            .rev()
            .enumerate()
            .map(|(i, x)| {
                let beats = (x.end - x.start).round();
                let last = i + 1 == self.changes.len().min(16);
                if last {
                    format!("{} ({} beats so far)", x.chord.name(key), (at - x.start).round().max(beats))
                } else {
                    format!("{} ({beats} beats)", x.chord.name(key))
                }
            })
            .collect();
        let pattern = match self.predict(at) {
            Some(p) => {
                let cycle: Vec<String> = p.cycle.iter().map(|c| c.name(key)).collect();
                let when = if p.change_in <= 0.5 {
                    format!("the change to {} is due now", p.next.name(key))
                } else {
                    format!("the change to {} is expected in about {} beats", p.next.name(key), p.change_in.round())
                };
                json!({
                    "repeating_cycle": cycle.join(" -> "),
                    "times_heard": (p.times * 10.0).round() / 10.0,
                    "expectation": format!(
                        "{} lasted {} beats last time round; {when}",
                        p.current.name(key),
                        p.usual
                    ),
                })
            }
            None => json!("no repeating cycle yet"),
        };
        json!({ "chord_changes_oldest_first": recent, "pattern": pattern })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{Mode, Quality};

    fn ch(root: u8) -> Chord {
        Chord { root, quality: Quality::Maj7 }
    }

    /// Plays `chords` for `beats` beats each, starting at beat 0.
    fn play(p: &mut Progression, chords: &[u8], beats: u32) -> f64 {
        let mut beat = 0.0;
        for &c in chords {
            for _ in 0..beats {
                p.hear(beat, Some(ch(c)));
                beat += 1.0;
            }
        }
        beat
    }

    #[test]
    fn repeated_cycle_predicts_next_chord_and_timing() {
        let mut p = Progression::default();
        // C Am F G, C Am F G, then 2 beats into C.
        let end = play(&mut p, &[0, 9, 5, 7, 0, 9, 5, 7], 4);
        p.hear(end, Some(ch(0)));
        p.hear(end + 1.0, None);
        p.hear(end + 2.0, Some(ch(0)));
        let pr = p.predict(end + 3.0).expect("cycle");
        assert_eq!(pr.cycle, vec![ch(9), ch(5), ch(7), ch(0)]);
        assert_eq!(pr.next, ch(9));
        assert_eq!(pr.usual, 4.0);
        assert_eq!(pr.change_in, 1.0);
        assert!(pr.times >= 2.0);
        // At the change itself it's due.
        assert!(p.predict(end + 4.0).unwrap().change_in <= 0.0);
    }

    #[test]
    fn melody_notes_recolouring_a_chord_dont_split_it() {
        let mut p = Progression::default();
        let (c, c9) = (ch(0), Chord { root: 0, quality: Quality::Maj9 });
        let am = Chord { root: 9, quality: Quality::Min7 };
        let am9 = Chord { root: 9, quality: Quality::Min9 };
        for (i, chord) in [c9, c, c, c, am, am9, am, am, c, c9, c, c, am9, am, am, am].into_iter().enumerate() {
            p.hear(i as f64, Some(chord));
        }
        let pr = p.predict(16.0).expect("C Am C Am is a cycle");
        assert_eq!(pr.cycle.len(), 2);
        assert!(pr.next.same_family(c));
        assert_eq!(pr.change_in, 0.0);
    }

    #[test]
    fn misreadings_are_merged_away() {
        let (c, f) = (ch(0), ch(5));
        let dm9 = Chord { root: 2, quality: Quality::Min9 };
        let em = Chord { root: 4, quality: Quality::Min };
        // C F C F with F's middle beats misread: two beats as Dm9 (F plus a melody D), or one
        // as Em. Either way F stays one change and the cycle holds.
        for (detour, beats) in [(dm9, 2), (em, 1)] {
            let mut p = Progression::default();
            let mut beat = 0.0;
            for (chord, n) in [(c, 4), (f, 4), (c, 4), (f, 1), (detour, beats), (f, 3 - beats)] {
                for _ in 0..n {
                    p.hear(beat, Some(chord));
                    beat += 1.0;
                }
            }
            let pr = p.predict(beat).expect("C F C F still a cycle");
            assert!(pr.next.same_family(c), "{detour:?}");
            assert_eq!(pr.change_in, 0.0);
        }
        // A real two-beat change that isn't a superset stays.
        let mut p = Progression::default();
        let g = ch(7);
        for (i, chord) in [c, c, g, g, c, c].into_iter().enumerate() {
            p.hear(i as f64, Some(chord));
        }
        assert_eq!(p.changes.len(), 3);
    }

    #[test]
    fn no_prediction_without_repetition() {
        let mut p = Progression::default();
        play(&mut p, &[0, 9, 5, 7, 2], 4);
        assert!(p.predict(20.0).is_none());
        let json = p.to_json(Key { tonic: 0, mode: Mode::Major }, 20.0);
        assert_eq!(json["pattern"], "no repeating cycle yet");
        assert_eq!(json["chord_changes_oldest_first"].as_array().unwrap().len(), 5);
    }
}
