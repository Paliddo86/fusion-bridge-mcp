---
name: fusion-web-signin
description: Sign an Adom user into Autodesk's WEB surfaces (library.io, the Fusion hub portal <hub>.autodesk360.com, Autodesk Account) from a container, using pup plus ONE stored Google credential - because Fusion's desktop app being signed in does NOT sign in the web. Covers the one-time credential_set the user runs, the Continue-with-Google SSO path, what 2FA does, and why the AI never sees the password. Trigger words - library.io login, autodesk sign in, autodesk360, fusion hub portal, hub invite, managed folder sharing, sign into autodesk in pup, continue with google, credential_set, fusion_web_signin.
---

# Signing an Adom user into Autodesk's web surfaces

**The gap this closes.** Fusion the desktop app is signed in as the user, and the Fusion bridge
drives it happily. But the things you need for SHARING - library.io (managed libraries and their
folders), the hub portal `<hub>.autodesk360.com` (Members and Roles, invites), Autodesk Account -
are WEB properties with their own session. pup starts with none, and none of Fusion's tokens carry
over. Measured 2026-09-01: `library.io` and `fusion.autodesk.com` both landed logged out in pup on
two machines, and `adom.autodesk360.com` redirects straight to `signin.autodesk.com`.

## The design: one Google credential per user, every Autodesk surface

Every Autodesk sign-in page offers **Continue with Google**, and Adom already owns the user's
Google identity (that is how `adom-google` works). So an Adom user stores ONE credential and every
Autodesk web surface becomes drivable - no per-site passwords, no password ever reaching the AI.

**One-time, run BY THE USER** (the secret goes to the OS keychain; the index keeps host+username
only, and `credential_set` never echoes it back):

```bash
adom-bridge credential_set '{"host":"accounts.google.com",
                             "username":"<you>@<domain>",
                             "password":"<your Google password>"}'
```

Then, from any container, for any Autodesk web surface:

```bash
adom-bridge fusion_web_signin '{"url":"https://library.io/"}'
adom-bridge fusion_web_signin '{"url":"https://<hub>.autodesk360.com/","sessionId":"hub"}'
```

`fusion_web_signin` opens a BACKGROUND pup window, detects the Autodesk sign-in page, clicks
Continue with Google, and lets **pup** fill the credential from the vault. The bridge and the AI
never hold the secret.

## What it returns, and the two honest stops

| result | meaning |
|---|---|
| `alreadySignedIn: true` | that pup profile already had the session; nothing to do |
| `signedIn: true` | signed in; the session persists in the pup profile, so later runs are silent |
| `errorCode: human_credential_required` | no stored credential for this user - the response carries the exact `credential_set` line for them to run |
| `errorCode: second_factor_required` | Google wants the second factor; only the user can approve it (phone prompt). After one approval the pup profile keeps the session |
| `errorCode: google_sso_unavailable` | the page did not hand off to Google - retry with `provider:"password"` |

**Never** invent, guess, or ask for the password in chat, and never click through a 2FA prompt on
the user's behalf: those are the two stops this verb exists to report cleanly rather than fake.

## Why an Adom user wants this (the sharing story)

Fusion's own Library Manager banner says it plainly: *"Import private libraries into this Fusion
Hub to avoid permission errors for team members. We recommend using Hub libraries going forward."*
Distribution therefore happens through **hub membership**, and hub membership is managed on the
web:

- hub portal > **Members and Roles > Invite** (email address; the invitee must accept)
- the inviting admin needs an assigned license; a basic-access user cannot invite
- a **team** hub can set Project Administrator / Editor / Viewer per member; a **personal** hub can
  invite to a project but cannot manage permissions at all

None of that is reachable from the Fusion API or the desktop ribbon - it is a web surface, which is
exactly why this verb exists.

## Related

- `dev-skills/fusion-cloud-libraries` - what a `.flbr` is, how a library becomes a library.io
  managed library, and the measured cross-account result (a managed library does NOT cross
  Autodesk accounts on its own).
