//! Builds Jev's options from the musical context and turns its answers into a decision.
//! Code decides *which* options are sensible; Jev picks among them.

use serde_json::{Value, json};

use crate::jev::{Answers, Question};
use crate::listener::{Playing, Summary};
use crate::rng::Rng;
use crate::theory::{Chord, Key};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Pad,
    Arpeggio,
    Melody,
    Echo,
    Drone,
    Rest,
    // Bass-mode patterns.
    BassRoot,
    BassPulse,
    BassRootFifth,
    BassOctave,
    BassWalk,
    BassSyncopated,
    /// Notes mode: Jev picked each beat's note or chord.
    Notes,
}

/// What the robot is: an ambient texture player, the bass player, or a player whose every
/// note/chord is picked by Jev.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    Ambient,
    Bass,
    Notes,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Ambient => "ambient",
            Part::Bass => "bass",
            Part::Notes => "notes",
        }
    }

    /// Beats per decision unless the user chose: notes mode goes one beat at a time so Jev
    /// hears its own last pick; the others decide once a bar.
    pub fn default_step(self, beats_per_bar: u32) -> f64 {
        match self {
            Part::Notes => 1.0,
            _ => beats_per_bar as f64,
        }
    }
}

impl Role {
    pub fn key(self) -> &'static str {
        match self {
            Role::Pad => "pad",
            Role::Arpeggio => "arpeggio",
            Role::Melody => "melody",
            Role::Echo => "echo",
            Role::Drone => "drone",
            Role::Rest => "rest",
            Role::BassRoot => "root",
            Role::BassPulse => "pulse",
            Role::BassRootFifth => "root_fifth",
            Role::BassOctave => "octave",
            Role::BassWalk => "walk",
            Role::BassSyncopated => "syncopated",
            Role::Notes => "notes",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Role::Pad => "Sustained, soft chord pad holding the harmony",
            Role::Arpeggio => "Slow, gentle broken-chord arpeggio",
            Role::Melody => "Sparse, floating counter-melody in a different register from the human",
            Role::Echo => "Answer the human's last phrase by echoing it, transformed, in another octave",
            Role::Drone => "Low drone on the root, barely moving",
            Role::Rest => "Silence: only when the human is busy and needs space",
            Role::BassRoot => "One long held root note; only for the sparsest, most static moments",
            Role::BassPulse => "Steady repeated root notes on the beat, a heartbeat",
            Role::BassRootFifth => "Alternate the root and the fifth",
            Role::BassOctave => "Bounce between the root and its octave",
            Role::BassWalk => "Walking bass: stepwise scale notes moving away from the root",
            Role::BassSyncopated => "Syncopated riff on the root and fifth, off-beat accents",
            Role::Notes => "Individually picked notes and chords",
        }
    }
}

pub const ENERGY_LEVELS: [&str; 4] = [
    "Almost nothing: one soft note or silence for the whole bar",
    "Long sustained tones, very soft, one or two events per bar",
    "Gentle movement: a few notes per bar at a relaxed pace",
    "Flowing: continuous but still soft and unhurried motion",
];

/// One beat in notes mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Note(u8),
    Chord(Chord),
    /// Keep whatever is sounding.
    Hold,
    Rest,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub role: Role,
    pub chord: Chord,
    /// 0.0 ..= 3.0, see ENERGY_LEVELS.
    pub energy: f64,
    /// Notes mode: what to play on each beat of the window. Empty otherwise.
    pub slots: Vec<Slot>,
    /// Short human-readable account of how it was chosen, for the log.
    pub note: String,
}

/// What the robot actually did in one window, as Jev will be told.
#[derive(Clone, Debug)]
pub struct RobotStep {
    pub role: Role,
    /// Notes it started (or kept ringing) in the window.
    pub notes: Vec<u8>,
    pub label: String,
}

pub struct Context<'a> {
    pub key: Key,
    pub key_known: bool,
    pub bpm: f64,
    pub beats_per_bar: u32,
    /// Length of each decision window in beats.
    pub step_beats: f64,
    /// Completed windows, oldest first: what the human played and what the robot played in each.
    pub timeline: Vec<(Summary, Option<RobotStep>)>,
    /// The window in progress: the human so far, and what the robot is playing in it.
    pub current: &'a Summary,
    pub robot_now: Option<RobotStep>,
    pub robot_chord: Chord,
    /// The robot's most recent melody note (notes mode steps from it).
    pub last_note: Option<u8>,
    /// Where the human has been playing (average pitch).
    pub human_center: f64,
    pub part: Part,
}

