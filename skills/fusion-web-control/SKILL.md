---
name: fusion-web-control
description: >
  The library-import lane the wiki drives through Adom Bridge web control, and the same
  verbs an AI calls directly. Pre-flight planning with measured ETAs, the four-artifact
  evidence bundle, per-part 3D binding, progress polling, and every failure contract that
  now refuses rather than reporting a false pass. Read before importing a library into
  Fusion, before rendering a progress bar for one, or when a verb returns
  addin_restart_required, build_in_progress, blocked_by_dialog or no_evidence_captured.
  Trigger words - import a library into fusion, cloud libraries export, web control,
  fusion_create_library, fusion_plan_library_import, fusion_prove_result,
  fusion_build_library_3d, multi-part library, progress bar, ETA, evidence frames,
  addin_restart_required, restart fusion add-in, library import stuck.
discovery_triggers:
  - "import library into fusion"
  - "fusion web control"
  - "fusion_create_library"
  - "fusion_prove_result"
  - "addin_restart_required"
  - "library import progress"
---

# Driving a Fusion library import (web control, and the same verbs for you)

**There is ONE verb surface.** The wiki calling through Adom Bridge web control and you
calling `adom-bridge fusion_*` hit the same dispatcher. Anything web control can do, you
can do, with the same arguments and the same responses. There is no separate web-control
allowlist and no verb only one side can reach.

## The lane, in order

```
fusion_plan_library_import {parts}     -> pre-flight: state + costed plan, NO work done
fusion_create_library {libraryPath, stepPath}   -> one call: bind + prove, returns runId
   (or, for multi-part)
fusion_open_lbr {lbrPath}              -> dialog-guarded; a bad .lbr FAILS
fusion_build_library_3d {lbrPath, parts[]}      -> returns {runId, statusVerb}
fusion_prove_result {runId}            -> poll: stage, percent, etaSec, evidence[]
```

`fusion_prove_result` is the single poll surface for **both** the create chain and the
detached 3D build. No second poll verb to learn.

## The one-call bundle lane: `fusion_install_library_bundle` (#68)

The Fusion twin of `kicad_install_library_bundle`, so `adom-wiki library install --format
fusion` and the page's Export button read ONE contract and the recipe has one owner (this
bridge, not webcontrol.js). Stage the bundle, make one call, poll one verb:

```
write_file / write_file_chunk        -> C:/Temp/MyLib.zip   (<Lib>.lbr + <Part>.step models)
fusion_install_library_bundle {zipPath:"C:/Temp/MyLib.zip", libraryName?:"MyLib"}
   -> {installed:true, libraryPath, partsTotal, partsToBind, unbound[], partMetadata,
       fusion{status,message}, plan[], estimatedSec, runId, statusVerb:"fusion_prove_result"}
fusion_prove_result {runId}          -> poll every ~15s until stage:done; read verdict
```

- **Async by contract.** It returns immediately. A STEP bind is 3-15 minutes and cannot fit
  the 135s web-control envelope; there is no blocking mode on purpose (one contract shape).
- **Partial ships.** A deviceset with no model in the bundle is a NAMED skip in `unbound[]`
  (with `why`), the rest still bind and prove. `partsBound < partsTotal` is the signal.
- **Pairing is by name**: deviceset name, then package name, then a normalised (alnum,
  case-folded) match, then a prefix match. A one-deviceset, one-model bundle pairs regardless
  of names. `modelsUnused[]` lists STEPs nothing claimed.
- **Placed somewhere stable**: `~/Documents/Adom Libraries/<libraryName>/`, never a temp dir.
- **The rest is the existing chain**: preflight gate, part-number lint (`partMetadata`,
  reported not blocking), the `fusion{status}` pre-flight, the bind -> prove stages, the four
  proof frames per part, the banner, and the `addin_restart_required` payload the CALLER
  turns into the two-button ask. `capture:false` only places the files.
- `lbrPath` instead of `zipPath` takes a bare staged `.lbr` with its STEPs beside it.

## 1. Always pre-flight before you commit a user to anything

`fusion_plan_library_import {parts: [...]}` **does no work and changes nothing.** It answers
the four questions a caller needs before starting:

