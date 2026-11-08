# Porting the Fusion bridge core to another OS (Kyle: this is your page)

The workspace is `core` (OS-blind), `lbr` (the .lbr engine, pure), `os-win` (ours, shipped),
`os-mac` (yours, a compiling skeleton), and `bridge` (the exe, which picks the OS crate by
target and nothing else).

## The whole OS boundary is two traits

`fusion_core::host::HostProcess` and `fusion_core::host::HostPaths`. That is it. If you find
yourself wanting a third, first ask whether the thing is an Adom Bridge verb: window
enumeration, clicks, UIA, screenshots, process list/kill, captions, the demo panel are all ab
verbs and identical on both OSes, so the core calls them through `fusion_core::ab` and never
needs the OS.

Every `todo!()` in `os-mac/src/lib.rs` names what the Windows crate does for that method, so
each one is a translation:

| method | Windows (os-win) | macOS (yours) |
|---|---|---|
| `fusion_processes` | Toolhelp32 snapshot (poll every few seconds; must be in-process) | `proc_listpids` / `sysctl` for `Autodesk Fusion` |
| `launch_fusion` | CreateProcess + CREATE_BREAKAWAY_FROM_JOB (survives bridge respawn) | `open -a "Autodesk Fusion"` |
| `is_elevated` | IsUserAnAdmin | `geteuid() == 0` |
| `fusion_executable` | newest `webdeploy/production/<hash>/FusionLauncher.exe` by mtime | `/Applications/Autodesk Fusion.app` |
| `addin_dirs` | `%APPDATA%/Autodesk/FusionAddins` + the legacy `API/AddIns` dirs (Fusion moved it; write to all) | `~/Library/Application Support/Autodesk/...` (confirm on a real box) |
| `fusion_log_dir` | `%LOCALAPPDATA%/Autodesk/Autodesk Fusion 360/<userId>/logs` (CEF alert toasts live only here) | the equivalent under `~/Library/Application Support` |
| `bridge_state_dir` | `~/.adom/fusion-bridge` | same |
| `scratch_dir` | `%TEMP%/adom-fusion` | `$TMPDIR/adom-fusion` |

## The add-in is shared

`addin/AdomBridge` is Python, runs inside Fusion, and Fusion's Python API is identical on
macOS. It is a thin executor (HTTP listener, main-thread marshal, script exec); the bridge
sends it script templates. You ship the same folder; only `addin_dirs` differs.

## Rules that keep the core portable

- `core` never depends on `windows`, `cocoa`, `objc` or any OS crate. `tools/check-portable.sh`
  fails the build if it does.
- No OS path string appears in `core`; every path comes from `HostPaths`.
- The verb catalog is `contracts/describe.json`, frozen from `describe.py`. Both bridges serve
  it identically; the router asserts every catalogued verb has a handler.
- Naming: the host app is Adom Bridge, "ab". `tools/check-naming.sh` fails on any remnant of the
  retired names.

## Building

```
cd rust && cargo build --workspace        # on Linux CI this builds core, lbr and the exe with a stub host
cargo build -p adom-fusion-bridge         # on Windows picks os-win; on macOS picks os-mac
cargo test -p fusion-core                 # the frozen catalog must parse and be complete
```

## Status (2026-09-03)

- **Phase 0 + Phase 1 shipped on the insiders tier (1.9.350).** ab launches
  `adom-fusion-bridge.exe --port N` (`spawn.kind: exe` in bridge.json); the exe spawns
  `server.py` beside it as a Python sidecar on a free loopback port and proxies every
  unported verb to it with the caller headers intact. `/status` is served by the exe and
  folds in the sidecar's rich status; a dead sidecar shows red at once with its exit code.
- **fusion-os-win is real**: Toolhelp32 poll, `cmd /c start "" /min` launch with
  CREATE_BREAKAWAY_FROM_JOB, IsUserAnAdmin, webdeploy / add-in / AppLogFile paths ported
  from `fusion_detect.py`, `install_addin.py`, `server.py`. The sidecar is tied to the
  exe's lifetime with a job object (KILL_ON_JOB_CLOSE + BREAKAWAY_OK, so the sidecar's own
  Fusion launch can still break away). Proven: hard-kill the exe, the sidecar is gone.
- **Python for the sidecar** comes from `ADOM_SIDECAR_PYTHON`, else ab's `runtimes` verb
  over the loopback API, else a scan of `~/.adom/adom-runtimes/python-*` that skips
  `.broken` quarantines. ab cannot yet provision Python for an exe bridge (asked on
  adom/adom-bridge); on a machine with only this bridge the sidecar is absent until then.
- **Kyle**: `fusion-os-mac` still carries `todo!()` stubs; `tools/check-portable.sh` only
  guards the crates the Windows exe links, so your crate can land at its own pace. The exe
  side is `rust/bridge/src/main.rs`; nothing in it is Windows-specific except `host()`.
- **Guards run in the release**: `check-portable.sh` (core is OS-blind, no `todo!()` in a
  linked crate) and `check-naming.sh`, then the mingw cross-build, then the exe joins the
  zip at its root beside `server.py`.
- **Native verbs in the exe so far (2026-09-03, parity-checked against the Python bridge on
  the same box):** `fusion_alerts`, `fusion_action_ledger`, `fusion_verb_times`, `fusion_library_parts`, `fusion_preflight_lbr`, `fusion_aps_get` (GET), `fusion_aps_browse`, `fusion_aps_search`, `fusion_aps_recent`, `fusion_aps_file_info`, `fusion_aps_versions`, `fusion_aps_download` (byte-identical file), `fusion_aps_create_folder`, `fusion_aps_upload`, `fusion_aps_status` (all on the shared `~/.adom/fusion-aps` token store, parity-checked on the laptop hub). The pattern
  for the next one: match the verb in `main.rs`, answer the same JSON shape (plus
  `shell: "rust"`), keep the Python hint text verbatim, and prove parity with a spare-port
  smoke before the release. The `.lbr` engine, bundle staging, and alert parser are ported
  with goldens; the APS client (token store + Data Management calls) is the next leaf, carrying the new permissions and member verbs.
- **Sidecar Python on a fresh box:** the exe asks ab for `runtimes {ensure:"python"}` (ab
  2.1.14+) and waits, bounded, for `state: ready`. Older ab answers the snapshot only.
