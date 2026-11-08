---
name: fusion-aps-signin
description: Drive the Autodesk sign-in flow in the user's real browser (native-browser extension, nbrowser_*) to set up APS (Autodesk Platform Services) for the Fusion bridge's fusion_aps_* cloud search — open a dedicated BACKGROUND window (no foregrounding needed), MONITOR the sign-in, and auto-retrieve + enter the emailed one-time passcode (OTP) via adom-google before it expires. Handles native email+password AND Google/Apple/Microsoft SSO (warm-session account pick, no password) plus the "Account already exists → Link Google account" interstitial. Use when signing into Autodesk/APS, registering an APS app, configuring fusion_aps_search, or any "log me into Fusion/Autodesk for APS" request. Trigger words — aps signin, sign in to autodesk, autodesk otp, fusion aps login, register aps app, aps client id, fusion_aps_signin, autodesk verification code, link google account autodesk, google sso autodesk, callback listener expired, localhost refused to connect.
---

# Fusion / APS sign-in (driven, with auto-OTP)

Goal: get the user signed into **aps.autodesk.com** in their real browser so you can
register an APS app and configure the Fusion bridge's `fusion_aps_*` search. The hard
part is Autodesk's **email OTP**, which **expires fast** — the user may walk away ("get a
cup of coffee"), so you must **monitor** the flow and turn the code around in seconds.

All browser steps use the native-browser extension verbs (`nbrowser_*`) against the
ab-connected laptop: `adom-bridge --target <laptop> nbrowser_* '{...}'`.

## Working principles (non-negotiable)

- **DO IT YOURSELF. NEVER ask the user to do anything manually that you can do.** That is the
  entire point of the Adom AI ecosystem — the AI drives, the user doesn't. Don't hand the
  user a "go create the app and paste me the Client ID" cop-out; drive the whole flow
  (open windows, fill forms, fetch the OTP, click through) to completion yourself. Only
  surface a real decision (a genuine fork) — never offload mechanical work.
- **NEVER navigate the user's CURRENT page/window away from them** — that hijacks their live
  work (e.g. their Edge tab) and is unacceptable. **ALWAYS open a NEW window**
  (`nbrowser_open_window {url}`) for whatever you need and drive ONLY that window's
  `sessionId`. Opening your own windows and driving them is expected; reusing/navigating a
  window the user is viewing is not. `desktop_bring_to_front` to surface a window for the
  user is fine.

## Golden rules (learned the hard way)

1. **OTP expires in ~a few minutes. Be FAST.** Don't go fetch-and-think between resend and
   entry — fetch the code and enter it in the same burst. We burned the 1st code dithering;
   3rd round worked by resending and entering immediately.
2. **MONITOR the sign-in, don't fire-and-forget.** Poll the page state every few seconds
   (the user may step away). Detect each transition: email → OTP → "code expired" →
   "account already exists" → "linked" → portal. Never assume; re-read the page.
3. **Open a DEDICATED browser window** with `nbrowser_open_window` and **target it by
   `sessionId`** in every `nbrowser_eval/type/click`. The extension **refuses to drive PWA /
   installed-app windows** (e.g. "Hydrogen - Chat") and `nbrowser_eval` defaults to the
   foreground tab — which is often the wrong (PWA) window.
4. **Foreground it with `desktop_bring_to_front`** (the ab verb) so the user can see/act on
   the login. `nbrowser_focus_window` is NOT implemented.
5. **Proactively use adom-google.** Autodesk emails the OTP — you can read it via the
   `adom-google` Gmail API passthrough. If adom-google isn't set up, TELL the user it exists
   and offer to set it up (`adom-google auth`) so you can fetch the code for them. Gmail for
   Adom = **john@adom.inc** (work account).

## PROVEN END-TO-END, 2026-08-22 (jlauer12@gmail.com, Google-federated, fully in the BACKGROUND)

A complete real run, start to signed-in APS token, driven entirely through `nbrowser_*` on the
user's real profile with **no foregrounding**. Read this before the step-by-step — it captures the
two things that actually make or break the flow.

### ⏱ THE #1 KILLER: the callback listener lives only ~180 seconds

