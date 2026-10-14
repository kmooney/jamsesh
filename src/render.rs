//! Turns a decision into timed note events for one bar. Pure code, no Jev.

use crate::decide::{Decision, Role, Slot, notes_range};
use crate::listener::Summary;
use crate::rng::Rng;
use crate::theory::{Chord, Key};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    On(u8, u8),
    Off(u8),
    /// MIDI expression (CC11): how loud sustained notes are, without re-attacking them.
    Expression(u8),
}

/// Expression when nothing is shaping it (the MIDI default).
const FULL_EXPRESSION: u8 = 127;

/// The robot's voice. Remembers sustained pad/drone notes so they can ring across bars.
#[derive(Default)]
pub struct Robot {
    held: Vec<u8>,
    /// Last expression sent (None: never set, so the synth's default).
    expression: Option<u8>,
    /// The octave shift the held pad was voiced for; a change re-voices it rather than
    /// voice-leading back toward the old register.
    pad_octave: i32,
}

pub struct BarInput<'a> {
    pub decision: &'a Decision,
    pub key: Key,
    pub bar_start: f64,
    pub beats: f64,
    /// The human's bar that just finished (what an echo answers).
    pub last_human: Option<&'a Summary>,
    /// Average pitch the human has been playing around, to stay out of their way.
    pub human_center: f64,
    /// Average human velocity, to match their dynamics.
    pub human_vel: f64,
    /// Octaves to shift everything the robot plays (set live with [ and ]).
    pub octave: i32,
}

impl Robot {
    /// Notes currently sustained by a pad or drone.
    pub fn held(&self) -> &[u8] {
        &self.held
    }

    pub fn render(&mut self, input: &BarInput, rng: &mut Rng) -> Vec<(f64, Msg)> {
        let d = input.decision;
        if d.role == Role::Pad {
            return self.render_pad(input);
        }
        // Only the pad shapes expression; everything else plays at full expression.
        let mut out = Vec::new();
        if self.expression.is_some_and(|e| e != FULL_EXPRESSION) {
            out.push((input.bar_start, Msg::Expression(FULL_EXPRESSION)));
            self.expression = Some(FULL_EXPRESSION);
        }
        if matches!(d.role, Role::Notes | Role::Chords) {
            out.extend(self.render_slots(input));
            return out;
        }
        let start = input.bar_start;
        let dyn_scale = (0.7 + input.human_vel / 127.0 * 0.5).clamp(0.7, 1.2);
        let vel = |extra: f64| ((30.0 + d.energy * 11.0 + extra) * dyn_scale).clamp(12.0, 110.0) as u8;

        // Sustained parts: keep ringing if nothing changed.
        let sustained = match d.role {
            Role::Drone => {
                let root = 36 + d.chord.root;
                let root = if root > 43 { root - 12 } else { root };
                Some(vec![root, root + 7])
            }
            Role::BassRoot => Some(vec![bass_root(d.chord)]),
            _ => None,
        }
        .map(|notes| notes.into_iter().map(|n| transpose(n, input.octave)).collect::<Vec<_>>());
        if sustained.is_some() && sustained.as_ref() == Some(&self.held) {
            return out;
        }
        for &n in &self.held {
            out.push((start + 0.1, Msg::Off(n)));
        }
        self.held.clear();

        let fresh = out.len();
        let shifted_already = sustained.is_some();
        match d.role {
            // Pad, notes and chords returned early above.
            Role::Rest | Role::Notes | Role::Chords | Role::Pad => {}
            Role::Drone | Role::BassRoot => {
                let notes = sustained.unwrap_or_default();
                for (i, &n) in notes.iter().enumerate() {
                    out.push((start + i as f64 * 0.08, Msg::On(n, vel(-6.0))));
                }
                self.held = notes;
            }
            Role::Arpeggio => self.arpeggio(input, rng, vel(0.0), &mut out),
            Role::Melody => self.melody(input, rng, vel(4.0), &mut out),
            Role::Echo => {
                if !self.echo(input, &mut out) {
                    self.melody(input, rng, vel(4.0), &mut out);
                }
            }
            Role::BassPulse | Role::BassRootFifth | Role::BassOctave | Role::BassWalk | Role::BassSyncopated => {
                self.bass(input, rng, vel(10.0), &mut out)
            }
        }
        // Drone notes were shifted above; shift everything else generated this step.
        if !shifted_already {
            for (_, m) in &mut out[fresh..] {
                *m = match *m {
                    Msg::On(n, v) => Msg::On(transpose(n, input.octave), v),
                    Msg::Off(n) => Msg::Off(transpose(n, input.octave)),
                    e => e,
                };
            }
        }
        out
    }

