# Sign-in, sessions and automatic updates

Everything about getting Fusion running, signed into the right Autodesk account, and kept current,
without interrupting the person using the machine.

## Lifecycle

```bash
adom-bridge fusion_readiness '{}'   # installed? running? ready? (never launches)
adom-bridge fusion_start '{}'       # launch + wait for the add-in
adom-bridge fusion_stop  '{}'       # graceful close
adom-bridge fusion_kill  '{}'       # force-kill a wedged session
```

`fusion_readiness` is the verb to poll. It live-detects on **every** call rather than trusting a
bridge-start snapshot, because that snapshot is wrong in both directions: it reported
`installed: false` on a machine where Fusion had since been installed, and `ready: true` on one
where Fusion had been removed.

**Never ask the user to restart Fusion or enable the add-in.** That is the bridge's job.

### `restartPolicy`: auto, ask or force

Fusion loads add-ins only at launch, so an add-in that was installed or updated while Fusion was
open needs a restart. Who decides is a pref: `restartPolicy` in `~/.adom/fusion-bridge/prefs.json`,
or the env var `ADOM_FUSION_RESTART_POLICY`. The default is `auto`.

**A busy add-in never needs a restart.** While a long native Fusion call holds the add-in's main
thread, not even its status probe answers. The bridge treats that silence as work, not as an add-in
Fusion never loaded, whenever it has evidence: one of its own requests to the add-in is still in
flight, or the add-in answered earlier inside the same Fusion process (and its port still takes
connections). Every restart path asks for that evidence first: the verb gate, the timeout
diagnosis, the readiness launch observer and a warm `fusion_start`. The reply is
`fusion_addin_busy` with the evidence, and readiness reports it as `addinBusy`. Before this, a
93-second script read as a missing add-in and the bridge restarted Fusion with the user's
documents open.

- **`auto`**: the bridge restarts Fusion in the background and answers `addin_restarting`. No
  confirmation toast is raised. Documents Fusion can save are saved first. It stops short of losing
  work: when the add-in cannot save, Fusion is closed the way a person closes it (it closes at once
  when nothing is unsaved), and a never-saved document or a save prompt for a document the bridge
  did not make ends the restart with `restart_needs_decision`. Nothing is killed or discarded, and
  for the next 30 minutes a restart is asked for (the toast) instead of tried again. The restart is
  recorded in the action ledger (`fusion_action_ledger`) with a summary of what each step did.
- **`ask`**: for a machine a person shares with the AI. A verb that needs the restart answers
  `addin_restart_required` with `userMessage`, `openDocuments` and `unsavedDocuments`, the
  confirmation toast is raised, and the bridge waits for the person.
- **`force`**: the behaviour `auto` had from 1.10.6 to 1.12.7, for a machine only an AI drives:
  never-saved documents are discarded (named in `discarded` and in the ledger), and when the
  add-in cannot save at all the restart is a kill plus a start.

**The bridge never restarts Fusion over a dialog.** A dialog in front of Fusion is read through
UIA (title, text, button names) and expired safely: the known ones through their rule, the unknown
ones with WM_CLOSE, which is Cancel or the X and never saves or discards. After expiring a dialog
the gate re-probes the add-in for up to 20 s and then runs the verb or answers
`dialog_expired_retry`. A dialog that survives comes back as `blocked_by_dialog` with
`dialogs[] {hwnd, title, category, text, buttons, resolution}`, so the AI answers it by button
name. A long operation in flight (a STEP bind can starve the add-in for 20 to 60 s) answers
`fusion_addin_busy`, not a restart. Library chains restart Fusion only when the caller passes
`allowRestart:true` with a `restartReason`.

## `needs_signin`: the fast answer while Fusion is signing in

While Fusion's main window says "Signing in" or "Welcome to Fusion", every add-in verb
short-circuits with `errorCode: needs_signin` in well under a second. The reply carries
`needsSignin`, `mainTitle` and a hint that points at the sign-in skill. It is the same signal
`fusion_readiness` gives, and it is never a reason to restart Fusion. Before 1.10.1 the same state
took over a minute to come back, as `addin_restart_required`, which sent callers down the wrong
path. When you see `needs_signin`, finish the sign-in below and re-run the verb.

## Signing in through the right browser profile

Fusion fires its OAuth at the **OS-default browser**. If your Autodesk seat lives in a different
Chrome profile (work vs personal), Fusion signs into the wrong account or stalls.

```bash
adom-bridge fusion_signin '{"profile":"chrome:you@work.com"}'
```

What it does: reads the full authorize URL (client_id, PKCE, state, request_id) out of the browser's
**History DB**, then re-opens it in the profile you named. The URL completes back to the running
Fusion through the `idmgr/callback` plus `autodesk://` protocol handoff, matched by `request_id`, so
the browser no longer has to be the OS default.

