# Sign-in, sessions and automatic updates

Everything about getting Fusion running, signed into the right Autodesk account, and kept current,
without interrupting the person using the machine.

## Lifecycle

```bash
adom-desktop fusion_readiness '{}'   # installed? running? ready? (never launches)
adom-desktop fusion_start '{}'       # launch + wait for the add-in
adom-desktop fusion_stop  '{}'       # graceful close
adom-desktop fusion_kill  '{}'       # force-kill a wedged session
```

`fusion_readiness` is the verb to poll. It live-detects on **every** call rather than trusting a
bridge-start snapshot, because that snapshot is wrong in both directions: it reported
`installed: false` on a machine where Fusion had since been installed, and `ready: true` on one
where Fusion had been removed.

**Never ask the user to restart Fusion or enable the add-in.** That is the bridge's job.

## Signing in through the right browser profile

Fusion fires its OAuth at the **OS-default browser**. If your Autodesk seat lives in a different
Chrome profile (work vs personal), Fusion signs into the wrong account or stalls.

```bash
adom-desktop fusion_signin '{"profile":"chrome:you@work.com"}'
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
adom-desktop fusion_signin_2fa '{"code":"123456"}'
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

## Automatic updates

Fusion ships updates constantly and nags with an "Update Now / Update Later" panel and a countdown.

The bridge clicks it **for you, in the background**, so you never see it. Fusion downloads and swaps
to the new build on the next launch.

```bash
adom-desktop fusion_set_auto_update '{}'                  # read current setting
adom-desktop fusion_set_auto_update '{"enabled":false}'   # opt out, STICKY
```

On by default for everyone. Opting out persists to `~/.adom/fusion-bridge/prefs.json` and is
honoured permanently; once off, the bridge never touches your update prompts again.

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