pub struct Request {
    pub state: Value,
    pub questions: Vec<(String, Question)>,
    roles: Vec<Role>,
    chords: Vec<(String, Chord)>,
    /// When the human is playing chords, the robot follows them instead of choosing.
    human_chord: Option<Chord>,
    robot_chord: Chord,
    key: Key,
    /// The robot's role in each recent step, oldest first.
    recent_roles: Vec<Role>,
    /// Step length in bars, so repetition is judged in musical time.
    step_bars: f64,
    /// Notes mode: the options offered for every beat, and how many beats.
    slot_options: Vec<(String, Slot)>,
    slot_count: usize,
}

/// Melody range for the robot in notes mode: out of the human's way.
pub fn notes_range(human_center: f64) -> (u8, u8) {
    if human_center < 62.0 { (60, 84) } else { (48, 72) }
}

fn robot_json(step: &RobotStep, key: Key) -> Value {
    json!({
        "played": step.label,
        "notes": step.notes.iter().map(|&n| key.note_name(n)).collect::<Vec<_>>(),
    })
}

pub fn build(ctx: &Context) -> Request {
    let key = ctx.key;
    let window = window_name(ctx.step_beats, ctx.beats_per_bar);
    let last_human = ctx.timeline.last().map(|(h, _)| h);
    let human_chord = if ctx.current.playing.is_chordal() {
        ctx.current.chord.or_else(|| last_human.and_then(|s| s.chord))
    } else {
        None
    };
    let recently_played =
        ctx.current.playing != Playing::Silent || last_human.is_some_and(|s| s.playing != Playing::Silent);

    // Both parts, window by window, so Jev hears the conversation rather than two monologues.
    let timeline: Vec<Value> = ctx
        .timeline
        .iter()
        .map(|(h, r)| json!({ "human": h.to_json(key), "you": r.as_ref().map(|r| robot_json(r, key)) }))
        .collect();
    let state = json!({
        "style": "ambient: slow, spacious, soft and sustained textures",
        "tempo_bpm": ctx.bpm.round(),
        "meter": format!("{}/4", ctx.beats_per_bar),
        "window_length_beats": ctx.step_beats,
        "key": if ctx.key_known { key.name() } else { format!("not yet clear (assuming {})", key.name()) },
        "your_instrument": match ctx.part {
            Part::Ambient => "ambient textures",
            Part::Bass => "bass",
            Part::Notes => "melody and chords, chosen note by note",
        },
        "timeline_oldest_first": timeline,
        "this_window": {
            "human_so_far": ctx.current.to_json(key),
            "you": ctx.robot_now.as_ref().map(|r| robot_json(r, key)),
        },
    });

    let mut recent_roles: Vec<Role> = ctx.timeline.iter().filter_map(|(_, r)| r.as_ref().map(|r| r.role)).collect();
    recent_roles.extend(ctx.robot_now.as_ref().map(|r| r.role));
    let mut req = Request {
        state,
        questions: Vec::new(),
        roles: Vec::new(),
        chords: Vec::new(),
        human_chord,
        robot_chord: ctx.robot_chord,
        key,
        recent_roles,
        step_bars: ctx.step_beats / ctx.beats_per_bar as f64,
        slot_options: Vec::new(),
        slot_count: 0,
    };

    if ctx.part == Part::Notes {
        build_notes(ctx, &mut req, &window);
        return req;
    }

    let (roles, instructions) = match ctx.part {
        Part::Bass => {
            let roles = vec![
                Role::BassRoot,
                Role::BassPulse,
                Role::BassRootFifth,
                Role::BassOctave,
                Role::BassWalk,
                Role::BassSyncopated,
                Role::Rest,
            ];
            let q = format!(
                "You are the bass player in an ambient jam with the human described in the state. \
                 Keep the bassline gently moving and varied, following the human's energy. \
                 Which bass pattern should you play for the {window}?"
            );
            (roles, q)
        }
        _ => {
            // Don't fight the human's harmony: while they play chords, stay out of the chord-playing roles.
            let mut roles = if human_chord.is_some() {
                vec![Role::Melody, Role::Echo, Role::Drone, Role::Rest]
            } else {
                vec![Role::Pad, Role::Arpeggio, Role::Melody, Role::Echo, Role::Drone, Role::Rest]
            };
            if !recently_played {
                roles.retain(|r| *r != Role::Echo);
            }
            let q = format!(
                "You are an ambient musician improvising with the human described in the state. \
                 What should you play for the {window} to best complement them?"
            );
            (roles, q)
        }
    };

    req.questions.push((
        "role".into(),
        Question::Choice {
            instructions,
            options: roles.iter().map(|r| (r.key().to_string(), r.describe().to_string())).collect(),
        },
    ));
    req.questions.push((
        "energy".into(),
        Question::Score {
            instructions: format!("How much activity should your part have for the {window}, given the human's playing?"),
            levels: ENERGY_LEVELS.iter().map(|s| s.to_string()).collect(),
        },
    ));
    req.roles = roles;

    if human_chord.is_none() {
        let mut options = vec![("stay".to_string(), format!("Stay on {}", ctx.robot_chord.name(key)))];
        req.chords.push(("stay".to_string(), ctx.robot_chord));
        for (numeral, chord) in key.diatonic_sevenths() {
            if chord != ctx.robot_chord {
                let name = chord.name(key);
                options.push((name.clone(), format!("Move to {name} ({numeral} in {})", key.name())));
                req.chords.push((name, chord));
            }
        }
        req.questions.push((
            "next_chord".into(),
            Question::Choice {
                instructions: format!(
                    "Which chord should the accompaniment play for the {window}? Ambient music favours slow \
                     harmonic motion, smooth voice-leading and fitting the human's melody notes."
                ),
                options,
            },
        ));
    }
    req
}

