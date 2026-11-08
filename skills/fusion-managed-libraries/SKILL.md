---
name: fusion-managed-libraries
description: Fusion's library.io MANAGED libraries - create, link, unlink, push, pull, and the ONLY way to share one across Autodesk accounts (managed-folder membership, invited by email from library.io's website). Every step is a verb; none of it needs the AI to click a ribbon. Also documents why managed libraries do NOT scale as a distribution channel and what to do instead. Use when a user asks to publish a library to library.io, share a library with another account or another user, link a library to a managed library, push a new version, or asks why a shared library is not showing up in someone else's Fusion. Trigger words - managed library, library.io, share library, publish library, managed folder, cross account, link library, push library, pull library, Library Manager, private libraries.
---

# Managed libraries on library.io

The verbs (all shipped, all proven live 2026-09-01 on ConfRoomROG):

| Verb | Does |
|---|---|
| `fusion_managed_library_create` | make a NEW managed library from the hub library in front |
| `fusion_managed_library_link {name?}` | no name: opens the picker, returns its rows as records, cancels. With name: selects + confirms |
| `fusion_managed_library_unlink` | detach from the managed library |
| `fusion_managed_library_push` | push a NEW VERSION up |
| `fusion_managed_library_pull {onUpdatePrompt?}` | pull the newest version down |
| `fusion_library_manager {tab}` | open Library Manager on hub / public / private |
| `fusion_alerts {sinceSec}` | Fusion's own toasts, which are invisible to Win32 AND UIA |

**A LIBRARY must be in front** for any of these. `fusion_open_by_urn {urn}` or
`fusion_open_lbr {filePath}` first - and verify, because `fusion_activate_document` can report
success while the visible tab never changed. The honest oracle is the window TITLE plus the
ribbon in a screenshot (a library shows COMPONENT / DOCUMENT / MANAGE / UTILITIES; a design
shows SOLID / SURFACE / MESH).

## ⛔ Every action ends in a dialog, and every one BLOCKS the add-in

This is the single most important operational fact. Clicking a LIBRARY.IO menu item never
completes an action; it opens a dialog, and an unanswered dialog leaves the add-in reporting an
empty `activeDocument` ("add-in is busy with a long-running command") until something answers it.

| Action | Dialog | Answer |
|---|---|---|
| Push (linked) | **Create New Version** [Create] [Cancel] | Create - this is what actually pushes |
| Push (unlinked) | **Create Managed Library** | that is `..._create`'s job |
| Link (confirmed) | **Update** [Merge] [**Replace**] [Cancel] | Cancel by default: Replace DESTROYS the local library |
| Pull (differs) | **Update** (same 3 buttons) | Merge |
| Pull (current) | **Up to Date** [OK] | OK, reported as `upToDate` |

So the success test for each verb is "the dialog I expected appeared AND I answered it AND it
closed" - never "no alerts". These dialogs are CEF-drawn and expose NOTHING to UIA, so they are
answered by clicking a fraction of the dialog's own rect from `desktop_list_windows` (those rects
are already in the input verbs' physical-px space - do NOT route through a screenshot coordMap,
the shotId is not readable from inside the bridge).

Verify a dialog CLOSED by waiting for its ABSENCE. A helper that returns the moment it SEES the
window reports the window list's one-beat lag as "still open".

## Sharing across Autodesk accounts: managed-folder membership

**A managed library does NOT cross accounts on its own.** Proven: a library created under one
account was absent from the other account's Library Manager. Both accounts even show a folder
with the SAME name (each account's own personal folder), so name matching means nothing.

The real mechanism is one dialog on library.io's WEBSITE. There is no API: the authenticated
library.io endpoints reject the APS token and there is no published write API.

1. Sign in to <https://library.io/> as the OWNING account. It lands on
   `library.io/hubs/<id>-<Folder>/libraries`.
2. Click **Manage Members** (Owner only) -> the *Managed Folder Members* dialog: a member table
   plus "Invite new members". The fine print matters: "Invited collaborators will be able to view
   **and edit** all contents in this Managed Folder". There is no read-only role.
3. Send the invite. The invitee shows as **Pending**.
4. The invitee gets mail from `info@library.io` with **Accept** / **Reject**. Accept -> Autodesk
   sign-in -> "Congratulations! You've been successfully added to <Folder>."
5. They now see the folder as **Member** and can **Import into Fusion Hub** each library, which
   brings the **3D models across**, not just symbols and footprints.

### ⚠️ The step nobody would guess: PUSH after they join

Membership alone does not surface a library in the member's Fusion. Measured: after the invite
was accepted, only the library that had been pushed since the invite appeared (13 -> 14 entries);
two others stayed invisible even after reopening Library Manager to force a refetch. Pushing a
new version of a second library made it appear too (14 -> 15).

**So distribution is: invite once, then `fusion_managed_library_push` each library once.** A
library not pushed since a member joined looks like sharing is broken when it is not.

The member's folder picker (Source > Library.io > My Folders) then lists TWO folders with the
same name, theirs and yours. Filter by folder, not by name.

## ⛔ Do not build mass distribution on this

For a team of ten it is fine. As a channel for many users it fails on its own terms:

- membership is granted only through a website, one email at a time, with no API;
- each user must accept an invite by hand;
- every library must be re-pushed after each new member joins;
- the invite grants **edit** rights on everything in the folder, with no read-only role;
- import is slow (minutes for a handful of packages).

**The scalable alternative** is the ordinary bind chain: mint each package's `.f3d` ONCE, cache
the bytes centrally, then per user upload that same f3d into THEIR OWN hub and inject THEIR urn.
A `urn:adsk.wipprod:fs.file:vf.<id>` is scoped to a hub and access-controlled, so pasting one
account's urn into another account's library is not a shortcut - Fusion's own Library Manager
banner warns "Import private libraries into this Fusion Hub to avoid **permission errors for team
members**", and Autodesk's own sharing COPIES content into the consumer's hub rather than
referencing the owner's. See `fusion-multipart-libraries` for the bind chain.

## Signing in to the web half: pup already has the session

Do NOT reach for `credential_set` first, and do NOT drive the user's real browser. pup's default
shared **adom-you** profile carries live Google sessions, so library.io sign-in is passwordless:
`pup_open_window {url:"https://library.io/"}`, click **Sign In**, click **Continue with Google**,
click the account row. Done.

Two pup quirks that cost a cycle: `pup_click` needs `selector` or `x`+`y` (there is no `text`), so
tag the element first with `pup_eval` (`e.id="..."`) and click that; and `pup_fill` takes
**`text`**, not `value`.

pup cannot act as an account it has no session for. To read mail for a DIFFERENT identity (say to
accept an invite), use **`adom-google --account <name>`** rather than driving Gmail in a browser -
see the `adom-google` skill.
