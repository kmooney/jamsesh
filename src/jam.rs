//! The jam session: clock, scheduler, bar-by-bar decisions.
//!
//! Timeline per bar N: the human plays; about one Jev round-trip before bar N ends we
//! summarize what we've heard and ask Jev; its answer is rendered into bar N+1. If the
//! answer is late the robot carries on with its previous decision.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io::{Write as _, stdout};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use midir::{Ignore, MidiInput, MidiOutput};

use crate::decide::{self, Context, Decision, Request, RobotStep, Role, Slot};
use crate::jev::{Answers, Jev};
use crate::listener::{Listener, Summary};
use crate::midi_io::{Out, find_input, find_output};
use crate::progression::Progression;
use crate::render::{BarInput, Msg, Robot};
use crate::rng::Rng;
use crate::theory::{Key, Mode, rank_keys};
use crate::viz::{Command, Viz, Who};

pub struct Config {
    pub input: Option<String>,
    pub output: Option<String>,
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub human_channel: u8,
    pub robot_channel: u8,
    pub click_channel: u8,
    pub click: bool,
    pub key: Option<Key>,
    pub mock: bool,
    pub demo: bool,
    pub bars: Option<i64>,
    /// Beats per decision window.
    pub step_beats: f64,
    /// The user set --step; keep it when the part changes.
    pub step_explicit: bool,
    /// How many past windows to describe to Jev.
    pub history: usize,
    pub temperature: f64,
    pub variety: f64,
    pub log_jev: bool,
    pub part: decide::Part,
    /// Starting octave shift for the robot.
    pub octave: i32,
    /// Open the graphical window instead of driving from the terminal.
    pub gui: bool,
}

macro_rules! say {
    ($($arg:tt)*) => {{
        print!("{}\r\n", format!($($arg)*));
        let _ = stdout().flush();
    }};
}

/// Beat position that follows tempo changes smoothly.
struct Clock {
    anchor: Instant,
    anchor_beat: f64,
    bpm: f64,
}

impl Clock {
    fn beat_at(&self, t: Instant) -> f64 {
        self.anchor_beat + t.saturating_duration_since(self.anchor).as_secs_f64() * self.bpm / 60.0
    }

    fn now(&self) -> f64 {
        self.beat_at(Instant::now())
    }

    fn set_bpm(&mut self, bpm: f64) {
        let now = Instant::now();
        self.anchor_beat = self.beat_at(now);
        self.anchor = now;
        self.bpm = bpm;
    }

    fn secs_until(&self, beat: f64) -> f64 {
        (beat - self.now()) * 60.0 / self.bpm
    }
}

enum Kind {
    Robot(Msg),
    Human([u8; 3]),
    Click(u8, u8),
}

struct Item {
    beat: f64,
    seq: u64,
    kind: Kind,
}

impl PartialEq for Item {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Item {
    // Reversed: BinaryHeap pops the earliest event first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.beat.total_cmp(&self.beat).then(other.seq.cmp(&self.seq))
    }
}

pub struct Shared {
    out: Mutex<Out>,
    listener: Mutex<Listener>,
    clock: Mutex<Clock>,
    viz: Mutex<Viz>,
    /// Note-ons heard from the human so far, so the jam loop can ask Jev again when there's
    /// new input.
    notes_heard: AtomicU64,
}

/// What the GUI reads.
impl Shared {
    pub fn now(&self) -> f64 {
        self.clock.lock().unwrap().now()
    }

    pub fn bpm(&self) -> f64 {
        self.clock.lock().unwrap().bpm
    }

    pub fn viz(&self) -> std::sync::MutexGuard<'_, Viz> {
        self.viz.lock().unwrap()
    }
}

/// A note from the keyboard (or the demo player): pass it through to Reaper and remember it.
fn handle_human(shared: &Shared, raw: &[u8]) {
    if raw.len() < 2 {
        return;
    }
    shared.out.lock().unwrap().human(raw);
    let beat = shared.clock.lock().unwrap().now();
    let mut l = shared.listener.lock().unwrap();
    let mut viz = shared.viz.lock().unwrap();
    let data2 = raw.get(2).copied().unwrap_or(0);
    match raw[0] & 0xF0 {
        0x90 if data2 > 0 => {
            shared.notes_heard.fetch_add(1, AtomicOrdering::Relaxed);
            l.note_on(raw[1], data2, beat);
            viz.note_on(Who::Human, raw[1], beat);
        }
        0x80 | 0x90 => {
            l.note_off(raw[1], beat);
            viz.note_off(Who::Human, raw[1], beat);
        }
        0xB0 if raw[1] == 64 => l.sustain(data2 >= 64, beat),
        _ => {}
    }
}

