---
name: fusion-verb-tests
description: The standard end-to-end verb test suite for the fusion bridge - run the daily tier every day against a real box, the weekly tier before releases, triage failures, and keep the plan in lockstep with the verb catalog so no verb ships untested
---

# fusion-verb-tests - the standard daily verb test

Every described verb of this bridge, tested end-to-end the way a user's AI actually calls
it: through `adom-bridge --target <box>` over the real relay, against real Fusion, real
APS, and the canonical BQ25792 board. Built for daily runs ahead of the public release.

## Run it

```bash
cd ~/project/adom-desktop-fusion-bridge
python3 tests/run_verb_tests.py --target AdomLapper --tier daily    # every day, ~5-10 min
python3 tests/run_verb_tests.py --target AdomLapper --tier weekly   # before releases
python3 tests/run_verb_tests.py --target AdomLapper --only fusion_aps_search   # one verb
```

Exit code 0 = all green. Reports land in `tests/reports/verb-test-<stamp>.json`.
Use the ai-thread the runner sets (`fusion-verb-tests`) when investigating by hand, so
the user's Activity Log shows the test run as one coherent actor.

## The high-level product sections (John, 2026-08-14 - report against THESE)

A run must be readable as "which product areas are green", not a flat verb list. John's
framing, saved verbatim so it is never forgotten:

