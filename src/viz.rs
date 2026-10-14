//! What the GUI needs to draw, updated by the jam loop, plus the commands it can send back.

use std::collections::VecDeque;

use crate::decide::Part;
use crate::jev_view::{Exchange, History};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Human,
    Robot,
}

#[derive(Clone, Debug)]
pub struct VizNote {
    pub who: Who,
    pub note: u8,
    pub start: f64,
    /// None while the key is still down.
    pub end: Option<f64>,
}

/// A text label pinned to a beat in the roll.
#[derive(Clone, Debug)]
pub struct Label {
    pub who: Who,
    pub beat: f64,
    pub text: String,
}

#[derive(Default)]
pub struct Viz {
    pub notes: Vec<VizNote>,
    pub labels: Vec<Label>,
    pub key: String,
    pub octave: i32,
    pub click: bool,
    pub temperature: f64,
    pub part: Option<Part>,
    /// A part change waiting for the next bar line.
    pub pending_part: Option<Part>,
    /// Latest decision, for the header.
    pub status: String,
    /// Recent Jev requests and responses, oldest first.
    pub exchanges: VecDeque<Exchange>,
    /// Total exchanges ever recorded, so the GUI can tell when a new one arrives.
    pub exchange_count: u64,
    /// Jev's probabilities over time since the part last changed.
    pub confidence: History,
}

impl Viz {
    pub fn note_on(&mut self, who: Who, note: u8, beat: f64) {
        self.note_off(who, note, beat);
        self.notes.push(VizNote { who, note, start: beat, end: None });
    }

    pub fn note_off(&mut self, who: Who, note: u8, beat: f64) {
        for n in self.notes.iter_mut().filter(|n| n.who == who && n.note == note && n.end.is_none()) {
            n.end = Some(beat);
        }
    }

    pub fn all_off(&mut self, beat: f64) {
        for n in self.notes.iter_mut().filter(|n| n.end.is_none()) {
            n.end = Some(beat);
        }
    }

    /// Pin a label, unless it just repeats that lane's previous one.
    pub fn label(&mut self, who: Who, beat: f64, text: String) {
        if self.labels.iter().rev().find(|l| l.who == who).is_some_and(|l| l.text == text) {
            return;
        }
        self.labels.push(Label { who, beat, text });
    }

    pub fn record(&mut self, e: Exchange) {
        self.confidence.add(&e);
        self.exchanges.push_back(e);
        while self.exchanges.len() > 40 {
            self.exchanges.pop_front();
        }
        self.exchange_count += 1;
    }

    /// Forget what has scrolled out of view.
    pub fn prune(&mut self, before: f64) {
        self.notes.retain(|n| n.end.is_none_or(|e| e >= before));
        self.labels.retain(|l| l.beat >= before);
    }
}

/// Things the GUI (or terminal keys) can ask the jam loop to do.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Bpm(f64),
    Octave(i32),
    Temperature(f64),
    ToggleClick,
    Part(Part),
    Quit,
}
