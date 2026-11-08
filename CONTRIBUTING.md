# Contributing

The Fusion bridge is open, and contributing back is part of the product. Its wiki page is the
repository: every release pushes the whole source tree (the Rust exe, the Fusion add-in, the verb
contract, the skills, the tests) to [adom/fusion-bridge](https://wiki.adom.inc/adom/fusion-bridge),
and pull requests are reviewed there. The wiki repo is the one canonical, public copy, MIT-licensed;
you do not need GitHub for anything below.

If your AI just worked around the bridge instead of using it, that workaround is a contribution
waiting to happen: file it, or fix it and send the change back. Both take minutes with the
`adom-wiki` CLI. An AI can follow the same steps from the `fusion-contributing` skill, which ships
in the package (`skills/fusion-contributing/SKILL.md`). Every failing reply from the exe carries a
`_contribute` field with the same pointer, and `fusion_describe` carries a `contribute` block, so
nobody has to find this file first.

## Before you start

You need three things. All are free.

1. **An Adom Wiki account.** Sign in at [wiki.adom.inc](https://wiki.adom.inc); issues, branches and
   pull requests are tied to it.
2. **The `adom-wiki` CLI.** Install it with
   `bash <(curl -fsSL https://wiki.adom.inc/static/bootstrap.sh)`, or download a binary from the
   Releases tab of [adom/adom-wiki-cli](https://wiki.adom.inc/adom/adom-wiki-cli). Check it with
   `adom-wiki whoami`.
3. **A Windows box with Fusion and Adom Bridge**, only if you want to run your change against real
   Fusion before sending it. Adom Bridge ships with [Hydrogen](https://wiki.adom.inc/adom/hydrogen),
   which is free. Reading the code, filing issues and sending small fixes need none of this.

## Two-way street

- **You can extend this bridge** (a dialog rule, a hint, a verb, a skill, support for a new Fusion
  build) and send it back as a pull request on the wiki page.
- **You can fork it** if your changes diverge too much, for example a private vendor SDK
  integration. `bridge.json` is the contract with Adom Bridge. Fork freely under MIT.

## 1. Report first (two minutes, always worth it)

```bash
adom-wiki issue create adom/fusion-bridge --category bug-report \
  --title "<verb>: <what happened in one line>" \
  --body-file report.md
```

Put in `report.md`: the exact call and its full reply JSON (the `_hint` and `errorCode` matter),
the `fusion_readiness` output (bridge version, add-in version, box), the Fusion build, what you
expected, and the workaround you used. Attach a screenshot or the exported board if one exists. A
report with the reply JSON is usually fixed the same day. The maintainer thread answers on the
issue.

## 2. Get the source

```bash
adom-wiki repo clone adom/fusion-bridge --dir fusion-bridge     # HEAD of master, files only
cd fusion-bridge
```

Where things live:

| Path | What it is |
|---|---|
| `rust/bridge/src/` | the exe Adom Bridge spawns (`adom-fusion-bridge.exe`): verb dispatch in `main.rs`, the pass-through envelope in `addin.rs`, dialogs and restarts in `lifecycle.rs`, chains in `chains.rs`, APS search in `main.rs`, staging in `fusionpath.rs` |
| `rust/core/src/` | shared logic: `dialog_rules.json` (dialog text to category and resolution), readiness, window facts, APS |
| `addin/AdomBridge/` | the Python add-in that runs inside Fusion: one handler per verb under `commands/`, registered in `commands/__init__.py` |
| `describe.py` and `contracts/describe.json` | the verb contract `fusion_describe` serves. `tools/freeze-contract.py` regenerates the JSON |
| `skills/` | the user skills the package installs |
| `tests/run_verb_tests.py` | the daily verb test. Every described verb must be triaged there or the run goes red |
| `README.md` and `docs/` | the page and its feature guides. `python3 tools/gen-readme-verbs.py` regenerates the README verb catalog from the contract |
| `CHANGELOG.md` | newest first. Every change gets an entry |

## 3. The common contributions

- **A dialog the bridge could not answer** (the reply was `blocked_by_dialog` with the box's text
  and buttons): add a rule to `rust/core/src/dialog_rules.json` (`needles` from the text, a
  `category`, a `resolution` that names the safe button) and a case to the tests in
  `rust/core/src/dialogs.rs`. The JSON is the only rule table the exe reads; the Python
  `handlers/dialog_classify.py` belongs to the retired Python bridge and no longer needs a matching
  entry. Never make the bridge close a window that is not plainly a dialog: Fusion's own panels
  (a floating Browser is titled "Fusion360") close documents when closed.
- **A wrong or missing hint**: hints are plain strings next to the reply that carries them (`_hint`
  in the Rust or the Python handler). Say what to do next, not what went wrong twice.
- **A new verb**: a handler in `addin/AdomBridge/commands/`, its name in `commands/__init__.py`
  and in `ADDIN_COMMANDS` in `rust/bridge/src/addin.rs`, an entry in `describe.py`, then
  `python3 tools/freeze-contract.py`, a `t(...)` line in `tests/run_verb_tests.py`, and
  `python3 tools/gen-readme-verbs.py` so the README catalog lists it.
- **A skill**: a folder under `skills/<name>/SKILL.md` with frontmatter (`name`, `public`,
  `description` with the trigger words users actually say).

## 4. Build and try it on your own box

Prerequisites once: Rust stable, the cross target and MinGW for the Windows exe.

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get install -y mingw-w64            # Linux or WSL; on Windows build natively
cd rust && cargo test -p adom-fusion-bridge  # unit tests, including the dialog rules
cargo build --release --target x86_64-pc-windows-gnu -p adom-fusion-bridge
```

Try it without touching the released install: pin the bridge cache on the box so Adom Bridge stops
updating it, copy your build in, let Adom Bridge respawn it, and unpin when you are done.

```bash
adom-bridge --target <box> --ai-thread <you> bridge_dev_mode '{"name":"fusion360","on":true}'
adom-bridge --target <box> --ai-thread <you> send_files '{"filePaths":["rust/target/x86_64-pc-windows-gnu/release/adom-fusion-bridge.exe"],"dest":"C:/Users/<user>/AppData/Local/Adom Bridge/bridges-cache/fusion360"}'
adom-bridge --target <box> --ai-thread <you> fusion_readiness '{}'      # bridgeVersion is your build
adom-bridge --target <box> --ai-thread <you> bridge_dev_mode '{"name":"fusion360","on":false}'   # when done
```

An add-in change (`addin/`) also needs the add-in folder copied into the cache and a Fusion
restart (`fusion_restart_for_addin {save:true}`), because Fusion loads add-ins at launch.

### Running your own build without signing it

You never need to sign anything to contribute. `cargo build` gives you an unsigned
`adom-fusion-bridge.exe`, and on most machines it simply runs. The one thing that can stop it is
Windows 11 **Smart App Control**, which blocks unsigned programs ("Part of this app has been
blocked"). Check your box first, in PowerShell:

```powershell
(Get-MpComputerStatus).SmartAppControlState     # Off, Eval or On
```

- **Off**: your build runs as is. Copy it in behind the dev pin (above) and test. This is the case
  on most developer machines and on any PC upgraded from an older Windows.
- **Eval** (evaluation mode): Windows is still deciding. An unsigned build may be allowed or
  blocked; try it.
- **On**: Windows blocks the unsigned exe. Either test on another box where it is off, or turn it off
  in Windows Security > App & browser control > Smart App Control. Know that Windows does not let you
  turn it back on afterwards without resetting the PC, so prefer a spare or development machine.

Two more notes:

- A build you compile on the box, or copy over with `send_files`, carries no "downloaded from the
  internet" mark, so Windows SmartScreen does not prompt for it.
- If none of your machines can run an unsigned exe, say so on your pull request. A maintainer will
  run your change on a signed build and post the results there.

### We sign the final exe

You send source, never a binary. When a maintainer merges your pull request and ships it, the release
build is compiled from that source and **code-signed as Adom Industries, Inc.** with a trusted
timestamp before it is published, and the release refuses to publish an exe whose signature does not
verify. Everyone who installs the bridge gets that signed exe, so Smart App Control is never a problem
for them.

**Run the verb suite against your box** before you send a change that touches behavior. It drives
every daily-tier verb the way an AI does and prints a scorecard:

```bash
python3 tests/run_verb_tests.py --target <box> --tier daily            # the whole daily tier, about 10 minutes
python3 tests/run_verb_tests.py --target <box> --only fusion_show_schematic,fusion_export_eagle_source
```

A new verb needs its own `t(...)` line in that file, or the coverage check turns the run red.

## 5. Send the pull request

```bash
adom-wiki branch create adom/fusion-bridge fix/<short-name>
adom-wiki repo push adom/fusion-bridge --branch fix/<short-name> \
  --message "<what and why, one line>" --files <the files you changed>
adom-wiki pr create adom/fusion-bridge --from fix/<short-name> \
  --title "<what it fixes>" --body-file pr.md
```

In `pr.md`: the issue it answers, what changed, and how you verified it (box, Fusion build, the
verb call and its reply before and after). Add a CHANGELOG entry at the top of `CHANGELOG.md`
under the next version. House style: plain sentences, no em-dashes.

## Review, revisions and approval

- **Who reviews.** The maintainer thread for this bridge. It reads the change, runs it, and answers
  on the pull request, usually within a day.
- **Asked for changes?** Push more commits to the same branch with
  `adom-wiki repo push adom/fusion-bridge --branch fix/<short-name> ...`. The pull request updates;
  there is no need to open a new one. Reply on it with what you changed.
- **What gets a fast yes.** One concern per pull request, the evidence (the call and its reply
  before and after), a test when the change is testable, and a CHANGELOG entry.
- **What gets a no.** A change that makes the bridge foreground Fusion, type a password, discard
  someone's unsaved work, or hide a failure. We will explain why, and you keep your fork.
- **Credit.** Your commits are kept as authored, and the CHANGELOG entry names the change.

## How shipping works

The maintainer thread reviews the pull request and merges it into master (a true merge, history
kept, your authored commits preserved). A maintainer bumps `BRIDGE_VERSION` and the `version` in
`bridge.json` together, builds the release zip and publishes it with `tools/release.sh`. Every
release is an insiders release first: the insiders manifest moves, and the public manifest moves
only after the pre-production gate (`tools/promote.sh`). You will see the version on the issue. A
pull request that comes with its evidence merges fast.

Publishing needs an Adom Wiki API token, which stays on maintainer machines, so nothing ships
automatically from a merge.

## bridge.json

See [the SDK guide on the Adom Wiki](https://wiki.adom.inc/adom/adom-desktop-bridges) for the
authoritative schema. What matters for this bridge:

- `version`: semver, and it must match `BRIDGE_VERSION`.
- `spawn.kind: "exe"` with `entrypoint: "adom-fusion-bridge.exe"`: one sha256-verified binary, no
  interpreter.
- `spawn.port: 0`: Adom Bridge picks an ephemeral port and passes it with `--port <N>`. Do not
  hardcode a port.
- `spawn.healthEndpoint: "/status"`: path only. Adom Bridge builds the URL from the runtime port.
- `verbPrefixes: ["fusion_"]`: the verb prefix this bridge owns.

## Versioning

- **Patch**: bug fixes, hardening, new hints, no change to a verb's shape.
- **Minor**: new verbs, new optional arguments, backward-compatible behavior changes.
- **Major**: a breaking change to a verb's shape, or a verb removed. Talk to the maintainers on an
  issue first.

## Code of conduct

Be kind. We are building this for engineers who need their CAD tools to work with AI without
giving up local control of their files. If your pull request is declined, we will explain why, and
you can fork.

## License

MIT. See [LICENSE](LICENSE).
