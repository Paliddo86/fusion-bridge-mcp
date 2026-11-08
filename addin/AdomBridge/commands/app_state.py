"""App state command — list open documents, active workspace, and Fusion state."""

import adsk.core


def handle_get_app_state(app: adsk.core.Application, args: dict) -> dict:
    """Get the current state of Fusion 360: open documents, active workspace, etc.

    Returns information about every open document/tab, which one is active,
    the current workspace, and product type.
    """
    # The workspace and whether an electronics editor is in front, read from the API: this read is
    # polled (readiness, the dashboard, waits), and a text command here (Electron.run DISPLAY) ended
    # the user's MOVE in progress every time (issue #131)
    from .ui_state import active_workspace as _ws, active_workspace_id as _ws_id, is_electronics as _is_el, user_command_active as _busy
    active_workspace = _ws(app)
    workspace_id = _ws_id(app)
    is_electronics = _is_el(app)
    user_command = _busy(app)

    # Active document
    active_doc_name = None
    try:
        doc = app.activeDocument
        if doc:
            active_doc_name = doc.name
    except Exception:
        pass

    # Active product type
    product_type = None
    try:
        product = app.activeProduct
        if product:
            product_type = product.productType
    except Exception:
        pass

    # All open documents
    documents = []
    try:
        for i in range(app.documents.count):
            doc = app.documents.item(i)
            doc_info = {
                "name": doc.name,
                "isActive": (doc.name == active_doc_name) if active_doc_name else False,
            }

            # documentType may not exist on all document types
            try:
                doc_info["type"] = _document_type_name(doc.documentType)
            except Exception:
                doc_info["type"] = "unknown"

            # isSaved may not be available on all document types
            try:
                doc_info["isSaved"] = doc.isSaved
            except Exception:
                pass

            # Try to get data file info (cloud save path)
            try:
                df = doc.dataFile
                if df:
                    doc_info["dataFile"] = {
                        "name": df.name,
                        "id": df.id,
                    }
                    try:
                        doc_info["dataFile"]["parentFolder"] = df.parentFolder.name
                    except Exception:
                        pass
            except Exception:
                pass

            documents.append(doc_info)
    except Exception as e:
        documents = [{"error": f"Failed to enumerate documents: {e}"}]

    # Build context-aware hint
    hints = []
    if is_electronics:
        hints.append("Electronics workspace active — fusion_electron_run, fusion_board_info, "
                      "fusion_export_source, fusion_export_eagle_source are available.")
    else:
        hints.append("Not in Electronics workspace. Use fusion_show_2d_board to enter board view, "
                      "or fusion_open_cloud_file to open an electronics project (.fprj).")
    hints.append("Check fusion_state {screenshot:false} (dialogs + focusEvents) before running commands if something looks stuck.")
    hint = " ".join(hints)

    return {
        "success": True,
        "output": f"{len(documents)} document(s) open, active workspace: {active_workspace}",
        "data": {
            "activeWorkspace": active_workspace,
            "activeDocument": active_doc_name,
            "productType": product_type,
            "isElectronics": is_electronics,
            "userCommandActive": user_command,
            "activeWorkspaceId": workspace_id,
            "documents": documents,
        },
        "_hint": hint,
    }


def _document_type_name(doc_type) -> str:
    """Convert a DocumentType enum to a readable string."""
    type_map = {
        0: "FusionDesign",
        1: "FusionDesign",  # DesignDocumentType
        2: "Drawing",
        3: "Electronics",
        4: "PCB",
    }
    # The enum value might be an int or an enum object
    try:
        val = int(doc_type)
        return type_map.get(val, f"Unknown({val})")
    except (TypeError, ValueError):
        return str(doc_type)
