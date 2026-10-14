//! MIDI port selection and the shared output (human thru, robot, click).

use anyhow::{Context, Result, anyhow};
use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort};

use crate::render::Msg;

/// Port names Reaper-bound output is likely to have (loopMIDI / Windows MIDI Services loopbacks).
const OUTPUT_HINTS: [&str; 3] = ["jamsesh", "loopmidi", "loop"];

pub fn list_ports() -> Result<()> {
    let input = MidiInput::new("jamsesh-list")?;
    let output = MidiOutput::new("jamsesh-list")?;
    println!("MIDI inputs:");
    for p in input.ports() {
        println!("  {}", input.port_name(&p)?);
    }
    println!("MIDI outputs:");
    for p in output.ports() {
        println!("  {}", output.port_name(&p)?);
    }
    Ok(())
}

/// Print every message arriving on an input port, for checking routing.
pub fn monitor(wanted: &str, secs: u64) -> Result<()> {
    let mut midi = MidiInput::new("jamsesh-monitor")?;
    midi.ignore(midir::Ignore::None);
    let (port, name) = midi
        .ports()
        .into_iter()
        .filter_map(|p| midi.port_name(&p).ok().map(|n| (p, n)))
        .find(|(_, n)| n.to_lowercase().contains(&wanted.to_lowercase()))
        .ok_or_else(|| anyhow!("no input port matching {wanted:?}"))?;
    println!("listening on {name} for {secs}s…");
    let _conn = midi
        .connect(&port, "jamsesh-monitor", |t, raw, _| {
            let kind = match raw.first().map(|s| s & 0xF0) {
                Some(0x90) if raw.get(2).copied().unwrap_or(0) > 0 => "note on ",
                Some(0x80) | Some(0x90) => "note off",
                Some(0xB0) => "cc      ",
                Some(0xE0) => "bend    ",
                _ => "other   ",
            };
            let ch = raw.first().map(|s| (s & 0x0F) + 1).unwrap_or(0);
            println!("{:>10.3}s  ch {ch:>2}  {kind}  {raw:02X?}", t as f64 / 1e6);
        }, ())
        .map_err(|e| anyhow!("could not open {name}: {e}"))?;
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Ok(())
}

pub fn find_input(midi: &MidiInput, wanted: Option<&str>, output_name: &str) -> Result<(MidiInputPort, String)> {
    let ports: Vec<(MidiInputPort, String)> =
        midi.ports().into_iter().filter_map(|p| midi.port_name(&p).ok().map(|n| (p, n))).collect();
    let found = match wanted {
        Some(w) => ports.into_iter().find(|(_, n)| n.to_lowercase().contains(&w.to_lowercase())),
        // Default: first port that isn't our own loopback, to avoid a feedback loop.
        None => ports.into_iter().find(|(_, n)| {
            let l = n.to_lowercase();
            !OUTPUT_HINTS.iter().any(|h| l.contains(h)) && n != output_name
        }),
    };
    let (port, name) = found.ok_or_else(|| anyhow!("no matching MIDI input (run `jamsesh ports`)"))?;
    if name == output_name {
        return Err(anyhow!("input and output are the same port ({name}); that would feed back"));
    }
    Ok((port, name))
}

pub fn find_output(midi: &MidiOutput, wanted: Option<&str>) -> Result<(MidiOutputPort, String)> {
    let ports: Vec<(MidiOutputPort, String)> =
        midi.ports().into_iter().filter_map(|p| midi.port_name(&p).ok().map(|n| (p, n))).collect();
    let matches = |n: &str, w: &str| n.to_lowercase().contains(&w.to_lowercase());
    let found = match wanted {
        Some(w) => ports.into_iter().find(|(_, n)| matches(n, w)),
        None => OUTPUT_HINTS
            .iter()
            .find_map(|h| ports.iter().find(|(_, n)| matches(n, h)).cloned()),
    };
    found.ok_or_else(|| {
        anyhow!(
            "no MIDI output port found{}.\n\
             jamsesh needs a virtual MIDI cable to reach Reaper (channels are chosen separately with\n\
             --human-channel / --robot-channel). Install loopMIDI (winget install TobiasErichsen.loopMIDI),\n\
             add a port named \"jamsesh\" in it, and enable that port as a MIDI input in Reaper.\n\
             Or pass --output <port name>; `jamsesh ports` lists what exists.",
            wanted.map(|w| format!(" matching \"{w}\"")).unwrap_or_default()
        )
    })
}

