---
name: fusion-no-modal-dialogs
description: >
  NEVER let Fusion raise a modal dialog from automation. A modal freezes the add-in's main
  thread (so every later verb reads as a crash), dings the user's speaker with an alert
  sound, and cannot be reliably cleaned up after the fact. Every command that CAN prompt
  must pre-answer it (SET CONFIRM) or be proven unnecessary offline first. Read before
  writing any code that issues EDIT, EXPORT, WRITE, ADD, SHOW, GRID, OPEN, or CLOSE against
  Fusion Electronics, and before "fixing" a dialog by dismissing it.
  Trigger words - create new footprint, create new symbol, dialog popped, modal, blocking
  dialog, dismiss dialog, SET CONFIRM, EDIT .pac, EDIT .sym, add-in not responding, Fusion
  hung, unsaved changes prompt, save changes dialog, alert sound, speaker ding.
discovery_triggers:
  - "create new footprint"
  - "create new symbol"
  - "fusion dialog"
  - "blocking modal"
  - "SET CONFIRM"
  - "add-in not responding"
---

# Never let Fusion draw a dialog

**The doctrine (John, verbatim, after the THIRD report of the same modal):**

> "why do you still allow these dialogs to pop up? i've asked you 100's of times to never
> allow that. you can pre-empt it by always deterministically closing out items from code
> and telling the api you call to 'not save changes' or you could literally save it
> somewhere programatically so 'there are no changes' and then you wouldn't get prompts like
> that. dialogs are dangerous cuz they're usually modals and kill programmatic code from
> running, especially async tasks you're calling into fusion. and it dings the computer's
> speaker with an error alert audio sound which is annoying to the user."

## Why a dialog is a SEVERITY-1 bug, not an inconvenience

1. **It freezes the add-in's main thread.** Fusion's API is single-threaded. A modal sits on
   that thread, so the add-in's HTTP server stops answering. Every subsequent verb times out
   and reads as "add-in not responding" or "Fusion crashed". One un-answered prompt looks
   exactly like a dead bridge, and the debugging goes somewhere else entirely.
2. **It kills in-flight async work.** Any chain already running (a library import, a matrix
   run, a capture sweep) stalls mid-way and leaves half-built state behind.
3. **It dings the user's speaker.** An alert sound at a human who did not touch anything, on
   a machine they are using for something else.
4. **Dismissal is a race you lose.** Sweeping the dialog after it appears means it was
   already drawn, already sounded, and already blocked the thread. When Fusion is slow the
   sweep arrives late and the modal just sits there.

## The rule

> **A command that can prompt must either be proven unnecessary offline, or carry its answer
> with it. Never issue one and plan to clean up.**

Three techniques, in priority order. Use the highest one that applies.

### 1. Prove it offline. Do not ask Fusion a question you can answer yourself.

The `.lbr` on disk lists every symbol, package, and deviceset. `EDIT <name>.pac` on a name
that is not in the library is what raises **"Create new footprint 'X'?"**. That is knowable
in milliseconds without Fusion.

```python
resolved, err = _resolve_artifact_name(name, "pac", lbr_path)
if err:
    return err          # EDIT is NEVER issued. No dialog is possible.
```

**The trap that made this fail for months:** the guard needs the `.lbr` path, and
`fusion_capture_library_views` never took one, so the single verb that EDITs the most names
was the one verb structurally unable to run the guard. It "handled" the prompt by raising it
and dismissing it. If a guard cannot reach the data it needs, **give it the data** (here:
`_remember_open_library()` persists the open `.lbr` so any later verb can pre-check). Do not
downgrade to sweeping.

### 2. Pre-answer it. Declare intent so the modal is never drawn.

EAGLE/Electron has a deterministic answer built in, and it is not a dismissal - the dialog
is never rendered at all:

```
SET CONFIRM YES    # we INTEND to create this artifact
SET CONFIRM NO     # inspect only; a prompt means a bad name, decline it
```

Every `electron_run` that can prompt takes `confirm: "yes" | "no"`. **`EDIT` with no declared
intent defaults to `"no"`**, because an undeclared EDIT is precisely the call that raised
John's dialog, and declining leaves no stray artifact.

This answers John's "why can't creating a footprint be triggered so the dialog doesn't show
up?" - it can: `confirm: "yes"` **is** the create, performed without a question.

### 3. Remove the reason to ask.

For save/close prompts, do not answer "don't save" - arrange for there to be nothing to
save. Save programmatically first, or close with the explicit no-save flag, so Fusion has no
question. See `_close_docs_opened_since` and `handle_close_all_documents({force, closeUntitled})`.

## Dismissal is a BACKSTOP, and it must shout

Keep a sweep as a last resort, but treat every firing as a **defect report**: it means a
prevention hole exists. Log it loudly and record it, so it cannot rot into the normal path.

```python
dismissed = _dismiss_dialogs_bg()
if dismissed:
    print("[Fusion Bridge] ⚠ PRE-CHECK HOLE: EDIT %s.%s still raised a dialog" % (name, ext))
    _bridge_action_ledger("dialog_precheck_hole", {...})
```

If you find yourself writing a hint that tells the CALLER to run
`fusion_dismiss_blocking_dialogs` afterwards, stop. You are telling a user to sweep up a
mess you chose to make.

## Never swallow a guard's failure

The first version of `_remember_open_library` ended in `except Exception: pass`. It raised
`NameError` on every call (this module imports `time` inside functions, not at module level),
so the guard was **silently disarmed while the code read as correct**. The offline test
caught it only because it asserted on the EDITs actually sent, not on the guard's presence.

- A prevention guard that fails quietly is worse than none: it removes the pressure to notice.
- `except Exception: pass` around a safety mechanism is a bug, always. Log it.
- **Test what reached Fusion, not what the code says.** Stub the add-in proxy, record every
  command, and assert the dangerous one is absent:

```python
bad = [e for e in edits if "TJA1042T.pac" in e]
assert not bad, "EDIT was issued -> dialog would pop"
```

## A verb's docstring is not evidence that it does not prompt

`fusion_close_all_documents` says "Close ALL open documents **without save dialogs**". Two
hours after writing this skill I called it to clear a workspace and it raised Fusion's Save
box (Name: TJA1042T, Location: Admin Project) at John. I trusted the sentence instead of the
behaviour - the same mistake this whole document is about, made on the very next action.

**The actual rule Fusion enforces:** `doc.close(False)` discards silently for a scratch
`Untitled`, but a document that has a **NAME and no dataFile has never been saved**, so
Fusion has nowhere to write it and asks - *even when saveChanges is False*. There is no API
to discard such a document silently.

Since it is knowable before the call (`dataFile is None` and `isModified` and the name is
not `Untitled*`), it gets decided before the call: those docs are **refused**, returned in
`data.wouldPrompt` with `errorCode: unsaved_named_document`, and `close()` is never invoked
on them. The caller then picks a dialog-free path - save with an explicit
`{name, projectName, folderPath}` (no prompt, because you supply exactly what the dialog
would ask for), or `save: true`.

**Generalise it:** when a verb claims it cannot prompt, that claim is a hypothesis. The test
is what reached Fusion and what appeared on screen, not what the docstring asserts.

## Closing a workspace before a restart, in order

1. Enumerate documents first (`fusion_get_app_state`) and look at `isSaved` per doc.
2. Scratch `Untitled` + unmodified: `closeUntitled: true` closes them silently.
3. Saved docs: close normally.
4. Named + never saved + modified: **save it with an explicit destination, then close.**
   Never call close and hope.

## Commands that prompt (assume prompting unless proven otherwise)

| Command | Prompt | Prevention |
|---|---|---|
| `EDIT <new>.sym/.pac/.dev` | "Create new ...?" | Resolve name offline; else `confirm` |
| `EXPORT` onto an existing file | "Overwrite?" | Pre-delete the target, `SET CONFIRM YES` |
| `WRITE` | Save As | Never call from automation; pass an explicit path |
| `ADD` | component picker | Never from automation |
| `SHOW` (bare) | waits for a click | Always pass the object name: `SHOW <name>` highlights and returns (allowed since 1.10.3) |
| `GRID` (no params) | grid settings | Always pass params |
| closing a dirty document | "Save changes?" | Save first, or close with `force` |
| opening a malformed `.lbr` | "has errors and cannot be opened" | `_preflight_lbr` refuses it offline |

## The check before you ship any Fusion-touching code

1. List every command this path can send.
2. For each, ask: **can it prompt?** If unsure, assume yes.
3. For each that can: is it proven unnecessary offline, or does it carry `confirm`?
4. Is there a dismissal sweep doing load-bearing work? If yes, that is the bug.
5. Does any guard swallow its own failure? Make it shout.
6. Test by recording the commands that actually reach Fusion.

## Save checkpoints and close completed scratch documents

After edits, inspect `documentHygiene` in bridge responses. `isSaved` and `isModified`
are separate: a previously saved document can have new edits; a pristine blank Untitled
is not a dirty board. Unknown properties are not proof of a clean state.

At each completed routing/DRC milestone and before ending a task:

1. Save the working document, or export a fresh native checkpoint to a durable task folder.
2. Verify the file exists, has content, and can be reopened; log its path and hash with the
   exact document identity. Exporting/`Document.CopyToDesktop` may leave the live tab
   `isSaved=false, isModified=true`; do not report that as a Fusion save.
3. Close completed documents opened by this task, one exact identity at a time, after
   checkpoint verification. Do not infer ownership from a name prefix, close all tabs,
   delete recovery data, or discard edits in a pre-existing user document.
4. Re-enumerate documents. Leave only ongoing work and pre-existing user documents;
   Fusion may retain one pristine Untitled tab.

If Fusion reports that recovery-file creation failed, checkpoint immediately and then
save or close the affected task document. The warning alone does not establish why
recovery failed; inspect disk/path/permissions if it recurs in a clean session. Never
silence the warning by disabling recovery. The advisory response does not inspect
recovery health and does not automatically save or close documents.

## What the bridge does when a dialog still appears (1.10.17)

The bridge never restarts Fusion over a dialog. Before any verb that needs the add-in, and
after a verb times out behind one, it reads every dialog through UIA (title, text, button
names), applies the rule for the ones it knows, sends WM_CLOSE (Cancel or the X: never a
save or a discard) to the ones it does not, and runs the verb when that clears the screen
(`dialog_expired_retry` means re-run). A dialog that survives comes back as `blocked_by_dialog`
with `dialogs[]` (text and buttons) first and ab's parent/child screenshots second: answer it by
button name with `desktop_ui_click {hwnd, name}` or cancel it with `desktop_close_window {hwnd}`,
then re-run. Never restart Fusion for a dialog.

**What the bridge will never close (1.10.37).** A window no rule knows gets WM_CLOSE only when it is
plainly a dialog: smaller than Fusion's main window, not an EAGLE editor window, and carrying a
Cancel/Close/OK/No-style button. Fusion keeps other top-level Qt windows (EAGLE's own schematic and
board editors, window-sized frames), and WM_CLOSE on one of those closes documents and can exit
Fusion: it did on ConfRoomROG on 2026-09-22 (page issue #97). Such a window is left open and comes
back in `leftOpen[]` (from `fusion_dismiss_blocking_dialogs`) or as `blocked_by_dialog` with its text
and a `notClosableWhy`. Read the text and decide; do not send Escape or WM_CLOSE at it yourself.