    /// A sustained pad. A chord change is voice-led: shared notes keep ringing, the rest move to
    /// the nearest voicing (in the pad's register), nothing is re-struck. Loudness follows the
    /// planned energy through expression, ramped over up to two beats, so it swells and fades
    /// with the human without new attacks.
    fn render_pad(&mut self, input: &BarInput) -> Vec<(f64, Msg)> {
        let d = input.decision;
        let start = input.bar_start;
        let mut out = Vec::new();

        let target = (40.0 + d.energy * 25.0).clamp(20.0, 127.0) as u8;
        match self.expression {
            Some(from) if from.abs_diff(target) >= 3 => {
                let span = input.beats.min(2.0);
                for k in 1..=4u8 {
                    let v = from as f64 + (target as f64 - from as f64) * k as f64 / 4.0;
                    out.push((start + span * (k - 1) as f64 / 4.0, Msg::Expression(v.round() as u8)));
                }
                self.expression = Some(target);
            }
            Some(_) => {}
            None => {
                out.push((start, Msg::Expression(target)));
                self.expression = Some(target);
            }
        }

        let base: u8 = if input.human_center >= 57.0 { 45 } else { 57 };
        let voicing = |low: u8| -> Vec<u8> { d.chord.open_voicing(low).into_iter().map(|n| transpose(n, input.octave)).collect() };
        let home = voicing(base);
        let notes = if self.held.is_empty() || self.pad_octave != input.octave {
            home.clone()
        } else {
            voice_lead(&self.held, d.chord, &home)
        };
        self.pad_octave = input.octave;
        if notes == self.held {
            return out;
        }
        let dyn_scale = (0.7 + input.human_vel / 127.0 * 0.5).clamp(0.7, 1.2);
        let vel = ((34.0 + d.energy * 10.0) * dyn_scale).clamp(12.0, 100.0) as u8;
        // New notes first, old ones a moment later, so the change is legato.
        for (i, &n) in notes.iter().filter(|n| !self.held.contains(n)).enumerate() {
            out.push((start + i as f64 * 0.03, Msg::On(n, vel)));
        }
        for &n in self.held.iter().filter(|n| !notes.contains(n)) {
            out.push((start + 0.12, Msg::Off(n)));
        }
        self.held = notes;
        out
    }

    /// Notes and chords modes: one picked note or chord per beat. Whatever sounds rings until something
    /// else is picked (or a rest), even across windows.
    fn render_slots(&mut self, input: &BarInput) -> Vec<(f64, Msg)> {
        let (lo, _) = notes_range(input.human_center);
        // Louder with the planned energy, scaled toward the human's dynamics like the other parts.
        let dyn_scale = (0.7 + input.human_vel / 127.0 * 0.5).clamp(0.7, 1.2);
        let vel = ((32.0 + input.decision.energy * 14.0) * dyn_scale).clamp(20.0, 110.0) as u8;
        let mut out = Vec::new();
        for (k, slot) in input.decision.slots.iter().enumerate() {
            let at = input.bar_start + k as f64;
            let (notes, v) = match slot {
                Slot::Hold => continue,
                Slot::Rest => (Vec::new(), vel),
                Slot::Note(n) => (vec![*n], vel),
                Slot::Chord(c) => (c.open_voicing(lo - 12), vel.saturating_sub(10)),
            };
            for &n in &self.held {
                out.push((at, Msg::Off(n)));
            }
            let notes: Vec<u8> = notes.into_iter().map(|n| transpose(n, input.octave)).collect();
            for (i, &n) in notes.iter().enumerate() {
                out.push((at + i as f64 * 0.03, Msg::On(n, v)));
            }
            self.held = notes;
        }
        out
    }

