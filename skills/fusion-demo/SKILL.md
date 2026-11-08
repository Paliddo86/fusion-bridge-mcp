---
name: fusion-demo
user-invocable: true
description: >
  Run a FIRST-TIME-USER demo of the Fusion 360 bridge - the guided tour Hydrogen Desktop's installer
  fires when someone new wants to see what this thing does. Covers the one verb (fusion_demo), the
  staged loop, and the two rules that make or break it: FINISH the Autodesk sign-in (never leave a new
  user parked on the sign-in page) by driving the user's NATIVE browser via ABE/nbrowser_* (already
  signed into Autodesk) instead of pup (anonymous), and SET UP APS cloud search rather than just
  mentioning it, then prove it with a live sample search. Then show real work in order: electronics
  PROJECT -> SCHEMATIC -> 2D BOARD -> 3D BOARD. Trigger words - fusion demo, demo the bridge, show me
  fusion, first time user demo, hd installer demo, demo verb, guided tour, showcase fusion, new user
  onboarding demo, fusion_demo.
---

# Demoing the Fusion bridge to a brand-new user

> **Two modes since v1.9.9:** bare `fusion_demo {}` is the staged ONBOARDING flow this
> skill describes (HD installer contract, unchanged). `fusion_demo {"mode": "tour"}` is
> the async SHOWCASE tour: every content type (symbol, footprint, device, 3D chip, and
> with a `query` also project/schematic/2D/3D board) off bundled sample data, with
> structured progress via `fusion_demo_progress` for a real progress bar and a final
> screenshot gallery. Background by default; `foreground: true` lets the user watch.
> Full tour contract: `docs/fusion-demo-tour.md` on this page's repo.

## Why this exists

HD's installer ran a "demo" that opened Fusion and then **sat on the Autodesk sign-in page doing
nothing** (John, 2026-07-20). A first-time user watched an app open and stall. That is the single
worst first impression this bridge can make, and this skill exists to make it impossible.

**A demo is not "launch the app." A demo finishes the blockers and shows real work.**

## The one verb

```bash
adom-bridge fusion_demo '{}'                       # start / resume the tour
adom-bridge fusion_demo '{"query":"BQ25792"}'      # demo a specific design
adom-bridge fusion_demo '{"stage":"skip_aps"}'     # user declined cloud-search setup
```

It is **staged and resumable**. Every call advances as far as it safely can and returns:

| Field | Use it for |
|---|---|
| `stage` | where you are: `not_installed`, `signin`, `aps_setup`, `open_design`, `done`, ... |
| `done` | keep calling `fusion_demo` until this is `true` |
| `narrate` | say this to the user, verbatim-ish. It is written for a newcomer, not an engineer |
| `screenshots` | `[{label,path}]` - SHOW these; labels are `project`, `schematic`, `board_2d`, `board_3d` |
| `steps` | what it actually did, for your own logging |
| `_hint` | the exact next action, including the `nbrowser_*` calls for the browser leg |

Loop: call it, do what `_hint` says, narrate, call it again. Do not improvise a different order.

## Rule 1: FINISH the sign-in, in the user's NATIVE browser

`fusion_demo` does not just tell you to do this - at `stage:"signin"` it **already opened the
Autodesk sign-in in the user's own browser, in the BACKGROUND**, so their current work is never
interrupted. The response tells you where it landed:

| Field | Meaning |
|---|---|
| `signinOpened` | true if the background tab is up |
| `signinWhere` | the EXACT place, e.g. "Chrome - John Personal (jlauer12@gmail.com)" - say this verbatim |
| `signinProfileChoices` | every profile (describe/email/displayName/active/live/extensionInstalled) |
| `signinProfile` / `signinSession` | the ABE profile + sessionId (`fusion-signin`) to drive it |

### Pick the profile by EVIDENCE, never by "whichever is active"

`fusion_demo` probes **every live profile** with
`nbrowser_login_state {profile, url:"https://accounts.autodesk.com/"}` and picks the one that
actually holds an Autodesk session (auth cookies), reporting why in `signinProfileReason` and the
per-profile `autodesk` / `autodeskConfidence` / `autodeskCookies` in `signinProfileChoices`.

This exists because the first version just grabbed the *active* profile and landed in John's
**personal** Chrome with no analysis at all - on another machine that would have been simply wrong.
A user's Autodesk seat can live in any profile, and it is frequently NOT the one they happen to be
using. **Ask the browser; do not assume.**

- **Relay the reason** to the user ("I used X because it's the profile actually signed into Autodesk").
- If `signinProfileAmbiguous` is true (nothing signed in anywhere, or several are), **ASK which
  account their Autodesk login belongs to** rather than picking for them.
- Profiles that are asleep or lack the extension come back `unprobed` - say so instead of silently
  skipping them.

