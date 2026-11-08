"""Close document command — close a Fusion 360 document without the save dialog."""

import adsk.core



def _stop_running_command(app, doc_name: str) -> dict:
    """PRE-EMPT Fusion's "Can't close window while command is running! Running command:
    Edit. Stop now?" modal (John, 2026-08-30: "u should never allow dialogs like this to
    show up. you need to pre-empt any of these with your logic"). Closing a document while
    an editor command (EDIT, ROUTE, MOVE, ...) is active makes Fusion ASK; the answer is
    always "yes, stop it" for a document the bridge is closing, so decide it here, before
    the close, by terminating the active command first. Returns what was stopped."""
    info = {"stopped": None}
    try:
        ui = app.userInterface
        cmd = ""
        try:
            cmd = str(ui.activeCommand or "")
        except Exception:
            cmd = ""
        if cmd and cmd.lower() not in ("selectcommand", "select", "none"):
            try:
                ui.terminateActiveCommand()
                info["stopped"] = cmd
            except Exception as e:
                info["stopError"] = str(e)[:120]
        try:
            # Belt and braces for the electronics editors: an ESC-equivalent that ends
            # a half-entered command without drawing anything.
            app.executeTextCommand("Commands.Abort")
        except Exception:
            pass
        try:
            # Force select mode and let the UI settle before anyone re-reads activeCommand.
            sel = ui.commandDefinitions.itemById("SelectCommand")
            if sel:
                sel.execute()
            adsk.doEvents()
        except Exception:
            pass
        try:
            after = str(ui.activeCommand or "")
        except Exception:
            after = ""
        info["after"] = after
        # STILL RUNNING = a close WOULD draw "Can't close window while command is running!
        # Stop now?" (drawn on ConfRoomROG 2026-08-31 despite terminateActiveCommand). Per
        # the no-modal doctrine, refuse the close instead; the caller closes it later.
        info["wouldPrompt"] = bool(after and after.lower() not in ("selectcommand", "select", "none"))
    except Exception as e:
        info["stopError"] = str(e)[:120]
    return info