    fn arpeggio(&self, input: &BarInput, rng: &mut Rng, vel: u8, out: &mut Vec<(f64, Msg)>) {
        let d = input.decision;
        let low = if input.human_center >= 60.0 { 48 } else { 60 };
        let base = d.chord.open_voicing(low);
        let mut pool: Vec<u8> = base.iter().chain(base.iter().map(|n| n + 12).collect::<Vec<_>>().iter()).copied().collect();
        pool.retain(|&n| n <= 88);
        pool.sort_unstable();
        pool.dedup();
        if rng.chance(0.4) {
            pool.reverse();
        }
        let step = step_for(d.energy).min(input.beats);
        let steps = (input.beats / step).round() as usize;
        let offset = rng.below(pool.len());
        for i in 0..steps {
            let t = input.bar_start + i as f64 * step;
            let n = pool[(offset + i) % pool.len()];
            let v = vel.saturating_sub(if i % 2 == 1 { 6 } else { 0 });
            out.push((t, Msg::On(n, v)));
            out.push((t + step * 1.8, Msg::Off(n)));
        }
    }

    fn melody(&self, input: &BarInput, rng: &mut Rng, vel: u8, out: &mut Vec<(f64, Msg)>) {
        let d = input.decision;
        let (lo, hi) = if input.human_center < 62.0 { (67, 86) } else { (48, 67) };
        let scale = input.key.pentatonic();
        let pitches: Vec<u8> = (lo..=hi).filter(|n| scale.contains(&(n % 12))).collect();
        if pitches.is_empty() {
            return;
        }
        // Notes per 4 beats, scaled to the window; a fraction becomes a chance of one more note.
        let per_bar = match d.energy {
            e if e < 0.75 => 1.0,
            e if e < 1.5 => 2.0,
            e if e < 2.5 => 3.5,
            _ => 6.0,
        };
        let expected = per_bar * input.beats / 4.0;
        let count = expected.floor() as usize + rng.chance(expected.fract()) as usize;
        if count == 0 {
            return;
        }
        // Onsets on a half-beat grid, first note often on the downbeat.
        let slots = (input.beats * 2.0) as usize;
        let mut onsets: Vec<usize> = Vec::new();
        if rng.chance(0.5) {
            onsets.push(0);
        }
        while onsets.len() < count.min(slots) {
            let s = rng.below(slots);
            if !onsets.contains(&s) {
                onsets.push(s);
            }
        }
        onsets.sort_unstable();

        // Start on a chord tone near the middle of the range, then drift stepwise.
        let chord_pcs = d.chord.pcs();
        let mid = pitches.len() / 2;
        let mut idx = (0..pitches.len())
            .filter(|&i| chord_pcs.contains(&(pitches[i] % 12)))
            .min_by_key(|&i| i.abs_diff(mid))
            .unwrap_or(mid);
        for (j, &slot) in onsets.iter().enumerate() {
            let t = slot as f64 * 0.5;
            let next = onsets.get(j + 1).map(|&s| s as f64 * 0.5).unwrap_or(input.beats + 1.0);
            let n = pitches[idx];
            out.push((input.bar_start + t, Msg::On(n, vel)));
            out.push((input.bar_start + next - 0.05, Msg::Off(n)));
            let step: i32 = [-2, -1, -1, 1, 1, 2][rng.below(6)];
            idx = (idx as i32 + step).clamp(0, pitches.len() as i32 - 1) as usize;
        }
    }

    fn bass(&self, input: &BarInput, rng: &mut Rng, vel: u8, out: &mut Vec<(f64, Msg)>) {
        let d = input.decision;
        let root = bass_root(d.chord);
        let fifth = root + 7;
        let beats = input.beats;
        // (offset, note, length) in beats.
        let mut notes: Vec<(f64, u8, f64)> = Vec::new();
        let mut every = |step: f64, len: f64, pick: &dyn Fn(usize) -> u8| {
            let mut i = 0;
            while (i as f64) * step < beats {
                notes.push((i as f64 * step, pick(i), step * len));
                i += 1;
            }
        };
        match d.role {
            Role::BassPulse => every(if d.energy >= 2.5 { 0.5 } else { 1.0 }, 0.8, &|_| root),
            Role::BassRootFifth => {
                every(if d.energy < 1.5 { 2.0 } else { 1.0 }, 0.9, &|i| if i % 2 == 0 { root } else { fifth })
            }
            Role::BassOctave => {
                every(if d.energy >= 2.0 { 0.5 } else { 1.0 }, 0.7, &|i| if i % 2 == 0 { root } else { root + 12 })
            }
            Role::BassWalk => {
                let scale = input.key.scale();
                let up = rng.chance(0.5);
                let mut n = root;
                let mut t = 0.0;
                while t < beats {
                    notes.push((t, n, 0.9));
                    n = next_scale_note(n, &scale, up);
                    t += 1.0;
                }
            }
            Role::BassSyncopated => {
                for (t, n, len) in [(0.0, root, 1.2), (1.5, root, 0.8), (2.5, fifth, 0.4), (3.0, root + 12, 0.8)] {
                    if t < beats {
                        notes.push((t, n, len));
                    }
                }
            }
            _ => {}
        }
        for (t, n, len) in notes {
            let accent = if t.fract() == 0.0 { 0 } else { 8 };
            out.push((input.bar_start + t, Msg::On(n, vel.saturating_sub(accent))));
            out.push((input.bar_start + t + len, Msg::Off(n)));
        }
    }