**Never say "it's in your browser" or even "your Chrome".** A power user runs several profiles -
personal, work, media - and a vague pointer sends them hunting through windows. Always name the
browser AND the identity from `signinWhere`. If `signinProfileChoices` has more than one, say which
one you used and offer to switch to another. If the profile they want shows
`extensionInstalled:false`, it needs the one-time extension install in THAT specific profile
(extensions are per-profile).

**Always tell the user where it is waiting, then offer all three paths:**

1. **They finish it themselves**, whenever they like - it is sitting there, backgrounded.
2. **You bring it to the front** for them (foreground that nb window) if they would rather do it now.
3. **You drive it for them** - ASK first (`AskUserQuestion`), then click through
   "Continue with Google/Apple/Microsoft" on their warm session.

Never silently sit and wait, and never foreground it uninvited.

Then:

- **Use ABE (`nbrowser_*`), never pup.** pup is an anonymous browser, so Autodesk demands a full
  login with password + 2FA. The user's real Chrome/Edge profile is **typically already signed into
  Autodesk**, so the same flow is one click. `nbrowser_profiles` lists their real profiles; ABE
  exposes these verbs even before the browser extension is installed.
- Click Fusion's **Sign In** in the BACKGROUND (`desktop_find_control` + `desktop_ui_click`), never a
  coordinate click, never steal focus.
- That fires the OAuth at the **default** browser. If that is the signed-in profile it completes
  itself; if it lands logged-out, re-open the auth URL with
  `nbrowser_open_window {profile:"chrome:<their account>"}`.
- The handoff back is the **Autodesk Identity Manager** overlay: tick "Always allow", click "Open".
- **Never type their password or 2FA.** If a secret is demanded, `fusion_notify_owner` toasts their
  machine so they type it. Sign-in codes expire in ~2 minutes; if it expires, click Sign In again for
  a fresh one.

Full detail: the **fusion-autodesk-signin** skill.

## Rule 2: SET UP APS, don't just name-drop it

When `stage:"aps_setup"`, actually configure it (`fusion_aps_status` -> `fusion_aps_set_client_id`
if unconfigured -> `fusion_aps_signin` driven in the native browser -> poll until `tokenLive:true`).
Then the demo runs a **live sample search** so the user watches their whole Autodesk hub return
results in about two seconds.

If they decline, call `fusion_demo {stage:"skip_aps"}` and still TELL them what it is: server-indexed
search across the team hub in seconds. The old in-app search took 30+ minutes and crashed Fusion, so
it is disabled - APS is the only cloud-search path. See **fusion-aps-search**.

## Rule 3: the tour order is fixed

**electronics PROJECT -> SCHEMATIC -> 2D BOARD -> 3D BOARD.** That sequence tells the story of how a
board is actually made, and each step is screenshotted so you can show it. Always open the PROJECT
(`EcadDesignProductType`), never a `.brd`/`.sch`/3D child - a child opens an isolated, often empty
view and the demo looks broken.

## Landing it

Close by offering next steps in the user's own words, not verb names: "export the Gerbers", "make me
a SOIC-8 with my part number etched on it", "check this against JLCPCB rules", "find my other
boards". The full catalog is `fusion_describe`.

## If Fusion isn't installed: offer it, and say what they actually get

Do not just report "not installed". OFFER, and be concrete AND honest about the tiers:

- **Free 30-day trial: fully featured.** Every step of this demo works - electronics design,
  schematic, 2D board, 3D board, Gerber/BOM/CPL exports, APS cloud search. This is the one to start
  on; there is no reason to hedge about it.
- **After the trial: Fusion for Personal Use is free** for non-commercial work. Still genuinely
  useful for hobby PCBs, but it LIMITS electronics (roughly 2 layers, 2 schematic sheets, a modest
  board area) and some export options. Say that plainly rather than letting them discover it.
- **Even an expired / read-only install still OPENS and VIEWS designs** (it only blocks
  save/export/modify), so nothing they make gets locked away.

Then `fusion_install_fusion` (no shell approval needed, streams the installer, 10-30 min), poll
`fusion_readiness` until `installed:true`, and call `fusion_demo` again to pick the tour back up.
Full flow: **fusion-onboarding**.

## If something else is missing

- Nothing to open -> ask which board, or `fusion_aps_search` and open the first hit.
- Never end a demo on an error screen. If a stage fails, say what you tried, and offer the next thing.

## The guided electronics tour (from 1.9.360): `fusion_demo {mode:"tour"}`

John, 2026-09-04: Adom is an electronics prototyping company, so the tour demos electronics
and never a mechanical part. Eight beats, all on the bundled sample part (ADS8588SIPM, 64
pins), each with a caption, pre-rendered narration and on-screen motion:

| # | beat | what happens | motion |
|---|---|---|---|
| 1 | Open the sample library | `open_lbr` of the staged sample; caption with the pin and pad counts | none |
| 2 | Symbol: the full pinout | symbol editor, `WINDOW FIT` | 2D zoom in, in, out, fit |
| 3 | Footprint: pads and layers | footprint editor | 2D zoom |
| 4 | 3D chip: minted and bound | the library chain's bind core on the sample (STEP onto the footprint, saved to the hub, bound); the bound f3d is opened in the 3D canvas | iso, orbits, zoom |
| 5 | A real board: the schematic | the BQ25792 charger found in the hub (`fusion_aps_search`, exact name, Main project preferred), opened by urn, `show_schematic`; `args.board` / `args.boardUrn` override; a hub without it gets the generated solo board around the sample part instead (`hubBoard: false`, the caption says so) | 2D zoom |
| 6 | The board in 2D | the board document in front, `show_2d_board` | 2D zoom |
| 7 | The board in 3D | `show_3d_board`, waits for a Design with bodies | iso, orbits, zoom |
| 8 | Cloud search over your hub | `fusion_aps_search` for the part; skipped honestly when not signed in | none |

Multipart libraries are MENTIONED in the narration and never shown: a real 3D bind is minutes
per part, so a shown multipart beat is a nightmare, not a demo (John).

**Captions are 32px on one line.** ab's "medium" grew to 48px in v2.0.84 and the tour's
captions overlapped the glass remote (John, 2026-09-04); the tour passes `fontSize: 32`, keeps
every line under about 85 characters, and records ab's caption warnings (`too_wide_for_screen`)
in `captions[].warnings`, so a wrap never passes silently.

**Measured before it moves.** Before any gesture the tour checks the Fusion window is in
front (z-order, one extra raise, 1.5 s), its title is the expected document, the canvas is
painted (distinct colours in the central 60%) and stable (two captures 0.8 s apart identical).
Motion runs only on a verified window and is itself verified by capture (`viewChanged`). On a
box driven over remote desktop the viewer sits on top of everything; the record says so
(`frontNote`) and still proves layout and motion. Fusion is placed on 80% of the primary work
area for the show (saved bounds, a leftovers file) and restored at the end.

**The remote.** ab's glass panel (`desktop_demo_panel`, id `fusion-tour`, `reason` on every
show, stage-advance shows send only id and stage) with play, pause, next, back, mute and the X
(= stop). Presses reach the tour instantly through a loopback callback (`/demo/event` on the
exe, ab 2.1.17+) and a poll backstop deduped by `at`; `fusion_demo_control {demoId, action}` is
the dashboard's path onto the same FIFO. Every press is acked with a caption at the top of the
screen the instant it arrives and recorded in `presses[]` with its source (`callback`,
`poll`, `dashboard`, `*-synthetic`). Pause holds at the next gesture; a Fusion operation in
flight (the two-minute bind, a document opening) finishes first. Back is the previous beat.
Mute stops the local audio (session-only); unmute replays the beat's narration. Stop ends the
tour, silences narration, clears captions, hides the panel, restores the window and closes the
tour's documents.

**⛔ Never poll the panel yourself.** `desktop_demo_panel {action:"poll", id:"fusion-tour"}`
DRAINS the press queue, and the tour is the consumer. Verify only through
`fusion_demo_progress`: `step`, `beatIndex`/`beatCount`, `percent`, `paused`, `muted`,
`stopping`, `panel`, `presses[]`, `captions[]`, per-step `verified` / `placed` /
`foregrounded` / `motion`, `shots[]` (one evidence frame per beat, `imagesAs:"paths"` to skip
the base64), `windowsRestored`, `tourDocsClosed` (`"closing"` while the documents close after
`done:true`, so the end never reads as dead air).

Measured on ConfRoomROG 2026-09-04 (exe 1.9.359+): eight of eight beats, every beat front,
title, painted and stable on the first attempt, 2D and 3D motion verified by capture, the 3D
chip bound in 80 to 90 s, tour done in about five minutes. Pause, play, mute, unmute, back and
stop were each honoured from the dashboard path, from the loopback callback, and from real OS
clicks on the panel. `{action:"press"}` (the synthetic press hook) needs ab 2.1.26+.

`legacy:true` still runs the previous step-list tour.

## The 3D stop is a REAL bind (from 1.9.345)

The tour's 3D stop used to peek at the package generator with the footprint only, and the
3D board then carried Fusion's placeholder slab while the caption claimed a minted chip.
Since 1.9.345 the stop runs the library chain's bind core on the bundled sample (STEP onto
the footprint, orientation, pin-1 check, f3d saved to the hub, binding written into the
sample library), so the device view, 2D board and 3D board all carry the real body.
Measured on ConfRoomROG 2026-09-03: `bound3d: true`, tour 353s end to end, zero failures,
nine frames. If the bind cannot complete the tour falls back to the generator view and the
3D-board caption says so instead of claiming a body that is not there. Read `bound3d` and
`bind` in `fusion_demo_progress`.
