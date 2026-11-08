---
name: fusion-aps-org-setup
description: How an ORG (or a solo user) gets Fusion cloud search (APS) working on every box with one admin setup - where the org's APS app ID lives (the members-only <org>/org-settings wiki page), how a new box adopts it in two calls, how the first admin registers the Autodesk app and publishes the ID for everyone else, and what each person still does themselves (one Autodesk sign-in). Read this whenever fusion_aps_status says configured:false, before telling anyone to register an APS app, and before copying an ID between machines. Trigger words - aps not configured, adom-wiki settings, settings get autodesk-aps, aps_not_configured, fusion_aps_set_client_id, APS client id, share aps id, org settings, org-settings, autodesk-aps.json, set up aps for my company, aps first time setup, new box aps, cloud search not set up, consent window not visible.
---

# APS for a whole org: one admin setup, then everyone just signs in

Parent skill: **fusion-aps-search** (what APS is, quotas, search verbs). Sign-in driving: **fusion-aps-signin**.

## The model in four sentences

Fusion cloud search uses Autodesk Platform Services (APS) with PKCE, so there is NO secret and no server: only a public **app ID** (Client ID) plus each person's own Autodesk sign-in. The app ID is **per org**: one admin registers one Autodesk app, once. It lives on the wiki at **`<org>/org-settings`**, a private page only org members can read, in `integrations/autodesk-aps.json`. A new box adopts it in two calls, and the person signs in as themselves.

## A. New box, org already set up (the common case, about a minute)

```bash
adom-wiki whoami                                   # "orgs": your org slug(s)
adom-wiki settings get <org> autodesk-aps          # -> { "clientId": "...", "secret": false, ... }
adom-bridge --target <box> fusion_aps_set_client_id '{"clientId":"<clientId>"}'
adom-bridge --target <box> fusion_aps_signin '{}'
adom-bridge --target <box> fusion_aps_status '{}'  # poll until signedIn:true
```

- **Which org.** Use the one `whoami` lists. If you belong to several, ask the user which org this box works for: never guess.
- **SETTINGS_NOT_FOUND** means the org has not published one, or you are not a member. Go to section B.
- The wiki read uses YOUR wiki token inside the container. The desktop box never needs a wiki token.
- `adom-wiki settings show <org>` lists every integration the org has published.
- Sign-in opens a browser window on the box. With a warm Autodesk session it completes by itself. Otherwise the person signs in there: never type their password or 2FA code for them.
- Since 1.10.33 the bridge finds a sign-in window that needs a human and puts it on the primary monitor, in front. If one is still missing: `desktop_list_windows`, find "Sign in - Autodesk" (a window at -32000 is parked off-screen), then `desktop_set_window_bounds {hwnd,x,y,w,h}` and `desktop_bring_to_front {hwnd, reason}`.
- Whose Autodesk login? The same account Fusion uses on that box is the natural choice: search then sees exactly what that person sees in Fusion.

**Never** read another machine's `~/.adom/fusion-aps/config.json` to find the ID. It looks like credential hunting, permission systems block it, and the wiki settings exist so you do not have to.

## B. First time for this org (one admin, once)

1. At [aps.autodesk.com](https://aps.autodesk.com): sign in, accept the Free plan, create a developer hub if asked, create an app of type **Desktop, Mobile, Single-Page App** (PKCE).
2. Enable the **Data Management API** on the app. Without it sign-in fails with AUTH-001 before any login.
3. Add every callback `fusion_aps_status` lists (six localhost URLs on ports 8917, 8918, 8920).
4. Copy the Client ID. `fusion_aps_set_client_id {clientId}`, then `fusion_aps_signin`.
5. **Share it with the org** so nobody repeats steps 1 to 4:

```bash
adom-wiki settings set <org> autodesk-aps clientId=<id> kind=public-pkce-client-id
```

`settings set` refuses anything secret-shaped and writes nothing in that case. If `<org>/org-settings` does not exist yet, it prints the one `adom-wiki page create org-settings ...` command to run first; run it, then repeat the `set`. A solo user does the same with their own username as the owner.

## The file

```json
{
  "schema": "adom.org-integration/1",
  "integration": "autodesk-aps",
  "owner": "<org slug>",
  "clientId": "<public app id>",
  "kind": "public-pkce-client-id",
  "secret": false
}
```

`secret` is always `false` in this repo. Real secrets go in page secrets (`adom-wiki secret`), which are write-only.

## What each person still does themselves

One Autodesk sign-in per box, in the browser window the bridge opens. Tokens are per person and per box and are never shared or published. Refresh is automatic; a box that was offline long enough for the refresh token to lapse just runs `fusion_aps_signin` again.

## Status of the standard

`<org>/org-settings` and the `adom-wiki settings show|get|set` commands (adom-wiki 1.7.55+) came out of [adom/adom-wiki-cli issue 59](https://wiki.adom.inc/adom/adom-wiki-cli/issues/59). The page is created on first write, never automatically, so SETTINGS_NOT_FOUND always means "this org has not set one up". Open on the wiki side: a dedicated page type for settings (today the page is typed `skill`).
