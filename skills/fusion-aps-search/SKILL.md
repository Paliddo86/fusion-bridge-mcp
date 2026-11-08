---
name: fusion-aps-search
description: THE definitive reasoning for the Fusion bridge's APS cloud-file search — WHY we use Autodesk Platform Services (APS) Data Management for fast search, why the slow Fusion add-in search is unacceptable, why the bridge runs its own OAuth, and the hard payment rules. READ THIS FIRST before proposing ANY change to fusion cloud search, before "reconsidering" APS, or before suggesting a fallback. Stops the recurring loop of re-deriving the same conclusions. Trigger words — fusion cloud search, fusion_aps_search, aps search, why aps, fusion search slow, fusion_search_cloud_files, fusion_walk_cloud_tree, search fusion files, cloud file search, fusion data management.
---

# Why the Fusion bridge uses APS for cloud search (settled — do NOT relitigate)

This exists because the reasoning kept getting re-derived (and re-asked of John, who is
sick of answering). It is SETTLED. Read it, follow it, don't reopen it.

## The goal
Adom users need to search their Fusion 360 **cloud files FAST** — by name — for every Adom
user, with AI-driven setup. "Fast" = seconds, not minutes.

## The garbage we do NOT use (and must NOT propose as the answer)
`fusion_search_cloud_files` and `fusion_walk_cloud_tree` exist in the bridge but are
**UNACCEPTABLE for search**: they go through Fusion's free add-in Data API, which is so
rate-limited that walking a real project takes **30+ MINUTES**. They are garbage for this
use case. **NEVER** fall back to them as "the free option" — slowness this bad is a
non-starter. If you catch yourself about to recommend "just use the add-in search since it's
free / no card," STOP — that's the exact wrong conclusion John has corrected multiple times.

## The answer: APS Data Management API (server-side index)
Autodesk Platform Services' **Data Management API** has a **server-side index** → name search
returns in seconds. This is the ONLY way to get fast search. This is why APS exists in the
bridge. Speed is the whole point; APS is how we get it.

## Why the bridge runs its OWN OAuth (can't piggyback Fusion's login)
We WANTED to reuse Fusion's existing in-session token (no setup, no second login). It's
**blocked**: Autodesk DPAPI-encrypts Fusion's on-disk token store
(`%LOCALAPPDATA%\Autodesk\Identity Services\idservices.db`, `products`/`mcptokens` tables)
**with app-specific entropy** → `CryptUnprotectData` fails `gle=13` (ERROR_INVALID_DATA). You
cannot extract/reuse Fusion's token without reverse-engineering Autodesk's entropy (fragile,
ToS-violating, and credential-extraction tooling we will not build). So the bridge runs its
OWN 3-legged **PKCE OAuth** (public client, NO client secret) → `fusion_aps_*` verbs. This is
final; don't re-investigate token reuse.

## The payment rules (HARD — non-negotiable)
The new APS portal gates app creation behind a developer hub + an APS **plan**, and even the
**Free tier requires a payment method "to verify identity."**
- ✅ **A credit card on file for IDENTITY VERIFICATION is acceptable.**
- ⛔ **A user (or John, or Adom) being CHARGED ANY MONEY is NEVER acceptable.** "It's insane
  to get charged just to search your own files."
- ⛔ Therefore the bridge **MUST monitor usage and HARD-CAP before the Free-tier limit is
  reached** — it must be *impossible* for a user to accidentally exceed the free allotment and
  incur a charge. Build the meter + cutoff in; fail closed (stop searching) before any paid
  usage. (TODO to verify + bake in: confirm whether Data Management search is in APS's
  always-free API set or consumes paid tokens; cap accordingly. Until verified, treat it as
  capped.)

