mod brain;
mod decide;
mod gui;
mod jam;
mod jev;
mod jev_view;
mod listener;
mod midi_io;
mod progression;
mod render;
mod respond;
#[cfg(test)]
mod responsiveness;
mod rng;
mod theory;
mod viz;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use serde_json::json;

use jev::{Jev, Question};
use theory::Key;

#[derive(Parser)]
#[command(about = "Jam with Jev over MIDI: your keyboard in, you + a robot musician out to Reaper")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List MIDI input and output ports.
    Ports,
    /// Print what arrives on a MIDI input port (e.g. `monitor jamsesh`), to check routing.
    Monitor {
        port: String,
        #[arg(long, default_value_t = 30)]
        secs: u64,
    },
    /// Send a sample musical request to Jev and print the raw response.
    Probe,
    /// Start jamming.
    Jam(JamArgs),
}

#[derive(clap::Args)]
struct JamArgs {
    /// MIDI input (your keyboard); substring of the port name. Default: first non-loopback port.
    #[arg(short, long)]
    input: Option<String>,
    /// MIDI output going to Reaper; substring of the port name. Default: a port named jamsesh/loopMIDI. "none" = dry run.
    #[arg(short, long)]
    output: Option<String>,
    /// Tempo; change it live with +/-.
    #[arg(short, long, default_value_t = 70.0)]
    bpm: f64,
    #[arg(long, default_value_t = 4)]
    beats_per_bar: u32,
    /// Channel (1-16) your playing is passed through on.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=16))]
    human_channel: u8,
    /// Channel (1-16) the robot plays on.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=16))]
    robot_channel: u8,
    /// Channel (1-16) for the metronome click (GM woodblocks, notes 76/77).
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=16))]
    click_channel: u8,
    /// Play the click throughout (it always plays during the count-in); toggle live with c.
    #[arg(long)]
    click: bool,
    /// Lock the key instead of detecting it, e.g. "D minor".
    #[arg(short, long)]
    key: Option<String>,
    /// Random decisions instead of Jev (no network).
    #[arg(long)]
    mock: bool,
    /// Add a scripted human part, for trying it without the keyboard.
    #[arg(long)]
    demo: bool,
    /// Beats between decisions: 1 = every beat, 2 = every half bar; default one bar.
    #[arg(short, long)]
    step: Option<f64>,
    /// How many past steps (yours and the robot's) to describe to Jev.
    #[arg(long, default_value_t = 8)]
    history: usize,
    /// What the robot plays: ambient textures, bass lines, Jev-picked notes or chords, or a sustained pad.
    #[arg(short, long, value_enum, default_value_t = decide::Part::Ambient)]
    part: decide::Part,
    /// Shift the robot up or down this many octaves (-3..3); change live with [ and ].
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    octave: i32,
    /// Open the graphical piano-roll window (buttons for tempo, octave, click).
    #[arg(long)]
    gui: bool,
    /// Stop after this many bars.
    #[arg(long)]
    bars: Option<i64>,
    /// 0 = always take Jev's top pick; 1 = sample its probabilities as given.
    #[arg(long, default_value_t = 0.5)]
    temperature: f64,
    /// 0 = let the robot repeat a pattern forever; 1 = strongly push it to change after a few steps.
    #[arg(long, default_value_t = 0.7)]
    variety: f64,
    /// Append every Jev state/response to jev_log.jsonl.
    #[arg(long)]
    log_jev: bool,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Ports => midi_io::list_ports(),
        Cmd::Monitor { port, secs } => midi_io::monitor(&port, secs),
        Cmd::Probe => probe(),
        Cmd::Jam(a) => {
            let key = match &a.key {
                Some(k) => Some(Key::parse(k).ok_or_else(|| anyhow!("can't read key {k:?}; try \"D minor\""))?),
                None => None,
            };
            let step = a.step.unwrap_or(a.part.default_step(a.beats_per_bar));
            let bpb = a.beats_per_bar as f64;
            if !(step >= 0.5 && ((bpb / step).fract() == 0.0 || (step / bpb).fract() == 0.0)) {
                return Err(anyhow!("--step must divide the bar (e.g. 1, 2) or be whole bars (e.g. {}, {})", bpb, bpb * 2.0));
            }
            jam::run(jam::Config {
                input: a.input,
                output: a.output,
                bpm: a.bpm.clamp(30.0, 240.0),
                beats_per_bar: a.beats_per_bar,
                human_channel: a.human_channel - 1,
                robot_channel: a.robot_channel - 1,
                click_channel: a.click_channel - 1,
                click: a.click,
                key,
                mock: a.mock,
                demo: a.demo,
                bars: a.bars,
                step_beats: step,
                step_explicit: a.step.is_some(),
                history: a.history.clamp(1, 32),
                temperature: a.temperature.clamp(0.0, 2.0),
                variety: a.variety.clamp(0.0, 1.0),
                log_jev: a.log_jev,
                part: a.part,
                octave: a.octave.clamp(-3, 3),
                gui: a.gui,
            })
        }
    }
}

fn probe() -> Result<()> {
    let jev = Jev::from_env()?;
    let state = json!({
        "style": "ambient",
        "key": "C major",
        "human_chords_recent": ["Cmaj7", "Am7", "Fmaj7"],
        "human_melody_last_bar": ["E5", "G5"],
    });
    let questions = [
        ("human_left_space", Question::Noul {
            instructions: "Is the human leaving space for a musical response?".into(),
            when_true: "Few notes, sustained or silent".into(),
            when_false: "Busy, actively playing".into(),
        }),
        ("next_chord", Question::Choice {
            instructions: "Which chord best continues this ambient progression?".into(),
            options: vec![
                ("Dm9".into(), "ii - D minor 9".into()),
                ("G6".into(), "V - G 6".into()),
                ("Cmaj7".into(), "I - C major 7".into()),
                ("Em7".into(), "iii - E minor 7".into()),
            ],
        }),
        ("energy", Question::Score {
            instructions: "How much energy should the robot play with next bar?".into(),
            levels: decide::ENERGY_LEVELS.iter().map(|s| s.to_string()).collect(),
        }),
    ];
    let a = jev.ask(&state, &questions)?;
    println!("latency: {:?}", a.latency);
    println!("{}", serde_json::to_string_pretty(&a.raw)?);
    println!(
        "parsed: left_space={:?} next_chord={:?} probs={:?} energy={:?}",
        a.noul("human_left_space"),
        a.choice("next_chord"),
        a.probabilities("next_chord"),
        a.score("energy")
    );
    Ok(())
}