/// Notes mode: offer concrete notes near the robot's last note, and concrete chords, for every
/// beat of the window; Jev picks one option per beat.
fn build_notes(ctx: &Context, req: &mut Request, window: &str) {
    let key = ctx.key;
    let (lo, hi) = notes_range(ctx.human_center);
    let last = ctx.last_note.unwrap_or((lo + hi) / 2);
    let harmony = req.human_chord.unwrap_or(ctx.robot_chord);
    let scale = key.scale();

    let mut options: Vec<(String, String, Slot)> = Vec::new();
    let from = last.saturating_sub(7).max(lo);
    let to = (last + 7).min(hi);
    for n in from..=to {
        if !scale.contains(&(n % 12)) {
            continue;
        }
        let name = key.note_name(n);
        let fit = if harmony.pcs().contains(&(n % 12)) {
            format!("chord tone of {}", harmony.name(key))
        } else {
            format!("scale tone, colour over {}", harmony.name(key))
        };
        let motion = match n as i32 - last as i32 {
            0 => "repeat your last note".to_string(),
            d if d > 0 => format!("{d} semitones above your last note"),
            d => format!("{} semitones below your last note", -d),
        };
        options.push((format!("note_{name}"), format!("Single note {name}: {fit}, {motion}"), Slot::Note(n)));
    }

    let mut chords: Vec<Chord> = req.human_chord.into_iter().collect();
    for (_, c) in key.diatonic_sevenths() {
        if !chords.contains(&c) {
            chords.push(c);
        }
    }
    for c in chords {
        let name = c.name(key);
        let voicing: Vec<String> = c.open_voicing(lo - 12).iter().map(|&n| key.note_name(n)).collect();
        let why = if Some(c) == req.human_chord { ", the chord the human is holding" } else { "" };
        options.push((format!("chord_{name}"), format!("Chord {name} ({}){why}", voicing.join(" ")), Slot::Chord(c)));
    }
    options.push(("hold".into(), "Keep what you are playing ringing".into(), Slot::Hold));
    options.push(("rest".into(), "Silence".into(), Slot::Rest));

    req.slot_count = (ctx.step_beats.round() as usize).max(1);
    for k in 0..req.slot_count {
        let beat = if req.slot_count == 1 {
            format!("the {window}")
        } else {
            format!("beat {} of the {window} (other beats are chosen separately)", k + 1)
        };
        req.questions.push((
            format!("beat_{}", k + 1),
            Question::Choice {
                instructions: format!(
                    "You are improvising an ambient melody with chords alongside the human in the state. \
                     What should you play on {beat}? Favour smooth melodic motion and space."
                ),
                options: options.iter().map(|(k, d, _)| (k.clone(), d.clone())).collect(),
            },
        ));
    }
    req.slot_options = options.into_iter().map(|(k, _, s)| (k, s)).collect();
    req.roles = vec![Role::Notes];
}

fn window_name(step: f64, beats_per_bar: u32) -> String {
    let bpb = beats_per_bar as f64;
    if step == bpb {
        "next bar".into()
    } else if step > bpb {
        format!("next {} bars", step / bpb)
    } else if step == 1.0 {
        "next beat".into()
    } else {
        format!("next {step} beats")
    }
}

