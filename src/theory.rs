//! Deterministic music theory: note names, keys, chord recognition, key estimation.

const SHARP: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLAT: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const ROMAN: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub tonic: u8,
    pub mode: Mode,
}

impl Key {
    pub fn uses_flats(self) -> bool {
        let relative_major = match self.mode {
            Mode::Major => self.tonic,
            Mode::Minor => (self.tonic + 3) % 12,
        };
        matches!(relative_major, 1 | 3 | 5 | 6 | 8 | 10)
    }

    pub fn pc_name(self, pc: u8) -> &'static str {
        let names = if self.uses_flats() { &FLAT } else { &SHARP };
        names[(pc % 12) as usize]
    }

    pub fn note_name(self, note: u8) -> String {
        format!("{}{}", self.pc_name(note), note as i32 / 12 - 1)
    }

    pub fn name(self) -> String {
        let mode = match self.mode {
            Mode::Major => "major",
            Mode::Minor => "minor",
        };
        format!("{} {}", self.pc_name(self.tonic), mode)
    }

    pub fn scale(self) -> [u8; 7] {
        let steps = match self.mode {
            Mode::Major => [0, 2, 4, 5, 7, 9, 11],
            Mode::Minor => [0, 2, 3, 5, 7, 8, 10],
        };
        steps.map(|s| (self.tonic + s) % 12)
    }

    /// Pentatonic subset: no avoid-notes, so it floats over any diatonic chord.
    pub fn pentatonic(self) -> [u8; 5] {
        let steps = match self.mode {
            Mode::Major => [0, 2, 4, 7, 9],
            Mode::Minor => [0, 3, 5, 7, 10],
        };
        steps.map(|s| (self.tonic + s) % 12)
    }

    pub fn tonic_chord(self) -> Chord {
        let quality = match self.mode {
            Mode::Major => Quality::Maj7,
            Mode::Minor => Quality::Min7,
        };
        Chord { root: self.tonic, quality }
    }

    /// Diatonic seventh chords with roman numerals, skipping the half-diminished one.
    pub fn diatonic_sevenths(self) -> Vec<(String, Chord)> {
        let s = self.scale();
        (0..7)
            .filter_map(|d| {
                let root = s[d];
                let iv = |k: usize| (s[(d + k) % 7] + 12 - root) % 12;
                let quality = match (iv(2), iv(4), iv(6)) {
                    (4, 7, 11) => Quality::Maj7,
                    (3, 7, 10) => Quality::Min7,
                    (4, 7, 10) => Quality::Dom7,
                    _ => return None,
                };
                let numeral = match quality {
                    Quality::Min7 => ROMAN[d].to_lowercase(),
                    _ => ROMAN[d].to_string(),
                };
                Some((numeral, Chord { root, quality }))
            })
            .collect()
    }

    /// Parses "C major", "f# minor", "Bbm", "A".
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let split = if s.len() > 1 && matches!(s.as_bytes()[1], b'#' | b'b') { 2 } else { 1 };
        let tonic = pc_from_name(&s[..split])?;
        let mode = match s[split..].trim().to_lowercase().as_str() {
            "" | "maj" | "major" => Mode::Major,
            "m" | "min" | "minor" => Mode::Minor,
            _ => return None,
        };
        Some(Key { tonic, mode })
    }
}

