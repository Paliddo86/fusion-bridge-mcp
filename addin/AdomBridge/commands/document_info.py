"""Document info and activation commands.

document_info — lightweight metadata about ALL open documents plus active doc details.
activate_document — switch to a specific open document tab by name or type.
"""

import adsk.core


def _document_type_str(doc_type) -> str:
    """Convert a DocumentType enum to a human-readable string."""
    if doc_type is None:
        return "unknown"
    type_map = {
        0: "FusionDesign",
        1: "FusionDesign",
        2: "Drawing",
        3: "Electronics",
        4: "PCB",
    }
    try:
        val = int(doc_type)
        return type_map.get(val, f"Unknown({val})")
    except (TypeError, ValueError):
        return str(doc_type)


def handle_document_info(app: adsk.core.Application, args: dict) -> dict:
    """Get metadata about all open documents and the active document.

    Returns every open document/tab with name, type, and active status,
    plus detailed cloud info for the active document. Uses only in-memory
    data — no cloud API calls.

    This is the command to use when Claude needs to know what tabs are open
    and which one is focused, without expensive cloud searches.
    """
    # Active workspace
    from .ui_state import active_workspace as _ws
    active_workspace = _ws(app)   # API only: this read is polled (issue #131)

    # Active document name (for isActive flag)
    active_doc_name = None
    active_doc = None
    try:
        active_doc = app.activeDocument
        if active_doc:
            active_doc_name = active_doc.name
    except Exception:
        pass

    # Enumerate ALL open documents
    open_documents = []
    try:
        for i in range(app.documents.count):
            doc = app.documents.item(i)
            doc_info = {
                "name": doc.name,
                "isActive": (doc.name == active_doc_name) if active_doc_name else False,
            }
            try:
                doc_info["documentType"] = _document_type_str(getattr(doc, 'documentType', None))
                doc_info["documentTypeRaw"] = str(getattr(doc, 'documentType', None))
            except Exception:
                doc_info["documentType"] = "unknown"

            try:
                doc_info["isSaved"] = doc.isSaved
            except Exception:
                pass

            # Cloud file info (lightweight — just the cached ID)
            try:
                df = doc.dataFile
                if df:
                    doc_info["cloudFileId"] = df.id
                    doc_info["isCloudDocument"] = True
                    try:
                        doc_info["cloudProjectName"] = df.parentProject.name
                    except Exception:
                        pass
                else:
                    doc_info["isCloudDocument"] = False
            except Exception:
                doc_info["isCloudDocument"] = False

            open_documents.append(doc_info)
    except Exception as e:
        open_documents = [{"error": f"Failed to enumerate documents: {e}"}]

    # Detailed info for the active document
    active_detail = {}
    if active_doc:
        try:
            active_detail["documentName"] = active_doc.name
        except Exception:
            pass

        try:
            df = active_doc.dataFile
            if df:
                active_detail["isCloudDocument"] = True
                try:
                    active_detail["cloudFileId"] = df.id
                except Exception:
                    pass
                try:
                    active_detail["documentVersion"] = df.versionNumber
                except Exception:
                    pass
                try:
                    active_detail["cloudProjectName"] = df.parentProject.name
                except Exception:
                    pass
                try:
                    # Build folder path
                    folder = df.parentFolder
                    parts = []
                    depth = 0
                    while folder and depth < 10:
                        parts.append(folder.name)
                        try:
                            folder = folder.parentFolder
                        except Exception:
                            break
                        depth += 1
                    if parts:
                        parts.reverse()
                        active_detail["cloudFolderPath"] = "/".join(parts)
                except Exception:
                    pass
            else:
                active_detail["isCloudDocument"] = False
        except Exception:
            active_detail["isCloudDocument"] = False

        try:
            active_detail["isSaved"] = active_doc.isSaved
        except Exception:
            pass

    # Product type
    try:
        product = app.activeProduct
        if product:
            active_detail["productType"] = product.productType
    except Exception:
        pass

    data = {
        "activeWorkspace": active_workspace,
        "activeDocument": active_doc_name,
        "openDocuments": open_documents,
        "openDocumentCount": len(open_documents),
        **active_detail,
    }

    return {
        "success": True,
        "output": f"{len(open_documents)} document(s) open, "
                  f"active: {active_doc_name or '(none)'}, "
                  f"workspace: {active_workspace}",
        "data": data,
        "_hint": "Use fusion_get_app_state to check workspace type. "
                 "For exports: fusion_export_gerbers, fusion_export_bom, fusion_export_cpl, "
                 "fusion_export_source (.fsch/.fbrd), fusion_export_eagle_source (.sch/.brd). "
                 "Use fusion_activate_document to switch tabs.",
    }