def handle_close_document(app: adsk.core.Application, args: dict) -> dict:
    """Close a document in Fusion 360 without triggering the save dialog.

    Args (all optional):
        name: Name of the document to close. If omitted, closes the active document.
        save: If True, save before closing. Default False (discard changes, no dialog).

    This is essential for automation — a normal close triggers a modal save dialog
    that blocks the Fusion main thread and hangs the add-in until the user clicks.
    """
    target_name = args.get("name")
    save_changes = args.get("save", False)

    try:
        if target_name:
            # Find the document by name
            doc = None
            for i in range(app.documents.count):
                d = app.documents.item(i)
                if d.name == target_name:
                    doc = d
                    break
            if doc is None:
                # List available docs for a helpful error
                available = [app.documents.item(i).name for i in range(app.documents.count)]
                return {
                    "success": False,
                    "error": f"Document '{target_name}' not found. Open documents: {available}",
                    "_hint": "Call fusion_document_info to list open documents, or use fusion_close_document with no name to close the active document.",
                }
        else:
            # Close the active document
            doc = app.activeDocument
            if doc is None:
                return {
                    "success": False,
                    "error": "No active document to close.",
                    "_hint": "Call fusion_document_info to check open documents — nothing is currently active.",
                }

        doc_name = doc.name

        # Check if document has unsaved changes (for reporting)
        is_saved = True
        try:
            is_saved = doc.isSaved
        except Exception:
            pass

        # Close the document. Passing False skips the save dialog entirely.
        _stopped = _stop_running_command(app, doc_name)
        if _stopped.get("wouldPrompt"):
            return {"success": False, "errorCode": "command_running",
                    "error": ("Refusing to close %r: Fusion command %r is still running and the close "
                              "would draw 'Can't close window while command is running'. Nothing was "
                              "closed and no dialog was raised." % (doc_name, _stopped.get("after"))),
                    "data": {"document": doc_name, "runningCommand": _stopped.get("after")},
                    "_hint": "Wait for the command to finish (or end it in Fusion) and close again."}
        # A MODIFIED CLOUD DOCUMENT PROMPTS EVEN WITH saveChanges=False (John, 2026-08-30:
        # "Unsaved changes to LAN7800" while the chain re-opened its own library). Saving it
        # first is the only dialog-free close, and it costs one version of a bridge document.
        _saved_first = False
        try:
            if not save_changes and doc.dataFile is not None and bool(doc.isModified):
                if not args.get("saveToClose"):
                    # never save a version the caller did not ask for (BQ25792 v228, 2026-10-28)
                    return {"success": False, "errorCode": "unsaved_cloud_document", "document": doc_name,
                            "error": ("%r is a saved cloud document with unsaved changes; Fusion asks to save it even "
                                      "when told not to, so it was not closed and nothing was saved." % doc_name),
                            "_hint": "Pass save:true (or saveToClose:true) to save a version and close, or keep it open."}
                doc.save("adom bridge: saved before close to avoid Fusion's prompt")
                _saved_first = True
        except Exception:
            pass
        doc.close(save_changes)

        remaining = app.documents.count
        new_active = None
        try:
            if app.activeDocument:
                new_active = app.activeDocument.name
        except Exception:
            pass

        return {
            "success": True,
            "output": f"Closed '{doc_name}' (save={save_changes}, had_unsaved={'yes' if not is_saved else 'no'}). {remaining} document(s) remaining.",
            "data": {
                "closedDocument": doc_name,
                "commandStopped": _stopped.get("stopped"),
                "savedFirstToAvoidPrompt": _saved_first,
                "savedBeforeClose": save_changes,
                "hadUnsavedChanges": not is_saved,
                "remainingDocuments": remaining,
                "activeDocument": new_active,
            },
            "_hint": f"{remaining} document(s) still open."
                     + (f" Active: '{new_active}'. " if new_active else " No active document. ")
                     + "Use fusion_get_app_state to check workspace, or fusion_open_cloud_file to open another document.",
        }

    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to close document: {e}",
            "_hint": "Retry with {\"save\": false} to discard changes, or call fusion_screenshot_fusion to check for a blocking Save dialog and answer it with fusion_dismiss_blocking_dialogs.",
        }


