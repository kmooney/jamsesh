//! Does Jev respond to the human? Sends Jev the requests the jam would build for contrasting
//! scripted human parts (robot history fixed), then for contrasting robot histories (human
//! fixed), and measures how far its answers move in each case. Hits the real API, so it's
//! ignored by default:
//!
//!     cargo test responsiveness -- --ignored --nocapture

use crate::decide::{self, Context, Part, Role, RobotStep};
use crate::jev::Jev;
use crate::listener::{Listener, Summary};
use crate::progression::Progression;
use crate::rng::Rng;
use crate::theory::{Chord, Key, Mode, Quality};

const BARS: f64 = 5.0;
const BPB: f64 = 4.0;

/// (start beat, note, velocity, length)
type Script = Vec<(f64, u8, u8, f64)>;

fn human_scripts() -> Vec<(&'static str, Script)> {
    let mut sparse_high = Script::new();
    for (i, n) in [88, 91, 84, 86, 88].iter().enumerate() {
        sparse_high.push((i as f64 * 4.0, *n, 35, 3.5));
    }
    let mut busy_low_loud = Script::new();
    let run = [48, 51, 53, 55, 58, 55, 53, 51];
    for i in 0..(BARS * BPB * 2.0) as usize {
        busy_low_loud.push((i as f64 * 0.5, run[i % run.len()], 110, 0.45));
    }
    let mut chords = Script::new();
    for (bar, chord) in [[45, 52, 55, 60], [41, 48, 52, 57], [48, 55, 59, 64], [43, 50, 55, 59], [45, 52, 55, 60]].iter().enumerate() {
        for &n in chord {
            chords.push((bar as f64 * 4.0, n, 70, 3.9));
        }
    }
    let mut phrase_then_space = Script::new();
    let phrase = [72, 74, 76, 79, 76, 74, 72, 69, 72, 74, 76, 77];
    for (i, n) in phrase.iter().enumerate() {
        phrase_then_space.push((i as f64, *n, 85, 0.9));
    }
    vec![
        ("silent", Script::new()),
        ("sparse soft high notes", sparse_high),
        ("busy loud low run", busy_low_loud),
        ("chords Am F C G", chords),
        ("phrase, then 2 bars of space", phrase_then_space),
    ]
}

fn robot_histories(part: Part) -> Vec<(&'static str, RobotStep)> {
    let c = Chord { root: 0, quality: Quality::Maj7 };
    let step = |role: Role, notes: Vec<u8>| RobotStep { role, notes };
    match part {
        Part::Chords | Part::Notes | Part::Pad => vec![
            ("robot kept striking Cmaj7", step(Role::Chords, c.open_voicing(48))),
            ("robot kept playing G7", step(Role::Chords, Chord { root: 7, quality: Quality::Dom7 }.open_voicing(43))),
            ("robot resting", step(Role::Rest, vec![])),
        ],
        _ => vec![
            ("robot pad Cmaj7", step(Role::Pad, c.open_voicing(57))),
            ("robot melody", step(Role::Melody, vec![79, 76, 74])),
            ("robot resting", step(Role::Rest, vec![])),
        ],
    }
}

/// The request the jam would send three quarters of the way through the last window.
fn request(part: Part, script: &Script, robot: &RobotStep) -> decide::Request {
    let mut l = Listener::default();
    for &(start, note, vel, len) in script {
        l.note_on(note, vel, start);
        l.note_off(note, start + len);
    }
    let step = part.default_step(BPB as u32);
    let end = BARS * BPB;
    let window_start = end;
    let current_start = end - step;
    let mut progression = Progression::default();
    for b in 0..(current_start as usize) {
        progression.hear(b as f64, l.summarize(b as f64, b as f64 + 1.0).chord);
    }
    let windows = ((current_start / step) as usize).min(8);
    let timeline: Vec<(Summary, Option<RobotStep>)> = (0..windows)
        .rev()
        .map(|k| {
            let from = current_start - (k + 1) as f64 * step;
            (l.summarize(from, from + step), Some(robot.clone()))
        })
        .collect();
    let current = l.summarize(current_start, current_start + step * 0.75);
    let pitches: Vec<f64> = timeline.iter().filter_map(|(s, _)| s.mean_pitch).collect();
    let human_center = if pitches.is_empty() { 60.0 } else { pitches.iter().sum::<f64>() / pitches.len() as f64 };
    decide::build(&Context {
        key: Key { tonic: 0, mode: Mode::Major },
        key_known: true,
        bpm: 80.0,
        beats_per_bar: BPB as u32,
        step_beats: step,
        timeline,
        current: &current,
        robot_now: Some(robot.clone()),
        robot_chord: Chord { root: 0, quality: Quality::Maj7 },
        last_note: Some(76),
        human_center,
        part,
        progression: &progression,
        window_start,
        heard: step * 0.75,
        prior: None,
    })
}

