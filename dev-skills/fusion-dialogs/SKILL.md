---
name: fusion-dialogs
description: DEVELOPER-only. USE WHEN writing or debugging any Fusion verb that drives the UI, opens a library artifact, closes a document, or reports "add-in not responding" - and WHEN a modal has blocked the main thread or a caller says the add-in crashed. Covers preventing dialogs rather than dismissing them, classifying Fusion's generic-titled modals by BODY TEXT, and telling busy / hung / modal / dead apart. Trigger words - fusion dialog, modal, Create new footprint, save prompt, blocking dialog, add-in not responding, main thread blocked, dismiss dialog, phantom dialog.
user-invocable: false
---

# Fusion dialogs: prevent, then detect, then answer

**The rule, in John's words (2026-08-21): "you should never allow dialogs like that to ever
even show cuz they screw you up, so you should pre-emptively make sure they never show up."**

A modal in Fusion is not a cosmetic annoyance. Fusion runs one command at a time on its main
thread, and a modal **holds that thread until somebody answers it**. So every dialog we cause
turns into:

- the verb that raised it hanging,
- every later verb timing out,
- `fusion_addin_not_responding` reported to the caller,
- a human concluding the add-in crashed and restarting Fusion,

which is a spectacular amount of damage from a Yes/No box. Ranked responses:

| rank | approach | when |
|---|---|---|
| 1 | **Never raise it** - check preconditions before asking Fusion | always possible for anything we control |
| 2 | **Detect + answer** deterministically | dialogs raised by Fusion itself (licensing, crash, update) |
| 3 | Report it with a screenshot | genuinely user-decidable only (seat conflict) |

Rank 3 is a last resort, not a default. "Run `fusion_dismiss_blocking_dialogs` afterwards" is
NOT an acceptable design - it hands the caller a mess we chose to make.

## The case that taught this: "Create new footprint 'X'?"

`EDIT <name>.pac` on a name that is **not in the open library** makes Fusion offer to create
it. That modal then blocks the main thread. The bridge shipped for months with this hint at
the call site:

> "EDIT on a missing name pops a 'Create new?' dialog - run fusion_dismiss_blocking_dialogs
> if one appeared."

We *knew*, documented it, and still caused it. That is the exact inversion this skill exists
to prevent.

**The fix is a precondition check, not a sweeper.** `_resolve_artifact_name()` parses the .lbr
(EAGLE libraries are XML, so `<symbol>/<package>/<deviceset>` name attributes are the source
of truth), resolves exact then unique case-insensitive matches, and if the name is absent
**refuses before touching Fusion**:

```
errorCode: "artifact_not_in_library"
available: [... what the library actually holds ...]
_hint:     "Deliberately refused BEFORE touching Fusion ... Nothing was opened and no
            dialog was raised."
```

A caller gets the real answer (wrong name, here are the right ones) in milliseconds instead
of a hang. **Generalise it: before any command that can prompt, verify the thing you are
about to name actually exists.**

## Detection: the title is often useless

Fusion titles several unrelated modals just `Fusion`. A title-only classifier cannot tell
"Create new footprint" from anything else, which is why ours missed it completely.

**The identity lives in the BODY TEXT.** Two ways to read it, both background and read-only:

- `desktop_find_controls {hwnd}` - Qt publishes the prompt as Text controls in the a11y tree.
  This is what `_dialog_text()` uses.
- `desktop_screenshot_window {hwnd}` - ab returns the window **plus its owned child windows**
  as separate images. Use this when UIA is unavailable (some Qt modals expose nothing), and
  when you need to SHOW a human what is blocking.

`classify_blocking_dialogs()` therefore matches on title first, then falls back to body text
whenever the title is generic, and reports `matchedOn: "title" | "bodyText"`.

## Answering: pick the answer that does not hide the bug

For `create_confirm` the bridge answers **No**, deliberately. "Yes" would invent an empty
footprint, the surface would "succeed", and the real fault - a wrong name - would surface
days later as a mysteriously blank part. **Prefer the answer that fails loudly over the one
that produces a plausible artifact.**

Same reasoning as the transfer-integrity rule ab landed on: a zero-filled gap makes a file of
exactly the right length that is silently wrong.