- **Readiness is a chain, not a leaf.** `_handle_fusion_readiness` is 489 lines with side
  effects (prefs, add-in auto-restart, seat-dialog auto-resolve, Fusion update handling).
  It ports in Phase 3 behind the drill, not as a Phase 2 leaf; the pieces it needs
  (Toolhelp poll, launcher path, log dir, alert parser, add-in probe) already exist natively.
- **The import chains run in the exe (2026-09-03, `rust/bridge/src/chains.rs`), behind the
  pref `chainsOwner: "exe"` in `~/.adom/fusion-bridge/prefs.json`, exactly like
  `readinessOwner`.** `fusion_create_library`, `fusion_install_library_bundle` and
  `fusion_prove_result` are orchestrated by the exe step for step from server.py: the same
  run store (`%TEMP%/fusion-prove-runs.json` + `fusion-prove-last.json`, so cancel and
  polling work from either side and `owner: "exe"` is the only tell), the same stage plan
  with `timings.json` estimates, the launch gate (`chain_addin_restart_info`, the
  `user_restart_hold.json` hold), the bind (`build_library_3d` on the sidecar, busy pre-wait,
  40x3s main-thread-busy retries), the per-deviceset proof (`prove_library`, shared-package
  skips, one transient retry), the cloud publish and the closing hygiene. The leaves that
  still drive Fusion run on the sidecar through the `chain_*` executor verbs; the bundle
  unpack/pair/place/split is the lbr crate. Frames stream while a proof call is in flight:
  the exe passes `_frameSink` (a file path) and `_handle_prove_library` appends one JSON
  line per frame, which the exe tails every 1.5s and publishes to the run store, so a
  poller sees the ganged view arrive last exactly as before.
- **Lesson (cost one soak run): a `std::sync::Mutex` locked twice in ONE statement
  deadlocks the thread silently.** `Some(100.0 * (*m.lock().unwrap() + x))` followed by
  `Some(y - *m.lock().unwrap())` in the same call expression keeps the first guard alive as
  a temporary until the statement ends, so the second lock waits forever; no panic, no
  log line, the run just sits at its last saved label while the banner watchdog keeps
  re-sending. Progress counters a single chain thread owns are a `Cell`, and any guard
  whose value feeds a long call is cloned into a local and dropped before the call
  (`let last = m.lock().unwrap().clone();` then the HTTP call), never held through an
  `if let` body.
- **Phase 4 has started: the add-in pass-through is exe-owned behind `addinOwner: "exe"`
  (`rust/bridge/src/addin.rs`).** Every ADDIN_COMMANDS verb is sent to the add-in on :8774
  by the exe: the two busy gates (own long command, add-in /status busy), the per-command
  timeouts, assembly_bom's treatAsUnit defaults, the raw close_document upload gate, the
  stale-add-in enrichment, the API-error classifier and the background annotation are
  native and word-for-word. The Python etiquette that still drives Win32 stays on the
  sidecar as executors the exe calls around the HTTP hop: `chain_pre_verb` (document
  snapshot, foreground reason or the background envelope, dialog pre-dismiss for a walk),
  `chain_addin_gate` (the preamble: installed / running / auto-start / hang gate / add-in
  restart contract, extracted verbatim into `_addin_preamble`), `chain_sentinel` (the
  save-prompt sentinel around a doc-switching call), `chain_diagnose_addin_timeout`,
  `chain_dialog_array` (mutating verbs), `chain_verb_indicator` (taskbar bar / flash) and
  `chain_post_verb` (documents we opened, bridge docs, the surface shot, the chokepoint
  sweep, the etiquette note, the recent-actions ledger). The Python dispatcher calls the
  SAME extracted functions (`_window_etiquette_pre/_post`, `_addin_preamble`), so the two
  shells cannot drift. Porting those seven executors natively (Win32 envelope, UIA clicks
  through ab, window facts) is what stands between here and deleting the sidecar.
- **More native leaves (2026-09-03 evening):** `fusion_set_auto_update`, `fusion_set_seat_takeover`,
  `fusion_set_window_sizing`, `fusion_set_driving_badge` (pref toggles, always native),
  `fusion_addin_status`, `fusion_plan_library_import`, `fusion_notify_owner` (loopback
  notify_user with the CLI fallback), `fusion_describe` (the frozen contract), and behind
  `addinOwner:"exe"`: `fusion_get_preferences` / `fusion_set_preference` (the preference
  script is `rust/bridge/src/pref_script.py`, pinned byte-for-byte to server.py's
  `_PREF_SCRIPT` by a test), `fusion_reset_workspace`, `fusion_close_window` and
  `fusion_screenshot_fusion` (ab's desktop_close_window / desktop_screenshot_window, never
  our own Win32), `fusion_dismiss_blocking_dialogs` (the title classifier is
  `fusion_core::dialogs` with `dialog_rules.json` generated from `handlers/dialog_classify.py`
  and goldens in `core/tests/golden/dialog_titles.json`; the dialog rows come from ab's
  desktop_enum_windows, body text from desktop_find_controls). The desktop-shaped work goes
  through ab verbs on purpose (John's standing rule): the exe never grows its own EnumWindows.
- **Next native block, scoped 2026-09-03 late: the launch handler.** `_handle_launch`
  (server.py, ~340 lines) is what `chain_addin_gate`, `fusion_start`, the readiness
  self-heal and the timeout diagnosis's hang branch all lean on. Its pieces: detect (native
  already), the warm path (add-in probe, busy guard, the user restart hold, a 3x8s
  persistence gate, kill-then-relaunch), recovery-file relocation, `ensure_fusion_running`
  (the breakaway launch is native; the background envelope and the seat sweep are not),
  launch-dialog classification (native classifier now), `_resolve_seat_dialog` (UIA clicks
  through ab, capped at ten), `ensure_reasonable_window` (SetWindowPos through ab or a
  future ab verb), the add-in wait with the timings progress block, and the `launch|cold`
  and `launch|warm` samples (`timings::record` exists). Port it as `fusion_start` behind
  the same `addinOwner` pref, prove it through a real stop/start on ConfRoomROG, then
  `chain_addin_gate` collapses into it and the hang branch (`_confirmed_hung`) follows.
  After that the sidecar holds only the demo tour, the managed-library choreography, APS
  sign-in and the bind/prove leaves (`build_library_3d`, `prove_library`, `open_lbr`).