| `fusion.status` | What it means | What to do |
|---|---|---|
| `not_installed` | Fusion is not on this machine | Offer to install it; the plan includes the install stage (10-30 min) |
| `installed_not_running` | Just needs launching | Proceed; launch is a stage, ~60s, folded into the ETA |
| `needs_signin` | Running, but at the Autodesk sign-in screen (bridge 1.10.1+) | Drive the sign-in (fusion-autodesk-signin skill); board verbs return this at once, never a restart diagnosis |
| `blocked_by_dialog` | A dialog survived the bridge's safe expiry (rules, then WM_CLOSE) | Read `dialogs[]` (text, buttons); answer by button name, then re-run; never restart |
| `addin_restarting` | Running and signed in, add-in not loaded; the bridge is restarting Fusion itself (restartPolicy auto, the default: documents are saved first, and nobody's unsaved work is discarded) | Poll `fusion_readiness`, then continue |
| `fusion_addin_busy` | Running, add-in loaded but working (a long command starves even its status probe); readiness says `addinBusy` | Wait and poll `fusion_readiness`; never restart for it |
| `restart_needs_decision` | A restart stopped short of unsaved work the bridge could not save (nothing killed or discarded) | Show `userMessage`; restart again once the work is saved |
| `addin_restart_required` | Same, on a box whose prefs.json sets `restartPolicy: ask` | Show `userMessage`, then `fusion_restart_for_addin` on a yes |
| `ready` | Good to go | Start |

It returns `plan[]` with a per-stage `estimatedSec` and `confidence`, plus `estimatedSec`
and `estimatedSecIfAlreadyOpen`. **The bind estimate scales with the part count** — a
multi-part library does one STEP import per part, which is the slowest stage — so a 7-part
bundle is costed as 7, not as 1.

`confidence: "measured"` means this machine has actually run that stage before and the
number is its own p50. `"typical"` means it is still the seeded literal. Say which, if you
show the user a time.

## 2. The add-in restart contract (the one that burned a real user)

A new user installed ab with **Fusion already open**. The bridge wrote a fresh add-in;
Fusion loads add-ins only at launch, so the running Fusion could not see it, and every verb
failed with a bare "add-in not responding". He had no way to know the product was fine and
merely needed a relaunch.

Any verb needing the add-in now returns:

```json
{ "errorCode": "addin_restart_required", "restartRequired": true,
  "userMessage": "Fusion needs a quick restart to finish setting up the Adom add-in.
                  We can save your open documents first.",
  "documentsKnown": true, "openDocuments": 3, "unsavedDocuments": 2,
  "remedyVerb": "fusion_restart_for_addin" }
```

- **Show `userMessage`.** An ab toast also fires, so the human is told even if you drop the payload.
- **Branch on `documentsKnown`.** The counts come from the add-in, which by definition is not answering; when it cannot be reached they are `null` rather than guessed.
- **Ask before restarting when a human is present.** Then `fusion_restart_for_addin {save:true}`, which saves first and **refuses** with `unsaved_named_document` rather than discarding a never-saved document.

### The chain asks too (proven 2026-08-29, bridge 1.9.255)

With the add-in's server stopped on purpose inside a running Fusion, `fusion_create_library`
now stops at "asking you whether to save and restart it" with `errorCode:
addin_restart_required`, `userMessage`, `documentsKnown`, `remedyVerb` and `notificationId`
in the run state, fires the two-button toast, and waits (30 min). Fusion's process is
untouched while it waits (same pid before and during). Answers:
- `fusion_restart_for_addin {save:true}` (the page button; under `restartPolicy: ask` also the toast's "Save and restart"):
  Fusion restarts, the add-in answers, the SAME run continues to `proven` with
  `restartChoice: "restart"`.
- `fusion_restart_for_addin {decline:true}` (the page's "I'll handle it"; the toast button
  records the same hold): the run fails `user_owns_restart`, Fusion is untouched, and
  `fusion_start` refuses with `user_owns_restart` until the user restarts Fusion themselves.
Before 1.9.255 the chain fired nothing and `_handle_launch` killed Fusion after a 30 s gate;
only direct add-in verbs ever asked. `fusion_addin_status` also said `ok` to a dead add-in;
it now returns `addin_not_answering`.

## 3. Reading progress honestly

`fusion_prove_result` gives `stage`, `stageLabel`, `plan[]`, `stagesDone`, `percent`,
`etaSec`, `confidence`, and `evidence[]` that fills in **as each artifact lands**, in
assembly order: **symbol → footprint → 3d → ganged**. The ganged view is last by design, as
the closing proof. A 3D build additionally reports `minting <part> (3/7)`, `partsDone`,
`partsTotal` and `parts[]` with each `wip_urn`.

**A user can stop a run.** Render a Cancel button while you poll and wire it to
`fusion_cancel_run {runId}` (bridge 1.9.273+). It is cooperative: the run finishes the part
it is on (a STEP import in progress cannot be interrupted, up to ~60s), keeps every 3D
package already minted and binds them, closes its scratch, and leaves the library open in
Fusion (not saved to the cloud). Your next poll shows `cancelRequested: true`, then
`stage: "cancelled"`, `verdict: "cancelled"`, `cancelledAt` (which stage) and how many parts
were done. Never kill the bridge to stop a run: that is `bridge_restarted_mid_run`, and it
leaves the scratch documents and an unsaved library behind.

**`stage: "done"` does NOT mean it worked.** It means the chain finished. The score is the
top-level `verdict` (`proven` / `not_proven`). For a multi-part build, `done` means the run
completed, not that every part bound — **check `parts[]` for any `success: false`** before
telling a user their library is complete.

## 4. Failure contracts — these refuse instead of lying

Every one of these exists because it once reported success and someone believed it.

| errorCode | Meaning | Response |
|---|---|---|
| `no_evidence_captured` | The chain finished with ZERO frames | Not a success. Investigate |
| `blocked_by_dialog` | A blocking dialog was up; **`dialogText` carries what Fusion is showing** | Show the text; clear the condition |
| `build_in_progress` | A 3D build is still running, so a capture would photograph the build editor | Wait, then re-capture |
| `wrong_editor_captured` | A frame was taken outside the Electronics Library editor | The frame does not show what it claims |
| `no_3d_package_bound` | No `<package3d>`, so the only thing to photograph is a grey Placeholder | Bind 3D first |
| `unsaved_named_document` | A never-saved document cannot be closed silently | Save with an explicit destination first |
| `no_path_argument` | No library path given; **the file was never examined** | Do not re-stage the file; fix the argument |
| `artifact_not_in_library` | The name is not in the `.lbr`, so no EDIT was issued | Pick from `available[]` |

**The doctrine behind the whole table:** a non-result reported as success is the worst
failure an evidence pipeline can have, because it is the one nobody re-checks. Prefer a
refusal that names the cause.

## 5. Arguments are forgiving; paths are Windows-local

`fusion_open_lbr` accepts `filePath`, `lbrPath`, `libraryPath` or `path` — you should never
have to guess our spelling. Paths must be **Windows-local** (`C:/...`): the bridge runs on
the user's machine, and a container path means nothing to it.

**Paths with spaces (fixed 1.9.247).** Fusion's `executeTextCommand` is space-delimited. The
bridge used to build `Document.newDesignFromLocal C:/Users/john/Documents/Adom Libraries/...`
raw, so Fusion opened `.../Adom` and answered `3 : Filename not found`; the add-in's own
quote-then-8.3 fallback never ran because the bridge bypasses it. Measured inside Fusion's
Python: the quoted form ALSO fails with error 3. (History: until 1.10.10 the bridge then used the 8.3 short path; since 1.10.11 it STAGES the file into a space-free folder instead, because Fusion titled documents after the alias.) The Windows 8.3 short path with forward
slashes (`C:/Users/john/DOCUME~1/ADOMLI~1/RAY-DA~1/part.lbr`) opens first try. Every
bridge-side text command that embeds a path now goes through `_fusion_text_path()`. A user
whose profile folder has a space (`C:/Users/John Doe`) hits this on every import otherwise.

## 6. Never let Fusion draw a dialog

A modal freezes the add-in's single main thread, so the bridge stops answering and every
later verb reads as a crash. See the **fusion-no-modal-dialogs** skill. Short version:
prove a command unnecessary offline, or pre-answer it — never issue one and plan to sweep up.

## Part numbers and variants (an EE cannot order without these)

Geometry is not the part. MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN and LCSC_PN, and the
variant model that lets an automotive-grade part and a baseline one share one symbol, one
footprint and one 3D body, are covered in **`fusion-part-metadata`**. Check
`fusion_library_parts {libraryPath}` before calling any library complete - `ok: true` from
preflight only means the file OPENS.

## Every library is multi-part. One part is N=1.

There is no single-part import verb to choose, deliberately. **Part count is data, not a
different operation**, and a caller should never have to know how many parts a library holds
before they can pick a verb.

```
fusion_create_library {parts: [{package, lbrPath, modelPath}, ...]}   <- any count
fusion_create_library {libraryPath, stepPath}                          <- sugar for N=1
```

The second form is normalised into a one-element `parts` list, so both run the **identical**
code path, through the same binder, with the same per-part progress and an ETA that scales
with the count. A one-part library is not a special case with its own bugs.

`fusion_bind_library_3d` and `fusion_attach_3d_package` still exist as **low-level
primitives** for a single deliberate bind. They are not the import lane, and reaching for
them because "this library only has one part" is the mistake this unification removes.

**History, so nobody re-introduces it:** this used to be a singular family
(`create_library{stepPath}`, `bind_library_3d{stepPath}`, `attach_3d_package{modelPath}`)
beside a plural one (`build_library_3d{parts[]}`). Two families, split on the wrong axis.

## Evidence scales with the library: 4 frames per component

Every part is proved, not just the first. A **10-component library streams 40 frames** —
four per component (symbol → footprint → 3d → ganged), in assembly order, published the
instant each is captured.

Each frame carries `part`, so a front end that renders frames as they arrive can group them
per component rather than showing one undifferentiated pile:

```json
{ "stageLabel": "proving U7 (7 of 10)",
  "evidenceCount": 26,
  "partsToProve": 10,
  "provedParts": [ {"part":"U1","proven":true}, ... ],
  "evidence": [ {"part":"U7","step":"footprint","ok":true,"screenshot":"..."} ] }
```

**The library is proven only if EVERY part is.** One good part does not vouch for nine
unchecked ones — `prove.partsProven` vs `prove.partsTotal` is the real score, and
`stage: "done"` still only means the chain finished.

## The in-Fusion banner (what the user sees at Fusion during an import)

While a chain runs, a docked "ADOM IS DRIVING FUSION" palette shows the same stageLabel,
percent and ETA the wiki sees, plus a "please do not click, type or close documents"
advisory. It resolves green ("Library imported, N of N parts proven") or red with the error,
lingers ~20s, and hides. Drive it directly with `fusion_activity_banner {action, title,
detail, percent, etaSec, tone}` if you orchestrate the steps yourself. It is an ADVISORY:
Fusion has no read-only mode, so a determined click still lands. Verified on hardware.

## A notice that offers an action must carry the choice

John, 2026-08-28: *"i just clicked 'got it' on the restart notice, but why do you not give
me a choice? i have no idea if 'got it' means you're restarting or not."* The toast's body
promised "we will save your documents first" while its only button was an acknowledgement:
the text made an offer the UI could not keep. That is lazy, and it leaves the user exactly
where Adonis was.

Under `restartPolicy: ask` (NOT the default since 1.10.6: the default `auto` raises no toast and the bridge restarts Fusion itself when the add-in is truly gone, never over a dialog and never over unsaved work it could not save) the restart notice offers **"Save and restart"** and **"I'll handle it"**, and the
bridge honours whichever is pressed (`_watch_restart_choice` polls `notify_response` and
either runs `fusion_restart_for_addin {save:true}` or records the decline). Both land in
the action ledger, so "why did my Fusion restart" has a timestamped answer.

Rule for any toast: if the body describes something the bridge *could do*, the buttons must
let the user say yes or no to exactly that. A bare "Got it" is only for pure FYI.

## Prove is for web control; an AI thread binds only (1.10.17)

`fusion_install_library_bundle` and `fusion_create_library` decide the prove step from the
caller: web control (anonymous, the wiki page shows the frames) proves; an AI thread (caller
headers set) binds and places only and gets `verdict: bound_not_proven` with `partsBound` and
the library path. `prove:true` asks for the four frames per part (about 300 s per library);
`fusion_capture_library_views {packages}` shows one part on demand. A chain never restarts
Fusion on its own: pass `allowRestart:true` with `restartReason` or it surfaces
`addin_restart_required` and waits, resumable; any restart it did perform is in `restarts[]`.