struct Pending {
    step: i64,
    /// Which request this is for its step (1 = the first); later ones saw more of the human.
    ask: u32,
    /// Beat the request was sent on: it heard the human up to here.
    sent: f64,
    /// Answers from before a part change are dropped.
    generation: u64,
    request: Request,
    result: Result<Answers>,
}

#[derive(Clone)]
struct KeyTracker {
    hist: [f64; 12],
    key: Key,
    locked: bool,
    /// Bars' worth of playing heard; the key isn't trusted until there are a few.
    bars_heard: f64,
}

impl KeyTracker {
    fn known(&self) -> bool {
        self.locked || self.bars_heard >= 3.0
    }

    /// Add a window of `bars` bars' length. Returns Some on a key change.
    fn update(&mut self, window: &[f64; 12], bars: f64) -> Option<Key> {
        let decay = 0.93f64.powf(bars);
        for (h, w) in self.hist.iter_mut().zip(window) {
            *h = *h * decay + w;
        }
        if window.iter().sum::<f64>() > 0.0 {
            self.bars_heard += bars;
        }
        if self.locked || !self.known() {
            return None;
        }
        let ranked = rank_keys(&self.hist);
        let (best, best_r) = ranked[0];
        let current_r = ranked.iter().find(|(k, _)| *k == self.key).map(|(_, r)| *r).unwrap_or(0.0);
        if best != self.key && best_r - current_r > 0.15 {
            self.key = best;
            return Some(best);
        }
        None
    }
}

struct RawMode(bool);