The verb **clicks Fusion's Sign In button for you**, in the background. Never tell the user to click
it. It is a UIA-invokable control (`...QTSignInDialog.backgroundWidget.signin_button`), so no
foreground steal, no cursor move.

### SSO beats passwords

If the target profile already holds a Google/Apple/Microsoft identity, the whole login completes
with **no password and no 2FA**. Two rules make that work:

1. Click **"Continue with Google"** (or Apple/Microsoft) **before** typing any email. Entering an
   email commits you to the password path.
2. It must be a **fresh** flow. Re-navigating a `flowId` that already advanced dead-ends on the
   password screen with no way back.

### The ~2-minute clock

Fusion's sign-in request **expires in about two minutes**. A captured URL older than ~150s returns
`stage: "stale"`; restart Fusion for a fresh request rather than completing a dead one, which yields
"Sign-in request expired" and no handoff.

### When a human is required

If Autodesk demands a password with no saved credential, that is a genuine wall. The bridge
**toasts the user** from code (you do not have to ask it to) and hands the calling AI a mechanical
way to proceed. For an emailed 6-digit code:

```bash
adom-bridge fusion_signin_2fa '{"code":"123456"}'
```

which types it via background UIA. If `adom-google` is available, read the code out of Gmail and
submit it rather than asking the user for it.

Passwords and 2FA secrets are never typed by the bridge on the user's behalf beyond this explicit
verb.

## One login should cover APS too

Fusion sign-in and [APS](aps-cloud-search.md) are both Autodesk logins. They cannot share a token
(Autodesk DPAPI-encrypts Fusion's store with app entropy, deliberately), but they **can share the
warm browser SSO session**. So APS consent runs right after a Fusion sign-in while that session is
warm, and `fusion_readiness` reports APS state on every launch. The user should authenticate once.

## Active sessions

An Autodesk seat allows a limited number of concurrent sessions. When the account is already signed
in elsewhere, Fusion blocks startup with an **Active Sessions Exceeded** dialog listing the other
machines. It is a hard modal, nothing proceeds until it is answered, so the bridge treats it like any
blocking dialog: read it, then **suspend the stale session** ("Suspend Fusion on the computer
selected below and continue on this computer") rather than blind-clicking a default that could sign
you out of a machine mid-work.

![Fusion's Active Sessions Exceeded dialog listing the other signed-in machine](fz-sessions.png)

## Automatic updates

Fusion ships updates constantly and nags with an "Update Now / Update Later" panel and a countdown.

The bridge clicks it **for you, in the background**, so you never see it. Fusion downloads and swaps
to the new build on the next launch.

```bash
adom-bridge fusion_set_auto_update '{}'                  # read current setting
adom-bridge fusion_set_auto_update '{"enabled":false}'   # opt out, STICKY
```

On by default for everyone. Opting out persists to `~/.adom/fusion-bridge/prefs.json` and is
honoured permanently; once off, the bridge never touches your update prompts again.

### Bridge updates: insiders and public manifests

The bridge itself updates through Adom Bridge, which polls the manifest on launch and every 4
hours. There are two manifests. Every release is an insiders release first: it lands on the
insiders manifest (`insidersManifestUrl` in `bridge.json`, on the private adom/insiders page). The
public manifest (`adom-bridge-fusion-manifest.json` on the bridge's wiki page, the
`updateManifestUrl`) moves only when a maintainer promotes a version that passed the
pre-production gate. So a box on the insiders tier can run a newer version than a public box, and
`fusion_readiness` tells you which version a box actually runs (`bridgeVersion`). A feature in the
changelog that a public box does not have yet is usually this, not a failed update.

Honest limit: the nag only enters the UIA tree while its "View Job Status" panel is **open**, and
there is no API to enumerate a pending update while it is closed. So the bridge sweeps on a schedule
and catches the nag whenever Fusion shows it, rather than pre-empting it.

## Never steal the foreground

The bridge runs on a machine someone is working on. Background UIA (`desktop_ui_click`,
`desktop_ui_set`) is always tried first: no focus steal, no cursor move. A foreground click is a last
resort for controls with no UIA node, and when one is unavoidable the bridge puts a short on-screen
caption up first saying **why**. Windows it opens for OAuth are tracked and closed automatically once
the sign-in completes, so the desktop is not left littered.

## Is there a CLI for Fusion auth?

Effectively no, and this was investigated on a real machine.
`AdskIdentityManager.exe` takes flags, but they are IPC plumbing (`--process_name`,
`--server_name`), not auth commands: it is a long-running Identity SDK broker and Fusion is its
client over that named channel. There is no `--login`/`--user`/`--token`, and tokens live in a
DPAPI-encrypted store, so there is no headless way to initiate or inject a login. Driving the GUI in
the background, as above, is the answer.