> i see the fusion bridge now having several high-level sections:
> - basic driving of fusion gui including all electronics tabs like proj mgr, schematic,
>   2d board, 3d board, library, symbol, footprint
> - fusion cad 3d including bom manager (oliver's feature)
> - mcp server in fusion
> - python add-in in fusion that gives us access to all of fusion's python cmds and
>   electron.run
> - aps fusion feature
> - other fusion api's where we have to be authenticated and does auth still work and did
>   our session tokens stay valid

The runner encodes this as `SECTION_ORDER` / `_SECTION_BY_VERB` (plus two buckets the
list implies but doesn't name: `exports` and `bridge-lifecycle`), stamps a `section` on
every result, and prints a per-section rollup at the end of each run; the report JSON
carries the same rollup under `sections`. When you summarize a run to John, lead with
the section scorecard. A new verb gets a section in `_SECTION_BY_VERB` in the same
commit it gets a plan line.

**Known section gaps (be honest about these when reporting):**
- *gui-driving*: the daily tier proves proj-mgr/schematic/2d/3d tab driving via the
  BQ25792 flow; the LIBRARY, SYMBOL, and FOOTPRINT editor tabs are only exercised
  weekly (open_lbr, capture_library_views, generate_package), and there is no
  dedicated symbol-editor / footprint-editor tab drive yet.
- *mcp-server*: daily only asserts `fusion_mcp_status`; a real MCP round-trip
  (`fusion_mcp_call`) is weekly.
- *auth-tokens*: daily proof is real, not a mock: `fusion_aps_get /project/v1/hubs` is
  an authenticated APS round-trip (token refresh included), and the BQ25792 cloud open
  proves Fusion's own signed-in session is alive.
- *exports*: weekly only (gerbers/BOM/CPL/GLB/stackup write to the scratch dir).

## The tiers, and why a verb is where it is

| Tier | What qualifies | Examples |
|---|---|---|
| **daily** | Read-only, or state-changing but reversible with the runner restoring what it touched | readiness, all APS reads, open BQ25792 + the three views, inspect_bodies, assembly_bom, run_modeling_script returning a constant, uninstall dryRun |
| **negative** | Verbs that MUST refuse - a refusal is the PASS | the two deprecated crash verbs; the `expectDocument` guard fed a wrong name on both a mutating and a READ verb |
| **weekly** | Heavy or write-bearing flows: exports to the scratch dir, library opens, electron view commands, cloud reads that walk hubs | export_step/gerbers/bom, open_lbr, generate_package, list_cloud_projects |
| **manual** | Anything that needs a human present or touches their session: sign-in flows, notify toasts, real uninstall, prefs UI driving, doc-closing verbs that could eat their tabs | fusion_signin, fusion_kill, fusion_close_all_documents, fusion_demo |

**Catalog drift is a failing test.** The runner diffs `describe._T` against its own plan;
a verb described but untriaged fails the run with `untriaged`. Adding a verb to the
bridge means adding a line to the plan in the same commit.

## What the runner cannot judge - the AI's part of the ritual

The runner asserts response shapes. Four things still need eyes each daily run:

1. **The board actually rendered.** After the open + view stages, take a background
   `desktop_screenshot_window` of Fusion and LOOK: BQ25792 in the 3D view, dark canvas,
   not a dialog, not an empty tab.
2. **Timing drift.** Each result carries `sec`. A verb that passed but took 5x its
   usual time is tomorrow's failure; compare against the previous report.
3. **The box was left as found.** No stray dialogs, and `fusion_get_app_state` at the
   end should match the start. The runner handles the mechanical part (see the
   end-of-run cleanup below); the eyes-on part is confirming nothing weird survived.
4. **Failures get filed, not just logged.** A red verb gets an issue on
   adom/fusion-bridge with the runner's JSON snippet, the bridge + add-in versions
   (`fusion_readiness`), and repro args, per the issue-reporting skill.

## Fixtures

- **Cloud**: `Molecules/XRP/Power/BQ25792.fprj` in project `Main` - the canonical board.
  Nothing in the daily tier writes to the cloud.
- **Local samples**: `examples/ADS8588SIPM.step` + `examples/ADS8588SIPM.fusion.lbr`
  (weekly library flows; staged to the box via `send_files` at run time).
- **Scratch on the box**: `%TEMP%/fusion-verb-tests` - all export outputs; safe to wipe.
- **Cloud writes** (weekly library builds only) go to the Adom AI Workspace folder and
  are cleaned with `fusion_cleanup_cloud_files`; never the project root, per the
  fusion-cloud-hygiene skill.

## End-of-run cleanup (John, 2026-08-14: "clean up also means closing fusion and other windows you opened including pup windows")

Cleanup is not just deleting temp files. **Close every window the test session caused
to exist.** The contract: *if the run opened it, the run closes it; if the user already
had it open, leave it alone.*

The runner does the box side automatically at the end of every run:
- wipes the `%TEMP%\fusion-verb-tests` scratch dir;
- snapshots whether Fusion was running BEFORE the run (the warmup app-state read), and
  if the run started Fusion, closes it with `fusion_stop`. If Fusion was already up it
  is the user's session and is LEFT RUNNING - never close a Fusion the user opened.

The AI running the session owns the rest, every time, unprompted:
- **Pup windows**: close every pup window/tab your test thread opened
  (`pup_close_window` on your own sessionIds). The one exception is a window you just
  opened deliberately to SHOW John results, that one stays until he is done with it,
  then gets closed too. Windows owned by OTHER threads are not yours; only take one
  over (takeover:true) when John has asked for cleanup and the window is unambiguous
  debris (placeholder pages), never when it holds real work.
- **Local debris**: /tmp working files, one-off scripts, stale build zips (release zips
  live in wiki releases; keep only the current one locally). Anything reusable gets
  promoted into `tools/` instead of lingering in /tmp.
- **Reports stay**: `tests/reports/` is the record, never part of cleanup.

## Long verbs vs the relay window (learned 2026-08-14, the hard way)

ab's relay caps a verb call at ~60s. A verb whose work can exceed that (cloud opens,
optimized-GLB export, the demo tour) MUST be async: fire, return an id instantly, poll a
status verb. Two live lessons:
- `fusion_export_optimized_glb` sync form died at the relay on a cold Fusion session, and
  the ORPHANED export kept grinding Fusion's main thread for 15+ minutes, wedging every
  later verb (each retry piled another export onto the queue). Recovery: fusion_stop /
  fusion_kill + fusion_start. NEVER probe a long sync verb "to see how long it takes" -
  use the async form (`async:true` -> exportId -> poll fetch).
- The runner aborts after 4 consecutive `target_not_found` results: that is the relay
  losing the box (ab auto-update, laptop roaming), not 70 verb failures. Re-run when
  `adom-bridge targets` shows the box stable; treat flap-aborted reports as void.

## Reading a failure

- `wrong_document` where a PASS was expected: the guard fired - which document was
  actually active? The runner opens BQ25792 first; a failure here usually means the
  open itself failed upstream.
- `bridge_restarting`: ab respawned the bridge mid-run (normal right after an install);
  re-run the verb before calling it a failure.
- `fusion_addin_not_responding` + a dialogs array: a modal is up; screenshot it - that
  is the real finding.
- Every refusal from the negative tier passing means the safety rails hold; a
  deprecated verb suddenly SUCCEEDING is a sev-1 finding, not a pass.

## Cadence

Daily: **installed** - the dev container's crontab runs `tests/daily_verb_cron.sh` at
11:47 UTC (~6:47am Central). It runs `--tier daily` against AdomLapper, appends to
`tests/reports/daily-cron-history.txt`, and on a red run auto-files an issue on
adom/fusion-bridge with the scorecard tail, so a failure is a filed finding before
anyone asks. The AI judgment ritual (screenshots, timing drift, box-left-as-found)
still happens on the next interactive session; the cron covers the shape checks.
Before any release: `--tier weekly` plus a by-hand pass of the manual tier's
release-rehearsal subset (make_3d_package, build_library_3d) on a box you can watch.