`fusion_aps_signin` binds a localhost listener (port 8917/8918/8920) that captures the auth code,
and that listener **self-closes after 180s** (`aps.py start_signin`, the `_serve` deadline). The
whole SSO + OTP + account-linking sequence below can easily take LONGER than 3 minutes, and if it
does the redirect lands on **"This site can't be reached — localhost refused to connect"**, the code
is lost, and `fusion_aps_status` still shows the OLD user. This bit us twice.
- **Mint the authUrl LAST, then move fast.** Do any slow parts (waking the profile, finding the
  account, one-time account-linking) on a THROWAWAY first pass, THEN call `fusion_aps_signin` and
  race the now-fast, already-linked flow to the callback inside 180s.
- **If you see "localhost refused to connect":** the listener died, nothing is broken — just call
  `fusion_aps_signin` again for a fresh 180s window and rush it. The second pass is much faster
  because SSO is warm and any linking is already done.
- Suppress the bridge's own auto-open (`fusion_aps_signin {noOpen:true, autoOpen:false}`), grab the
  returned `authUrl`, and drive it in YOUR nb window so nothing foregrounds.

### The Google/Apple/Microsoft-SSO branch (no password, ever)

Many Autodesk accounts are federated. In the user's real profile the SSO session is warm, so:
1. Email-first page → type the email → **Next** may redirect to `accounts.google.com/...accountchooser`
   ("Sign in with Google → Choose an account to continue to Autodesk"). Click the account tile by
   text (`nbrowser_click {text:"user@gmail.com"}`) — the warm session means **no password prompt**.
2. Autodesk still sends an **email OTP** ("One-time passcode") → fetch + fill the segmented boxes
   (steps 3-4 below).
3. First-time federation shows **"Account already exists … Link Google account"** → click
   **Link Google account**. The OTP already proved ownership, so **no Autodesk password is needed**;
   it lands on "Google account linked → Continue in N seconds" and auto-proceeds to consent.
4. Consent page ("Authorize application — Adom Fusion Bridge is requesting permission") →
   `nbrowser_click {text:"Allow"}` → callback → token captured.

### Fetch the OTP for a SPECIFIC account, and verify WHO you got

- The account whose inbox holds the code is the one signing in — target it explicitly:
  `adom-google --account personal ...` (jlauer12) / `--account media`. The `adom-google gmail`
  subcommand only handles attachments; use `adom-google --account <a> api "https://gmail.googleapis.com/..."`
  for search/read (see step 3).
- After the callback, confirm identity with `fusion_aps_status` → `signedInAs.email`. As of **bridge
  1.9.139** this is correct per-account; before it, a cache keyed on the JWT prefix (identical header
  across all Autodesk tokens) made every account display as the FIRST one signed in. If you see the
  wrong `signedInAs` on an OLD bridge, update it — the token itself was always right.

### Reuse-window note

`nbrowser_navigate {sessionId}` on a window YOU opened (owned session) is fine for re-pointing it at
a fresh authUrl during a retry — the never-navigate rule only protects the user's OWN windows.

## Step-by-step

### 1. Open + foreground a dedicated window at the APS portal
```
adom-bridge --target <laptop> nbrowser_open_window '{"url":"https://aps.autodesk.com/myapps/"}'
# capture sessionId (e.g. s29) from the result; use it in EVERY later call
adom-bridge --target <laptop> desktop_bring_to_front '{...the browser window...}'   # so the user sees it
```
Read state (always pass the sessionId):
```
nbrowser_eval '{"sessionId":"s29","expression":"JSON.stringify({url:location.href,title:document.title,body:document.body.innerText.slice(0,300)})"}'
```
If `location.href` is `signin.autodesk.com`, the profile has **no active Autodesk session** —
the user must start sign-in (enter email, or an SSO button). You won't type their password.

### 2. Monitor for the OTP screen
Poll `nbrowser_eval` every ~3-5s. The OTP page title is **"Linkable email OTP - Autodesk"**
with six single-char boxes `input[name=pin-1..pin-6]` and body "Enter the code from your
email. … Email sent."