def handle_activate_document(app: adsk.core.Application, args: dict) -> dict:
    """Switch to a specific open document tab by name or type.

    Searches all open documents and activates the matching one. This is
    essential for automation — Claude can switch between schematic, board,
    and library tabs without asking the user to click.

    Args:
        name: Document name to activate (case-insensitive substring match).
        documentType: Document type to activate — "Electronics", "PCB",
                      "FusionDesign", "Drawing". If both name and type are
                      given, both must match.

    If multiple documents match, activates the first match and lists all
    matches in the response so Claude can refine.
    """
    target_name = args.get("name", "").strip().lower()
    target_type = args.get("documentType", "").strip().lower()
    target_data_file = args.get("dataFileName", "").strip()  # EXACT match on dataFile.name

    if not target_name and not target_type and not target_data_file:
        return {
            "success": False,
            "error": "Specify 'name' (substring) or 'dataFileName' (exact cloud-file name) "
                     "or 'documentType' (Electronics, PCB, FusionDesign, Drawing).",
            "_hint": "Call fusion_document_info to list open documents by index, or fusion_open_cloud_file to open a new document, then retry.",
        }

    # Find matching documents
    matches = []
    try:
        for i in range(app.documents.count):
            doc = app.documents.item(i)
            name_match = True
            type_match = True
            datafile_match = True

            if target_data_file:
                df_name = None
                try:
                    df_name = doc.dataFile.name if doc.dataFile else None
                except Exception:
                    df_name = None
                datafile_match = (df_name == target_data_file)

            if target_name:
                name_match = target_name in doc.name.lower()

            if target_type:
                doc_type_str = _document_type_str(getattr(doc, 'documentType', None)).lower()
                doc_type_raw = str(getattr(doc, 'documentType', None)).lower()
                type_match = (target_type in doc_type_str or
                              target_type in doc_type_raw)

            if name_match and type_match and datafile_match:
                matches.append(doc)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to enumerate documents: {e}",
            "_hint": "Call fusion_get_app_state to check if Fusion is in a usable state; a modal dialog may be blocking.",
        }

    if not matches:
        # List what IS open for debugging
        available = []
        try:
            for i in range(app.documents.count):
                doc = app.documents.item(i)
                available.append({
                    "name": doc.name,
                    "type": _document_type_str(getattr(doc, 'documentType', None)),
                })
        except Exception:
            pass

        return {
            "success": False,
            "error": f"No open document matching name='{target_name or '*'}' "
                     f"type='{target_type or '*'}'",
            "_hint": "Call fusion_document_info to list open documents by index, or fusion_open_cloud_file to open a new document, then retry.",
            "data": {
                "openDocuments": available,
                "hint": "Use fusion_document_info to see all open documents.",
            },
        }

    # Activate the first match THAT ACCEPTS. A board and its 3D PCB view are two open documents
    # with ONE name, and Fusion throws InternalValidationError activating one of them from a
    # library tab (AdomLapper, 2026-09-20), so a same-name sibling is tried before giving up.
    # Exact-name matches go first so "foo" never lands on "foo-old" when "foo" is open.
    matches.sort(key=lambda d: 0 if d.name.lower() == target_name else 1)
    target_doc = matches[0]
    want_name = target_doc.name
    activated = None
    last_err = None
    for cand in matches:
        try:
            activated = cand.activate()
            target_doc = cand
            want_name = cand.name
            last_err = None
            if activated is not False:
                break
        except Exception as e:
            last_err = e
    try:
        if last_err is not None:
            raise last_err
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to activate '{target_doc.name}': {e}",
            "_hint": "Call fusion_get_app_state to check if Fusion is in a usable state; a modal dialog may be blocking.",
            "data": {"hint": "The document may be in a state that prevents activation."},
        }

    # VERIFY, do not assume. Document.activate() returns a boolean and Fusion can decline
    # (a running command, a modal, a document still loading). Reporting success without
    # reading the result back is how "activate_document lies" got its name: callers bound
    # 3D or drove the ribbon believing a library was in front when an empty design was.
    real_active = None
    try:
        real_active = app.activeDocument.name if app.activeDocument else None
    except Exception:
        real_active = None
    if activated is False or (real_active is not None and real_active != want_name):
        return {
            "success": False,
            "errorCode": "activation_refused",
            "error": f"Fusion did not switch to '{want_name}' — the active document is "
                     f"{real_active!r} (activate() returned {activated!r}).",
            "_hint": "Something is holding the front tab: check fusion_check_dialogs for a modal "
                     "and fusion_alerts for a toast, then retry. Do NOT treat this as activated.",
            "data": {"requested": want_name, "activeDocument": real_active,
                     "activateReturned": activated},
        }

    # Query workspace after activation
    from .ui_state import active_workspace as _ws
    new_workspace = _ws(app)

    response = {
        "success": True,
        "output": f"Activated '{target_doc.name}' — workspace: {new_workspace}",
        "data": {
            "activatedDocument": target_doc.name,
            "verifiedActiveDocument": real_active,
            "documentType": _document_type_str(getattr(target_doc, 'documentType', None)),
            "activeWorkspace": new_workspace,
        },
    }

    # If multiple matches, list them all so Claude can refine
    if len(matches) > 1:
        response["data"]["allMatches"] = [
            {"name": m.name, "type": _document_type_str(getattr(m, 'documentType', None))}
            for m in matches
        ]
        response["data"]["hint"] = (
            f"{len(matches)} documents matched — activated the first one. "
            "Use a more specific name to target a different one."
        )

    return response