/// Jev's raw distribution per question, plus (as "final <question>") the robot's weighted
/// distributions after the response plan, and the plan itself as text.
fn ask(jev: &Jev, req: &decide::Request) -> (Answered, String) {
    let a = jev.ask(&req.state, &req.questions).expect("Jev answered");
    let mut out: Answered = req
        .questions
        .iter()
        .map(|(name, _)| {
            let probs = a.probabilities(name);
            // A noul answer has no distribution; chart it as yes/no.
            let probs = if probs.is_empty() { a.noul(name).map(|p| vec![("yes".into(), p), ("no".into(), 1.0 - p)]).unwrap_or_default() } else { probs };
            (name.clone(), probs)
        })
        .collect();
    let w = decide::weigh(req, Some(&a), 1.0, 0.7, &mut Rng::from_time());
    out.extend(w.dists.into_iter().map(|(n, d)| (format!("final {n}"), d)));
    (out, crate::respond::describe(&w.perception, &w.plan))
}

/// Total variation distance: 0 = identical, 1 = no overlap.
fn tvd(a: &[(String, f64)], b: &[(String, f64)]) -> f64 {
    let mut keys: Vec<&String> = a.iter().chain(b).map(|(k, _)| k).collect();
    keys.sort();
    keys.dedup();
    let p = |v: &[(String, f64)], k: &String| v.iter().find(|(x, _)| x == k).map(|(_, p)| *p).unwrap_or(0.0);
    keys.iter().map(|k| (p(a, k) - p(b, k)).abs()).sum::<f64>() / 2.0
}

type Answered = Vec<(String, Vec<(String, f64)>)>;

fn dist<'a>(a: &'a Answered, question: &str) -> Option<&'a [(String, f64)]> {
    a.iter().find(|(n, _)| n == question).map(|(_, d)| d.as_slice())
}

/// Mean distance between every pair of requests that asked `question`.
fn mean_pairwise(answers: &[Answered], question: &str) -> f64 {
    let asked: Vec<_> = answers.iter().filter_map(|a| dist(a, question)).collect();
    let mut total = 0.0;
    let mut n = 0;
    for i in 0..asked.len() {
        for j in i + 1..asked.len() {
            total += tvd(asked[i], asked[j]);
            n += 1;
        }
    }
    total / n.max(1) as f64
}

fn top(dist: &[(String, f64)]) -> String {
    let mut d = dist.to_vec();
    d.sort_by(|a, b| b.1.total_cmp(&a.1));
    d.iter().take(3).map(|(k, p)| format!("{k} {:.0}%", p * 100.0)).collect::<Vec<_>>().join(", ")
}

#[test]
#[ignore = "calls the Jev API"]
fn responsiveness() {
    let jev = Jev::from_env().expect("OPENROUTER_API_KEY");
    for part in [Part::Ambient, Part::Bass, Part::Chords, Part::Notes, Part::Pad] {
        println!("\n=== {} ===", part.name());
        let robots = robot_histories(part);
        let humans = human_scripts();

        let (by_human, human_plans): (Vec<_>, Vec<_>) = humans.iter().map(|(_, s)| ask(&jev, &request(part, s, &robots[0].1))).unzip();
        let fixed_human = &humans[1].1;
        let (by_robot, robot_plans): (Vec<_>, Vec<_>) = robots.iter().map(|(_, r)| ask(&jev, &request(part, fixed_human, r))).unzip();
        let (again, _) = ask(&jev, &request(part, &humans[0].1, &robots[0].1));
        println!("-- response plan");
        for ((label, _), plan) in humans.iter().zip(&human_plans) {
            println!("   human {label:<30} {plan}");
        }
        for ((label, _), plan) in robots.iter().zip(&robot_plans) {
            println!("   {label:<36} {plan}");
        }

        let mut names: Vec<String> = by_human.iter().chain(&by_robot).flat_map(|a| a.iter().map(|(n, _)| n.clone())).collect();
        names.sort();
        names.dedup();
        for name in &names {
            println!("-- {name}");
            let show = |a: &Answered| dist(a, name).map(top).unwrap_or_else(|| "(not asked)".into());
            for ((label, _), a) in humans.iter().zip(&by_human) {
                println!("   human {label:<30} {}", show(a));
            }
            for ((label, _), a) in robots.iter().zip(&by_robot) {
                println!("   {label:<36} {}", show(a));
            }
            let twice = match (dist(&by_human[0], name), dist(&again, name)) {
                (Some(a), Some(b)) => format!("{:.2}", tvd(a, b)),
                _ => "-".into(),
            };
            println!(
                "   movement: human varied {:.2} | robot history varied {:.2} | same request twice {twice}",
                mean_pairwise(&by_human, name),
                mean_pairwise(&by_robot, name),
            );
        }
    }
}
