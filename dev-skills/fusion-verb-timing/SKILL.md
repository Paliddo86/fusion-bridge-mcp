---
name: fusion-verb-timing
description: DEVELOPER-only. USE WHEN adding or changing any slow Fusion verb, WHEN a caller reports something is hanging or "not responding", or WHEN you need to know how long a verb really takes on a given machine. Covers the two timing stores (rolling window for live estimates, append-only log for later analysis), the taskbar activity indicator paced by those measurements, and the rule that a guessed number must never be presented as a measured one. Trigger words - verb timing, how long does it take, p50, p90, verb-times.jsonl, activity indicator, taskbar progress, progress bar, slow verb, estimate, measured history.
user-invocable: false
---

# Measuring verb run times, and showing them honestly

Two stores, because they answer different questions. Keep both.

| store | file | answers |
|---|---|---|
| rolling window (last 10 per key) | `~/.adom/fusion-bridge/timings.json` | "how long will THIS verb take, on THIS machine, right now?" |
| append-only log (every run) | `~/.adom/fusion-bridge/verb-times.jsonl` | "what changed?", "which verb owns the 546s?", "did 1.9.9x regress the 3D package?" |

A rolling window can only answer what it still remembers. A log can be re-analysed for a
question nobody thought to ask up front, which is most of the interesting ones.

## Recording

`dispatch_command` times EVERY verb and calls `timings.log_run(verb, seconds, ok)`, which
writes both stores. Nothing to remember when adding a verb: it is measured automatically.

Record failures too. A verb that fails FAST is a different bug from one that is slow, and you
cannot tell them apart from durations alone. That is why the log carries `ok`.

Reading it back:

```python
timings.verb_estimate("show_symbol")   # (seconds, "measured" | "typical" | "unknown")
timings.verb_report()                  # per verb: count / p50 / p90 / max / failures
```

`fusion_verb_times` exposes the report as a verb, so the numbers are readable from the
Dashboard instead of requiring someone to SSH in and parse a file.

**p90 is the column to look at when hunting a slow surface.** A p50 of 15s next to a p90 of
400s is not "sometimes slow", it is two different code paths wearing one name, usually a cold
first-open versus a warm re-open.

## Showing it: the activity indicator

`fusion_set_activity_indicator {mode}` - `progress` | `flash` | `both` | `none`, sticky per
user, and a card in the Dashboard's Settings.

- **progress** paints a real taskbar bar via `desktop_taskbar {progress:{state,value}}`,
  paced by `verb_estimate` for that specific verb, and clears the instant the verb returns.
  On failure it flashes `error` (red) for a beat first, so an outcome is visible even if
  nobody was watching the moment it happened.
- **flash** is the older attention flash. It says "something is happening" and nothing else.
- The bar runs 0 to 95 and **holds**. It never reaches 100 while work continues, because a
  bar sitting at 100 during live work is the same lie as an ETA of zero.

## The honesty rule, which is the whole point

**A verb with no measured history runs the bar INDETERMINATE (marquee), never a percentage.**

We have made the opposite mistake twice, and both times it cost a real debugging session:

- an estimate revised as `elapsed * 1.25` pinned percent at exactly 80 forever, so a stuck run
  looked identical to a healthy one;
- a "couple of minutes" string that was never measured, never narrowed, and could not be drawn
  as a bar at all.

The confidence ladder is `measured` (3 or more real samples here) > `typical` (1-2 samples, or
a seeded literal) > `unknown`. Render `unknown` as motion without a number. ab dims a
non-`measured` block in its own window for the same reason: a bridge that guesses should never
look as authoritative as one that measured.

## When a caller says "it is hanging"

Check the measurements before believing it:

1. `fusion_verb_times` - is this verb normally slow? A 546s p90 means the caller is impatient,
   not that the bridge is broken.
2. `fusion_state.timings` - what has THIS machine actually done?
3. `errorCode` - `fusion_addin_busy` carries `elapsedSeconds`; compare it against p90 before
   concluding anything is wrong. Past roughly 2x p90 it is fair to call it stuck.

That comparison is the difference between "slow" and "stuck", and it is a measurement rather
than a judgement call. See `fusion-dialogs` for the neighbouring trap: a modal holding the main
thread looks exactly like a slow verb from outside.


## Multi-stage chains: per-stage estimates that improve with every use

John's requirement (2026-08-21): web-control must show "as accurate of progress bars as we
can", per user machine, with NO dedicated measurement pass - measure from live runs and
update the averages after every call, forever.

The method, as implemented in `fusion_create_library`:

1. **Name each stage as a timing key** (`stage:convert`, `stage:bind`, `stage:prove`) and
   `log_run()` its actual when it completes. Same store as everything else, so the rolling
   per-machine p50 updates on every run automatically. Different computers get different
   numbers by construction - the store lives on the box.
2. **Publish the plan up front.** The chain's persisted state carries
   `plan: [{stage, estimatedSec, confidence}]` built from `estimate("stage:X", seed)`.
   Seeds are used only until 1-2 real samples exist, then the machine's own history wins
   (`measured` at 3+ samples).
3. **Compute progress AT READ TIME, not with a ticker.** The poll verb derives
   `percent = (sum of completed stages' estimates + elapsed-of-current, capped at 95% of
   its estimate) / total` from `stagesDone` + `stageStartedAt`. Every poll is fresh, there
   is no background thread to die, and a wedged stage shows `overdue: true` instead of a
   frozen number.
4. **Cap the current stage's credit at 95% of its estimate.** A stage must never appear
   finished before it reports finished - the bar creeps and holds, exactly like the
   single-verb rule.
5. **Confidence is the WORST of the plan** in spirit: all-measured -> `measured`,
   all-unknown -> `unknown`, else `typical`. ab dims non-measured bars, so a chain that is
   still learning looks less authoritative than one with history - which is honest.

Why not a measurement pass: a pass measures one day on one box. The live loop measures
every real run on every real box, so estimates track the machine as it changes (SSD swap,
cloud latency, Fusion updates). The first run on a fresh box shows seeds at `typical`; by
the third run the user has personal `measured` numbers. That is the whole design: the bar
gets better BECAUSE the user uses it.