/// Turn answers into a decision. With no answers (mock mode, or Jev failed) choices are random.
/// `variety` (0..1) discourages repeating the same role many steps in a row.
pub fn resolve(req: &Request, answers: Option<&Answers>, temperature: f64, variety: f64, rng: &mut Rng) -> Decision {
    if req.slot_count > 0 {
        return resolve_notes(req, answers, temperature, rng);
    }
    let mut note = String::new();

    let role_probs = distribution(answers, "role", req.roles.iter().map(|r| r.key()), rng);
    // Temperature first, then the repetition penalty, so sharpening can't undo it.
    // Each bar a role has already had in the last four, beyond the first, multiplies it by (1 - variety).
    let weights: Vec<f64> = req
        .roles
        .iter()
        .zip(&role_probs)
        .map(|(r, p)| {
            let used_bars = recent_use(&req.recent_roles, *r, req.step_bars);
            p.max(1e-9).powf(1.0 / temperature.max(0.05)) * (1.0 - variety).max(0.01).powf((used_bars - 1.0).max(0.0))
        })
        .collect();
    let i = rng.weighted(&weights, 1.0);
    let role = req.roles[i];
    note.push_str(&format!("{} {:.0}%", role.key(), role_probs[i] * 100.0));
    let jev_top = role_probs.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(j, _)| j);
    if answers.is_some() && jev_top != Some(i) {
        note.push_str(&format!(" (Jev preferred {})", req.roles[jev_top.unwrap_or(0)].key()));
    }

    let energy = match answers.and_then(|a| a.score("energy")) {
        Some(s) => (s + (rng.f64() - 0.5) * 0.4).clamp(0.0, 3.0),
        None => rng.f64() * 3.0,
    };

    let chord = if let Some(c) = req.human_chord {
        c
    } else if req.chords.is_empty() {
        req.robot_chord
    } else {
        let probs = distribution(answers, "next_chord", req.chords.iter().map(|(k, _)| k.as_str()), rng);
        let i = rng.weighted(&probs, temperature);
        note.push_str(&format!(", {} {:.0}%", req.chords[i].1.name(req.key), probs[i] * 100.0));
        req.chords[i].1
    };

    Decision { role, chord, energy, slots: Vec::new(), note }
}

fn resolve_notes(req: &Request, answers: Option<&Answers>, temperature: f64, rng: &mut Rng) -> Decision {
    let mut slots = Vec::with_capacity(req.slot_count);
    let mut picks = Vec::new();
    for k in 0..req.slot_count {
        let probs = distribution(answers, &format!("beat_{}", k + 1), req.slot_options.iter().map(|(k, _)| k.as_str()), rng);
        let i = rng.weighted(&probs, temperature);
        let (name, slot) = &req.slot_options[i];
        slots.push(*slot);
        picks.push(format!("{} {:.0}%", name.trim_start_matches("note_").trim_start_matches("chord_"), probs[i] * 100.0));
    }
    let chord = slots
        .iter()
        .rev()
        .find_map(|s| if let Slot::Chord(c) = s { Some(*c) } else { None })
        .or(req.human_chord)
        .unwrap_or(req.robot_chord);
    Decision { role: Role::Notes, chord, energy: 1.5, slots, note: picks.join(", ") }
}

/// A short name for what a notes-mode decision plays, e.g. "E4 · hold · Am7".
pub fn slots_label(slots: &[Slot], key: Key) -> String {
    slots
        .iter()
        .map(|s| match s {
            Slot::Note(n) => key.note_name(*n),
            Slot::Chord(c) => c.name(key),
            Slot::Hold => "~".into(),
            Slot::Rest => "-".into(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many of the last four bars the robot spent on `role`.
fn recent_use(recent: &[Role], role: Role, step_bars: f64) -> f64 {
    let steps = (4.0 / step_bars).ceil() as usize;
    recent.iter().rev().take(steps).filter(|r| **r == role).count() as f64 * step_bars
}

/// Probabilities for `keys` in order, from Jev when available, otherwise random.
fn distribution<'a>(answers: Option<&Answers>, question: &str, keys: impl Iterator<Item = &'a str>, rng: &mut Rng) -> Vec<f64> {
    let jev = answers.map(|a| a.probabilities(question)).unwrap_or_default();
    let probs: Vec<f64> = keys
        .map(|k| {
            if jev.is_empty() {
                rng.f64()
            } else {
                jev.iter().find(|(o, _)| o == k).map(|(_, p)| *p).unwrap_or(0.0)
            }
        })
        .collect();
    let total: f64 = probs.iter().sum();
    if total <= 0.0 {
        return vec![1.0 / probs.len() as f64; probs.len()];
    }
    probs.iter().map(|p| p / total).collect()
}