### 3. Fetch the freshest OTP from Gmail (adom-google)
Check auth first: `adom-google status` (need a refresh token + gmail scope). Then:
```bash
# list newest Autodesk passcode mail (strip the leading "HTTP 200 ..." line)
MID=$(adom-google api "https://gmail.googleapis.com/gmail/v1/users/me/messages" \
      -q "q=from:autodesk subject:passcode newer_than:1h" -q "maxResults=1" \
      | tail -n +2 | python3 -c "import sys,json;print(json.load(sys.stdin)['messages'][0]['id'])")
# get it, pull the 6-digit code from the snippet ("Code: 205753")
adom-google api "https://gmail.googleapis.com/gmail/v1/users/me/messages/$MID" -q "format=full" \
  | tail -n +2 | python3 -c "import sys,json,re;d=json.load(sys.stdin);print(re.search(r'Code:\s*(\d{6})',d['snippet']).group(1))"
```
Always take the **newest** message (the email footer also contains `000000`/`212121` noise —
match `Code:\s*(\d{6})`, not any 6 digits).

### 4. Enter the code — digit-by-digit (boxes are maxlen-1, NO auto-advance)
Bulk-typing the whole string only fills box 1. Focus each box, type its digit:
```bash
for i in 1 2 3 4 5 6; do
  D=$(echo "$CODE" | cut -c$i)
  nbrowser_eval  '{"sessionId":"s29","expression":"document.querySelector('input[name=pin-'$i']').focus()"}'
  nbrowser_type  '{"sessionId":"s29","text":"'$D'"}'
done
nbrowser_click '{"sessionId":"s29","text":"Next"}'
```
Verify before submitting: read `[...].map(i=>i.value).join("")` == the code.

### 5. If "This code has expired" → resend + redo FAST
```
nbrowser_click '{"sessionId":"s29","text":"Resend"}'
# wait ~12s, fetch the NEWEST code (step 3), enter immediately (step 4)
```

### 6. "Account already exists" (Google-federated account)
Body: "An Autodesk account already exists with the email … sign in with Google … link …".
Click **"Link Google account"**. If the Google session is already active in Chrome it links
instantly ("Google account linked … Continue in 5 seconds") and lands in the portal — no
account picker. If a Google picker appears, pick **john@adom.inc**.

### 7. First-time APS → accept Terms of Service
First portal visit shows "Welcome aboard! … accept our Terms of Service". Accept it.

### 7.5 Developer-profile form (REQUIRED before you can create an app — you must fill it)
First-time APS gates app creation behind a one-time **developer/company profile** at
`https://aps.autodesk.com/myapps/create` ("Ready for next steps?"). YOU fill it from the
user's profile memory (`user_profile.md` — company, website, address). Fields seen:
`company`, `website`, `country`, `stateProvince`, `city`, `postalCode` (plain text) and
`roleId`, `accountPurpose` (**MUI comboboxes**) + a `tosAgreed` checkbox.
- Plain text: set via the native value setter + dispatch `input`+`change`+`blur` (MUI only
  enables Submit after these fire):
  ```
  nbrowser_eval set vals {company:"Adom Industries, Inc.",website:"https://adom.inc",
    city:"Fort Worth",stateProvince:"TX",postalCode:"76177"}  // country prefills "US"
  ```