- **Native since the chain proof:** the timings store (`bridge/src/timings.rs`: rolling
  window, run log, verb estimate; stage samples now feed the estimate on BOTH shells, which
  Python had never done) and the taskbar indicator through ab's `desktop_taskbar`; the
  post-op dialog array and the failure-dialog guard for mutating verbs (classifier plus
  ab screenshots, no sidecar hop).
- **The etiquette layer is native (2026-09-03, late):** `bridge/src/etiquette.rs` carries
  the document registry (snapshot before, diff after, `bridgeDocs` in prefs), the foreground
  rule (reason plus the 3s heads-up through ab's notify_user, focus events to
  `focus-events.jsonl`) and the background envelope (`HostProcess::background_envelope`, the
  Win32 port in os-win: foreground-lock timeout, minimize animation off,
  LockSetForegroundWindow, the watcher that pushes a Fusion window back down unless the user
  brought it forward), the driving badge through desktop_taskbar, the save-prompt sentinel
  (Don't Save through desktop_ui_click for OUR documents only; an unattributed prompt is
  cancelled with WM_CLOSE only after two sightings with the main thread blocked), the
  chokepoint sweep (upload-close No, create-confirm No, launch-error OK) and the etiquette
  note. The three sidecar executors it replaced (`chain_pre_verb`, `chain_post_verb`,
  `chain_sentinel`) stay registered for the Python path and can be deleted when the sidecar
  goes. One difference to know: Python posted a WM_KEYDOWN Escape at an unattributed prompt;
  the exe sends WM_CLOSE through desktop_close_window, the same Cancel semantics without
  a PostMessage of our own. The surface shot for `capture:true` serves ab's own safe-size
  frame (at most 1400px) where Python resampled to 640px.
- **The launch handler is native (2026-09-04): `fusion_start` and the add-in preamble
  (`bridge/src/launch.rs`).** The warm path (busy guard, the user's restart hold, the 3x8s
  persistence gate, kill-then-relaunch), the breakaway launch plus the background envelope,
  the launch-dialog sweep (native classifier), the seat loop, window sizing through ab's
  `display` and `desktop_set_window_bounds` (restore:false, so nothing activates), the add-in
  wait with the measured progress block and the taskbar launch watch, and the `launch|cold`
  and `launch|warm` samples. `gate` is `_addin_preamble` word for word: auto-start, the hang
  branch, the restart contract. Proven on ConfRoomROG: warm start answered natively; after a
  stop, the cold start relaunched Fusion in the background and a gated verb answered once
  readiness flipped. Known shape, inherited from Python: a warm-but-silent add-in walks the
  persistence gate, the kill and the 50s launch loop in one call and overruns ab's 90s relay
  budget; the work completes, the caller sees `timed_out` and polls readiness. The sidecar
  still serves the Win32/UIA leaves under it: `chain_dismiss_recovery` (relocate plus the
  Recovered Documents dismissal), `chain_hung_check` (IsHungAppWindow plus the 20s probe),
  `chain_clean_incomplete_webdeploy`, `resolve_seat_dialog`, `chain_addin_restart_info` (the
  toast), `restart_for_addin`, `kill`/`stop`, and `chain_diagnose_addin_timeout`.
