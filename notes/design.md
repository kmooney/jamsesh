# Design decisions

## Timing: a fixed grid, decisions ahead of time

- The clock counts **beats**, anchored so tempo changes re-anchor without a jump. Everything is scheduled in beats, so changing BPM mid-jam just works.
- Jev decides per **step** (default one bar; `--step 2` or `1` for faster). The request for step N+1 goes out `1.5 × measured latency + 0.1 s` before step N ends (at least a quarter beat), using what you've played so far.
- If the answer is late, the robot reuses its last decision (notes mode holds instead, since replaying picked notes would stutter). In testing, the release build made every deadline, even deciding every beat at 70 BPM.
- There's a one-bar count-in; the first robot decision lands in bar 2 because it needs a bar of listening first.

## Routing: one port, three channels

Windows' MIDI API usually allows one program per input device, so jamsesh opens the keyboard and passes it through. One loopMIDI port carries everything, split by channel: you (1), robot (2), click (10). Reaper then treats it as one MIDI device with per-track channel filters.

Robot note-offs are reference-counted: if two overlapping events play the same note, the note only stops when the last one ends. On exit, every channel gets sustain-off and all-notes-off.

## Musical analysis in code

- **Chord recognition:** score every template (triads, sevenths, sus, add9, 9ths...) against the pitch classes held in the step, weighted by duration, with a bonus when the root is the bass note. That's how C6 and Am7 (same notes) are told apart.
- **Key:** Krumhansl-Schmuckler correlation on a decaying pitch histogram. It isn't trusted until about 3 bars have been heard and needs a clear margin to change, because an early version flipped C major → E minor on a single Cmaj7 voicing. `--key` locks it.
- **Register:** the robot plays away from where you've been playing (above you if you're low, below if you're high).
- **Sustain pedal:** notes released under the pedal count as sounding until it lifts.

## The GUI (Bevy 0.19)

- Bevy needs the main thread, so the jam loop runs on a background thread. The GUI reads a shared snapshot (active notes, a note history, step labels, recent Jev exchanges) and sends commands back over a channel, the same commands the terminal keys produce. Drawing can never delay the music.
- The roll uses pooled sprites re-laid out each frame; notes grow from the "now" line and scroll down, tracker-style.
- In this Bevy version 2D text drew underneath sprites regardless of depth, so roll labels are UI text converted to screen coordinates, and always on top.
- The default font lacks the "·" and emoji glyphs, so separators are "|" and the brain icon is drawn procedurally.
- `JAMSESH_SCREENSHOT=<file>` makes the app press a few buttons, open the Jev panel, save a screenshot of its own window and quit. That's how the GUI was checked without touching the desktop.

## Parts and switching

- `ambient`, `bass`, `notes` share the same pipeline and differ in the options offered and how a decision is rendered.
- Switching lands on a bar line. Notes mode uses one-beat steps, so the step grid can change: histories are keyed by beat position (not step number) so your part and the robot's still line up, and Jev answers already in flight for the old part are discarded (a generation counter).