- Comboboxes — can't just set `.value`; click to open, then **click the option by text**
  (click-by-text finds it even when not in the visible top-N; typing to filter often does
  NOT work). The fields and good picks for Adom:
  - `roleId` = **Business owner/Entrepreneur** (John is the founder)
  - `accountPurpose` = **Business**
  - **Industry (select up to 3)** — REQUIRED, and it's the one that silently blocks Submit.
    It has NO `name` attr (MUI multiselect) — find it by its label "Industry (select up to 3)"
    (map each input to its nearest ancestor's label text + getBoundingClientRect, then click
    the field's center coords). Pick **Industrial machinery** (Adom = PCB/electronics mfg).
- Tick `tosAgreed` (click the "I agree to the Autodesk Platform Services" label). **It
  re-resets to false after combobox interactions / re-renders — re-verify it's checked right
  before Submit.**
- **Submit silently no-ops while ANY required field is invalid, with the button still
  `enabled` and NO scrape-able error.** When Submit won't advance, **take a screenshot
  (`nbrowser_screenshot {sessionId}`) and READ it** — inline "Please fill out this field"
  tooltips (e.g. the empty Industry box) only show visually, not in the DOM you're scraping.
  This is the fastest way to find a missed required field.
- Field coords from `getBoundingClientRect` are in PAGE space (what `nbrowser_click {x,y}`
  wants) — do NOT use screenshot pixel coords (the shot is downscaled, different origin).
Known profile values (Adom / John): Company "Adom Industries, Inc.", Website https://adom.inc,
4400 Alliance Gateway Fwy, Fort Worth, TX 76177, US, john@adom.inc.

### 7.55 Get the FREE APS plan (drive the whole checkout yourself — incl. the card)
Newer APS gates app creation behind a **developer hub**, which requires an **APS plan**. The
**Free tier is $0** but Autodesk requires a **payment method "to verify identity"** (no charge).
**The AI drives 100% of this**, including entering the card. Live-verified flow:
1. New window → `https://aps.autodesk.com/hubs/` → the "It seems you don't have a hub yet"
   page → **Step 1 "Get an APS plan" → click "View options"** (or go straight to
   `https://www.autodesk.com/products/autodesk-platform-services/overview`).
2. On the pricing tiers (3 columns: **Free** | Prepay $99 | Pay-as-you-go), click the
   **leftmost "Add to Cart"** = the **Free** tier. (Get all `Add to Cart` rects, sort by x,
   take the smallest — Free is the left column. NEVER click the paid columns.)
3. Cart drawer shows **"APS Free — $0.00", SUBTOTAL $0.00**. Click **Check out**.
4. `checkout.autodesk.com` — a 3-step accordion: **Cart → Customer details → Payment**, with
   **Order summary TOTAL $0.00**. Click **Continue**; an abandon-cart modal ("Don't leave your
   cart behind!") may pop — click **CONTINUE CHECKOUT**, then **Continue** again. Customer
   details auto-fill from the Autodesk account. It lands open on **Payment**.
5. ⚠️ **VERIFY TOTAL = $0.00 before doing anything in Payment.** If it's ever nonzero, STOP —
   never proceed (hard rule: no user is ever charged).
6. **Payment — almost always a card is ALREADY on file (no card entry needed).** **Almost
   every Adom user has a PAID Fusion 360 subscription**, which means Autodesk already has their
   payment method on file. So the Payment step will typically show an **existing card** —
   **SELECT it** and move on. The AI completes this with **zero** card handling and zero user
   input. Only in the rare case of NO card on file do you fall back to collecting one from the
   user (their own card, ID-verification only; **TREAT AS SENSITIVE — never echo/store/log
   it**). Either way, then click **Submit order** (agrees to Autodesk's $0 Online Purchase
   Terms; auto-renews at $0).
7. After the $0 order confirms, continue to the hub.

### 7.6 Create the developer hub (manage.autodesk.com)
Per Autodesk docs: `manage.autodesk.com` → **Products and Services → Hubs → Create hub** →
select **APS developer hub** → enter a name (e.g. "Adom") + optional description → **Create
and Activate**. (The account may already show an org hub named "Adom" at
`manage.autodesk.com/hubs-list`; if a usable APS developer hub already exists, use it.)
If routes 404 / loop, open a FRESH window and retry — don't blind-click, and don't offload it.

### 7.7 (was 7.6) Register the app — drive it, don't hand it off
After the profile, newer APS routes app creation through a **developer hub**:
`/myapps/create` → `/hubs/@personal/applications/create` may 404 ("Not found"), and `/myapps`
redirects to "you don't have a hub yet" with a **Create hub** button that bounces to
`manage.autodesk.com/hubs-list` (a DIFFERENT portal; the account may already list an org hub
like "Adom"). This area is non-linear and account-structural. If it 404s / loops, **don't blind-click** —
open a FRESH window, reload, and work the hub→app path yourself (create/select the hub, then
the app). Drive it to a Client ID yourself; do NOT offload it to the user. Once you have the
Client ID, `fusion_aps_set_client_id` → `fusion_aps_signin` → `fusion_aps_search`.

### 8. Register the APS app (then configure the bridge)
Create App → enable **Data Management API** → **Callback URL =
`http://localhost:8917/fusion-aps/callback`** (must match the bridge's `REDIRECT_URI`
EXACTLY) → create → copy the **Client ID** (PKCE = public client; the secret is unused).
Then:
```
adom-bridge --target <laptop> fusion_aps_set_client_id '{"clientId":"<CLIENT_ID>"}'
adom-bridge --target <laptop> fusion_aps_signin '{}'      # opens browser; bg listener on :8917 stores the token
# poll fusion_aps_status until {"data":{"signedIn":true,"tokenLive":true}}
adom-bridge --target <laptop> fusion_aps_search '{"query":"<part name>"}'
```

## LIVE-VERIFIED end-to-end run (2026-06-26, Adom org) — the exact sequence
This whole flow was driven live and works. Replicate it per org:
1. **Sign in** (§1–7): new window → aps.autodesk.com → email → OTP from Gmail (adom-google) →
   "Account already exists" → **Link Google account** → lands in APS.
2. **Dev profile** (§7.5): fill from user profile; Industry is REQUIRED (screenshot to catch
   it); role=Business owner, purpose=Business, Industry=Industrial machinery; Submit.
3. **Free plan** (§7.55): aps.autodesk.com/hubs → "Get an APS plan" → View options → pricing:
   **leftmost "Add to Cart" = Free** → cart shows "APS Free $0.00" → Check out →
   checkout.autodesk.com 3-step accordion (Cart→Customer details→Payment), **TOTAL $0.00** →
   Continue (+ "CONTINUE CHECKOUT" on the abandon-cart modal) → **Payment step almost always
   already shows an existing card on file** (Fusion subscribers) ✅ → **Submit order** → "Order
   Confirmation". $0, existing card, no card entry.
4. **APS Developer Hub** (§7.6): manage.autodesk.com → Products and services → **Hubs** →
   **Create hub ▾ → APS Developer Hub** → name "Adom APS" → **Create & Activate** (takes a
   minute). NOTE: this is a CREDENTIAL container with ZERO files — NOT the team's data hub.
5. **Register the app** (§7.7): aps.autodesk.com/myapps → hub now selectable → **Create
   application** → Name "Adom Fusion Bridge" → app-type = **"Desktop, Mobile, Single-Page App"**
   (radio value `native` = **Authorization Code with PKCE**) → Create. ⭐ PKCE app type =
   **"Paid APIs are unavailable"** → structurally can't be charged (1st never-charge lock).
6. **Callbacks + APIs**: in App settings → Callback URL section, the default is
   `http://localhost:8080/`. Click **"+ Add URL" 5×** (→ 6 fields), fill all 6 with the
   bridge's registered set (`all_registered_callbacks()`): localhost+127.0.0.1 × ports
   8917/8918/8920. API Access defaults to all free APIs incl. **Data Management** (keep). Click
   **Save changes** (find it by coords — click-by-text can hit the wrong element).
7. **Client ID**: App settings → Client Credentials → copy the **Client ID** (public PKCE id;
   NO secret). Extract from the DOM, not a pixel read.
8. **Wire + sign in**: `fusion_aps_set_client_id {clientId}` → `fusion_aps_signin` (bridge
   opens consent, listens on the bound port) → poll `fusion_aps_status` until `signedIn:true` →
   `fusion_aps_search {query}` → prove it returns files from the team's shared **Main** folder.

## Never-charge — TRIPLE locked (don't weaken any of these)
1. The **PKCE app type cannot use paid APIs** (Autodesk-enforced).
2. The bridge's **free-endpoint allowlist** (`/project/v1/`, `/data/v1/` only).
3. The bridge's **monthly hard cap** (fails closed). Card on file = identity verification only.

## Port choice for the callback
Check availability LIVE on the box (no ab verb exists): `python` bind-test on 127.0.0.1 +
`netsh int ipv4 show excludedportrange protocol=tcp`. 8917/8918/8920 verified free + outside
Windows reserved ranges (5357, 27339, 50000-50059). The box runs ~67 listeners — don't assume.

## Gotchas
- `nbrowser_eval/type/click` without `sessionId` hit the foreground tab → often the wrong
  (PWA) window. Always pass the dedicated `sessionId`.
- `adom-google api` prints a `HTTP 200 GET …` header line before the JSON — `tail -n +2`.
- The bridge relay only forwards `output/data/success/error/_hint` — irrelevant here but
  relevant when reading `fusion_aps_*` results (fields live under `data`).
- Don't auto-enter the user's Autodesk **password** or drive their Google **consent** —
  identity decisions are theirs; automate the OTP + form-filling, not the credentials.

## ⭐ APS AUTH-001 self-heal — the app lost its Data Management API product (verified live 2026-07-14)

**Symptom:** `fusion_aps_signin` now returns `errorCode: "aps_app_missing_product"` (the bridge probes
the /authorize endpoint server-side and detects it BEFORE opening a browser), or you see the raw
`{"errorCode":"AUTH-001","developerMessage":"The client_id specified does not have access to the api
product"}` on the authorize page. It means the org's APS app at aps.autodesk.com/myapps lost its **Data
Management API** product entitlement (it can work for months, then break). The app config often still
*looks* correct — the fix is a forced re-commit.

**Recovery (drive with ABE/nbrowser — install the extension FIRST if it isn't; coordinate-clicking the
portal is fragile and Edge crashes):**
1. `nbrowser_open_window {sessionId, thread, purpose, profile:<warm Edge/Chrome>, url:"https://aps.autodesk.com/myapps/"}` → open the app whose Client ID matches (in the org's "Adom APS" dev hub).
2. App-settings → **API Access**: confirm **Data Management API** is checked. Then force a re-commit —
   `nbrowser_eval` a click on the **visible** Save button:
   `(()=>{var b=[...document.querySelectorAll("button")].filter(x=>/save changes/i.test(x.textContent)&&x.offsetParent!==null);if(b.length)b[0].click();return JSON.stringify({n:b.length})})()`
   (the bare text "Save changes" matches ~9 hidden duplicates, so `nbrowser_click {text}` is ambiguous —
   use the offsetParent filter). If Save is disabled, open the API dropdown and toggle one unneeded API
   off first to dirty the form.
3. **Wait 1–3 min** for Autodesk to propagate the entitlement.
4. Retry `fusion_aps_signin` → it now reaches the **consent** screen ("… is requesting permission …
   View/Manage/Write your data … **Allow**"). `nbrowser_navigate` the session to the fresh `authUrl`,
   then `nbrowser_click {text:"Allow"}` → the URL flips to `localhost:<port>/fusion-aps/callback?code=…`
   and the bridge's listener captures the token.
5. Poll `fusion_aps_status` until `signedIn:true`. Done — APS search is fast and never touches Fusion's
   main thread.

## The BEST sign-in path: the user's NATIVE Chrome/Edge via ABE (adom-browser-extension)

Autodesk's OTP/2FA and its "which profile is signed in" problem vanish if we drive the user's
OWN browser that is ALREADY logged into Autodesk. That is what ABE (adom-browser-extension) is
for: it exposes `nbrowser_*` verbs that open a URL in the user's real Chrome/Edge profile.

**Recommend ABE first, and hand-hold the install if it's missing:**
1. Check: does `adom-bridge --target <laptop> nbrowser_readiness '{}'` succeed? If not, ABE isn't
   installed.
2. Tell the user (honestly): "The smoothest Autodesk sign-in drives your own Chrome/Edge that's
   already logged in, no password typing, no OTP races. That needs the Adom Browser Extension
   (ABE). Let me walk you through installing it, one time."
3. Point them to the install: https://wiki.adom.inc/adom/adom-browser-extension and follow the
   `installing-the-extension` steps (load the MV3 extension, approve the native-host). Do as much
   as you can drive for them; only ask for the clicks Chrome forces a human to make.
4. Once ABE is live: `fusion_aps_signin` returns `needsExtensionOpen` with an `authUrl`; open it
   via `nbrowser_open_window` in their signed-in profile; the bridge's localhost listener captures
   the token; poll `fusion_aps_status` until `signedIn:true`. Then `fusion_aps_set_browser {how:
   "extension"}` so every future sign-in reuses that exact profile.

Fallback only if ABE truly can't be installed: `fusion_aps_signin {allowDefaultBrowser:true}`
(OS default browser) and drive the OTP per this skill.

## Say the honest WHY (apologize, then reassure)

Autodesk gives no fast Fusion file-search API; the only in-app option takes 30+ minutes and can
crash Fusion, so it's now disabled. APS (server-indexed) is the one thing that works. The cost is
a one-time OAuth sign-in. Tell the user that plainly, apologize for the friction, and reassure
them: once it's set up, cloud search is instant and it stays signed in (auto-refresh). It is
worth it.