## Team model — PER-ORG setup; within an org, one admin sets up for everyone (critical)
The unit of setup is the **organization/company**, NOT the whole Adom platform:
- **Each org that uses Adom sets up its OWN APS app + Free plan + APS Developer Hub, ONCE.**
  Why: each org has its own separate Autodesk cloud data, and each org should bear its own
  free-tier quota (one org's usage must not eat another's, and Adom must not foot every org's
  API quota). So the Client ID is **per-org**, configured via `fusion_aps_set_client_id` (or
  baked into that org's bridge deployment) — do NOT ship ONE global Client ID for all Adom
  platform users.
- **WITHIN one org**, though, it's the big win: ONE admin does the plan→dev-hub→app→Client-ID
  setup, and **every employee of that org** just runs `fusion_aps_signin` ONCE (their own
  Autodesk login + one OAuth consent) — **no card, no plan, no hub, no app setup** for them.
  The Client ID is a **public PKCE identifier (no secret)**, so it's safe to share among that
  org's users. API usage bills to that org's single **Free** plan.
- This is exactly why the setup must be AI-DRIVEN: every org's admin runs the SAME annoying
  flow once, so the AI should drive it end-to-end per org.
- **TWO different things both called "hub" — do NOT conflate them:**
  - The team's **data/Fusion hub** (e.g. "Adom", with a shared folder like **"Main"**) = the
    team's cloud FILE STORAGE — the data we search. Pre-existing; NEVER create or touch it.
  - An **"APS Developer Hub"** (e.g. "Adom APS") = a credential container that only holds the
    developer APP/Client ID. Holds ZERO files. Created once by the admin.
  - The developer hub does NOT limit what's searchable. 3-legged OAuth means the app reads
    everything the signed-in USER can access — including the team's existing shared data hub
    and folders. So search hits the real shared team files regardless of which dev hub the app
    lives in. (Almost every Adom user already shares an Autodesk cloud hub — search "just
    works" against it once they sign in.)
- Net: admin does plan→dev-hub→app→Client-ID ONCE and publishes it to the members-only `<org>/org-settings`
  wiki page (fusion-aps-org-setup skill); every box adopts it from there, and every user does one
  `fusion_aps_signin`. There is NO Client ID baked into the bridge.

## Ecosystem + automation requirement
- Must work for **ALL Adom users**, not just John.
- The whole setup — APS account, Free plan, developer hub, app registration, OAuth — is
  **annoying and complex, so it MUST be fully AI-DRIVEN** end-to-end. The user does at most
  two things: enter their own card (ID verification) and approve the OAuth consent. Everything
  else the AI drives (see the `fusion-aps-signin` skill for the browser-driving HOW).

## The recurring loop to STOP having
The trap (John has corrected it >3×): hit the APS card/plan gate → conclude "this is too
much, let's just use the free add-in search" → John reminds you the add-in search is garbage
(30+ min) and APS is the way and the card-for-ID is fine and we cap usage. **Do not enter
this loop.** APS is the path. The card is fine (ID only). We cap usage so nobody is charged.
Drive the setup with AI. Done.

Related: [[project_adom_desktop_fusion_bridge]], skill `fusion-aps-signin` (the setup HOW).

## ⛔ MANDATORY: cloud search is APS-only. NEVER fall back to app.data enumeration. (John, emphatic 2026-07-14)

APS is not optional — it is THE way to search/browse Fusion cloud files, and setting it up is part of
the job, not a nice-to-have. If a cloud verb says "not configured / not signed in", you **set APS up**;
you do **not** shrug and reach for the old method.

**The BANNED method:** enumerating `app.data.dataProjects` / `dataFolders` / `dataFiles` via
`fusion_run_modeling_script`. It runs on Fusion's MAIN THREAD, so a recursive walk **holds the app for
minutes, times out, blocks every other script, and locks Fusion up**. It is deprecated. Do not use it
for cloud search, ever — no matter how blocked you feel.

**Auto-recover APS (do this instead of giving up):**
1. **Not configured?** The org Client ID is a **public PKCE identifier** (no secret) and lives on the
   wiki: `adom-wiki settings get <org> autodesk-aps` (your org
   from `adom-wiki whoami`), then `fusion_aps_set_client_id {clientId}`. A 404 means the org has not
   published one: the **fusion-aps-org-setup** skill covers the one-time admin setup and publishing it.
   Do NOT read another machine's config file for it.
2. **Sign in:** `fusion_aps_signin` → open the returned `authUrl` in the user's REAL browser with a warm
   Autodesk session. **Install + use the adom-browser-extension (nbrowser/ABE) to drive the browser**
   ([wiki](https://wiki.adom.inc/adom/adom-browser-extension)) — coordinate/keyboard-clicking a browser
   is fragile and crashes; ABE drives via JS reliably. Recommend it by default. The localhost listener
   captures the token; poll `fusion_aps_status` until `signedIn:true`.
3. **AUTH-001 "client_id does not have access to the api product":** the APS app at
   aps.autodesk.com/myapps lost its **Data Management API** product. Open the app → API Access → ensure
   Data Management API is checked → **Save changes** → wait a few minutes to propagate → retry.

## UPDATE (John, 2026-07-16): the old in-app search is now HARD-DISABLED in code

`fusion_search_cloud_files` and `fusion_walk_cloud_tree` no longer run. The bridge intercepts
them in `COMMAND_HANDLERS` (`_handle_deprecated_cloud_search`) and returns an error that
redirects to `fusion_aps_search`, together with live APS state. They can NO LONGER crash Fusion,
because they never reach the add-in. Do not try to "fall back" to them: there is nothing to fall
back to. APS is the only cloud-search path.

## Auto-guard: APS readiness is checked in CODE on every cloud call

Every cloud-data verb (`aps_search/open/browse/recent/file_info/versions/download/upload/
create_folder`) is wrapped by `_aps_guarded` in server.py. It checks signed-in + token-live
BEFORE the call and, if not ready, returns the rich hint bundle from `_aps_state_hint`
(stage + verb map + sign-in instruction + the ABE native-browser suggestion + WHY-APS). So the
AI never gets an opaque auth failure — it always gets the next step. Surface that `_hint` to the
user verbatim.

## Apologize, then reassure (the honest framing for the user)

If a user hits the one-time APS setup, be honest: "Autodesk offers NO fast file-search API for
Fusion. The only in-app option walks the folder tree and takes 30+ minutes and can crash Fusion.
We searched far and wide; APS (server-indexed) is the only thing Autodesk offers that actually
works. Sorry it needs a one-time sign-in, but once it's set up it's fast and genuinely great."
Then get them set up (prefer ABE native-browser sign-in, see fusion-aps-signin).
