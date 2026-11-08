"""In-app parametric modeling for the Adom Bridge.

`run_modeling_script` executes a Python snippet against the live Fusion 360
modeling API (adsk.fusion) inside the user's open Fusion session. This is the
FREE, immediate path to programmatic CAD (sketches, extrudes, components,
parameters) — as opposed to the APS Fusion Automation API, which runs headless
in the cloud but BILLS per processing hour. Use this to build parametric parts
(e.g. a JST connector) from a script.

The snippet runs ON THE MAIN THREAD (the add-in marshals every command through a
CustomEvent), which is required by the Fusion modeling API. The snippet's globals
include `adsk`, `app`, `ui`, and a `design` (the active Fusion Design, created if
none is open). Anything the snippet `print()`s is captured; set a top-level
`result` variable to return structured data.
"""

import io
import re
import contextlib
import traceback

import adsk.core
import adsk.fusion


# ── Raw Fusion API errors -> stable codes + actionable hints ──────────────────
# Oliver, PR #34 item 3 / PR #38 (opened 2026-07-27 and 2026-08-10, and I left both
# unanswered for weeks - see the reply on those PRs). A modelling script that fails
# returns Fusion's raw exception text, which for the three failures below says nothing
# about what to DO. The Part-vs-Assembly one in particular is undocumented and cost a
# real session hours: the script is correct, the DESIGN is in the wrong mode.
#
# Matching is on substrings because Fusion's wording varies by version, and every rule
# FALLS BACK to the raw error rather than swallowing it - a misclassification must never
# hide the real message. Codes are stable; the raw text always rides along.
_MODELING_ERROR_RULES = [
    (("single component", "part design", "assembly design", "design type"),
     "part_design_single_component",
     "This design is in Fusion's SINGLE-COMPONENT (Part Design) mode, which forbids "
     "creating components - the script is fine, the design mode is wrong. Switch the "
     "design to Assembly Design (right-click the root in the browser, or set the design "
     "type), or model directly into the root component instead of adding one."),
    (("root component", "cannot be renamed", "name of the root"),
     "root_component_rename",
     "Fusion will not rename the ROOT component from the API - its name follows the "
     "DOCUMENT name. Rename the document instead (save-as with the name you want), or "
     "rename a child occurrence rather than the root."),
    (("object is invalid", "no longer valid", "has been deleted", "referenced object"),
     "deleted_entity_reference",
     "The script referenced an entity that no longer exists - Fusion invalidates handles "
     "when a feature is deleted, rolled back, or recomputed. Re-fetch the object from its "
     "collection immediately before use rather than holding a handle across operations."),
]


def _classify_modeling_error(err_text: str):
    """(errorCode, hint) for a raw Fusion exception, or (None, None) when unrecognised.

    Unrecognised is a legitimate answer: inventing a code for an error we do not
    understand would be worse than saying nothing, because a caller would branch on it.
    """
    t = (err_text or "").lower()
    for needles, code, hint in _MODELING_ERROR_RULES:
        if any(n in t for n in needles):
            return code, hint
    return None, None


def handle_run_modeling_script(app: adsk.core.Application, args: dict) -> dict:
    script = args.get("script") or args.get("code") or ""
    if not script:
        return {"success": False, "error": "No script provided.",
                "_hint": "Pass {\"script\": \"<python using adsk.fusion>\"}. Globals available: "
                         "adsk, app, ui, design (the ACTIVE Design, or None - see _hintDesign).",
                "_hintDesign": "`design` is None whenever the front document is not a Design "
                               "(a library, a drawing, the data panel). This verb will NOT "
                               "displace what the user is looking at to make one. Pass "
                               "createDesign:true if your script genuinely needs a fresh Design; "
                               "the response then reports createdDocument.",
                "_hintPartVsAssembly": "A Fusion PART DESIGN document holds exactly ONE "
                                       "component. Importing a second body into one fails with "
                                       "'2 : InternalValidationError : results', and the real "
                                       "reason is only visible as a CEF toast (read it with "
                                       "fusion_alerts): 'Part Design documents can only contain "
                                       "one component'. To gang several models into one view, "
                                       "create a fresh document with "
                                       "app.documents.add(adsk.core.DocumentTypes."
                                       "FusionDesignDocumentType) inside the script."}

    ui = app.userInterface
    # Ensure there's a design to model into — but ONLY if this script actually models.
    #
    # A LIBRARY (or a drawing, or the data panel) is not a Design, so an unconditional
    # documents.add() here yanks the user's library out of the front and leaves an empty
    # "Untitled" in its place. That silently broke every caller that inspects the session
    # around a script (the managed-library precheck ran a pure adsk.core hub query and got
    # its own library de-fronted before the ribbon clicks landed), and it littered Fusion
    # with Untitled designs. So: create only when the script references `design`, or when
    # the caller explicitly asks with createDesign:true.
    # The rule is about DISPLACEMENT, not about what the script says. Never shove an open
    # document out of the front to make a Design; but when there is nothing open at all there
    # is nothing to displace, so create one as before.
    #
    # (An earlier version of this guard sniffed the script for the token "design". That was
    # wrong: the 3D-bind chain models through `d = adsk.fusion.Design.cast(app.activeProduct)`
    # and never says "design", so a name-based test silently refuses the one caller that most
    # needs a Design.)
    created_document = None
    design = adsk.fusion.Design.cast(app.activeProduct)
    if not design:
        try:
            nothing_open = app.documents.count == 0
        except Exception:
            nothing_open = False
        if bool(args.get("createDesign")) or nothing_open:
            app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
            design = adsk.fusion.Design.cast(app.activeProduct)
            try:
                created_document = app.activeDocument.name if app.activeDocument else None
            except Exception:
                created_document = None
        else:
            # A library (or drawing, or the data panel) is in front. Hand the script a None
            # design rather than replacing what the user is looking at.
            design = None

    ns = {
        "adsk": adsk,
        "app": app,
        "ui": ui,
        "design": design,
        "__name__": "__adom_modeling__",
    }
    buf = io.StringIO()
    try:
        with contextlib.redirect_stdout(buf):
            exec(script, ns)
    except Exception as e:
        _code, _hint = _classify_modeling_error(str(e))
        _out = {"success": False, "error": str(e),
                "data": {"traceback": traceback.format_exc(), "stdout": buf.getvalue()},
                "_hint": "Modeling-script error (a code/geometry problem, NOT the Fusion license). "
                         "Read-only Fusion still creates geometry in memory; only cloud-save is blocked."}
        if _code:
            # The raw error STAYS in `error`; the classification is additive, so a caller
            # that does not know this code still sees exactly what it saw before.
            _out["errorCode"] = _code
            _out["_hint"] = _hint + " (Fusion's own message is in `error`.)"
        return _out

    res = ns.get("result")
    # Keep the return JSON-safe.
    if not isinstance(res, (str, int, float, bool, dict, list, type(None))):
        res = str(res)
    out = buf.getvalue().strip()
    return {"success": True,
            "output": out or "Modeling script ran successfully.",
            "data": {"result": res, "documentName": app.activeDocument.name if app.activeDocument else None,
                     "createdDocument": created_document}}