    /// Replay the human's last bar an octave away, snapped to the pentatonic. False if nothing to echo.
    fn echo(&self, input: &BarInput, out: &mut Vec<(f64, Msg)>) -> bool {
        let Some(last) = input.last_human else { return false };
        // Only the top note of each struck cluster, so chords echo as a line.
        let mut line: Vec<_> = Vec::new();
        for h in &last.onsets {
            if !last.onsets.iter().any(|o| (o.offset - h.offset).abs() < 0.12 && o.note > h.note) {
                line.push(h);
            }
        }
        if line.is_empty() {
            return false;
        }
        let shift: i32 = if input.human_center < 64.0 { 12 } else { -12 };
        let scale = input.key.pentatonic();
        for h in line.into_iter().take(8) {
            let n = snap((h.note as i32 + shift).clamp(24, 100) as u8, &scale);
            let t = input.bar_start + h.offset;
            let v = ((h.vel as f64) * 0.7).clamp(15.0, 100.0) as u8;
            out.push((t, Msg::On(n, v)));
            out.push((t + h.dur.clamp(0.25, input.beats), Msg::Off(n)));
        }
        true
    }
}

/// Move each ringing voice to a note of `chord`, trying every way of giving the voices the
/// chord's tones and keeping the one with the least total movement. Small penalties keep the
/// root in the bass and the pad near its home register, so it doesn't creep up or down.
fn voice_lead(held: &[u8], chord: Chord, home: &[u8]) -> Vec<u8> {
    let mut pcs = chord.pcs();
    // As many tones as voices: drop the fifth first (then the ninth), or double the root.
    while pcs.len() > held.len() {
        let fifth = (chord.root + 7) % 12;
        let drop = pcs.iter().position(|&p| p == fifth && p != chord.root).unwrap_or(pcs.len() - 1);
        pcs.remove(drop);
    }
    while pcs.len() < held.len() {
        pcs.push(chord.root % 12);
    }
    let nearest = |from: u8, pc: u8| -> u8 {
        (from.saturating_sub(6)..=from.saturating_add(6)).filter(|n| n % 12 == pc).min_by_key(|n| n.abs_diff(from)).unwrap_or(from)
    };
    let home_mean = home.iter().map(|&n| n as i32).sum::<i32>() / home.len().max(1) as i32;
    let mut best: Option<(i32, Vec<u8>)> = None;
    for perm in permutations(&pcs) {
        let mut notes: Vec<u8> = held.iter().zip(&perm).map(|(&h, &pc)| nearest(h, pc)).collect();
        notes.sort_unstable();
        let moves: i32 = held.iter().zip(&perm).map(|(&h, &pc)| nearest(h, pc).abs_diff(h) as i32).sum();
        let unison = notes.windows(2).filter(|w| w[0] == w[1]).count() as i32 * 12;
        let inversion = if notes[0] % 12 == chord.root % 12 { 0 } else { 3 };
        let mean = notes.iter().map(|&n| n as i32).sum::<i32>() / notes.len() as i32;
        let cost = moves + unison + inversion + (mean - home_mean).abs() / 2;
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, notes));
        }
    }
    best.map(|(_, n)| n).unwrap_or_else(|| home.to_vec())
}

fn permutations(items: &[u8]) -> Vec<Vec<u8>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, first);
            out.push(p);
        }
    }
    out
}

fn transpose(note: u8, octaves: i32) -> u8 {
    let mut n = note as i32 + 12 * octaves;
    while n > 127 {
        n -= 12;
    }
    while n < 0 {
        n += 12;
    }
    n as u8
}

/// The chord root in bass range, E1..D#2.
fn bass_root(chord: crate::theory::Chord) -> u8 {
    28 + (chord.root + 12 - 4) % 12
}