## Never report a blocked main thread as a crash

Three distinct states that look identical to a naive caller, all now distinguished:

| state | errorCode | remedy |
|---|---|---|
| a modal is holding the main thread | classified dialog + `mainThreadBusy` | answer or clear it |
| a long command is still running | `fusion_addin_busy` (+ `busyCommand`, `elapsedSeconds`) | WAIT - a staged surface measured 546s |
| the host stopped pumping messages | `fusion_host_hung` (+ `hungSignals`) | force-restart; nothing reaches it over IPC |
| the add-in genuinely is not serving | `fusion_addin_not_responding` (+ `hostResponding: true`) | restart Fusion; it only loads at launch |

Collapsing these into one error is what sent ab hunting a phantom dialog (#38) and made a
9-minute surface look like a dead add-in (#40).

### A kill needs KILL-GRADE evidence (learned 2026-08-21, three dead Fusions in one day)

`IsHungAppWindow` alone can NEVER authorize a force-restart. A heavy main-thread modeling
script (STEP import, saveAs) starves the add-in's HTTP thread via the GIL, so the 3s status
probe and the 5s health probe BOTH time out, and the host window legitimately stops pumping
messages - a working Fusion is indistinguishable from a hung one in a single snapshot. The
old gate taskkilled three healthy Fusions mid-work; the logs just stop mid-write, no crash
report, and the pattern reads as "something external keeps closing Fusion".

`_confirmed_hung()` (server.py) is the only path allowed to precede `kill`: hung NOW, plus a
refused 20-SECOND status probe (a merely starved add-in answers intermittently within that),
plus 180s+ of persistence since first observed. Anything less returns `fusion_addin_busy`
with "WAIT, do not restart". Same law as the busy guard from #41: a remediation that
destroys state needs a stronger signal than the absence of a fast answer.

## The deterministic-close rule (John, 2026-08-21, after the THIRD Save dialog)

> "save programmatically before a dialog pops up asking you to save. cuz that will make all
> your code more deterministic where you don't have to use a screenshot and analysis of it
> (which requires ai)"

**The operation that dirties a document resolves that document's state before returning.**
Not the janitor three minutes later. Not the next open tripping over it. Inside the verb:

1. Done rendering/minting from a scratch doc -> `_close_bridge_doc_verified(name)`:
   `close_document {save:false}`, then VERIFY via `document_info` the doc is gone; if
   Fusion raised Save-As anyway (never-saved docs can), answer it in the SAME call.
2. The `.lbr` on disk is the artifact. The Fusion document is a rendering surface -
   disposable by construction, so `save:false` is always correct for bridge scratch docs.
3. Determinism is the point: state checks (document lists, file contents) decide everything;
   a screenshot is evidence for humans, never the mechanism the code relies on.

The janitor and the sentinel remain as BACKSTOPS for docs dirtied by crashed runs or other
routes - but if they ever fire for a live verb's document, that verb has a bug.

### Closes must wait for the Hub upload to drain (John caught it live, 2026-08-21)

Create3DPackage / cloud saveAs uploads continue AFTER the API call returns. A close issued
while they run raises "Packages are being uploaded to your Fusion Hub... you will lose
these changes" - a modal that blocks the main thread AND whose default-looking button
destroys the upload. `app.hasActiveJobs` is the deterministic signal. Every bridge-issued
close (`_close_bridge_doc_verified`, prove's f3d close) polls it to False (cap 120s) BEFORE
closing, and DEFERS rather than closes when it will not drain. If the modal appears anyway,
the answer is ALWAYS No (work-preserving: cancels the close, loses nothing) - the dispatch
chokepoint auto-answers it, and the gated close retries after the drain. Never Yes.

## Checklist for a new verb that drives the UI

1. Can it prompt? List every dialog it could raise, then remove each cause.
2. Verify names/paths/state BEFORE issuing the command.
3. If a dialog is still possible, classify it by BODY TEXT and choose the answer that does
   not fabricate an artifact.
4. Never return "not responding" without checking busy vs hung vs modal first.
5. Never tell the caller to run a dismisser as part of the happy path.