- **Lifecycle and the timeout diagnosis are native (2026-09-04):** `bridge/src/lifecycle.rs`
  carries `fusion_stop`, `fusion_kill` (ab's process_list / process_kill first, the hidden
  taskkill sweep as the degraded path), `fusion_restart_for_addin` (save through the
  pass-through, stop or kill, native start, the round-trip wait; the async form still runs
  on the sidecar's handler because a thread cannot borrow the request context) and
  `_diagnose_addin_timeout` (health and status probes, native classifier plus the seat modal
  by shape, screenshots of the main window and every dialog through ab, the hang verdict
  through `chain_hung_check`). Proven on ConfRoomROG: a native stop closed the document and
  the window, and the native cold start that followed came back ready inside one call.
  Sidecar executors still under the exe: `chain_hung_check`, `chain_dismiss_recovery`,
  `chain_clean_incomplete_webdeploy`, `resolve_seat_dialog`, `chain_addin_restart_info`.
- **Readiness executors and the first orchestration are native (2026-09-04):**
  `bridge/src/readiness_exec.rs` carries the seat resolver (UIA Continue / Check again
  through ab, verified by the parent's owned-popup count), the recovery relocation and
  dismissal (No and Close through UIA where Python grabbed the foreground for Tab+Enter),
  and the kill-grade hang verdict; `IsHungAppWindow` and `GetProcessTimes` are the two new
  audited Win32 reads in os-win, behind `HostProcess::window_hung` / `family_cpu_ticks`.
  `bridge/src/orchestrations.rs` carries `fusion_open_lbr` (preflight, the verified close of
  a same-named bridge doc, `Document.newDesignFromLocal` through the pass-through with the
  8.3 short path from `HostPaths::short_path`, the slow-open state check, the symbol EDIT,
  the export verification, the part-metadata report, the post-open capture), proven on the
  Demo Board library. The remaining sidecar work is orchestrations (the show-surface
  family, make_test_project, capture_library_views, the bind and prove leaves, the cloud
  opens, board_stackup, generate_package, the GLB export), the demo tour and live feed,
  managed libraries, the sign-in family, install and uninstall, prefs_open/close,
  set_design_rules, fusion_state, apply_fusion_update, and the MCP proxy.
- **`fusion_show_symbol` / `fusion_show_footprint` are native (2026-09-04):** name resolution
  without asking Fusion (exact, case-insensitive, via the deviceset, difflib-style close
  match, stem prefix), the wrapper library for bare fragments (skeleton pinned to server.py's
  constants by a test), the active-library guard with reopen, the EDIT with the busy and
  Design-workspace retries, the Create-new prompt declined inside the call, WINDOW FIT and
  the background capture. Proven on the Demo Board library: a near-miss name resolved, a
  footprint shown from the remembered library, a missing name refused with `available`.
- **`fusion_show_library`, `fusion_show_project` (query and urn), the open-with-screenshot
  family (`show_schematic`, `show_2d_board`, `show_3d_board`, `open_schematic`, `open_board`,
  `import_electronics`) and `fusion_state` are native (2026-09-04).** The generated-test-project
  branch of show_project stays with the sidecar (its state is in-memory there). `fusion_state`
  reads the focus log from `focus-events.jsonl`, the timings store, ab's window rows, the
  native classifier and the seat-by-shape rule, the add-in status, the launch progress block,
  the driving badge the exe itself set, and captures through ab.
- **`fusion_capture_library_views` is native (2026-09-04):** name resolution up front, the
  build-in-flight guard (`builds_in_flight.json`, written by the sidecar's async build), EDIT
  per view with the background dialog sweep, WINDOW FIT, labeled captures copied to
  `C:/tmp/conduit-screenshots` (PNG through ab where Python without PIL wrote BMP), the
  workspace check that marks a frame `wrong_editor_captured`, and the 3D view through
  Create3DPackage / Package3DStop. **Ship discipline:** `tools/rust-ship.sh` is the gate
  before a commit or a deploy (both targets, tests, guards); a grep-filtered cargo pipeline
  once hid a type error and a broken commit was pushed.
- **The bind and prove leaves are native (2026-09-04): `bridge/src/mint.rs` and `prove.rs`.**
  `make_3d_package` sends the SAME Fusion-side script server.py builds (the template is
  derived from a Python-generated golden, `tests/golden/make3d_script.json`, and a test pins
  it byte for byte), through the pass-through; the workspace folder discovery and the APS
  upload fallback use the exe's APS client. `build_library_3d` loops the mint, injects every
  binding in one pass (lbr crate) and opens once; detached by default with progress in the
  shared run store and `builds_in_flight.json`. `bind_library_3d` is the single-part form.
  `prove_library` decides the verdict from the file (artifact names, connects), captures the
  four frames measured and workspace-stamped, counts the bodies in the bound f3d, checks the
  cloud thumbnail (`check_3d_thumbnail`, APS), and streams each frame to `_frameSink`. With
  `addinOwner: "exe"` the exe chain calls these natively instead of the sidecar's copies.
  **Field note:** the regression drill on 2026-09-04 stalled in bind three times running
  because Fusion on ConfRoomROG was applying an auto-update (readiness: `updating`) and
  restarted itself every minute; that is the environment, not the port.
- **More orchestrations native (2026-09-04, midday):** `capture_library_views`, `open_cloud_file`,
  `save_lbr`, `cleanup_cloud_files`, `inspect_bodies` (its script file pinned), `board_stackup`,
  the optimized-GLB export family (sync, fetch by jobId, the detached form polled by exportId),
  `set_design_rules` (the wiki rule packs, cached an hour), `apply_fusion_update` (the update
  offer found through ab's desktop_find_control, exit-class buttons only when verifiably idle),
  `install_fusion` (the official streamer, per-user when not elevated), `uninstall`, and
  `make_test_project` (the .sch/.brd generator, four Python-generated golden cases byte for
  byte). Two more discipline rules from today: `tools/rust-commit.sh` gates before it commits
  (a grep-filtered pipeline let two broken commits out), and every Fusion-side script the exe
  sends is derived from a golden the Python side generated, never retyped.
- **Drill three (2026-09-04, run through Fusion's auto-update):** verdict proven, 5 of 5, 20
  frames, even though the update storm killed the add-in during the bind of three parts;
  those parts proved on the bindings the fresh install carried over (`inject_package3d_bindings`
  merge). The chain's hygiene, the sidecar's auto-restart and the readiness `updating`
  detection all did their jobs; nothing in the port needed a hand.
- **Native prove, proven live (2026-09-04):** `fusion_prove_library` on the Demo Board's CH340G
  through the exe: proven, four frames at 1568x848, symbol and footprint and ganged stamped
  Electronics Library, the 3D frame stamped Design with one body counted, the scratch closed.
  The relay times the call out (a proof takes minutes) and the verdict is read back from the
  run store, exactly as with the Python bridge. `fusion_live_feed`, `fusion_aps_open`,
  `open_design` and the EAGLE import are native too.
- **Member and permission verbs** (Data MCP parity) live on the Manufacturing Data Model
  v3 GraphQL API (`/mfg/v3/graphql/public`, mutations `addProjectMembers` /
  `addHubMembers`), which is metered past a subscription's included quota since
  2026-08-17. They are NOT on the free-endpoint allowlist; they ship only behind an
  explicit `allowMetered:true` opt-in with the human's words attested, after John decides.

## Phase 3 plan: `fusion_readiness` (mapped 2026-09-03 from server.py 2025-2514)

Readiness is the launch observer, the self-healer and the state store in one 489-line
handler. Port it as FOUR layers, in this order, each shippable behind the drill:

1. **The snapshot (pure, in-process).** `installed` (webdeploy complete, os-win),
   `streaming` vs `running` (Toolhelp), `installerRunning`, the add-in HTTP probe
   (`127.0.0.1:8774/status`, 0.4 s), the MCP port probe, APS status (native already), and
   the prefs store (`<state>/prefs.json`: lastReadyAt, lastSignedInAt, launchStartedAt,
   launchHealedAt, launchHealAttempts). No side effects. Parity-test against Python on a
   quiet box: identical `installed/running/addinReady/ready` and the same hint branch.
2. **The window facts (ab verbs, never local Win32).** `needsSignin` from the Fusion
   process's own window title ("Signing in - Autodesk Fusion"), and the seat/licensing
   dialog as an OWNED POPUP of the main window: both come from `desktop_list_windows`
   (the owner/pid fields ab added). No screenshots, no UIA at this layer.
3. **The side effects, one at a time, each behind a pref flag so the sidecar and the exe
   never both act:** the seat-dialog auto-resolve (`desktop_ui_click` Continue plus a
   screenshot verify), the silent Fusion auto-update handling, the crash-report janitor
   (`RELAUNCH_CATEGORIES` dialogs cleared and ledgered), and the launch self-heal
   (`stop`/`kill` then `start` on a background thread after the measured overdue window,
   capped at four attempts). Every action goes through the ledger with caller provenance;
   `_host_op_active()` (the shared guard from issue #52 item 3) gates all of them.
4. **The progress block and the hints.** `timings.progress_block` for the launch (measured
   per start kind), the hint text per branch, `addinStartFailed` (the marker from issue
   #75), `lastReadyAt` / `lastSignedInAt`.

Gate for each layer: the daily battery green through the exe, the five-part drill, and the
electronics tour, on ConfRoomROG. The sidecar keeps serving readiness until layer 3 lands;
the exe answers `/status` from layer 1 alone the whole time (it already does).

Ask of ab before layer 2: `desktop_list_windows` rows carry `owner`, `pid`, `hung` (asked
in the roadmap's ab list); `desktop_wait_for_window` replaces the four polling loops.
- **Phase 3 layer 1 landed (2026-09-03):** the exe builds the same `/status` body as the
  Python bridge (`installed`, `exe_path`, `addins_dir`, `addin_installed`, `running`, the
  add-in probe, led / summary / tooltip), cached 5 s; with a sidecar present it rides as
  `nativeSnapshot` beside the sidecar's answer. Proven identical on ConfRoomROG.
- **Phase 3 layer 2 landed (2026-09-03):** window facts from ab's `desktop_list_windows`
  (owner, pid, image, kind, rect: no ab change was needed): the sign-in title and the seat
  dialog shape rule, with the field cases as unit tests. Porting it found a live false
  positive shared with Python: Fusion's 640x196 message tray is an OWNED, LANDSCAPE tool
  window titled "Fusion360", so both rules called it the seat dialog and readiness sat at
  ready:false on a healthy Fusion. Fixed on both sides in 1.9.354 (a tool window is never a
  modal); ConfRoomROG went ready:true the moment it installed.
- **Layer 2 proven under ab (2026-09-03, ConfRoomROG):** with the exe spawned by ab (so it
  holds the bridge token that exempts a bridge's own calls from ab's caller-identity gate),
  the native window facts came back live: no sign-in, no seat dialog, the real main title;
  the Python readiness agreed. A hand-launched exe cannot pass that gate and reports the
  facts as `windowFactsDegraded` with the refusal text, by design.
- **Flipping a test box to the exe by hand:** copy the exe into the bridge cache dir, edit
  ONLY the two `spawn` strings in `bridge.json` as text (a JSON round-trip through PowerShell
  changed the file's shape and ab fell back to its bundled 1.6.89 seed), then
  `refresh_bridges` (ab's registry re-reads bridge.json only on a rescan or an install) and
  `bridge_kill {confirm:true}`. The next `bridge_install` restores the manifest's shape.
- **Readiness in shadow (2026-09-03):** `fusion_readiness` is ported as a pure computation
  (`fusion-core::readiness`, sixteen Python-generated golden scenarios: result, hint text,
  pref writes), and the exe computes it from real probes beside the sidecar's answer as
  `nativeReadiness {result, shadowDiff}`. On a live ready Fusion the diff was only the
  second of the `lastReadyAt` / `lastSignedInAt` stamps (two writers). Side effects come
  back as `actions` and are not run until the exe owns readiness.
- **The sidecar's port must be outside ab's allocator range.** ab persists the port it
  assigned and, on a respawn, probes it first and routes to whatever answers there. The
  sidecar's ephemeral pick landed on that exact port once (Windows returns the last freed
  port) and ab bypassed the exe for a whole session. The sidecar now binds in 20000-29999;
  `/status` carries `exePort` and `sidecarPort` so the two are never confused again.
- **ab's bundled-bridge collapse can adopt the sidecar.** `BUNDLED_SPECS` calls any
  `python.exe` whose `/health` mentions Fusion "the fusion bridge" and re-pins routing to it,
  which bypassed the exe for a session. The sidecar therefore runs as
  `adom-fusion-sidecar.exe` (a copy of ab's python launcher beside its DLLs) and binds in
  20000-29999. Proven stable on ConfRoomROG; ab ask filed (adom-bridge, collapse should key
  on `spawn.kind`). Diagnose a bypass in one call: `fusion_shell_info` answers
  `shell: rust` only when the exe is in the path.
- **Shadow readiness across a real stop/start (ConfRoomROG, 2026-09-03):** identical at
  every stable state (ready, stopped, ready again). The one divergence was the poll at the
  instant the add-in came up: Python's 1.0 s add-in probe timed out under launch load and
  said "add-in not responding", while the exe's probe (1.5 s) got the add-in's answer and the
  round-trip passed. A boundary race, not a logic difference; the exe's answer was the true one.
- **Every request on its own thread (2026-09-03).** The shell's request loop was sequential,
  so a long verb (`fusion_start`, a bind) blocked ab's health poll and ab reaped and
  respawned the exe twice during one Fusion launch, orphaning the run: the same lesson the
  Python bridge learned on 2026-08-29. Now each request runs on its own thread (tiny_http
  hands them out concurrently); a blocking `fusion_start` through the exe produced no
  respawn and readiness agreed afterwards.
- **The registry lints every `.lbr` in the repo tree on `pkg publish`**, not only the files in
  `page.json.files`. Golden and fixture libraries therefore carry the `.lbrx` extension
  (they are deliberately broken or bound to a test account's cloud), and shipped samples
  carry no `<package3d>` urn (a urn is a pointer into the minting account, nothing for
  anyone else). `adom-lbr validate <file>` shows the same findings locally.
- **Readiness ownership switch (2026-09-03).** `prefs.json` `readinessOwner: "exe"` makes the
  exe serve `fusion_readiness` itself: it gathers, acts (seat dialog and update click through
  the sidecar's `resolve_seat_dialog` / `apply_fusion_update`, the crash janitor through
  `dismiss_blocking_dialogs {categories:["crash_report"]}`, launch self-heal through
  `stop`/`kill` + `start`, launch timing samples and pref writes natively), computes, and
  answers with `readinessOwner: "exe"`; the sidecar is not asked. Any other value (or no
  key) keeps the sidecar as owner with the exe shadowing beside it. Roll back by deleting
  the key. One owner at a time: the sidecar's own sweeps (seat takeover, update sweep) keep
  running in both modes, as before.
- **Owned readiness proven (ConfRoomROG, 2026-09-03):** with `readinessOwner: "exe"` the exe
  served every readiness poll through a real stop/start: the launch progress block ("Fusion is
  up - waiting for the Adom add-in to serve"), ready afterwards, the signed-in user fetched
  once and cached, `launchStartedAt` cleared, `lastReadyAt` written, and a new cold-start
  sample (38.5 s) in `timings.json`. Set the pref WITHOUT a byte-order mark: PowerShell's
  `Set-Content -Encoding UTF8` writes one, the Python bridge's pref loader then fails and
  rewrites the file without the key (the first attempt silently never engaged).

## 2026-09-04: the ALL-NATIVE drill is proven (run4)

First `fusion_create_library` drill with `readinessOwner`, `chainsOwner` AND `addinOwner` all `exe` on
ConfRoomROG (exe 2c352be38685): the chain, the bind leaf, the prove leaf and every add-in pass-through ran
in the exe; the sidecar served only the `chain_*` executors that have since been ported natively
(docs_open, close_docs_since, close_bridge_doc, sweep_orphans, publish_library, addin_restart_info).

| stage | seconds | result |
|---|---|---|
| bind | 692.8 | 5/5 packages bound (QFN-48, SOIC-16, LQFP-64, Crystal 3225, IN-S63BTG) |
| prove | 1909.4 | 5/5 proven, 20 evidence frames; CH340G re-proved after a timing miss (not a part fault) |

Verdict `proven`, no cloud publish requested. Ported in the same session, each behind
`tools/rust-commit.sh`: prefs_open/close, generate_package (EPG golden), mcp_*, library_manager +
managed_library_*, signin/signin_2fa/web_signin (+ janitor), chain executors, restart-choice watcher.
Lessons: (1) `cargo test | grep FAILED && exit 1 || true` under pipefail never exits on a red test -
the gate now runs cargo test bare; (2) a Python f-string's own escapes (backslash-newline) must be
decoded when a template is lifted from source; (3) `py_repr` must escape like CPython (newlines).

## 2026-09-04 (afternoon): the exe answers every verb; the sidecar is idle

Ported natively, each behind `tools/rust-commit.sh`: `prefs_open`/`prefs_close` (image-space clicks in
Fusion's Preferences through ab), `generate_package` (the EPG script lifted from the Python f-string with
its escapes decoded, pinned by `tests/golden/epg_script.json`), `mcp_status/tools/call/resources/enable`
(a plain JSON-RPC client of Autodesk's server on 27182), `library_manager` + `managed_library_create/
link/pull/push/unlink` (ribbon DIPs scaled by the window's real DPI and ClientToScreen, both new
`HostProcess` hooks in `os-win`; CEF dialogs answered by rect fraction; judged by Fusion's own alert log
with a watermark), `signin`/`signin_2fa`/`web_signin` (the OAuth URL read from Chrome/Edge History with
rusqlite, re-opened in the right profile, SSO driven over the extension, the walls toasted, a janitor
that closes the windows once the goal is met), `aps_signin` (PKCE with sha2/getrandom, a tiny_http
listener on the registered callback ports, the AUTH-001 probe, the remembered browser), the APS browser
prefs, `fusion_demo` (the staged first-run demo and the async tour with the desktop remote),
`demo_progress`/`demo_control`, the chain executors (`docs_open`, `close_docs_since`, `close_bridge_doc`,
`sweep_orphans`, `publish_library`, `addin_restart_info` with the restart-choice watcher), the webdeploy
cleanup, the auto add-in restart (posted to the exe's own `/command`, `launch::SELF_URL`), banners and
taskbar progress through a bare add-in hop (`addin::addin_http`), and the timeout diagnosis with no
sidecar fallback.

What still names the sidecar: `ExeChainHost::call`'s catch-all (every named verb is native), the
`ctx.sidecar` closure `call_addin` carries (unused on the exe-owned paths), and the readiness /
chains / add-in owner prefs, which default to the sidecar until Phase 4 flips them.

First native tour on ConfRoomROG (tour-f40cd85b): done in 248s, 7 views, the 3D chip bound (1 part),
the test project generated; the 2D/3D board switches failed because the generated project leaves its
SCHEMATIC document active - `show_2d_board` answers "Please open or create an electronics design".
Fixed by porting the test-project handle (`testProject` in prefs.json, `testproject::test_project_
activate`) and putting the board document in front before the board switches and the exports;
`show_project` without args now serves the generated project natively too.

## Phase 4, what is left, and the one command that flips the package (on John's word)

The exe now runs alone: every owner defaults to `exe`, the sidecar is spawned only when a pref says
`"sidecar"` (or `ADOM_FUSION_SIDECAR=1`), the add-in sync and the orphaned-run reconcile moved into
the exe's startup, and the native tour and the drill ran with no Python process on ConfRoomROG.
The PACKAGE still carries the Python bridge and `spawn.kind: python`, so a fresh install still
provisions an interpreter and starts server.py (which then owns nothing). `tools/exe-only-flip.sh`
makes the release exe-only (bridge.json spawn.kind exe, the Python files out of the zip, the
import checks out of release.sh); it edits the tree and commits nothing, so the diff is reviewed,
then `tools/release.sh` publishes insiders-first, and promotion to public is John's word.

## 2026-09-04 (late afternoon): the exe-only drill, no Python process on the box (run5)

Exe d038afe62530 with every owner defaulting to `exe` and no sidecar spawned (the only python.exe on
ConfRoomROG was the KiCad bridge's). `fusion_install_library_bundle` on the five-part drill bundle:

| stage | seconds | result |
|---|---|---|
| bind | 404.4 | 5/5 packages bound |
| prove | 1458.6 | 5/5 proven first pass, 20 evidence frames, no re-proves |

Verdict `proven`. Faster than run4 (693 s / 1909 s) with the same parts; the sidecar's absence cost
nothing. The native tour ran twice the same hour (7 views, then 9 after the board-document fix).

Landed after the drill started: the add-in sync and the orphaned-run reconcile at exe startup
(`addin_install.rs`), the janitor tick (`janitor.rs`: seat takeover, window sizing through ab with
two Win32 fact hooks, badge shedding, the bridge-doc Unsaved prompt, idle dirty bridge docs, stray
Create-new prompts, the crash reporter, the five-minute silent update), and the contract without
the 18 chain executor verbs (134). `tools/exe-only-flip.sh` holds the package flip for John's word.

## 2026-09-04 (evening): 1.9.357, the first exe-only release, on the insiders tier

Live checks before the flip, all on ConfRoomROG from the exe: Preferences open/close, MCP enable
(second pass, as the hint predicts), the EPG chip with a marking, the silent update (nothing
offered), `signin` (already signed in), `signin_2fa` (no window), APS sign-in (unconfigured branch:
no peer holds the org client id), Library Manager (Hub tab), the managed-library picker (six rows
read over UIA, cancelled), unlink (clean). Two fixes fell out: Library Manager is a top-level
window the dialog array does not list, and Fusion's morning update moved the ribbon tab row
about 11 DIP (the LIBRARY.IO DIPs are re-measured, the old group row is the second try).

`tools/exe-only-flip.sh` then made the package exe-only and `tools/release.sh --bump` published
1.9.357 to the insiders manifest (public untouched). `bridge_install` from that manifest on
ConfRoomROG: ab spawned `adom-fusion-bridge.exe` (kind exe), 134 verbs, ready, the KiCad bridge's
python the only interpreter on the box. Note: `bridge_install` overlays the cache dir rather than
clearing it, so a box that had the Python bridge keeps a stale server.py on disk (inert: the exe
spawns no sidecar unless a pref asks); a fresh box gets the 44-file zip only. Public promotion is
`tools/promote.sh 1.9.357 --attest "<John's words>"`.

## 2026-09-04 (evening): APS proven on both boxes; the typed electronics read model

`fusion_aps_signin` native, end to end: on AdomLapper the remembered Chrome profile consented on a
warm SSO session and the listener captured a fresh token by itself (jlauer12@gmail.com); on
ConfRoomROG the org client id was set, the consent opened through the default browser handler
(no extension there), the email was typed over the OS, John typed the password at the box, and
the ten-minute listener captured john@adom.inc. `fusion_aps_search` answers on both.

`fusion_electronics_query` (electronics.rs) is the roadmap's typed read model: Autodesk's MCP
`electronics_read` when the server answers (all 49 classes, filters, fields, pagination), else
the board_info XML for Board/Element/Library/Package/Layer/Signal/Wire/Error, one row shape,
`source` naming which. The fallback parser is pinned by `tests/fixtures/board_info_sample.json`
(the generated test project on ConfRoomROG).

## The guided electronics tour (evening of 2026-09-04)

`fusion_demo {mode:"tour"}` is `rust/bridge/src/tour.rs`: the KiCad bridge's tour rules on the
electronics content John asked for (library, symbol, footprint, minted 3D chip, generated
schematic, 2D and 3D board, cloud search; multipart libraries mentioned, never shown). What the
port learned:

- **A tour beat is measured before it moves.** `verify_beat` reads z-order, live title, canvas
  colours and two captures 0.8 s apart; motion (`electron_zoom` for the EAGLE editors, the
  viewport camera for Designs) runs only on a verified window and is verified by capture digest.
  On ConfRoomROG the remote-desktop viewer sits on top; `is_remote_viewer` soft-gates front and
  records `frontNote` instead of pretending.
- **One FIFO for every source.** The loopback callback (`POST /demo/event`, ab 2.1.17+), the
  poll backstop (deduped by `at`) and `fusion_demo_control` all land in `on_press`, which acks,
  records and queues. Exit sets an atomic the beat loop and every gesture check. ab 2.1.22 on
  ConfRoomROG predates the synthetic press hook (2.1.26), so pause/play/back/stop were proven
  through the callback wire, the dashboard path and real OS clicks on the panel.
- **A detached tour thread writes only to its own record.** A later tour may replace the live
  record while the earlier thread is still closing documents; the fallback that marks an
  ended thread's record failed once marked the NEW tour failed. `rec_for(id)` writes to the
  live record only when the id matches, else to the persisted file.
- **Mark done before the slow cleanup.** Closing the tour's documents takes a minute; the
  record now says done with `tourDocsClosed: "closing"` and fills the list afterwards.
- **The 3D board keeps the board document's name**, so waiting for the active document to
  change never returns; wait for a Design product with bodies instead (`wait_design`).
- **A distributed-design f3d does not import** ("cannot be created from a local file containing
  external references"); that killed the first mechanical draft before John redirected the
  content anyway.

- **The board beats show a real hub design.** John, later the same evening: the generated one-chip
  board looked lame, so beats 5 to 7 open the BQ25792 charger from the hub (exact-name search,
  Main project preferred, 56 s to open on ConfRoomROG) and fall back to the generated board only
  when the hub has none. A KiCad schematic cannot be imported into Fusion Electronics (EAGLE only),
  so the KiCad tour's board was not an option. Captions dropped to 32px on one line: ab's medium
  is 48px since v2.0.84 and covered the remote.
- **Never lock the record inside the record lock.** The tour stamps each clip and caption with
  `since_start()`, which reads the record; the first draft evaluated it inside `rec(|s| ...)`
  (which holds the same std Mutex), so the tour thread deadlocked itself on its first caption and
  every later `TOUR.lock()` hung: `fusion_demo` timed out while every other verb answered. Std
  mutexes are not reentrant; compute anything that reads the record before entering `rec`.
- **A dock step must carry no `$`.** Hydrogen's `/workspace/exec` (the route the dock falls back
  to when the editor extension has no exec port) expands `$var` and `$(...)` on the host before
  the container runs the command, so a `for i in $(seq 1 15)` wait loop in dockbar.json died as
  a shell syntax error with exit code null and the dock showed "no output" while its timer kept
  counting (ConfRoomROG, 2026-09-05; the same failure took the laptop's Hard Refresh Open the
  day before). The serve step is now `sh -c 'PATH=~/.local/bin:...; exec fusion-dashboard serve
  --print-url'`. Filed on adom/hydrogen.
- **The dock's exec reaps the step's process tree.** With the `$`-free step the dock got the URL
  in 3 s, but the server the dashboard had self-backgrounded (a plain child) was gone before the
  dock's readiness probe: "serving on port 8932" in the log, then "never answered within 120s".
  `fusion-dashboard serve --print-url` now launches the server through `setsid -f` (its own
  session) and falls back to the plain spawn where setsid is absent.

## 2026-09-05: the palette is the verbs' voice; busy gates ignore it

- John: the ab captions describe the demo, the in-Fusion palette describes the verb running
  right now (opening a library, minting 3D part 2 of 5, switching to the 2D board). `palette.rs`
  sends those through the add-in's activity_banner on a retrying thread; the pass-through
  chokepoint speaks for the plain verbs, the orchestrations for their own, the bind per part with
  an ETA from the per-part timing store (`mint.part`, so a 50-part library estimates itself).
- Both busy gates (our long-command gate and the add-in /status gate) now ignore an
  activity_banner in flight: two recorded runs lost four beats to "activity_banner running for
  0.1s" refusals before that.
- The tour's front gate counts Fusion's own popups (the palette host) and PopupHost as front.
- The taskbar overlay is the same 32 px PNG the Python bridge sent, not ab's larger built-in badge.
- Fusion is placed at 95% of the work area; captions are ab's medium on one line.
- **The palette is one worker, not one thread per message.** A thread per message that gave up
  when a newer message arrived meant nothing ever painted during a bind: Fusion's main thread is
  blocked by the STEP import, every message was superseded before it could land. One worker keeps
  trying to paint the newest message and succeeds the moment Fusion breathes.
- **Field notes from the recorded runs (2026-09-05):** the Adom screensaver on ConfRoomROG
  (10 min) takes the input desktop, so clicks are refused by UIPI and captures come back blank;
  the version-aware dashboard serve moves to the next port after a release (a driver must find the
  dashboard by URL, never by a fixed port); ab's whole-desktop recorder could not start a video
  source there, ffmpeg gdigrab records the full screen fine.
- **ab's demo panel does not leave the screen on hide** (2.1.27 and 2.1.33): hide answers
  hidden:true and poll says stopped, but the glass stays painted at its last stage. Filed as
  adom/adom-bridge issue 151; the tour's `panelHidden` record is honest about what ab reported,
  not about the pixels.
- **A release restarts the exe under a running tour.** ab pulled 1.9.365 mid-run and the record
  vanished (run 25). Pin the bridge cache on a demo box with `bridge_dev_mode` before recording,
  and never release while a tour runs anywhere you cannot see.
- **The bridge's own palette was being searched as a dialog (2026-09-05, run 35).** Every
  doc-switching verb paid 13 s in the post-envelope: the activity palette is a 760x820 Qt tool
  window titled "Adom", the dialog filter took it for a candidate prompt, and each sweep ran a
  UIA control search over its HTML (EDIT 16.6 s, execute_text_command 15 s). `qt_dialog_rows`
  now skips `BRIDGE_PALETTE_TITLE`; EDIT is 3.3 s, execute_text_command under 2 s. Lesson: when a
  verb is slow, ask the verb. Slow pass-throughs (2 s and up) now answer `_timing`
  {preSec, addinSec, dialogsSec, postSec} so the envelope can never hide behind the work again.
- **Fusion's electronics editors do not honour a WINDOW box on command (2026-09-05, runs 37
  and 38, about forty screenshot trials on the package editor).** `WINDOW (x1 y1) (x2 y2)`
  reads bare numbers in the editor's current grid unit (0.1 inch in a symbol editor), and even
  with an explicit `mm` suffix it is applied only sometimes and presented later or never; a
  glide of twelve boxes from one add-in call showed nothing, and separate calls behaved
  differently from one minute to the next. `WINDOW <factor>` (a smooth factor zoom) and the
  real pointer moving over the drawing (pads and pins highlight, tooltips name them) rendered
  every single time, so every 2D beat is built from those two and no plan pans by box. The
  lesson under the lesson: verify motion from the recording's frames (tools: ralph.py in the
  scratchpad, per-beat montage plus a blank and a motion number), never from a verb's `success`.
- **The activity palette floats over the canvas in the electronics editors** (its docking
  request reads back 0); the tour parks it over the right panel column with
  desktop_set_window_bounds, and the palette API's setPosition throws there.
- **Fusion Electronics asserts when a LINE completes a net (2026-09-07, ConfRoomROG, Fusion
  2705.1.11).** Drawing the last segment that removes a net's final airwire raised Fusion's own
  "unexpected condition" modal (pusher.cpp line 492, inLocator) on this box, regardless of
  command shape (+0 straight, whole-net LINE, pad-first direction, a stub short of the pad);
  the same fixture had passed on AdomLapper two days earlier on the same build. The copper
  lands; only the modal blocks the main thread, so every following add-in call hangs and the
  run reads as an add-in crash. Now classified as `fusion_unexpected_condition`, clicked OK by
  the post-envelope (polled for 1.2 s after any window-raising verb) and by the sentinel, and
  carried in the routing reply with a checkpoint hint; the document snapshot behind it is a
  3 s undiagnosed hop so a hung main thread costs seconds, not the relay's whole minute.

## Never an 8.3 path (2026-09-10)

Fusion's text commands cannot take a path with a space and reject the quoted form, and the old
answer (GetShortPathNameW) made Fusion title documents "0603WA~1", which silently broke every
name check. `fusionpath::for_fusion` stages the file and its siblings into a space-free folder
instead; `output_for_fusion` + `finish_output` do the same for files Fusion writes. If a new
verb hands Fusion a path, go through those, never through `short_path`.

## 2026-10-20: the mechanical layer in this pattern

The typed mechanical operations (`fusion_mechanical`, built in the 1.12.0-dev line) now follow the
exe/add-in split this page describes. The exe owns everything that does not need Fusion:
`fusion-core::mech` validates requests against the add-in's `schema.json`, encodes Python's canonical
JSON for request digests, and keeps the durable operation journal and its history rules.
`bridge/src/mech.rs` serves `fusion_mech_capabilities` and `fusion_mech_operation_status` from files,
so neither waits on Fusion's main thread, and carries each write through the ordinary add-in
pass-through. The add-in keeps the Fusion API work, a read-only journal reader for exploded-preview
proofs, and `commands/mechanical/handoff.py`.

- **Goldens from the Python, as everywhere else.** `tools/gen-mech-goldens.py` runs the dev.9 admission
  code (kept verbatim in `tests/mech_reference`) and freezes every distinct validation outcome per
  operation (2,600 cases), canonical JSON corner cases, capabilities, and nine history scenarios down to
  the journal bytes. The Python validator walks set intersections, so the corpus runs under eight hash
  seeds and keeps only the cases all eight answer alike. The offline add-in tests drive the add-in
  through `tests/mech_exe.py`, the same protocol in Python, so both sides share one reference.
- **A claim file arbitrates "never ran".** Inside one process the journal decided a timeout against the
  main thread with a compare-and-set. With the exe writing the journal, the add-in creates
  `dispatch/<id>.claim` exclusively before executing, and the exe creates it only after the pass-through
  returned without the add-in's answer. Whoever creates it first wins. The add-in also writes its exact
  result to `dispatch/<id>.result.json`, so a reply lost to a timeout is still recorded on the next status
  query or write.
- **The add-in's session is a file.** `session.json` carries the session ID, the Fusion PID and the
  transaction-probe state; the exe trusts it only while that PID runs.
- **serde_json needs `float_roundtrip`.** Its default float parser can land one ULP off the correctly
  rounded value, so a caller's number could reach Fusion changed and its digest differ from Python's.
  The workspace enables the feature.
- **Lesson: a source projection can carry stale copies.** The dev.9 source held older copies of add-in
  files, so applying it as a delta on its recorded base reverted later fixes (the `treatAsUnit` empty
  list, the MCP example) and replaced the export, BOM and mass verbs with mechanical wrappers. Those files
  are back to master's versions. Diff a projection against its real base before merging it.
