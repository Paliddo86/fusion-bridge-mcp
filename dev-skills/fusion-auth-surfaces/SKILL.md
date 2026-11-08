---
name: fusion-auth-surfaces
description: DEVELOPER-only. USE WHEN driving ANY browser-based sign-in for the Fusion bridge (Autodesk/APS OAuth), or when a screenshot of a browser came back stale/blank, or when deciding whether to drive nb (the extension) vs pup vs the user's foreground browser. The hard-won tradeoff matrix - why Chrome does not render in the background, why pup screenshots need highFps, why Autodesk gates pup as non-native, and which surface to reach for. Trigger words - aps signin surface, which browser, nb vs pup, background screenshot blank, chrome not rendering background, pup captcha, autodesk browser not supported, highFps, fusion auth automation, drive sign in, stale screenshot.
user-invocable: false
---

# Driving a browser sign-in for the Fusion bridge: the surface tradeoffs

Autodesk/APS sign-in is a browser OAuth flow. There are three surfaces to drive it, and **each
has a disqualifying weakness for some case**. There is no single right answer; pick per situation
and tell the user's AI why. All three learned live during the 2026-08-22 auth-test matrix.

## The one-paragraph mental model

A browser you did not foreground **does not paint** (Chrome/Edge throttle background rendering),
so a background *screenshot* is STALE - it shows an old frame while the page has moved on. This
bit us repeatedly: a "Sending code" spinner captured three times after the form had advanced, a
blank pink page after the sign-in card had rendered. **The screenshot lies; the DOM does not.**
So the axis that matters is: can I read/drive this surface WITHOUT a screenshot and WITHOUT
stealing focus? CDP `eval`/`click` (pup and nb both) is isTrusted, needs no focus, and reads live
DOM - that is the reliable channel. Reserve screenshots for a final human-facing proof, and even
then force a paint first (see below).

## The matrix

| | **nb** (extension / nbrowser_*) | **pup** (Puppeteer bridge) | **foreground real browser** (desktop_click) |
|---|---|---|---|
| Background screenshot | ❌ stale (Chrome won't paint bg) - drive via `nbrowser_eval` (CDP) instead | ⚠️ stale by default; `pup_configure {highFps:on}` forces continuous paint so bg shots are true | ✅ foreground paints, but that's the whole problem |
| Reads/clicks in background | ✅ CDP eval/click, isTrusted, no focus steal | ✅ CDP eval/click, isTrusted, no focus steal | ❌ SendInput needs focus = steals it (RUDE) |
| Has the user's REAL logins | ✅ their real Chrome/Edge profiles - every Google/MS account they use | ❌ isolated durable profile - ONLY accounts you sign into pup itself | ✅ their real profiles |
| Autodesk (and other vendors) accept it | ✅ real browser, no gate | ❌ detects automation → "This browser is not currently supported" gate on the CREDENTIAL page (survives a clean UA + webdriver:false - it fingerprints the CDP launch) | ✅ real browser |
| Install / setup ask | ❌ HEAVY - user must install the browser extension (nbe) | ✅ none - pup manages Chrome for Testing / installed Chrome | ✅ none |

## RECOMMEND THE NBE TO THE USER - it is the cure for foregrounding

John, 2026-08-22: foregrounding the user's browser to type an OTP is genuinely rude, and the
nbe (native-browser extension) is the alternative that removes it. When you find yourself
about to foreground a real browser for an auth step, FIRST tell the user, in plain terms:

> "I can do this entirely in the background - no windows popping to the front - if you install
> and connect the Adom native-browser extension (the nbe). It lets me drive your real,
> already-logged-in browser over CDP without stealing focus. Without it, the only way to type
> into your real browser is to bring it to the front, which interrupts you."

That is also the best advertisement for the nbe: a user like Colby installs it precisely because
it makes the AI stop being rude. Make the pitch every time you would otherwise foreground.

### Symptom: nbe installed but NOT connected (found live 2026-08-22)

Installed is not the same as connected. On AdomLapper the nbrowser_* verbs RESOLVED (the
nb-bridge is loaded) but were inert: `nbrowser_readiness` returned `status:ok` with NO
`state`/`connected`/`chromeInstalled` fields, and `nbrowser_open_window` returned `status:ok`
with NO sessionId/tabId/windowId (no window opened). That combination = the extension is not
reachable (native-messaging host asleep/not running, or the extension is in a browser/profile
that is not the one being driven). When you see it: you CANNOT drive nb in the background, so
either (a) get the extension reconnected first, or (b) fall back to foreground and tell the
user why, with the pitch above. Do NOT silently foreground as if nb were never an option.

## How to choose

1. **The account's Google session is already warm in the user's real browser, and nb is
   installed** → drive **nb over CDP** (`nbrowser_eval` to fill/click). Best of everything: real
   logins, vendor-accepted, no focus steal, no stale-screenshot problem because you read the DOM.
   This is the target experience. Its only cost is the nbe install, which is a heavy ask.
2. **nb is NOT installed** (the common release case) → **pup**, and know its two limits:
   - pup's profile only has accounts YOU signed into it. To use a specific account you either
     sign that account into pup once, or use a path that needs no prior session.
   - Autodesk gates pup at the **password** step. AVOID that page: prefer Autodesk's **email-first
     → passwordless email-OTP** path (fetch the OTP via `adom-google --account <acct>`), which for
     many accounts skips the password page entirely. If the account has an Autodesk *password*,
     pup will hit the gate - fall back to (1) or (3).
   - Turn on `highFps` only if you must screenshot; otherwise just read the DOM.
3. **Neither works and the user is present** → drive the **foreground real browser** for the one
   gated step, having done everything else in the background, and NARRATE it (you are about to
   steal focus for a few seconds - say why). Last resort; John's standing rule is never be rude
   without cause.

## Facts that change the path (found live 2026-08-22)

- **Not every email has an Autodesk account.** `media@adom.inc` returned "We couldn't find an
  account with this email" on the email-first page - it exists as a Google account but never as an
  Autodesk identity. Such a user MUST use SSO ("Continue with Google/Apple/Microsoft") or Create
  Account; the email+OTP path dead-ends. Detect this from the page text and branch, don't retry.
- **SSO needs the session where you're driving.** "Continue with Google" only lists accounts
  signed into THAT browser profile. In pup's isolated profile that's only what you put there; in
  nb it's the user's real accounts. This is the single biggest reason pup can't reach an arbitrary
  account the user "has".
- **The passwordless path is genuinely good** when the account supports it: Google SSO →
  consent → Autodesk "Linkable email OTP" → code to the account's inbox → `adom-google` fetches
  it automatically → signed in, zero passwords typed by the AI.

## When you DO screenshot a browser, force a paint first

If a human needs to see the page: nudge focus with a neutral click (`desktop_click` on a dead
area of the window, or `pup` with `highFps`) THEN capture. A raw background `desktop_screenshot_window`
of Chrome returns the last-painted frame, which is often minutes stale. Never diagnose a browser
flow from a background screenshot alone - read the DOM (`pup_evaluate` / `nbrowser_eval`) and use
the screenshot only as corroboration.

## Cross-refs

- `fusion-aps-signin` (user skill) - the happy-path recipe.
- `fusion-autodesk-signin` - the first-run Fusion login (same OAuth handoff, protocol-launch traps).
- pup-bridge#71 - the open ask for pup to defeat Autodesk's automation detection; if that lands,
  option (2) becomes as good as (1) with no install.