fn next_scale_note(note: u8, scale: &[u8], up: bool) -> u8 {
    (1..=2u8)
        .map(|d| if up { note + d } else { note - d })
        .find(|n| scale.contains(&(n % 12)))
        .unwrap_or(note)
}

fn step_for(energy: f64) -> f64 {
    match energy {
        e if e < 0.75 => 4.0,
        e if e < 1.5 => 2.0,
        e if e < 2.5 => 1.0,
        _ => 0.5,
    }
}

fn snap(note: u8, scale: &[u8]) -> u8 {
    (0..=6u8)
        .flat_map(|d| [note.saturating_sub(d), note.saturating_add(d)])
        .find(|n| scale.contains(&(n % 12)))
        .unwrap_or(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{Chord, Mode, Quality};

    fn decision(role: Role, energy: f64) -> Decision {
        Decision { role, chord: Chord { root: 5, quality: Quality::Maj7 }, energy, slots: Vec::new(), note: String::new(), perception: None }
    }

    fn input<'a>(d: &'a Decision, bar_start: f64) -> BarInput<'a> {
        BarInput {
            decision: d,
            key: Key { tonic: 0, mode: Mode::Major },
            bar_start,
            beats: 4.0,
            last_human: None,
            human_center: 60.0,
            human_vel: 64.0,
            octave: 0,
        }
    }

    #[test]
    fn pad_sustains_across_bars_and_releases_on_change() {
        let mut r = Robot::default();
        let mut rng = Rng::from_time();
        let pad = decision(Role::Pad, 1.0);
        let first = r.render(&input(&pad, 0.0), &mut rng);
        assert_eq!(first.iter().filter(|(_, m)| matches!(m, Msg::On(..))).count(), 4);
        assert!(r.render(&input(&pad, 4.0), &mut rng).is_empty(), "same pad keeps ringing");
        let rest = decision(Role::Rest, 0.0);
        let released = r.render(&input(&rest, 8.0), &mut rng);
        assert_eq!(released.iter().filter(|(_, m)| matches!(m, Msg::Off(_))).count(), 4);
    }

    #[test]
    fn octave_shift_moves_everything_and_restrikes_held_notes() {
        let mut r = Robot::default();
        let mut rng = Rng::from_time();
        let pad = decision(Role::Pad, 1.0);
        let low: Vec<u8> = r.held().to_vec();
        assert!(low.is_empty());
        r.render(&input(&pad, 0.0), &mut rng);
        let base = r.held().to_vec();
        let mut up = input(&pad, 4.0);
        up.octave = 1;
        let evs = r.render(&up, &mut rng);
        assert_eq!(r.held(), base.iter().map(|n| n + 12).collect::<Vec<_>>());
        assert_eq!(evs.iter().filter(|(_, m)| matches!(m, Msg::Off(_))).count(), base.len(), "old pad released");

        let bass = decision(Role::BassPulse, 1.0);
        let mut down = input(&bass, 8.0);
        down.octave = -1;
        let plain = Robot::default().render(&input(&bass, 8.0), &mut rng);
        let shifted = Robot::default().render(&down, &mut rng);
        let first_on = |e: &[(f64, Msg)]| e.iter().find_map(|(_, m)| if let Msg::On(n, _) = m { Some(*n) } else { None });
        assert_eq!(first_on(&shifted), first_on(&plain).map(|n| n - 12));
    }

    #[test]
    fn picked_notes_ring_until_replaced() {
        let mut r = Robot::default();
        let mut rng = Rng::from_time();
        let c = Chord { root: 0, quality: Quality::Maj7 };
        let mut d = decision(Role::Notes, 1.5);
        d.slots = vec![Slot::Note(72), Slot::Hold, Slot::Chord(c), Slot::Hold];
        let evs = r.render(&input(&d, 0.0), &mut rng);
        // E.g. C5 on beat 1, released on beat 3 when the chord comes in; the chord keeps ringing.
        assert!(evs.contains(&(0.0, Msg::On(72, evs.iter().find_map(|(_, m)| if let Msg::On(72, v) = m { Some(*v) } else { None }).unwrap()))));
        assert!(evs.contains(&(2.0, Msg::Off(72))));
        assert_eq!(r.held().len(), c.open_voicing(48).len());
        // Next window: a rest on its first beat releases the chord.
        d.slots = vec![Slot::Rest];
        let evs = r.render(&input(&d, 4.0), &mut rng);
        assert_eq!(evs.iter().filter(|(b, m)| *b == 4.0 && matches!(m, Msg::Off(_))).count(), c.open_voicing(48).len());
        assert!(r.held().is_empty());
    }

    #[test]
    fn pad_voice_leads_and_swells_without_restriking() {
        let mut r = Robot::default();
        let mut rng = Rng::from_time();
        let ons = |e: &[(f64, Msg)]| e.iter().filter(|(_, m)| matches!(m, Msg::On(..))).count();
        let offs = |e: &[(f64, Msg)]| e.iter().filter(|(_, m)| matches!(m, Msg::Off(_))).count();
        let expression = |e: &[(f64, Msg)]| e.iter().filter_map(|(_, m)| if let Msg::Expression(v) = m { Some(*v) } else { None }).last();

        let mut d = decision(Role::Pad, 1.0);
        d.chord = Chord { root: 0, quality: Quality::Maj7 };
        let first = r.render(&input(&d, 0.0), &mut rng);
        assert_eq!(ons(&first), 4);
        let c = r.held().to_vec();

        // Cmaj7 -> Am7 shares C, E and G: only the changed voice moves, nothing is re-struck.
        d.chord = Chord { root: 9, quality: Quality::Min7 };
        let change = r.render(&input(&d, 4.0), &mut rng);
        let kept = r.held().iter().filter(|n| c.contains(n)).count();
        assert!(kept >= 2, "shared notes keep ringing: {c:?} -> {:?}", r.held());
        assert_eq!(ons(&change), 4 - kept);
        assert_eq!(offs(&change), c.len() - kept);
        assert!(change.iter().all(|(_, m)| !matches!(m, Msg::On(n, _) if c.contains(n))), "no re-attack");

        // Same chord, more energy: no notes, just expression up.
        let before = expression(&first).unwrap();
        d.energy = 2.5;
        let swell = r.render(&input(&d, 8.0), &mut rng);
        assert_eq!(ons(&swell) + offs(&swell), 0);
        assert!(expression(&swell).unwrap() > before);

        // Another part afterwards gets full expression back.
        let bass = decision(Role::BassPulse, 1.0);
        assert_eq!(expression(&r.render(&input(&bass, 12.0), &mut rng)), Some(FULL_EXPRESSION));
    }

    #[test]
    fn pad_stays_in_its_register_over_a_long_progression() {
        let mut r = Robot::default();
        let mut rng = Rng::from_time();
        let mut d = decision(Role::Pad, 1.0);
        let chords = [(0, Quality::Maj7), (9, Quality::Min7), (5, Quality::Maj7), (7, Quality::Dom7), (2, Quality::Min7), (4, Quality::Min7)];
        let mut means = Vec::new();
        for i in 0..60 {
            let (root, quality) = chords[(i * 5) % chords.len()];
            d.chord = Chord { root, quality };
            r.render(&input(&d, i as f64 * 4.0), &mut rng);
            let held = r.held();
            assert!(held.iter().collect::<std::collections::HashSet<_>>().len() == held.len(), "no unisons: {held:?}");
            assert!(held.iter().all(|&n| d.chord.pcs().contains(&(n % 12))), "only chord tones: {held:?}");
            means.push(held.iter().map(|&n| n as f64).sum::<f64>() / held.len() as f64);
        }
        let (lo, hi) = means.iter().fold((f64::MAX, f64::MIN), |(a, b), &m| (a.min(m), b.max(m)));
        assert!(hi - lo <= 9.0, "register wandered from {lo} to {hi}");
    }

    #[test]
    fn every_note_on_has_an_off() {
        let mut rng = Rng::from_time();
        for role in [
            Role::Arpeggio,
            Role::Melody,
            Role::Echo,
            Role::BassPulse,
            Role::BassRootFifth,
            Role::BassOctave,
            Role::BassWalk,
            Role::BassSyncopated,
        ] {
            for e in [0.0, 1.0, 2.0, 3.0] {
                let d = decision(role, e);
                let evs = Robot::default().render(&input(&d, 0.0), &mut rng);
                let ons = evs.iter().filter(|(_, m)| matches!(m, Msg::On(..))).count();
                let offs = evs.iter().filter(|(_, m)| matches!(m, Msg::Off(_))).count();
                assert!(ons > 0 && ons == offs, "{role:?} e={e}: {ons} on / {offs} off");
            }
        }
    }
}
