# Fusion bridge promote-to-public gate, 1.10.6, 2026-09-08

Scope: the fusion-bridge subset of the insiders-tier pre-production test, run on winvm (fresh
bridge cache, PUBLIC Adom Bridge 2.1.29 installed for the run) and ConfRoomROG (Fusion 2705.1.11,
insiders ab 2.1.78). arav-rog was excluded (in use). AdomLapper untouched.

| check | box | result |
|---|---|---|
| package `adom-wiki pkg install adom/fusion-bridge` into an empty HOME | container | PASS: 29 fusion-* skills for both claude and codex, fusion-routing with its 2 references and the ruleWarnings text |
| fresh `bridge_install` from the insiders manifest on PUBLIC ab 2.1.29 | winvm | PASS: 1.10.6, entrypoint adom-fusion-bridge.exe, no server.py in a fresh cache |
| `fusion_readiness` without Fusion | winvm | PASS: bridgeVersion 1.10.6, installed false (honest) |
| `fusion_routing_state` without Fusion | winvm | PASS: host_app_not_installed, no add-in diagnosis |
| `fusion_describe` | winvm | PASS: 157 names (156 contract verbs plus describe) |
| `bridge_check_updates` | winvm | PASS: current 1.10.6 = latest, no update pending |
| `fusion_set_design_rules {action:list}` without Fusion | winvm | FAIL: ab's host-app gate refused it (host_app_not_installed) because bridge.json's hostAppOptionalVerbs lacked set_design_rules. Fixed in 1.10.7 (list needs the wiki, not Fusion) |
| readiness, add-in status, APS status | ConfRoomROG | PASS: ready, add-in 1.0.19, APS configured and signed in |
| open fixture, route NET_1, DRC, checkpoint export, close unsaved | ConfRoomROG | PASS: 6 nets read, 1 verified segment, DRC 0 errors / 0 warnings / 6 airwires (the five unrouted nets), export with hygiene hint, close by name |
| automatic restart on a stuck modal | ConfRoomROG | PASS (2026-09-07): addin_restarting at 19 s, verb answering at 121 s, no toast question |
| dock dashboard on the lockstep binary | container | PASS after restarting the dock instance: /health 1.10.6 (the dock had kept the 1.9.368 process running for three days after the binary changed) |

Not covered: the signed-out `needs_signin` path (no signed-out Fusion available without disturbing
a person); a Fusion install from scratch (out of scope, no Autodesk account on the VM).

Public-tier notes for the release: public ab is 2.1.29. It lacks the 2.1.72 spawn fix, so a public
user upgrading a cache that still holds a 1.9.x `server.py` (dev pins only; a normal public 1.9.361
cache has none) would get the Python bridge; remedy in CHANGELOG 1.10.5. `restartPolicy` defaults
to auto (1.10.6): document it in the public release notes.

winvm was left on public ab 2.1.29 with the 2.1.78 update staged: apply_update refuses while a
run_script awaiting human shell approval is in flight on that box.
