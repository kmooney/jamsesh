//! Builds Jev's options from the musical context and turns its answers into a decision.
//! Code decides *which* options are sensible; Jev picks among them.

use serde_json::{Value, json};

use crate::jev::{Answers, Question};
use crate::listener::{Playing, Summary};
use crate::progression::{Prediction, Progression};
use crate::respond::{self, Perception, Plan, Stance};
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
    /// Chords mode: Jev picked each beat's chord.
    Chords,
}

/// What the robot is: an ambient texture player, the bass player, a player whose every
/// note/chord (or, in chords mode, every chord) is picked by Jev, or a sustained pad whose
/// harmony Jev picks beat by beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    Ambient,
    Bass,
    Notes,
    Chords,
    Pad,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Ambient => "ambient",
            Part::Bass => "bass",
            Part::Notes => "notes",
            Part::Chords => "chords",
            Part::Pad => "pad",
        }
    }

    /// Beats per decision unless the user chose: notes, chords and pad go one beat at a time
    /// (so Jev hears its own last pick, and chord changes land on the right beat); the others
    /// decide once a bar.
    pub fn default_step(self, beats_per_bar: u32) -> f64 {
        match self {
            Part::Notes | Part::Chords | Part::Pad => 1.0,
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
            Role::Chords => "chords",
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
            Role::Chords => "Individually picked chords",
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
    /// Notes and chords modes: what to play on each beat of the window. Empty otherwise.
    pub slots: Vec<Slot>,
    /// Short human-readable account of how it was chosen, for the log.
    pub note: String,
    /// How the robot read the human for this decision (smoothed), carried into the next one.
    pub perception: Option<Perception>,
}

/// What the robot actually did in one window: its role (for repetition control) and notes
/// (named in the "hold" option).
#[derive(Clone, Debug)]
pub struct RobotStep {
    pub role: Role,
    /// Notes it started (or kept ringing) in the window.
    pub notes: Vec<u8>,
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
    /// The human's chord changes over the last several bars.
    pub progression: &'a Progression,
    /// Beat the window being decided starts on.
    pub window_start: f64,
    /// How far into the current window the human has been heard (the request goes out before
    /// it ends), in beats.
    pub heard: f64,
    /// The previous decision's reading of the human, to smooth this one with.
    pub prior: Option<Perception>,
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
    /// Code's own reading of the human, used where Jev doesn't answer.
    estimate: Perception,
    /// The robot has notes ringing (pad mode: "hold" keeps them, otherwise it stays silent).
    robot_sounding: bool,
    prior: Option<Perception>,
    step_beats: f64,
}

/// How a chord option relates to where the human's progression is heading, for its description.
/// The timing is spelled out both ways: Jev otherwise jumps to "what comes next" beats early.
fn anticipation(chord: Chord, prediction: Option<&Prediction>, key: Key) -> String {
    let Some(p) = prediction else { return String::new() };
    let due = p.change_in <= 0.5;
    let beats = p.change_in.round();
    if p.next.same_family(chord) {
        if due {
            format!(", where the human's repeating progression moves next (due now, after {})", p.current.name(key))
        } else {
            format!(
                ", the human's next chord but not yet: their pattern stays on {} for about {beats} more beats",
                p.current.name(key)
            )
        }
    } else if p.current.same_family(chord) {
        if due {
            format!(", but the human's pattern is due to move on to {} now", p.next.name(key))
        } else {
            format!(", where the human's pattern stays for about {beats} more beats")
        }
    } else {
        String::new()
    }
}

/// Melody range for the robot in notes mode: out of the human's way.
pub fn notes_range(human_center: f64) -> (u8, u8) {
    if human_center < 62.0 { (60, 84) } else { (48, 72) }
}

/// ", contains 3 of the human's 4 recent melody notes (E, G, B)" for a chord option.
fn melody_fit(chord: Chord, melody: &[u8], key: Key) -> String {
    if melody.is_empty() {
        return String::new();
    }
    let pcs = chord.pcs();
    let inside: Vec<&str> = melody.iter().filter(|pc| pcs.contains(pc)).map(|&pc| key.pc_name(pc)).collect();
    format!(", contains {} of the human's {} recent melody notes{}", inside.len(), melody.len(), match inside.is_empty() {
        true => String::new(),
        false => format!(" ({})", inside.join(", ")),
    })
}

pub fn build(ctx: &Context) -> Request {
    let key = ctx.key;
    let window = window_name(ctx.step_beats, ctx.beats_per_bar);
    let last_human = ctx.timeline.last().map(|(h, _)| h);
    let prediction = ctx.progression.predict(ctx.window_start);
    let mut human_chord = if ctx.current.playing.is_chordal() {
        ctx.current.chord.or_else(|| last_human.and_then(|s| s.chord))
    } else {
        None
    };
    // Ambient and bass simply follow the human's chords; when their progression is due to
    // change at the start of this window, follow it there instead of a window late.
    if !matches!(ctx.part, Part::Notes | Part::Chords | Part::Pad)
        && let (Some(h), Some(p)) = (human_chord, &prediction)
        && h.same_family(p.current)
        && p.change_in <= 0.5
    {
        human_chord = Some(p.next);
    }
    let recently_played =
        ctx.current.playing != Playing::Silent || last_human.is_some_and(|s| s.playing != Playing::Silent);

    // Only the human's past windows. The robot's own history used to be here too, and Jev's
    // answers followed it more than they followed the human; it still sees what it's playing now.
    let timeline: Vec<Value> = ctx.timeline.iter().map(|(h, _)| h.to_json(key)).collect();
    let mut windows: Vec<&Summary> = ctx.timeline.iter().map(|(h, _)| h).collect();
    windows.push(ctx.current);
    // Pitch classes of the human's recent melody notes (last two windows and this one), newest last.
    let mut melody: Vec<u8> = Vec::new();
    for s in windows.iter().rev().take(3).rev() {
        for h in &s.onsets {
            if !melody.contains(&(h.note % 12)) {
                melody.push(h.note % 12);
            }
        }
    }
    let state = json!({
        "style": "ambient improvisation",
        "tempo_bpm": ctx.bpm.round(),
        "meter": format!("{}/4", ctx.beats_per_bar),
        "window_length_beats": ctx.step_beats,
        "key": if ctx.key_known { key.name() } else { format!("not yet clear (assuming {})", key.name()) },
        "your_instrument": match ctx.part {
            Part::Ambient => "ambient textures",
            Part::Bass => "bass",
            Part::Notes => "melody and chords, chosen note by note",
            Part::Chords => "chords, chosen beat by beat",
            Part::Pad => "a sustained pad: long held chords under the human, changing only when the harmony moves",
        },
        "human_timeline_oldest_first": timeline,
        "human_activity": respond::activity_json(&windows, ctx.step_beats, ctx.heard, ctx.beats_per_bar as f64),
        "human_progression": ctx.progression.to_json(key, ctx.window_start),
        "human_this_window_so_far": ctx.current.to_json(key),
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
        estimate: respond::estimate(&windows, ctx.step_beats, ctx.heard),
        robot_sounding: ctx.robot_now.as_ref().is_some_and(|r| !r.notes.is_empty()),
        prior: ctx.prior,
        step_beats: ctx.step_beats,
    };

    if matches!(ctx.part, Part::Notes | Part::Chords | Part::Pad) {
        build_notes(ctx, &mut req, &window, prediction.as_ref(), &melody);
        req.questions.extend(respond::questions());
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
                "You are the bass player in an improvised jam with the human described in the state. \
                 Which bass pattern best fits what the human is playing for the {window}?"
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
                "You are improvising with the human described in the state. Which of these best fits \
                 what the human is playing right now, for the {window}?"
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
    req.roles = roles;

    if human_chord.is_none() {
        let stay_fit = melody_fit(ctx.robot_chord, &melody, key);
        let mut options = vec![("stay".to_string(), format!("Stay on {}{stay_fit}", ctx.robot_chord.name(key)))];
        req.chords.push(("stay".to_string(), ctx.robot_chord));
        for (numeral, chord) in key.diatonic_sevenths() {
            if chord != ctx.robot_chord {
                let name = chord.name(key);
                let ahead = anticipation(chord, prediction.as_ref(), key);
                let fit = melody_fit(chord, &melody, key);
                options.push((name.clone(), format!("Move to {name} ({numeral} in {}){fit}{ahead}", key.name())));
                req.chords.push((name, chord));
            }
        }
        req.questions.push((
            "next_chord".into(),
            Question::Choice {
                instructions: format!(
                    "Which chord should the accompaniment play for the {window}? Look for patterns in the \
                     harmony so far (human_progression and the timeline): if the chords repeat in a cycle, \
                     change when the pattern says the human will, rather than a window after hearing it (or \
                     before it's due). Otherwise let the human's melody lead: move when their recent notes \
                     fit another chord better than the current one."
                ),
                options,
            },
        ));
    }
    req.questions.extend(respond::questions());
    req
}

/// Notes mode: offer concrete notes near the robot's last note, and concrete chords, for every
/// beat of the window; Jev picks one option per beat. Chords mode offers only the chords; pad
/// mode offers moving the pad to a chord, keeping it, or letting it fade.
fn build_notes(ctx: &Context, req: &mut Request, window: &str, prediction: Option<&Prediction>, melody: &[u8]) {
    let pad = ctx.part == Part::Pad;
    let chords_only = matches!(ctx.part, Part::Chords | Part::Pad);
    let key = ctx.key;
    let (lo, hi) = notes_range(ctx.human_center);
    let last = ctx.last_note.unwrap_or((lo + hi) / 2);
    let harmony = req.human_chord.unwrap_or(ctx.robot_chord);
    let scale = key.scale();

    let mut options: Vec<(String, String, Slot)> = Vec::new();
    let from = last.saturating_sub(7).max(lo);
    let to = (last + 7).min(hi);
    for n in from..=to {
        if chords_only || !scale.contains(&(n % 12)) {
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

    // The key's chords, plus the human's if it's something else. A melody note colouring the
    // human's chord (Cmaj9, G6) doesn't make it a separate option: it would split Jev's vote
    // with Cmaj7 or G7 and make the robot flip between the two.
    let mut chords: Vec<Chord> = key.diatonic_sevenths().into_iter().map(|(_, c)| c).collect();
    if let Some(h) = req.human_chord
        && !chords.iter().any(|c| c.same_family(h))
    {
        chords.insert(0, h);
    }
    for c in chords {
        // A ringing pad staying on its chord is "hold".
        if pad && req.robot_sounding && c.same_family(ctx.robot_chord) {
            continue;
        }
        let name = c.name(key);
        let voicing: Vec<String> = c.open_voicing(lo - 12).iter().map(|&n| key.note_name(n)).collect();
        let why = if req.human_chord.is_some_and(|h| h.same_family(c)) {
            ", the chord the human is holding".to_string()
        } else if chords_only && c == ctx.robot_chord {
            ", strike your current chord again".to_string()
        } else {
            String::new()
        };
        let why = why + &melody_fit(c, melody, key) + &anticipation(c, prediction, key);
        let text = if pad { format!("Move the pad to {name}{why}") } else { format!("Chord {name} ({}){why}", voicing.join(" ")) };
        options.push((format!("chord_{name}"), text, Slot::Chord(c)));
    }
    let ringing = match ctx.robot_now.as_ref().map(|r| r.notes.as_slice()) {
        Some(notes) if !notes.is_empty() => {
            format!(" ({})", notes.iter().map(|&n| key.note_name(n)).collect::<Vec<_>>().join(" "))
        }
        _ => String::new(),
    };
    if pad {
        let hold = if req.robot_sounding {
            let fit = melody_fit(ctx.robot_chord, melody, key) + &anticipation(ctx.robot_chord, prediction, key);
            format!("Keep the pad on {}{fit}", ctx.robot_chord.name(key))
        } else {
            "Stay silent for now".to_string()
        };
        options.push(("hold".into(), hold, Slot::Hold));
        options.push(("rest".into(), "Let the pad fade out".into(), Slot::Rest));
    } else {
        options.push(("hold".into(), format!("Keep what you are playing ringing{ringing}"), Slot::Hold));
        options.push(("rest".into(), "Silence".into(), Slot::Rest));
    }

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
                instructions: if pad {
                    format!(
                        "You are playing a sustained pad under the human in the state: long, held chords \
                         that change only when the harmony moves. Study the human's chord changes \
                         (human_progression and the timeline) for patterns, and move the pad on the beat \
                         the pattern says the human will change, together with them: not after, not early. \
                         Without a pattern, let the human's melody lead: keep the chord while their notes \
                         fit it, move when they fit another better. What should the pad do on {beat}?"
                    )
                } else if chords_only {
                    format!(
                        "You are playing chord accompaniment alongside the human in the state. \
                         Study the human's chord changes (human_progression and the timeline) for patterns: \
                         a repeating cycle, how many beats each chord usually lasts. Change chords on the \
                         beat the pattern says the human will change, so you arrive on their next chord \
                         together with them: not a beat after, and not early. Without a pattern, let the \
                         human's melody lead: pick chords that contain their recent notes. What should you \
                         play on {beat}?"
                    )
                } else {
                    format!(
                        "You are improvising a melody with chords alongside the human in the state. \
                         Respond to what the human is playing, and use the patterns in their chord changes \
                         (human_progression) to anticipate the harmony coming next. What should you play \
                         on {beat}? Favour smooth melodic motion."
                    )
                },
                options: options.iter().map(|(k, d, _)| (k.clone(), d.clone())).collect(),
            },
        ));
    }
    req.slot_options = options.into_iter().map(|(k, _, s)| (k, s)).collect();
    req.roles = vec![if pad {
        Role::Pad
    } else if chords_only {
        Role::Chords
    } else {
        Role::Notes
    }];
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

/// Which roles suit the response, multiplied into Jev's probabilities. Jev's own role
/// picks are lopsided (often 99%+) and mirror the human (rest when they're silent, melody
/// when they're busy), so this is a filter: the stance decides which roles make sense, Jev
/// chooses among them. (Jev's probabilities are floored above zero, so an allowed role it
/// rated 0% can still be picked.)
fn role_fit(role: Role, plan: &Plan, bass: bool) -> f64 {
    use Role::*;
    const NO: f64 = 0.0;
    let by_stance = match (plan.stance, role) {
        // Fill the gap with melodic material, answering their phrase if possible.
        (Stance::Answer, Echo) => 1.5,
        (Stance::Answer, Melody | Arpeggio) => 1.0,
        (Stance::Answer, Pad | Drone | Rest) => NO,
        // Stay out of their way.
        (Stance::Support, Pad | Drone | Rest) => 1.0,
        (Stance::Support, Melody | Echo | Arpeggio) => NO,
        // Keep the music going on its own.
        (Stance::Carry, Melody | Arpeggio | Pad) => 1.0,
        (Stance::Carry, Echo | Drone | Rest) => NO,
        (Stance::Follow, Rest) => NO,
        _ => 1.0,
    };
    // Bass patterns: those whose busyness is near the planned energy (busier when answering).
    let busyness = match role {
        BassRoot => Some(0.4),
        BassRootFifth => Some(1.2),
        BassPulse => Some(1.7),
        BassOctave => Some(2.0),
        BassWalk => Some(2.3),
        BassSyncopated => Some(2.7),
        Rest if bass && plan.stance == Stance::Support => Some(0.0),
        Rest if bass => Some(-2.0),
        _ => None,
    };
    let target = plan.energy + if plan.stance == Stance::Answer { 0.8 } else { 0.0 };
    match busyness {
        Some(b) => 0.01 + (-(target - b).powi(2) / 0.4).exp(),
        None => by_stance,
    }
}

/// The same for notes/chords-mode beats: answer with notes (or a new strike), lay back with
/// holds and rests under a busy human, keep playing over a silent one.
fn slot_fit(slot: Slot, plan: &Plan, pad: bool) -> f64 {
    if pad {
        // A pad responds by moving its harmony or getting louder, rarely by stopping: mostly
        // it holds, moves more readily in the human's gaps, and only fades under a busy human.
        return match (plan.stance, slot) {
            (Stance::Answer, Slot::Chord(_)) => 1.5,
            (Stance::Support, Slot::Hold) => 1.5,
            (Stance::Support, Slot::Rest) => 0.3,
            (_, Slot::Rest) => 0.05,
            _ => 1.0,
        };
    }
    match (plan.stance, slot) {
        (Stance::Answer, Slot::Note(_)) => 3.0,
        (Stance::Answer, Slot::Chord(_)) => 1.5,
        (Stance::Answer, Slot::Hold) => 0.3,
        (Stance::Answer, Slot::Rest) => 0.05,
        (Stance::Support, Slot::Hold) => 2.0,
        (Stance::Support, Slot::Rest) => 1.5,
        (Stance::Support, Slot::Note(_)) => 0.15,
        (Stance::Carry, Slot::Note(_)) => 2.0,
        (Stance::Carry, Slot::Chord(_)) => 1.5,
        (Stance::Carry, Slot::Rest) => 0.1,
        (Stance::Follow, Slot::Rest) => 0.4,
        _ => 1.0,
    }
}

/// Before sampling: how the robot reads the human, its response, and the weighted
/// distribution for each of its own questions (probabilities, summing to 1).
pub struct Weighed {
    pub perception: Perception,
    pub plan: Plan,
    pub dists: Vec<(String, Vec<(String, f64)>)>,
}

fn normalize(w: Vec<f64>) -> Vec<f64> {
    let total: f64 = w.iter().sum();
    if total <= 0.0 { vec![1.0 / w.len().max(1) as f64; w.len()] } else { w.iter().map(|x| x / total).collect() }
}

/// Jev's probabilities sharpened/flattened by `temperature`, times the response weights (and,
/// for roles, the repetition penalty). Temperature goes first so it can't undo either.
pub fn weigh(req: &Request, answers: Option<&Answers>, temperature: f64, variety: f64, rng: &mut Rng) -> Weighed {
    let perception = respond::smooth(req.prior, respond::perceive(answers, req.estimate), req.step_beats);
    let plan = respond::plan(&perception);
    let temper = |p: f64| p.max(1e-9).powf(1.0 / temperature.max(0.05));
    let mut dists = Vec::new();
    if req.slot_count > 0 {
        for k in 0..req.slot_count {
            let name = format!("beat_{}", k + 1);
            let probs = distribution(answers, &name, req.slot_options.iter().map(|(k, _)| k.as_str()), rng);
            let pad = req.roles.first() == Some(&Role::Pad);
            let w = req.slot_options.iter().zip(&probs).map(|((_, s), p)| temper(*p) * slot_fit(*s, &plan, pad)).collect();
            dists.push((name, req.slot_options.iter().map(|(k, _)| k.clone()).zip(normalize(w)).collect()));
        }
    } else {
        let probs = distribution(answers, "role", req.roles.iter().map(|r| r.key()), rng);
        let bass = req.roles.contains(&Role::BassRoot);
        // Each bar a role has already had in the last four, beyond the first, multiplies it by (1 - variety).
        let w = req
            .roles
            .iter()
            .zip(&probs)
            .map(|(r, p)| {
                let used_bars = recent_use(&req.recent_roles, *r, req.step_bars);
                temper(*p) * role_fit(*r, &plan, bass) * (1.0 - variety).max(0.01).powf((used_bars - 1.0).max(0.0))
            })
            .collect();
        dists.push(("role".into(), req.roles.iter().map(|r| r.key().to_string()).zip(normalize(w)).collect()));
        if req.human_chord.is_none() && !req.chords.is_empty() {
            let probs = distribution(answers, "next_chord", req.chords.iter().map(|(k, _)| k.as_str()), rng);
            let w = probs.iter().map(|p| temper(*p)).collect();
            dists.push(("next_chord".into(), req.chords.iter().map(|(k, _)| k.clone()).zip(normalize(w)).collect()));
        }
    }
    Weighed { perception, plan, dists }
}

/// Turn answers into a decision. With no answers (mock mode, or Jev failed) Jev's choices are
/// random but the response to the human still comes from code's own reading of them.
/// `variety` (0..1) discourages repeating the same role many steps in a row.
pub fn resolve(req: &Request, answers: Option<&Answers>, temperature: f64, variety: f64, rng: &mut Rng) -> Decision {
    let w = weigh(req, answers, temperature, variety, rng);
    let heard = respond::describe(&w.perception, &w.plan);
    let pick = |rng: &mut Rng, name: &str| {
        let d = &w.dists.iter().find(|(n, _)| n == name).expect("weighed").1;
        let probs: Vec<f64> = d.iter().map(|(_, p)| *p).collect();
        rng.weighted(&probs, 1.0)
    };
    let jev_prob = |name: &str, key: &str| {
        answers.map(|a| a.probabilities(name)).unwrap_or_default().iter().find(|(k, _)| k == key).map(|(_, p)| *p)
    };

    if req.slot_count > 0 {
        let mut slots = Vec::with_capacity(req.slot_count);
        let mut picks = Vec::new();
        for k in 0..req.slot_count {
            let name = format!("beat_{}", k + 1);
            let (key, slot) = &req.slot_options[pick(rng, &name)];
            slots.push(*slot);
            let jev = jev_prob(&name, key).map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
            picks.push(format!("{}{jev}", key.trim_start_matches("note_").trim_start_matches("chord_")));
        }
        let chord = slots
            .iter()
            .rev()
            .find_map(|s| if let Slot::Chord(c) = s { Some(*c) } else { None })
            .or(req.human_chord)
            .unwrap_or(req.robot_chord);
        let note = format!("{} · {heard}", picks.join(", "));
        if req.roles[0] == Role::Pad {
            // The pad is one sustained chord, not per-beat slots; "hold" keeps its own chord.
            let (role, chord) = match slots.last() {
                Some(Slot::Chord(c)) => (Role::Pad, *c),
                Some(Slot::Hold) if req.robot_sounding => (Role::Pad, req.robot_chord),
                _ => (Role::Rest, req.robot_chord),
            };
            return Decision { role, chord, energy: w.plan.energy, slots: Vec::new(), note, perception: Some(w.perception) };
        }
        return Decision { role: req.roles[0], chord, energy: w.plan.energy, slots, note, perception: Some(w.perception) };
    }

    let i = pick(rng, "role");
    let role = req.roles[i];
    let mut note = role.key().to_string();
    if let Some(p) = jev_prob("role", role.key()) {
        note.push_str(&format!(" {:.0}%", p * 100.0));
    }
    let jev_top = answers
        .map(|a| a.probabilities("role"))
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| req.roles.iter().any(|r| r.key() == k))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((top, _)) = jev_top
        && top != role.key()
    {
        note.push_str(&format!(" (Jev preferred {top})"));
    }

    let chord = if let Some(c) = req.human_chord {
        c
    } else if req.chords.is_empty() {
        req.robot_chord
    } else {
        let i = pick(rng, "next_chord");
        let (key, chord) = &req.chords[i];
        let jev = jev_prob("next_chord", key).map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
        note.push_str(&format!(", {}{jev}", chord.name(req.key)));
        *chord
    };
    let energy = (w.plan.energy + (rng.f64() - 0.5) * 0.3).clamp(0.0, 3.0);
    Decision { role, chord, energy, slots: Vec::new(), note: format!("{note} · {heard}"), perception: Some(w.perception) }
}

/// A short name for what a notes- or chords-mode decision plays, e.g. "E4 · hold · Am7".
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{Mode, Quality};

    fn context<'a>(part: Part, current: &'a Summary, progression: &'a Progression) -> Context<'a> {
        Context {
            key: Key { tonic: 0, mode: Mode::Major },
            key_known: true,
            bpm: 70.0,
            beats_per_bar: 4,
            step_beats: part.default_step(4),
            timeline: Vec::new(),
            current,
            robot_now: None,
            robot_chord: Chord { root: 0, quality: Quality::Maj7 },
            last_note: Some(64),
            human_center: 60.0,
            part,
            progression,
            window_start: 16.0,
            heard: 0.75,
            prior: None,
        }
    }

    #[test]
    fn chords_mode_offers_only_chords_per_beat() {
        let current = Summary { onsets: Vec::new(), hist: [0.0; 12], playing: Playing::Silent, chord: None, avg_vel: None, mean_pitch: None };
        let progression = Progression::default();
        let req = build(&context(Part::Chords, &current, &progression));
        assert_eq!(req.slot_count, 1);
        assert!(req.slot_options.iter().all(|(_, s)| !matches!(s, Slot::Note(_))));
        assert!(req.slot_options.iter().filter(|(_, s)| matches!(s, Slot::Chord(_))).count() == 6);
        let d = resolve(&req, None, 0.5, 0.7, &mut Rng::from_time());
        assert_eq!(d.role, Role::Chords);

        let notes = build(&context(Part::Notes, &current, &progression));
        assert!(notes.slot_options.iter().any(|(_, s)| matches!(s, Slot::Note(_))));
    }

    #[test]
    fn pad_holds_its_own_chord_moves_or_fades() {
        let current = Summary { onsets: Vec::new(), hist: [0.0; 12], playing: Playing::Silent, chord: None, avg_vel: None, mean_pitch: None };
        let progression = Progression::default();
        let mut ctx = context(Part::Pad, &current, &progression);
        ctx.robot_now = Some(RobotStep { role: Role::Pad, notes: vec![48, 55, 64, 71] });
        let req = build(&ctx);
        let keys: Vec<&str> = req.slot_options.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"chord_Cmaj7"), "the ringing chord is 'hold', not a separate option: {keys:?}");
        assert!(keys.contains(&"hold") && keys.contains(&"rest") && keys.contains(&"chord_Am7"));
        assert!(req.slot_options.iter().all(|(_, s)| !matches!(s, Slot::Note(_))));
        let mut rng = Rng::from_time();
        for _ in 0..30 {
            let d = resolve(&req, None, 0.5, 0.0, &mut rng);
            assert!(d.slots.is_empty());
            assert!(matches!(d.role, Role::Pad | Role::Rest), "{:?}", d.role);
        }
    }

    /// C Am F G twice, four beats each, then C for three beats: Am is due at beat 36.
    fn cycling() -> Progression {
        let mut p = Progression::default();
        let chords = [0, 9, 5, 7, 0, 9, 5, 7, 0].map(|root| Chord { root, quality: if root == 9 { Quality::Min7 } else { Quality::Maj7 } });
        for (i, c) in chords.iter().enumerate() {
            let beats = if i == 8 { 3 } else { 4 };
            for b in 0..beats {
                p.hear((i * 4 + b) as f64, Some(*c));
            }
        }
        p
    }

    #[test]
    fn chord_options_point_at_the_humans_next_chord() {
        let current = Summary { onsets: Vec::new(), hist: [0.0; 12], playing: Playing::Silent, chord: None, avg_vel: None, mean_pitch: None };
        let progression = cycling();
        let mut ctx = context(Part::Chords, &current, &progression);
        ctx.window_start = 36.0;
        let req = build(&ctx);
        let Question::Choice { options, .. } = &req.questions[0].1 else { panic!("choice") };
        let am = options.iter().find(|(k, _)| k == "chord_Am7").expect("Am7 offered");
        assert!(am.1.contains("moves next (due now, after Cmaj7)"), "{}", am.1);
        assert!(req.state["human_progression"]["pattern"]["expectation"].as_str().unwrap().contains("due now"));
    }

    /// Beats 0..16 of `script` as one-bar windows: four done, the fifth in progress.
    fn ambient_request(script: &[(f64, u8, u8, f64)]) -> Request {
        let mut l = crate::listener::Listener::default();
        for &(start, note, vel, len) in script {
            l.note_on(note, vel, start);
            l.note_off(note, start + len);
        }
        let current = l.summarize(16.0, 19.0);
        let progression = Progression::default();
        let mut ctx = context(Part::Ambient, &current, &progression);
        ctx.step_beats = 4.0;
        ctx.window_start = 20.0;
        ctx.heard = 3.0;
        ctx.timeline = (0..4).map(|b| (l.summarize(b as f64 * 4.0, b as f64 * 4.0 + 4.0), None)).collect();
        build(&ctx)
    }

    #[test]
    fn the_robot_responds_to_the_human_not_the_other_way_round() {
        let mut rng = Rng::from_time();
        let roles = |req: &Request, rng: &mut Rng| -> Vec<Role> { (0..40).map(|_| resolve(req, None, 0.5, 0.0, rng).role).collect() };

        let busy: Vec<_> = (0..40).map(|i| (i as f64 * 0.5, 48 + (i % 7) as u8, 110, 0.45)).collect();
        let r = roles(&ambient_request(&busy), &mut rng);
        assert!(r.iter().all(|r| matches!(r, Role::Pad | Role::Drone | Role::Rest)), "lay back under a busy human: {r:?}");

        let r = roles(&ambient_request(&[]), &mut rng);
        assert!(r.iter().all(|r| matches!(r, Role::Melody | Role::Arpeggio | Role::Pad)), "carry a silent one: {r:?}");

        // A phrase in the first three bars, then nothing.
        let phrase: Vec<_> = (0..12).map(|i| (i as f64, 72 + (i % 5) as u8, 85, 0.9)).collect();
        let r = roles(&ambient_request(&phrase), &mut rng);
        assert!(r.iter().all(|r| matches!(r, Role::Echo | Role::Melody | Role::Arpeggio)), "answer the phrase: {r:?}");
    }

    #[test]
    fn ambient_follows_the_humans_progression_on_time() {
        let current = Summary {
            onsets: Vec::new(),
            hist: [0.0; 12],
            playing: Playing::Chords,
            chord: Some(Chord { root: 0, quality: Quality::Maj7 }),
            avg_vel: None,
            mean_pitch: None,
        };
        let progression = cycling();
        let mut ctx = context(Part::Ambient, &current, &progression);
        ctx.window_start = 36.0;
        let d = resolve(&build(&ctx), None, 0.5, 0.7, &mut Rng::from_time());
        assert_eq!(d.chord, Chord { root: 9, quality: Quality::Min7 });
        // Two beats earlier the change isn't due yet: stay with the human.
        ctx.window_start = 34.0;
        let d = resolve(&build(&ctx), None, 0.5, 0.7, &mut Rng::from_time());
        assert_eq!(d.chord, Chord { root: 0, quality: Quality::Maj7 });
    }
}
