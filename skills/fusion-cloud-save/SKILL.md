---
name: fusion-cloud-save
description: How saving works in Fusion 360 when 3D packages and libraries live in the Autodesk cloud (Hub) - why FINISH/Save uploads packages ASYNCHRONOUSLY, why you must give a library a cloud home BEFORE attaching 3D so a crash cannot wipe your work, and the hard rule to NEVER close or saveAs while a Hub upload is in flight (it loses packages or throws InternalValidationError / "save was cancelled"). Includes the poll-until-uploaded saveAs recipe. Read before saving any library or document that has had 3D packages attached, or whenever you see an upload/save dialog. Trigger words - fusion save, saveAs, save library, fusion hub upload, packages being uploaded, InternalValidationError, save was cancelled, library not saved, lost attaches, lost bindings, save to cloud, wip_urn upload, poll save, save while uploading, cloud persistence fusion.
---

# Saving in Fusion 360 when everything lives in the cloud

In Fusion's Electronics library flow, a 3D package is not embedded in the `.lbr` - it is a file in
your Autodesk **Hub (cloud)**, referenced by a `wip_urn`. That one fact drives every rule here:
**saving is asynchronous and networked, so it can be in flight, refused, or lost.** Pairs with
[fusion-driving](../fusion-driving/SKILL.md) (read the dialog before acting) and
[fusion-multipart-libraries](../fusion-multipart-libraries/SKILL.md) (the build-up that depends on
this).

## ⛔ The failure this skill prevents (2026-06-28)

A 10-part library had all 10 3D packages attached into the open library doc, **but the library was
never saved.** A close loop force-closed docs mid-upload, the close-confirm dialog was blind-clicked,
and the library closed. **All 10 attaches were gone** - because the bindings lived only in an
unsaved, in-memory document while the packages were still uploading. Two root causes: (1) no cloud
home for the library before the work, (2) acting while uploads were in flight.

## Rule 1 - FINISH/Save uploads the 3D package to the Hub ASYNCHRONOUSLY

After the Package3D generator's FINISH (`Package3DStop`), Fusion saves the 3D package and **starts
uploading it to your Hub in the background.** Control returns to you immediately, but the upload is
NOT done. The binding in the library points at that package's `wip_urn`. Until the upload completes,
the package is "in flight" and the library cannot be cleanly saved or closed.

## Rule 2 - give the library a CLOUD HOME before you attach (or save incrementally)

A library that exists only as a local `.lbr` opened in Fusion, with attaches made into it, lives
**only in memory** until you `saveAs` it to the cloud. A crash, a wrong dialog click, or a force-close
wipes everything. Two safe patterns:

- **Save-first:** open the `.lbr`, `saveAs` it to the cloud once (empty of 3D) so it has a Hub
  lineage, THEN attach 3D and save again. Now a mishap costs the current attach, not all of them.
- **Save-incrementally (build-up):** save the library after each part's attach (the build-up-one-by-one
  flow in [fusion-multipart-libraries](../fusion-multipart-libraries/SKILL.md)). Slower, but every
  part is durable the moment it lands.

Either way: **do not attach 10 parts into an unsaved in-memory library and save only at the end.**
That is the exact shape that lost the work.

## Rule 3 - NEVER close or saveAs while a Hub upload is in flight

This is the load-bearing rule. While packages upload:

- **Closing the doc** pops *"Packages are being uploaded... if you close you will lose these changes.
  Are you sure you want to close?"* Clicking **Yes loses the packages.** Click **No** and wait.
- **saveAs** is refused with *"this library cannot be saved while packages are being uploaded to your
  Fusion Hub. Save was cancelled,"* and the API throws **`InternalValidationError`.**

Never `close(False)` (force-discard) a document that may be uploading. Wait for the upload to drain
first.

## Rule 4 - the poll-until-uploaded saveAs recipe (verified working)

You cannot reliably detect "upload finished" from a single flag, so **poll the saveAs itself** - it
succeeds once the upload drains. This worked end to end on the R_100R single-part library:

```python
# find the library document, then poll saveAs past the upload window
saved = False
for attempt in range(10):
    time.sleep(8)                              # let the Hub upload make progress
    fusion_dismiss_blocking_dialogs            # only AFTER confirming it's the upload dialog (see fusion-driving)
    res = run_modeling_script("""
        lib = None
        for i in range(app.documents.count):
            d = app.documents.item(i)
            if any(d.products.item(j).productType == 'LibraryProductType'
                   for j in range(d.products.count)):
                lib = d
        out = {}
        if lib:
            lib.activate()
            try:
                out['saveAs'] = lib.saveAs('MyLibName', app.data.activeProject.rootFolder, 'comment', '')
            except Exception as e:
                out['err'] = str(e)[:60]        # InternalValidationError while still uploading
        result = out
    """)
    if res.get('saveAs'):
        saved = True; break
```

Notes:
- `saveAs(name, folder, comment, "")` on the `Document`; pass `app.data.activeProject.rootFolder` as
  the folder. Activate the doc first.
- An empty result (no `saveAs`, no `err`) usually means **the library doc was not found** (it got
  closed). That is the alarm bell that you lost the doc, not a transient - stop and investigate, do
  not keep polling a doc that no longer exists.
- 8s between tries with ~10 tries covers a typical few-MB package upload. Bigger models need longer.

## Rule 5 - confirm the save actually landed

After saveAs returns truthy, verify: reopen the saved library (or export it) and confirm the
`package3d` bindings are present (`package3d` / `package3dinstance` with the `wip_urn`). A truthy
saveAs plus a real binding on reopen is the only proof the work persisted. See
[fusion-libraries LIBRARY_FINDINGS](../fusion-libraries/LIBRARY_FINDINGS.md) for the on-disk markup.

## The save discipline in one line

Give the library a cloud home early, let every Hub upload drain before you touch the doc, poll the
saveAs past the upload window, and verify the binding on reopen. Saving is a network operation - treat
it like one.