impl RawMode {
    fn enable() -> Self {
        RawMode(crossterm::terminal::enable_raw_mode().is_ok())
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if self.0 {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

pub fn run(cfg: Config) -> Result<()> {
    if cfg.beats_per_bar < 2 {
        return Err(anyhow!("need at least 2 beats per bar"));
    }
    let jev = if cfg.mock { None } else { Some(Arc::new(Jev::from_env()?)) };

    let dry_run = cfg.output.as_deref() == Some("none");
    let (out, out_name) = if dry_run {
        (Out::connect(None, cfg.human_channel, cfg.robot_channel, cfg.click_channel)?, "none (dry run)".to_string())
    } else {
        let midi_out = MidiOutput::new("jamsesh")?;
        let (port, name) = find_output(&midi_out, cfg.output.as_deref())?;
        (Out::connect(Some((midi_out, port)), cfg.human_channel, cfg.robot_channel, cfg.click_channel)?, name)
    };

    let bpb = cfg.beats_per_bar as f64;
    let shared = Arc::new(Shared {
        out: Mutex::new(out),
        listener: Mutex::new(Listener::default()),
        // Start one bar early: that bar is the count-in.
        clock: Mutex::new(Clock { anchor: Instant::now(), anchor_beat: -bpb, bpm: cfg.bpm }),
        viz: Mutex::new(Viz::default()),
        notes_heard: AtomicU64::new(0),
    });

    let mut midi_in = MidiInput::new("jamsesh")?;
    midi_in.ignore(Ignore::All);
    let _input_conn = match find_input(&midi_in, cfg.input.as_deref(), &out_name) {
        Ok((port, in_name)) => {
            let s = shared.clone();
            let conn = midi_in
                .connect(&port, "jamsesh-in", move |_, raw, _| handle_human(&s, raw), ())
                .map_err(|e| anyhow!("could not open MIDI input {in_name}: {e} (is Reaper using it? disable it there)"))?;
            say!("input:  {in_name}  → thru on ch {}", cfg.human_channel + 1);
            Some(conn)
        }
        Err(e) if cfg.demo => {
            say!("input:  none ({e}); demo player only");
            None
        }
        Err(e) => return Err(e),
    };
    say!("output: {out_name}  (you ch {}, robot ch {}, click ch {})", cfg.human_channel + 1, cfg.robot_channel + 1, cfg.click_channel + 1);
    say!("robot:   {} (switch with 1 ambient, 2 bass, 3 notes, 4 chords, 5 pad)", cfg.part.name());
    say!("decider: {}", if cfg.mock { "mock (random)" } else { "Jev via OpenRouter" });

    let (cmd_tx, cmd_rx) = channel();
    let result = if cfg.gui {
        // The window must own the main thread, so the jam runs beside it.
        let beats_per_bar = cfg.beats_per_bar;
        let mut session = Session::new(cfg, shared.clone(), jev, cmd_rx, false);
        let jam = thread::spawn(move || session.run_loop());
        crate::gui::run(shared.clone(), cmd_tx.clone(), beats_per_bar);
        let _ = cmd_tx.send(Command::Quit);
        jam.join().map_err(|_| anyhow!("jam thread panicked"))?
    } else {
        let _raw = RawMode::enable();
        say!("keys: +/- tempo, [ ] robot octave, 1/2/3 part, c click on/off, b beat lines on/off, q quit\r\n");
        let _keep_sender = cmd_tx;
        Session::new(cfg, shared.clone(), jev, cmd_rx, true).run_loop()
    };
    shared.out.lock().unwrap().all_off();
    let now = shared.now();
    shared.viz().all_off(now);
    result
}

struct Session {
    cfg: Config,
    shared: Arc<Shared>,
    jev: Option<Arc<Jev>>,
    tx: Sender<Pending>,
    rx: Receiver<Pending>,
    queue: BinaryHeap<Item>,
    seq: u64,
    rng: Rng,
    robot: Robot,
    keys: KeyTracker,
    /// The human's completed windows, by start beat.
    history: Vec<(f64, Summary)>,
    /// The human's chord changes, beat by beat, over a longer span than `history`.
    progression: Progression,
    /// What the robot played in each recent window, by start beat (so both line up even
    /// after the step length changes).
    robot_steps: Vec<(f64, RobotStep)>,
    /// A part change requested for the next bar line.
    pending_part: Option<decide::Part>,
    /// Bumped on a part change so stale Jev answers are ignored.
    generation: u64,
    /// The first step after a part change has no decision yet.
    switching: bool,
    /// The robot's latest melody note in notes mode (before octave shift).
    last_note: Option<u8>,
    /// The latest decision's reading of the human, which the next one is smoothed with.
    last_perception: Option<crate::respond::Perception>,
    /// Per step: the newest answer so far (its ask number), the decision and the exchange for the panel.
    decisions: HashMap<i64, (u32, Decision, crate::jev_view::Exchange)>,
    last_decision: Decision,
    /// The step requests are currently for, how many have been sent for it, and whether the
    /// deadline request has gone out.
    target: i64,
    asks: u32,
    deadline_sent: bool,
    /// Requests on their way. New input while one is out sets `stale`, to ask again when it's back.
    inflight: u32,
    stale: bool,
    notes_seen: u64,
    /// Jev's round trip in seconds: running mean and mean deviation.
    latency_ema: f64,
    latency_dev: f64,
    click: bool,
    beat_log: bool,
    octave: i32,
    /// Robot note-ons already sent, with their beat, for the per-beat log.
    robot_played: Vec<(f64, u8)>,
    jev_log: Option<File>,
    commands: Receiver<Command>,
    /// Read keys from the terminal (off when the GUI is driving).
    terminal: bool,
}

impl Session {
    fn new(cfg: Config, shared: Arc<Shared>, jev: Option<Arc<Jev>>, commands: Receiver<Command>, terminal: bool) -> Self {
        let (tx, rx) = channel();
        let key = cfg.key.unwrap_or(Key { tonic: 0, mode: Mode::Major });
        let jev_log = if cfg.log_jev { File::create("jev_log.jsonl").ok() } else { None };
        {
            let mut viz = shared.viz();
            viz.octave = cfg.octave;
            viz.click = cfg.click;
            viz.temperature = cfg.temperature;
            viz.key = key.name();
            viz.part = Some(cfg.part);
        }
        Session {
            commands,
            terminal,
            click: cfg.click,
            beat_log: true,
            octave: cfg.octave,
            robot_played: Vec::new(),
            keys: KeyTracker { hist: [0.0; 12], key, locked: cfg.key.is_some(), bars_heard: 0.0 },
            last_decision: Decision {
                role: Role::Rest,
                chord: key.tonic_chord(),
                energy: 1.0,
                slots: Vec::new(),
                note: "start".into(),
                perception: None,
            },
            cfg,
            shared,
            jev,
            tx,
            rx,
            queue: BinaryHeap::new(),
            seq: 0,
            rng: Rng::from_time(),
            robot: Robot::default(),
            history: Vec::new(),
            progression: Progression::default(),
            robot_steps: Vec::new(),
            pending_part: None,
            generation: 0,
            switching: false,
            last_note: None,
            last_perception: None,
            decisions: HashMap::new(),
            target: -1,
            asks: 0,
            deadline_sent: false,
            inflight: 0,
            stale: false,
            notes_seen: 0,
            latency_ema: 0.3,
            latency_dev: 0.1,
            jev_log,
        }
    }

    fn bpb(&self) -> f64 {
        self.cfg.beats_per_bar as f64
    }

    fn schedule(&mut self, beat: f64, kind: Kind) {
        self.seq += 1;
        self.queue.push(Item { beat, seq: self.seq, kind });
    }

    fn run_loop(&mut self) -> Result<()> {
        let mut current_bar = i64::MIN;
        let mut current_step = i64::MIN;
        let mut current_beat = i64::MIN;
        loop {
            if !self.handle_keys() {
                return Ok(());
            }
            let now = self.shared.clock.lock().unwrap().now();

            while self.queue.peek().is_some_and(|i| i.beat <= now) {
                let item = self.queue.pop().unwrap();
                match item.kind {
                    Kind::Robot(m) => {
                        if let Msg::On(n, _) = m {
                            self.robot_played.push((item.beat, n));
                        }
                        let sounded = self.shared.out.lock().unwrap().robot(m);
                        let mut viz = self.shared.viz();
                        match m {
                            Msg::On(n, _) => viz.note_on(Who::Robot, n, item.beat),
                            Msg::Off(n) if sounded => viz.note_off(Who::Robot, n, item.beat),
                            Msg::Off(_) | Msg::Expression(_) => {}
                        }
                    }
                    Kind::Click(n, v) => self.shared.out.lock().unwrap().click(n, v),
                    Kind::Human(raw) => handle_human(&self.shared, &raw),
                }
            }

            while let Ok(p) = self.rx.try_recv() {
                self.receive(p);
            }

            let beat = now.floor() as i64;
            if beat != current_beat {
                if current_beat >= 0 {
                    let from = current_beat as f64;
                    let chord = self.shared.listener.lock().unwrap().summarize(from, from + 1.0).chord;
                    self.progression.hear(from, chord);
                    if self.beat_log {
                        self.log_beat(current_beat);
                    }
                }
                current_beat = beat;
            }

            let bar = (now / self.bpb()).floor() as i64;
            if bar != current_bar {
                current_bar = bar;
                if self.cfg.bars.is_some_and(|n| bar >= n) {
                    return Ok(());
                }
                self.start_bar(bar);
                if let Some(part) = self.pending_part.take() {
                    self.switch_part(part);
                    current_step = i64::MIN;
                }
            }

            let step_len = self.cfg.step_beats;
            let step = (now / step_len).floor() as i64;
            if step != current_step && now >= 0.0 {
                current_step = step;
                self.start_step(step);
            }

            // Ask Jev about the next step as soon as it starts and again whenever the human plays
            // something new (one of these at a time), then a last time as late as its answer can
            // safely arrive, so the decision has heard as much of the human as possible. The
            // newest answer that arrives before the step starts wins. Jev's round trip (about
            // 0.15-0.35 s) is the floor: that much of the human is always unheard.
            let next_step = (step + 1) as f64 * step_len;
            if now >= 0.0 {
                if self.target != step + 1 {
                    self.target = step + 1;
                    self.asks = 0;
                    self.deadline_sent = false;
                    self.stale = true;
                }
                let notes = self.shared.notes_heard.load(AtomicOrdering::Relaxed);
                if notes != self.notes_seen {
                    self.notes_seen = notes;
                    self.stale = true;
                }
                let remaining = self.shared.clock.lock().unwrap().secs_until(next_step);
                let safe = self.latency_ema + 2.0 * self.latency_dev + 0.03;
                let last = !self.deadline_sent && remaining <= safe;
                let early = self.stale && self.inflight == 0 && !self.deadline_sent && remaining > safe;
                if last || early {
                    self.deadline_sent |= last;
                    self.stale = false;
                    self.inflight += 1;
                    self.asks += 1;
                    self.request(step + 1, self.asks, step as f64 * step_len, now);
                }
            }

            // Sleep until the next thing happens, but stay responsive to keys and input.
            let next_bar = (bar + 1) as f64 * self.bpb();
            let next = self.queue.peek().map(|i| i.beat).unwrap_or(f64::MAX).min(next_step).min(next_bar);
            let wait = self.shared.clock.lock().unwrap().secs_until(next).clamp(0.0, 0.005);
            if wait > 0.0 {
                thread::sleep(Duration::from_secs_f64(wait));
            }
        }
    }

    /// Terminal keys and GUI commands. False means quit.
    fn handle_keys(&mut self) -> bool {
        let mut cmds = Vec::new();
        while self.terminal && event::poll(Duration::ZERO).unwrap_or(false) {
            let Ok(Event::Key(k)) = event::read() else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => cmds.push(Command::Quit),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => cmds.push(Command::Quit),
                KeyCode::Char('c') => cmds.push(Command::ToggleClick),
                KeyCode::Char('[') => cmds.push(Command::Octave(-1)),
                KeyCode::Char(']') => cmds.push(Command::Octave(1)),
                KeyCode::Char(',') => cmds.push(Command::Temperature(-0.1)),
                KeyCode::Char('.') => cmds.push(Command::Temperature(0.1)),
                KeyCode::Char('1') => cmds.push(Command::Part(decide::Part::Ambient)),
                KeyCode::Char('2') => cmds.push(Command::Part(decide::Part::Bass)),
                KeyCode::Char('3') => cmds.push(Command::Part(decide::Part::Notes)),
                KeyCode::Char('4') => cmds.push(Command::Part(decide::Part::Chords)),
                KeyCode::Char('5') => cmds.push(Command::Part(decide::Part::Pad)),
                KeyCode::Char('b') => {
                    self.beat_log = !self.beat_log;
                    say!("beat lines {}", if self.beat_log { "on" } else { "off" });
                }
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Up => cmds.push(Command::Bpm(2.0)),
                KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Down => cmds.push(Command::Bpm(-2.0)),
                _ => {}
            }
        }
        cmds.extend(self.commands.try_iter());
        for cmd in cmds {
            match cmd {
                Command::Quit => return false,
                Command::ToggleClick => {
                    self.click = !self.click;
                    self.shared.viz().click = self.click;
                    say!("click {}", if self.click { "on" } else { "off" });
                }
                Command::Part(part) => {
                    if part != self.cfg.part || self.pending_part.is_some() {
                        self.pending_part = Some(part);
                        self.shared.viz().pending_part = Some(part);
                        say!("robot part → {} from the next bar", part.name());
                    }
                }
                Command::Octave(delta) => {
                    self.octave = (self.octave + delta).clamp(-3, 3);
                    self.shared.viz().octave = self.octave;
                    say!("robot octave {:+} (from the next step)", self.octave);
                }
                Command::Temperature(delta) => {
                    // Round so repeated steps don't drift (0.1 isn't exact in binary).
                    self.cfg.temperature = ((self.cfg.temperature + delta) * 10.0).round().clamp(0.0, 20.0) / 10.0;
                    self.shared.viz().temperature = self.cfg.temperature;
                    say!("temperature {:.1} (from the next decision)", self.cfg.temperature);
                }
                Command::Bpm(delta) => {
                    let mut clock = self.shared.clock.lock().unwrap();
                    let bpm = (clock.bpm + delta).clamp(30.0, 240.0);
                    clock.set_bpm(bpm);
                    say!("tempo {bpm:.0} bpm");
                }
            }
        }
        true
    }

    /// Change what the robot plays, at a bar line. The step grid may change with it, so
    /// pending decisions (made for the old part and grid) are thrown away.
    fn switch_part(&mut self, part: decide::Part) {
        self.cfg.part = part;
        if !self.cfg.step_explicit {
            self.cfg.step_beats = part.default_step(self.cfg.beats_per_bar);
        }
        self.decisions.clear();
        self.generation += 1;
        // Start asking afresh for the new part's grid.
        self.target = -1;
        self.switching = true;
        let mut viz = self.shared.viz();
        viz.part = Some(part);
        viz.pending_part = None;
        // The confidence charts start over for the new part's questions.
        viz.confidence.reset();
        drop(viz);
        say!("robot part: {} (deciding every {} beat(s))", part.name(), self.cfg.step_beats);
    }

    /// Bar-level housekeeping: click, count-in, demo player.
    fn start_bar(&mut self, bar: i64) {
        let start = bar as f64 * self.bpb();
        self.shared.viz().prune(start - 128.0);

        if self.click || bar < 0 {
            for b in 0..self.cfg.beats_per_bar {
                let (note, vel) = if b == 0 { (76, 100) } else { (77, 70) };
                self.schedule(start + b as f64, Kind::Click(note, vel));
                self.schedule(start + b as f64 + 0.2, Kind::Click(note, 0));
            }
        }
        if bar < 0 {
            say!("count-in… ({} bpm)", self.shared.clock.lock().unwrap().bpm.round());
            return;
        }
        if self.cfg.demo {
            self.schedule_demo(bar, start);
        }
    }

    /// One decision window: close out the previous one, then play what was decided for this one.
    fn start_step(&mut self, step: i64) {
        let len = self.cfg.step_beats;
        let start = step as f64 * len;
        let bpb = self.bpb();

        if step > 0 {
            let prev = {
                let mut l = self.shared.listener.lock().unwrap();
                l.prune(start - (self.cfg.history as f64 + 2.0) * len.max(bpb));
                l.summarize(start - len, start)
            };
            if let Some(k) = self.keys.update(&prev.hist, len / bpb) {
                say!("key → {}", k.name());
            }
            self.history.push((start - len, prev));
            if self.history.len() > self.cfg.history.max(4) {
                self.history.remove(0);
            }
        }

        let (decision, source) = match self.decisions.remove(&step) {
            Some((_, d, exchange)) => {
                self.shared.viz().record(exchange);
                // The next step's reading of the human is smoothed with the one being played.
                self.last_perception = d.perception;
                (d, "")
            }
            None if step == 0 => (self.last_decision.clone(), ""),
            None if self.switching => {
                let mut d = self.last_decision.clone();
                d.note = "switching part".into();
                for s in &mut d.slots {
                    *s = Slot::Hold;
                }
                (d, " (switching part)")
            }
            None => {
                let mut d = self.last_decision.clone();
                d.note = "reused".into();
                // Replaying picked notes would sound like a stutter; just let them ring.
                for s in &mut d.slots {
                    *s = Slot::Hold;
                }
                (d, " (decision late — reusing last)")
            }
        };
        self.decisions.retain(|&s, _| s > step);
        self.switching = false;

        let (center, vel) = self.human_center();
        let key = self.keys.key;
        let events = self.robot.render(
            &BarInput {
                decision: &decision,
                key,
                bar_start: start,
                beats: len,
                last_human: self.history.last().map(|(_, s)| s),
                human_center: center,
                human_vel: vel,
                octave: self.octave,
            },
            &mut self.rng,
        );
        let mut played: Vec<u8> =
            events.iter().filter_map(|(_, m)| if let Msg::On(n, _) = m { Some(*n) } else { None }).collect();
        if played.is_empty() {
            played = self.robot.held().to_vec();
        }
        for (beat, m) in events {
            self.schedule(beat, Kind::Robot(m));
        }
        if let Some(n) = decision.slots.iter().rev().find_map(|s| if let Slot::Note(n) = s { Some(*n) } else { None }) {
            self.last_note = Some(n);
        }
        let robot_label = match decision.role {
            Role::Rest => "rest".to_string(),
            r @ (Role::Notes | Role::Chords) => format!("{} {}", r.key(), decide::slots_label(&decision.slots, key)),
            r => format!("{} {}", r.key(), decision.chord.name(key)),
        };
        self.robot_steps.push((
            start,
            RobotStep { role: decision.role, notes: played },
        ));
        if self.robot_steps.len() > self.cfg.history + 2 {
            self.robot_steps.remove(0);
        }

        let you = match self.history.last().map(|(_, s)| s) {
            Some(s) if step > 0 => match s.chord {
                Some(c) => format!("{} {}", s.playing.describe(), c.name(key)),
                None => s.playing.describe().to_string(),
            },
            _ => "—".into(),
        };
        let bar = (start / bpb).floor();
        let position = if len < bpb {
            format!("{:>4}.{}", bar, (start - bar * bpb) as i64 + 1)
        } else {
            format!("{bar:>6}")
        };
        say!(
            "bar {position} │ {:>3.0} bpm │ {:<9} │ you: {:<26} │ robot: {:<9} {:<7} e{:.1}  [{}]{source}",
            self.shared.clock.lock().unwrap().bpm,
            key.name(),
            you,
            decision.role.key(),
            match decision.role {
                Role::Rest => String::new(),
                Role::Notes | Role::Chords => decide::slots_label(&decision.slots, key),
                _ => decision.chord.name(key),
            },
            decision.energy,
            decision.note,
        );

        {
            let mut viz = self.shared.viz();
            if step > 0 {
                viz.label(Who::Human, start - len, you);
            }
            viz.label(Who::Robot, start, robot_label.clone());
            viz.key = key.name();
            viz.status = format!("{robot_label} · e{:.1} · {}", decision.energy, decision.note);
        }

        self.last_decision = decision;
    }

    /// One line for a finished beat: what each of you started in it.
    fn log_beat(&mut self, beat: i64) {
        let (from, to) = (beat as f64, (beat + 1) as f64);
        let key = self.keys.key;
        let names = |notes: &[u8]| notes.iter().map(|&n| key.note_name(n)).collect::<Vec<_>>().join(" ");

        let human: Vec<u8> =
            self.shared.listener.lock().unwrap().summarize(from, to).onsets.iter().map(|h| h.note).collect();
        let robot: Vec<u8> = self.robot_played.iter().filter(|(b, _)| *b < to).map(|(_, n)| *n).collect();
        self.robot_played.retain(|(b, _)| *b >= to);

        let you = if human.is_empty() { "·".to_string() } else { names(&human) };
        let held = self.robot.held();
        let bot = match (robot.is_empty(), held.is_empty()) {
            (false, _) => names(&robot),
            (true, false) => format!("~ {}", names(held)),
            (true, true) => "·".to_string(),
        };
        let bpb = self.cfg.beats_per_bar as i64;
        say!("  {:>5}.{} │ you: {:<30} │ robot: {bot}", beat.div_euclid(bpb), beat.rem_euclid(bpb) + 1, you);
    }

    /// Where the human has been playing recently: (mean pitch, mean velocity).
    fn human_center(&self) -> (f64, f64) {
        let recent: Vec<_> = self.history.iter().rev().take(4).map(|(_, s)| s).collect();
        let pitch: Vec<f64> = recent.iter().filter_map(|s| s.mean_pitch).collect();
        let vel: Vec<f64> = recent.iter().filter_map(|s| s.avg_vel).collect();
        let mean = |v: &[f64], d: f64| if v.is_empty() { d } else { v.iter().sum::<f64>() / v.len() as f64 };
        (mean(&pitch, 60.0), mean(&vel, 64.0))
    }

    fn request(&mut self, for_step: i64, ask: u32, step_start: f64, now: f64) {
        let current = self.shared.listener.lock().unwrap().summarize(step_start, now);
        // Fold the step-in-progress into the key guess without committing it.
        let mut peek = self.keys.clone();
        peek.update(&current.hist, (now - step_start) / self.bpb());
        let robot_at =
            |beat: f64| self.robot_steps.iter().find(|(b, _)| (*b - beat).abs() < 1e-6).map(|(_, r)| r.clone());
        let sent = self.history.len().saturating_sub(self.cfg.history);
        let timeline = self.history[sent..].iter().map(|(beat, h)| (h.clone(), robot_at(*beat))).collect();
        let (human_center, _) = self.human_center();
        let request = decide::build(&Context {
            key: self.keys.key,
            key_known: peek.known(),
            bpm: self.shared.clock.lock().unwrap().bpm,
            beats_per_bar: self.cfg.beats_per_bar,
            step_beats: self.cfg.step_beats,
            timeline,
            current: &current,
            robot_now: robot_at(step_start),
            robot_chord: self.last_decision.chord,
            last_note: self.last_note,
            human_center,
            part: self.cfg.part,
            progression: &self.progression,
            window_start: for_step as f64 * self.cfg.step_beats,
            heard: now - step_start,
            prior: self.last_perception,
        });
        let tx = self.tx.clone();
        let generation = self.generation;
        match &self.jev {
            None => {
                let _ = tx.send(Pending { step: for_step, ask, sent: now, generation, request, result: Err(anyhow!("mock")) });
            }
            Some(jev) => {
                let jev = jev.clone();
                thread::spawn(move || {
                    let result = jev.ask(&request.state, &request.questions);
                    let _ = tx.send(Pending { step: for_step, ask, sent: now, generation, request, result });
                });
            }
        }
    }

    fn receive(&mut self, p: Pending) {
        self.inflight = self.inflight.saturating_sub(1);
        if p.generation != self.generation {
            return;
        }
        let answers = match &p.result {
            Ok(a) => {
                let secs = a.latency.as_secs_f64();
                self.latency_dev = self.latency_dev * 0.7 + (secs - self.latency_ema).abs() * 0.3;
                self.latency_ema = self.latency_ema * 0.7 + secs * 0.3;
                Some(a)
            }
            Err(e) => {
                if !self.cfg.mock {
                    say!("jev error (step {}): {e:#}", p.step);
                }
                None
            }
        };
        if let (Some(log), Some(a)) = (self.jev_log.as_mut(), answers) {
            let line = serde_json::json!({ "step": p.step, "state": p.request.state, "response": a.raw });
            let _ = writeln!(log, "{line}");
        }
        // An older request answering late never replaces a newer one's decision.
        if self.decisions.get(&p.step).is_some_and(|(ask, _, _)| *ask > p.ask) {
            return;
        }
        let mut d = decide::resolve(&p.request, answers, self.cfg.temperature, self.cfg.variety, &mut self.rng);
        let exchange = self.exchange(&p, answers, &d);
        if let Some(a) = answers {
            // How much of the human it missed: from when it was sent to when its step starts.
            let blind = p.step as f64 * self.cfg.step_beats - p.sent;
            d.note = format!("{} · ask {}, {blind:.2} beats blind · {:.2}s", d.note, p.ask, a.latency.as_secs_f64());
        }
        self.decisions.insert(p.step, (p.ask, d, exchange));
    }

    /// The request and response for the GUI's brain panel (recorded if its decision is played).
    fn exchange(&self, p: &Pending, answers: Option<&Answers>, d: &Decision) -> crate::jev_view::Exchange {
        let key = self.keys.key;
        let beat = p.step as f64 * self.cfg.step_beats;
        let bpb = self.bpb();
        let bar = (beat / bpb).floor();
        let position = if self.cfg.step_beats < bpb {
            format!("bar {}.{}", bar, (beat - bar * bpb) as i64 + 1)
        } else {
            format!("bar {bar}")
        };
        let played = match d.role {
            Role::Rest => "rest".to_string(),
            r @ (Role::Notes | Role::Chords) => format!("{} {}", r.key(), decide::slots_label(&d.slots, key)),
            r => format!("{} on {}, energy {:.1}", r.key(), d.chord.name(key), d.energy),
        };
        crate::jev_view::Exchange {
            title: format!("{position}  |  {}  |  ask {}", self.cfg.part.name(), p.ask),
            latency: answers.map(|a| a.latency.as_secs_f64()),
            state: p.request.state.clone(),
            questions: p.request.questions.iter().map(|(k, q)| (k.clone(), q.to_json())).collect(),
            answers: answers.and_then(|a| a.raw.get("answers").cloned()),
            error: match &p.result {
                Err(e) if self.cfg.mock => Some(format!("mock mode, choices were random ({e})")),
                Err(e) => Some(format!("{e:#}")),
                Ok(_) => None,
            },
            decision: format!("{played}   [{}]", d.note),
        }
    }

    /// A scripted human, for testing without a keyboard: slow ambient changes plus a sparse melody.
    fn schedule_demo(&mut self, bar: i64, start: f64) {
        const CHORDS: [[u8; 4]; 4] = [[48, 55, 59, 64], [45, 52, 55, 60], [41, 48, 52, 57], [43, 50, 55, 59]];
        const MELODY: [u8; 8] = [76, 74, 72, 79, 76, 72, 74, 71];
        let bpb = self.bpb();
        let i = (bar / 2) as usize % CHORDS.len();
        // Play chords for a while, then melody only, so the robot sees both situations.
        let phase = (bar / 8) % 2;
        if phase == 0 {
            for n in CHORDS[i] {
                self.schedule(start + 0.01, Kind::Human([0x90, n, 60]));
                self.schedule(start + bpb - 0.1, Kind::Human([0x80, n, 0]));
            }
        }
        for k in 0..2 {
            if self.rng.chance(0.6) {
                let n = MELODY[(bar as usize * 2 + k) % MELODY.len()];
                let t = start + k as f64 * 2.0 + 0.5;
                self.schedule(t, Kind::Human([0x90, n, 70]));
                self.schedule(t + 1.2, Kind::Human([0x80, n, 0]));
            }
        }
    }
}