def handle_close_all_documents(app: adsk.core.Application, args: dict) -> dict:
    """Close ALL open documents without save dialogs.

    ⛔ One case genuinely cannot be closed silently and is REFUSED rather than prompted:
    a document with a NAME but no dataFile (never saved) that is MODIFIED. Fusion has no
    location to write it to, so close() raises the Save box even with saveChanges=False.
    Such docs are returned in data.wouldPrompt with errorCode unsaved_named_document.

    Closes every open document/tab in Fusion without triggering any save
    dialogs. Essential before restarting Fusion to avoid the 'Recovered
    Documents' modal on next launch.

    Args (all optional):
        save: If True, save each document before closing. Default False.
        closeUntitled: If True, ALSO close scratch "Untitled" documents that have no
            data file. Default False (the historical behaviour).
        force: With closeUntitled, also close Untitled docs that are MODIFIED. Default
            False, because a modified Untitled may hold real unsaved user work.
    """
    save_changes = args.get("save", False)
    close_untitled = bool(args.get("closeUntitled", False))
    force_untitled = bool(args.get("force", False))

    closed = []
    errors = []
    skipped = []
    would_prompt = []

    # Snapshot the current document list ONCE. Do NOT loop on app.documents.count,
    # because Fusion may auto-create an Untitled blank doc whenever all docs close,
    # which turns `while count > 0` into an infinite loop and times out the bridge.
    try:
        snapshot = []
        for i in range(app.documents.count):
            d = app.documents.item(i)
            snapshot.append(d)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to enumerate documents: {e}",
            "_hint": "Retry with {\"save\": false} to discard changes, or call fusion_screenshot_fusion to check for a blocking Save dialog and answer it with fusion_dismiss_blocking_dialogs.",
        }

    for doc in snapshot:
        try:
            doc_name = doc.name
        except Exception:
            doc_name = "<unknown>"
        try:
            # UNTITLED SCRATCH DOCS (page issue #59, 2026-08-26).
            # This used to skip EVERY Untitled with the reasoning "Fusion recreates them,
            # closing is pointless". That is true of the ONE blank doc Fusion keeps, and
            # false of the sixteen the library/bind flow left behind after three imports -
            # roughly five per import, unbounded across a run, and unclosable by any
            # caller, which is what starved Fusion until a 3D step blew its 60s budget.
            # Now opt-in: closeUntitled closes the UNMODIFIED ones (scratch by definition,
            # so nothing can be lost), and force is required to close a MODIFIED Untitled
            # because that one may hold real unsaved work. Fusion re-creates a single
            # blank doc if the last one closes, which satisfies "keep exactly one".
            has_data_file = False
            try:
                has_data_file = doc.dataFile is not None
            except Exception:
                has_data_file = False
            if not has_data_file and doc_name.lower().startswith("untitled"):
                if not close_untitled:
                    skipped.append(doc_name)
                    continue
                is_modified = False
                try:
                    is_modified = bool(doc.isModified)
                except Exception:
                    is_modified = False
                if is_modified and not force_untitled:
                    skipped.append(doc_name + " (modified - pass force:true to discard)")
                    continue
            # ⛔ A NAMED-BUT-NEVER-SAVED MODIFIED DOC MAKES close() PROMPT.
            # John, 2026-08-26: "why do i ever see this dialog popup if you are pre-emptively
            # saving programmatically?" He was looking at Fusion's Save box (Name: TJA1042T,
            # Location: Admin Project) raised by THIS function, whose docstring promises
            # "without save dialogs". doc.close(False) discards silently for a scratch
            # Untitled, but a doc that has a NAME and no dataFile has never been saved, so
            # Fusion has nowhere to put it and asks - even when saveChanges is False.
            # Per skills/fusion-no-modal-dialogs: never issue a command that will prompt.
            # This case is knowable here, before the call, so decide it here.
            if (not has_data_file and doc_name and
                    not doc_name.lower().startswith("untitled")):
                is_mod = False
                try:
                    is_mod = bool(doc.isModified)
                except Exception:
                    is_mod = False
                if is_mod and not save_changes:
                    would_prompt.append(doc_name)
                    continue
            _st = _stop_running_command(app, doc_name)
            if _st.get("wouldPrompt"):
                would_prompt.append(doc_name + " (command %s running)" % _st.get("after"))
                continue
            try:
                if not save_changes and has_data_file and bool(doc.isModified):
                    doc.save("adom bridge: saved before close to avoid Fusion's prompt")
            except Exception:
                pass
            doc.close(save_changes)
            closed.append(doc_name)
        except Exception as e:
            errors.append(f"{doc_name}: {e}")
            # Keep going — don't abort the whole batch on one failure

    result = {
        "success": len(errors) == 0 and not would_prompt,
        "output": f"Closed {len(closed)} document(s)"
                  + (f", skipped {len(skipped)} Untitled" if skipped else "")
                  + (f", REFUSED {len(would_prompt)} unsaved" if would_prompt else "")
                  + (f", {len(errors)} error(s)" if errors else ""),
        "data": {
            "closed": closed,
            "skipped": skipped if skipped else None,
            "wouldPrompt": would_prompt if would_prompt else None,
            "errors": errors if errors else None,
            "remainingDocuments": app.documents.count,
        },
    }
    if would_prompt:
        result["errorCode"] = "unsaved_named_document"
        result["error"] = (
            "Refused to close %s: never saved and modified, so Fusion would raise its Save "
            "box (it has no name/location to write to). NOTHING was closed for these and no "
            "dialog was raised." % ", ".join(repr(n) for n in would_prompt))
        result["_hint"] = (
            "Pick one, both are dialog-free: (a) save it first with an EXPLICIT destination "
            "- fusion_save_to_cloud {name, projectName, folderPath} - which never prompts "
            "because you supply what the dialog would ask for, then close; or (b) call this "
            "again with save:true, which saves through the same API path. Fusion's API "
            "cannot discard a named, never-saved document silently, so there is no third "
            "option that avoids the modal.")
    return result