fn pc_from_name(name: &str) -> Option<u8> {
    let mut chars = name.chars();
    let base: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let adj = match chars.next() {
        Some('#') => 1,
        Some('b') => -1,
        None => 0,
        _ => return None,
    };
    Some(((base + adj + 12) % 12) as u8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Power,
    Maj,
    Min,
    Sus2,
    Sus4,
    Dim,
    Aug,
    Maj6,
    Min6,
    Dom7,
    Maj7,
    Min7,
    HalfDim7,
    Add9,
    MinAdd9,
    Maj9,
    Min9,
}

impl Quality {
    // Order matters: simpler chords win ties.
    const ALL: [Quality; 17] = [
        Quality::Power,
        Quality::Maj,
        Quality::Min,
        Quality::Sus2,
        Quality::Sus4,
        Quality::Dim,
        Quality::Aug,
        Quality::Maj7,
        Quality::Min7,
        Quality::Dom7,
        Quality::Maj6,
        Quality::Min6,
        Quality::HalfDim7,
        Quality::Add9,
        Quality::MinAdd9,
        Quality::Maj9,
        Quality::Min9,
    ];

    pub fn intervals(self) -> &'static [u8] {
        match self {
            Quality::Power => &[0, 7],
            Quality::Maj => &[0, 4, 7],
            Quality::Min => &[0, 3, 7],
            Quality::Sus2 => &[0, 2, 7],
            Quality::Sus4 => &[0, 5, 7],
            Quality::Dim => &[0, 3, 6],
            Quality::Aug => &[0, 4, 8],
            Quality::Maj6 => &[0, 4, 7, 9],
            Quality::Min6 => &[0, 3, 7, 9],
            Quality::Dom7 => &[0, 4, 7, 10],
            Quality::Maj7 => &[0, 4, 7, 11],
            Quality::Min7 => &[0, 3, 7, 10],
            Quality::HalfDim7 => &[0, 3, 6, 10],
            Quality::Add9 => &[0, 2, 4, 7],
            Quality::MinAdd9 => &[0, 2, 3, 7],
            Quality::Maj9 => &[0, 2, 4, 7, 11],
            Quality::Min9 => &[0, 2, 3, 7, 10],
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Quality::Power => "5",
            Quality::Maj => "",
            Quality::Min => "m",
            Quality::Sus2 => "sus2",
            Quality::Sus4 => "sus4",
            Quality::Dim => "dim",
            Quality::Aug => "aug",
            Quality::Maj6 => "6",
            Quality::Min6 => "m6",
            Quality::Dom7 => "7",
            Quality::Maj7 => "maj7",
            Quality::Min7 => "m7",
            Quality::HalfDim7 => "m7b5",
            Quality::Add9 => "add9",
            Quality::MinAdd9 => "madd9",
            Quality::Maj9 => "maj9",
            Quality::Min9 => "m9",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub root: u8,
    pub quality: Quality,
}

impl Chord {
    pub fn name(self, key: Key) -> String {
        format!("{}{}", key.pc_name(self.root), self.quality.suffix())
    }

    pub fn pcs(self) -> Vec<u8> {
        self.quality.intervals().iter().map(|i| (self.root + i) % 12).collect()
    }

    /// Same root and basic triad, ignoring sixths, sevenths and added notes: Cmaj7, Cmaj9, C6
    /// and C are one chord for following a progression, where melody notes over a held chord
    /// keep changing its detected name.
    pub fn same_family(self, other: Chord) -> bool {
        let triad = |q: Quality| match q {
            Quality::Maj | Quality::Maj6 | Quality::Dom7 | Quality::Maj7 | Quality::Add9 | Quality::Maj9 => 0,
            Quality::Min | Quality::Min6 | Quality::Min7 | Quality::MinAdd9 | Quality::Min9 => 1,
            Quality::Dim | Quality::HalfDim7 => 2,
            Quality::Aug => 3,
            Quality::Sus2 | Quality::Sus4 => 4,
            Quality::Power => 5,
        };
        self.root == other.root && triad(self.quality) == triad(other.quality)
    }

    /// Spread, open voicing starting at the first root at or above `low`:
    /// root, fifth, then third and seventh (or ninth) an octave up.
    pub fn open_voicing(self, low: u8) -> Vec<u8> {
        let root = low + (self.root + 12 - low % 12) % 12;
        let iv = self.quality.intervals();
        let has = |i: u8| iv.contains(&i);
        let third = [4, 3, 2, 5].into_iter().find(|&i| has(i)).unwrap_or(4);
        let fifth = [7, 6, 8].into_iter().find(|&i| has(i)).unwrap_or(7);
        let top = [11, 10, 9].into_iter().find(|&i| has(i)).unwrap_or(14);
        let mut v = vec![root, root + fifth, root + 12 + third, root + 12 + top];
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Best-matching chord for a pitch-class weight profile.
pub fn detect_chord(weights: &[f64; 12], bass: Option<u8>) -> Option<Chord> {
    let max = weights.iter().cloned().fold(0.0, f64::max);
    if max <= 0.0 {
        return None;
    }
    let present: Vec<u8> = (0..12u8).filter(|&pc| weights[pc as usize] >= 0.2 * max).collect();
    if present.len() < 2 {
        return None;
    }
    let mut best: Option<(f64, Chord)> = None;
    for &root in &present {
        for q in Quality::ALL {
            let chord = Chord { root, quality: q };
            let pcs = chord.pcs();
            let matched = pcs.iter().filter(|p| present.contains(p)).count() as f64;
            let missing = pcs.len() as f64 - matched;
            let extra = present.iter().filter(|p| !pcs.contains(p)).count() as f64;
            let mut score = matched * 2.0 - missing * 2.5 - extra * 1.5;
            if bass == Some(root) {
                score += 1.5;
            }
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, chord));
            }
        }
    }
    best.filter(|(s, _)| *s > 0.0).map(|(_, c)| c)
}

const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// Krumhansl-Schmuckler key estimate: all 24 keys, best correlation first.
pub fn rank_keys(hist: &[f64; 12]) -> Vec<(Key, f64)> {
    let mut out = Vec::with_capacity(24);
    for tonic in 0..12u8 {
        for (mode, profile) in [(Mode::Major, &MAJOR_PROFILE), (Mode::Minor, &MINOR_PROFILE)] {
            let rotated: Vec<f64> = (0..12).map(|i| profile[(i + 12 - tonic as usize) % 12]).collect();
            out.push((Key { tonic, mode }, correlation(hist, &rotated)));
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().sum::<f64>() / n;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        num += (x - ma) * (y - mb);
        da += (x - ma).powi(2);
        db += (y - mb).powi(2);
    }
    if da == 0.0 || db == 0.0 { 0.0 } else { num / (da * db).sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights(notes: &[u8]) -> [f64; 12] {
        let mut w = [0.0; 12];
        for n in notes {
            w[(n % 12) as usize] += 1.0;
        }
        w
    }

    fn c_major() -> Key {
        Key { tonic: 0, mode: Mode::Major }
    }

    #[test]
    fn detects_common_chords() {
        let k = c_major();
        let name = |notes: &[u8]| detect_chord(&weights(notes), Some(notes[0] % 12)).map(|c| c.name(k));
        assert_eq!(name(&[48, 52, 55]).as_deref(), Some("C"));
        assert_eq!(name(&[45, 48, 52]).as_deref(), Some("Am"));
        assert_eq!(name(&[48, 52, 55, 59]).as_deref(), Some("Cmaj7"));
        assert_eq!(name(&[45, 52, 55, 60]).as_deref(), Some("Am7"));
        assert_eq!(name(&[43, 47, 50, 53]).as_deref(), Some("G7"));
        assert_eq!(name(&[50, 55, 57]).as_deref(), Some("Dsus4"));
        // Same pitch classes, bass decides: C6 vs Am7.
        assert_eq!(name(&[48, 52, 55, 57]).as_deref(), Some("C6"));
        assert_eq!(name(&[57, 60, 64, 67]).as_deref(), Some("Am7"));
    }

    #[test]
    fn estimates_keys() {
        let mut h = [0.0; 12];
        for pc in [0, 2, 4, 5, 7, 9, 11, 0, 4, 7] {
            h[pc] += 1.0;
        }
        assert_eq!(rank_keys(&h)[0].0, c_major());
        let mut h = [0.0; 12];
        for pc in [9, 11, 0, 2, 4, 5, 7, 9, 0, 4, 9] {
            h[pc] += 1.0;
        }
        assert_eq!(rank_keys(&h)[0].0, Key { tonic: 9, mode: Mode::Minor });
    }

    #[test]
    fn diatonic_and_parsing() {
        let k = Key::parse("C major").unwrap();
        let names: Vec<String> = k.diatonic_sevenths().iter().map(|(r, c)| format!("{r}:{}", c.name(k))).collect();
        assert_eq!(names, ["I:Cmaj7", "ii:Dm7", "iii:Em7", "IV:Fmaj7", "V:G7", "vi:Am7"]);
        assert_eq!(Key::parse("Bbm"), Some(Key { tonic: 10, mode: Mode::Minor }));
        assert_eq!(Key::parse("f# minor"), Some(Key { tonic: 6, mode: Mode::Minor }));
        assert_eq!(Key { tonic: 10, mode: Mode::Minor }.name(), "Bb minor");
    }

    #[test]
    fn voicing_is_open() {
        let v = Chord { root: 5, quality: Quality::Maj7 }.open_voicing(48);
        assert_eq!(v, vec![53, 60, 69, 76]); // F3 C4 A4 E5
    }
}
