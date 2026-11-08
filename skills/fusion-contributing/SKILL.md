---
name: fusion-contributing
public: true
description: "How to contribute to the Fusion bridge when you hit a bug, a missing verb, a wrong hint or a dialog the bridge cannot answer: where the source lives (the wiki page IS the repo), how to file the issue with the evidence, clone the source, make the change (dialog rules, hints, verbs, skills, the exe), build and try it on your own box behind a dev pin, and send a pull request back with adom-wiki. Read this the moment an AI has to WORK AROUND the bridge instead of using it. Trigger words: contribute, contribute back, send a PR, pull request, patch the bridge, modify the bridge, fix the bridge myself, bridge source code, where is the source, clone the bridge, add a dialog rule, add a verb, fix a hint, fork the bridge, community contribution, work around a bridge bug, bridge bug."
---

# Contributing to the Fusion bridge

The Fusion bridge is open. Its wiki page is the repository: every release pushes the whole
source tree (the Rust exe, the Fusion add-in, the verb contract, the skills, the tests) to
[adom/fusion-bridge](https://wiki.adom.inc/adom/fusion-bridge), and pull requests are reviewed
there. The wiki repo is the one canonical, public copy (MIT); no GitHub account is needed. If your AI just worked
around the bridge instead of using it, that workaround is a contribution waiting to happen:
file it, or fix it and send the change back. Both take minutes with the `adom-wiki` CLI you
already have.

## 1. Report first (two minutes, always worth it)

```bash
adom-wiki issue create adom/fusion-bridge --category bug-report \
  --title "<verb>: <what happened in one line>" \
  --body-file report.md
```

Put in `report.md`: the exact call and its full reply JSON (the `_hint` and `errorCode`
matter), `fusion_readiness` output (bridge version, add-in version, box), the Fusion build,
what you expected, and the workaround you used. Attach a screenshot or the exported board if
one exists. A report with the reply JSON is usually fixed the same day; the maintainer thread
answers on the issue.

## 2. Get the source

```bash
adom-wiki repo clone adom/fusion-bridge --dir fusion-bridge     # HEAD of master, files only
cd fusion-bridge
```

Where things live:

| Path | What it is |
|---|---|
| `rust/bridge/src/` | the exe ab spawns (`adom-fusion-bridge.exe`): verb dispatch in `main.rs`, the pass-through envelope in `addin.rs`, dialogs and restarts in `lifecycle.rs`, chains in `chains.rs`, APS search in `main.rs`, staging in `fusionpath.rs` |
| `rust/core/src/` | shared logic: `dialog_rules.json` (dialog text to category and resolution), readiness, window facts, APS |
| `addin/AdomBridge/` | the Python add-in that runs inside Fusion: one handler per verb under `commands/`, registered in `commands/__init__.py` |
| `describe.py` and `contracts/describe.json` | the verb contract `fusion_describe` serves; `tools/freeze-contract.py` regenerates the JSON |
| `skills/` | the user skills the package installs (this file is one) |
| `tests/run_verb_tests.py` | the daily verb test; every described verb must be triaged there or the run goes red |
| `CHANGELOG.md` | newest first; every change gets an entry |

## 3. The common contributions

- **A dialog the bridge could not answer** (the reply was `blocked_by_dialog` with the box's
  text and buttons): add a rule to `rust/core/src/dialog_rules.json` (`needles` from the text,
  a `category`, a `resolution` that names the safe button), and a case to the tests in `rust/core/src/dialogs.rs`. That JSON is the only rule table the exe reads; the retired Python
  bridge's `handlers/dialog_classify.py` needs no matching entry.
- **A wrong or missing hint**: hints are plain strings next to the reply that carries them
  (`_hint` in the Rust or the Python handler). Say what to do next, not what went wrong twice.
- **A new verb**: a handler in `addin/AdomBridge/commands/`, its name in
  `commands/__init__.py`, in `ADDIN_COMMANDS` in `rust/bridge/src/addin.rs`, an entry in
  `describe.py`, then `python3 tools/freeze-contract.py`, and a `t(...)` line in
  `tests/run_verb_tests.py`.
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

Try it without touching the released install: pin the bridge cache on the box so ab stops
updating it, copy your build in, let ab respawn it, and unpin when you are done.

```bash
adom-bridge --target <box> --ai-thread <you> bridge_dev_mode '{"name":"fusion360","on":true}'
adom-bridge --target <box> --ai-thread <you> send_files '{"filePaths":["rust/target/x86_64-pc-windows-gnu/release/adom-fusion-bridge.exe"],"dest":"C:/Users/<user>/AppData/Local/Adom Bridge/bridges-cache/fusion360"}'
adom-bridge --target <box> --ai-thread <you> fusion_readiness '{}'      # bridgeVersion is your build
adom-bridge --target <box> --ai-thread <you> bridge_dev_mode '{"name":"fusion360","on":false}'   # when done
```

An add-in change (`addin/`) also needs the add-in folder copied into the cache and a Fusion
restart (`fusion_restart_for_addin {save:true}`), because Fusion loads add-ins at launch.

## 5. Send the pull request

```bash
adom-wiki branch create adom/fusion-bridge fix/<short-name>
adom-wiki repo push adom/fusion-bridge --branch fix/<short-name> \
  --message "<what and why, one line>" --files <the files you changed>
adom-wiki pr create adom/fusion-bridge --from fix/<short-name> \
  --title "<what it fixes>" --body-file pr.md
```

In `pr.md`: the issue it answers, what changed, and how you verified it (box, Fusion build,
the verb call and its reply before and after). Add a CHANGELOG entry at the top of
`CHANGELOG.md` under the next version. House style: plain sentences, no em-dashes. The
maintainer thread reviews, merges into master (a true merge, history kept) and ships it in the
next insiders release, then public after the pre-production gate; you will see the version on
the issue. A PR that comes with its evidence merges fast.

## Two things that trip up a first contribution

- **Local builds are unsigned, and that is fine.** Contributors never sign anything: `cargo build` output runs
  as is unless Windows 11 Smart App Control is On. Check with `(Get-MpComputerStatus).SmartAppControlState`.
  Off: just run it behind the dev pin. On: test on another box (turning it off cannot be undone without a PC
  reset), or ask on the pull request for a maintainer to run it signed. Maintainers sign the final exe as
  Adom Industries when they ship the merged change; the release refuses an exe whose signature does not
  verify. CONTRIBUTING.md has the full steps.
- **Never make the bridge close a window that is not plainly a dialog.** Fusion's floating Browser panel is
  titled "Fusion360", and closing it closes documents. CONTRIBUTING.md has the review and approval steps.
