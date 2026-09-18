# Working with Jev

## The API as we use it

`POST https://openrouter.ai/api/alpha/decisions`, bearer auth with the OpenRouter key, model `~typesafe/jev-latest` (during this session it resolved to `typesafe/jev-1.13-20260917`).

The request is a `state` (any JSON) and named `questions`:

```json
{
  "model": "~typesafe/jev-latest",
  "state": { "key": "C major", "timeline_oldest_first": [ ... ] },
  "questions": {
    "role":   { "type": "choice", "instructions": "...", "criteria": { "pad": "Sustained soft chord pad", "rest": "Silence" } },
    "energy": { "type": "score",  "instructions": "...", "criteria": ["Almost nothing", "Long tones", "Gentle movement", "Flowing"] },
    "busy":   { "type": "noul",   "instructions": "...", "criteria": { "true": "...", "false": "..." } }
  }
}
```

Each answer carries a full distribution, not just a pick:

- **choice:** `choice`, `probabilities` per option key, `confidence`
- **score:** `score` (a real number between level indices), `probabilities` per level, `legend`, `confidence`
- **noul:** `noul`, the probability of true

Measured: about 0.45 s for the first request, then 0.12–0.3 s. Cost ≈ $0.00002–0.00006 per request (roughly 500–1,500 input tokens; output tokens are free).

## How we frame the questions

Jev never sees raw MIDI. Code turns playing into musical facts first (chord names, note names, "chords" vs "single-note melody", soft/medium/loud, low/middle/high register). Then code decides which options make sense right now, and Jev chooses among those:

- While you're playing chords, ambient mode stops offering the chord-playing roles (pad, arpeggio) and doesn't ask about the next chord; the robot follows yours.
- "Echo" isn't offered if there's nothing recent to echo.
- Notes mode offers only scale notes within a fifth of the robot's last note, each described musically ("chord tone of Am7, 2 semitones above your last note"), plus chords written out with their actual voicings.

Questions in one request are independent: Jev evaluates each without seeing the others' answers. So anything that depends on a previous pick needs a new request. That's why notes mode goes one beat at a time.

The state includes a timeline of *both* parts, window by window, so Jev hears the conversation:

```
human  chord: Am9 | notes: B4 | playing: chords and melody
you    notes: A4 | played: notes ~
```

## What moved Jev's answers

- **Words in the instructions leak into the choice.** Telling the bass player to "give the music a pulse" next to an option named `pulse` made Jev pick it ~70% of the time. Rewording ("keep the bassline gently moving and varied") dropped it to ~4%. Check instructions for words that match option names.
- **Style text is a strong prior.** "Silence and repetition are welcome" made `rest` the favourite even when the human was silent. Describing `rest` as "only when the human is busy and needs space" fixed it.
- **Descriptions matter as much as names.** Calling the held bass root "the foundation" made it win 90%+ of bars; "only for the sparsest, most static moments" let the moving patterns through.
- **Distributions are often flat.** Chord choices frequently had confidence around 0.15, which is useful: we sample from the distribution (`--temperature`) instead of always taking the top pick, which gives natural variety without randomness Jev wouldn't endorse.

## Keeping code in charge

Following the gist's advice ("software, not Jev, owns the workflow"):

- Timing, voicing, register and note-offs are all code.
- A late or failed answer never stops the music; the robot keeps its last decision (or holds, in notes mode).
- Repetition control is code: `--variety` multiplies down a role's weight for each bar it already had in the last four. It's applied after temperature, because sharpening the distribution first undid the penalty.
- Every exchange is inspectable: the GUI's Jev panel, or `--log-jev` for a JSONL file.