/// Everything leaving the program goes through here, on three channels.
pub struct Out {
    /// None = dry run: nothing is sent anywhere.
    conn: Option<MidiOutputConnection>,
    human_ch: u8,
    robot_ch: u8,
    click_ch: u8,
    /// How many overlapping note-ons each robot note has, so a retrigger isn't cut short.
    robot_active: [u8; 128],
}

impl Out {
    pub fn connect(port: Option<(MidiOutput, MidiOutputPort)>, human_ch: u8, robot_ch: u8, click_ch: u8) -> Result<Self> {
        let conn = match port {
            Some((midi, port)) => Some(
                midi.connect(&port, "jamsesh-out")
                    .map_err(|e| anyhow!("{e}"))
                    .context("could not open MIDI output")?,
            ),
            None => None,
        };
        Ok(Self { conn, human_ch, robot_ch, click_ch, robot_active: [0; 128] })
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Some(conn) = self.conn.as_mut() {
            let _ = conn.send(bytes);
        }
    }

    /// Forward a human channel-voice message onto the human channel.
    pub fn human(&mut self, raw: &[u8]) {
        if let Some(&status) = raw.first()
            && (0x80..0xF0).contains(&status)
        {
            let mut msg = raw.to_vec();
            msg[0] = (status & 0xF0) | self.human_ch;
            self.send(&msg);
        }
    }

    /// Send a robot note. Returns false for a note-off that didn't sound (the note is still held
    /// by an overlapping note-on).
    pub fn robot(&mut self, msg: Msg) -> bool {
        let ch = self.robot_ch;
        match msg {
            Msg::On(n, v) => {
                let n = n.min(127);
                if self.robot_active[n as usize] > 0 {
                    self.send(&[0x80 | ch, n, 0]);
                }
                self.robot_active[n as usize] = self.robot_active[n as usize].saturating_add(1);
                self.send(&[0x90 | ch, n, v.clamp(1, 127)]);
                true
            }
            Msg::Expression(v) => {
                self.send(&[0xB0 | ch, 11, v.min(127)]);
                true
            }
            Msg::Off(n) => {
                let n = n.min(127);
                let a = &mut self.robot_active[n as usize];
                if *a == 0 {
                    return false;
                }
                *a -= 1;
                let released = *a == 0;
                if released {
                    self.send(&[0x80 | ch, n, 0]);
                }
                released
            }
        }
    }

    pub fn click(&mut self, note: u8, vel: u8) {
        let ch = self.click_ch;
        if vel > 0 {
            self.send(&[0x90 | ch, note, vel]);
        } else {
            self.send(&[0x80 | ch, note, 0]);
        }
    }

    /// Silence everything we might have started.
    pub fn all_off(&mut self) {
        for n in 0..128u8 {
            if self.robot_active[n as usize] > 0 {
                self.send(&[0x80 | self.robot_ch, n, 0]);
            }
        }
        self.robot_active = [0; 128];
        // The pad may have turned expression down; leave the synth at its default.
        self.send(&[0xB0 | self.robot_ch, 11, 127]);
        for ch in [self.human_ch, self.robot_ch, self.click_ch] {
            self.send(&[0xB0 | ch, 64, 0]); // sustain off
            self.send(&[0xB0 | ch, 123, 0]); // all notes off
        }
    }
}
