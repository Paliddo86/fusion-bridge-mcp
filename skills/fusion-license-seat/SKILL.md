---
name: fusion-license-seat
description: Moving the Autodesk SEAT (the licence) between machines, which testing across several boxes needs constantly. One subscription runs ONE active Fusion at a time: fusion_account says who is signed in here and whether this box holds the seat, fusion_claim_seat takes it (the other machine suspends resumably), fusion_release_seat gives it up. Also what to do when a box is signed in as the WRONG person, and why the bridge never types a credential. Trigger words - autodesk seat, fusion licence, license, move the licence, switch boxes, Active Sessions Exceeded, Suspend Remote Session, seat modal, take the seat, release the seat, fusion_account, fusion_claim_seat, fusion_release_seat, signed in as the wrong user, sign fusion in, test on another box.
---

# The Autodesk seat: moving a licence between machines

One Autodesk subscription runs **one active Fusion session at a time**. Every machine can stay
signed in; only one holds the seat. Testing across ConfRoomROG, arav-rog and AdomLapper means
moving it, so it is three verbs, not a recipe.

## The three verbs

```bash
adom-bridge --target <box> fusion_account '{}'       # who is signed in here, does this box hold the seat
adom-bridge --target <box> fusion_claim_seat '{}'    # take the seat: the other machine suspends RESUMABLY
adom-bridge --target <box> fusion_release_seat '{}'  # give it up: closes Fusion, stays signed in
```

- **`fusion_account`** is read-only. It never starts Fusion and never signs anyone in or out. It
  returns the account (email, name, hub), Autodesk's own licence blob (`authorized`, `trial`,
  `expiresAt`, from `Application.LicenseInformation`) and `holdsSeat`.
- **`fusion_claim_seat`** starts Fusion if needed and answers Autodesk's "Active Sessions Exceeded"
  modal in the background with Continue. The other machine's Fusion **suspends resumably**: its
  documents are kept, and it resumes when it takes the seat back.
- **`fusion_release_seat`** closes Fusion cleanly and leaves the account signed in, so the next
  box claims the seat with no modal at all. An unsaved document refuses the close and says so;
  nothing is ever discarded.

## Moving the seat, the normal case

Both boxes are signed in as the same person, so just claim it where you want it:

```bash
adom-bridge --target arav-rog fusion_claim_seat '{}'      # arav-rog now holds it
adom-bridge --target ConfRoomROG fusion_account '{}'      # confirms ConfRoomROG lost it
```

Cleaner when you know you are done with a box (no modal for the next one):
`fusion_release_seat` on the old box, then `fusion_claim_seat` on the new one.

## When a box is signed in as the WRONG person

The seat verbs move a licence; they do not change WHO is signed in. Fusion's window exposes no
automatable controls for the account menu, and **the bridge never types a credential**. So:

1. `fusion_account` tells you who it is.
2. To change it, the person signs out in Fusion (the avatar, top right, then Sign out) or signs in
   on the sign-in screen.
3. `fusion_signin {auto:true, method:"google", account:"<email>"}` signs Fusion in. The METHOD is
   how that Autodesk account signs in (google, apple, microsoft, facebook or password). The bridge
   opens the sign-in in a window it can drive, clicks the provider, picks the account in the
   provider's chooser, clicks Open Product, and remembers the method and account for next time.
   - No method given and none remembered: the reply is `stage: method_required` with `methods[]`.
     **Ask the user once** which method they use; do not guess.
   - `account_choice_required` / `account_not_listed`: the provider needs to know which account,
     or that account is not signed into this browser. Ask; no toast is sent for these.
   - `password_required`: the only step that toasts the person. The bridge never types a password.
   - After Open Product, Chrome may ask "Open Autodesk Identity Manager?": tick Always allow once
     per browser profile and click Open. Then `fusion_account` shows the account.
4. A box whose Fusion sits on the sign-in screen answers `needsSignin` from `fusion_account` and
   from `fusion_claim_seat`, never a made-up failure.

## Where Fusion opens its sign-in, and why it matters

Fusion opens its sign-in in the machine's DEFAULT browser, and the bridge has to find that request
(it lives about two minutes). Where the default browser carries the Adom browser extension (Chrome
on ConfRoomROG and arav-rog), the bridge sees the tab at once and drives it. Where the default is a
browser WITHOUT the extension (Edge on AdomLapper, 2026-09-23), Fusion's request lands in a tab the
bridge cannot see, and Edge writes its history late, so `fusion_signin` keeps answering
`click_signin`. Fix it once per machine: make the extension's browser the default, or install the
extension in that browser. Until then, the person finishes that one sign-in by hand.

## What the seat modal looks like, and why you rarely see it

"Active Sessions Exceeded" and "Suspend Remote Session" are the transfer itself. The bridge
answers them in the background: `fusion_claim_seat` does it, `fusion_readiness` does it during a
launch, and the passive sweep does it mid-session (`fusion_set_seat_takeover` turns that sweep off
for a user who would rather decide each time). `fusion_resolve_seat_dialog` is the low-level click
if you are driving a launch by hand.

## Rules

- **Never kill Fusion to move a seat.** `fusion_release_seat` closes it cleanly and protects
  unsaved work; a kill can lose a document and leaves recovery files behind.
- **Claiming is not rude, it is the design.** The other machine suspends resumably. Still, when
  another person is working on that box, tell them, or use `fusion_notify_owner`.
- **A trial or expired licence is not a seat problem.** `fusion_account.license` shows `trial`
  and `expiresAt`; when it has run out, no amount of claiming helps.
