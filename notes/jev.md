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

The state used to include a timeline of *both* parts, so Jev would hear the conversation. It now carries only the human's timeline (plus their activity bar by bar); see below for why.

## What moved Jev's answers

- **Words in the instructions leak into the choice.** Telling the bass player to "give the music a pulse" next to an option named `pulse` made Jev pick it ~70% of the time. Rewording ("keep the bassline gently moving and varied") dropped it to ~4%. Check instructions for words that match option names.
- **Style text is a strong prior.** "Silence and repetition are welcome" made `rest` the favourite even when the human was silent. Describing `rest` as "only when the human is busy and needs space" fixed it.
- **Descriptions matter as much as names.** Calling the held bass root "the foundation" made it win 90%+ of bars; "only for the sparsest, most static moments" let the moving patterns through.
- **Distributions are often flat.** Chord choices frequently had confidence around 0.15, which is useful: we sample from the distribution (`--temperature`) instead of always taking the top pick, which gives natural variety without randomness Jev wouldn't endorse.

## Getting it to respond to the human

"It plays the same stuff in its own world" turned out to be measurable. `src/responsiveness.rs` sends Jev the requests the jam would build for contrasting scripted players with the robot's history fixed, then contrasting robot histories with the player fixed, and compares how far the answers move (total variation distance; the same request twice moves 0.00–0.07, so differences are real):

- **Jev followed the robot's own history more than the human.** Per-beat picks moved 0.50–0.55 with the robot's history vs 0.41–0.46 with the human's; the next chord 0.20 vs 0.07. If the robot had just played G7, Jev kept picking G7.
- **"What should you play" answers barely depended on the human.** Energy came out "long soft tones" ~60% whether the human was silent or playing a loud busy run; ambient picked pad 67–87% regardless; the next chord was "stay" 84–95%.
- **When they did respond, they mirrored.** Rest when the human was silent, melody when they were busy, rest after they finished a phrase and left space: the opposite of a duet.

What fixed it:

- **Ask Jev about the human, not about itself.** Questions like "how intensely is the human playing", "are they leaving space", "where is their playing heading" track the human strongly (intensity moved 0.90 with the player, 0.00 with the robot's history). Code maps the answers to a response (answer / support / carry / follow) and an energy.
- **Let the response decide which options fit, and Jev pick among them.** Jev's role picks are 90–99%+ one-sided, so down-weighting wasn't enough; a filter was (a strict zero for roles that don't fit the response).
- **Take the robot's own history out of the state** (it still knows what's ringing, via the "hold" option), and drop "soft" from the style line.
- **Describe the human in numbers too:** notes per beat and velocity bar by bar, beats since their last note; chord options say how many of the human's recent melody notes they contain.
- **Smooth per-beat readings** over about a bar, or the response flips every beat.

After: the robot's final choices move 0.55–0.82 with the human and 0.02–0.10 with its own history (ambient roles 0.57, which is the deliberate `--variety` penalty).

## Keeping code in charge

Following the gist's advice ("software, not Jev, owns the workflow"):

- Timing, voicing, register and note-offs are all code.
- A late or failed answer never stops the music; the robot keeps its last decision (or holds, in notes mode).
- Repetition control is code: `--variety` multiplies down a role's weight for each bar it already had in the last four. It's applied after temperature, because sharpening the distribution first undid the penalty.
- Every exchange is inspectable: the GUI's Jev panel, or `--log-jev` for a JSONL file.
