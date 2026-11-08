#!/usr/bin/env python3
"""mcp_server.py - Model Context Protocol (MCP) front door for the Fusion bridge.

    MCP client --stdio--> mcp_server.py --HTTP POST 127.0.0.1:8773/command--> server.py --> add-in --> Fusion

What it exposes
---------------
* TOOLS     - a curated set of the most-used `fusion_*` verbs with named, typed
              parameters, plus `fusion_call(verb, args)` as the escape hatch for
              the rest of the vocabulary (see `fusion_describe` for the list).
* RESOURCES - every bundled skill under `skills/<name>/SKILL.md` as
              `skill://<name>` markdown, a `skill://index` catalogue, and one
              resource per extra .md shipped beside a skill. Discovered by
              scanning the directory, so adding a skill adds a resource.
* PROMPTS   - one invocable prompt per skill, so a client that supports prompts
              can do `/fusion-routing` and get the skill text.

Conventions (verified against server.py and describe.py)
--------------------------------------------------------
* Transport  : POST http://127.0.0.1:8773/command, body {"command": <verb>, "args": {...}}.
               GET  http://127.0.0.1:8773/status for the bridge's health payload.
* Verb names : the vocabulary is `fusion_<x>` (describe/handlers), but the wire
               command carries the prefix stripped - `_dispatch_command_inner`
               strips `fusion_` itself (server.py, "Issue #327"). We strip it here
               too and send the bare name, so the call is explicit on the wire.
* Replies    : the bridge returns structured JSON (success, output, data, error,
               errorCode, _hint, toast, ...). Every tool returns that payload as a
               PRETTY-PRINTED JSON STRING (text content), never raising: a broken
               bridge, a timeout or a known refusal comes back as JSON with
               `success:false`, an `errorCode` and a `_hint` saying what to do.
* Timeouts   : taken per verb from the `timeoutSeconds` field of its describe
               entry, falling back to 60 s and capped at `TIMEOUT_CAP`.

CLI
---
    python3 mcp_server.py                 # serve over stdio (default)
    python3 mcp_server.py serve           # same
    python3 mcp_server.py --bridge-port 8773
    python3 mcp_server.py status          # bridge health, then exit
    python3 mcp_server.py list-skills     # skills found on disk, then exit
    python3 mcp_server.py list-verbs      # the verb vocabulary, then exit

Only stdlib is imported at module level; the `mcp` package (and pydantic, via the
`mcp` install) is imported inside `build_server()`, so `status` / `list-skills` /
`list-verbs` work on a machine without the SDK.
"""

import argparse
import asyncio
import inspect
import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Optional

REPO_ROOT = Path(__file__).resolve().parent
SKILLS_DIR = REPO_ROOT / "skills"

BRIDGE_HOST = "127.0.0.1"
DEFAULT_BRIDGE_PORT = 8773
BRIDGE_PORT_ENV = "FUSION_BRIDGE_PORT"

TIMEOUT_FALLBACK = 60.0
TIMEOUT_CAP = 600.0

_DESCRIBE_MODULE = None  # cached describe.py module (or False when unimportable)
_DESCRIBE_TRIED = False
_VERB_TIMEOUTS = None  # cached {verb: timeoutSeconds} from describe

_bridge_port = int(os.environ.get(BRIDGE_PORT_ENV, DEFAULT_BRIDGE_PORT))


def set_bridge_port(port):
    """Point every tool at a different bridge port (CLI --bridge-port / env)."""
    global _bridge_port
    _bridge_port = int(port)


# --------------------------------------------------------------------------------------
# Bridge transport
# --------------------------------------------------------------------------------------


def _bridge_url(path):
    return "http://%s:%d%s" % (BRIDGE_HOST, _bridge_port, path)


def _unreachable(exc):
    return {
        "success": False,
        "errorCode": "bridge_unreachable",
        "error": "Fusion bridge not reachable on %s:%d (%s: %s)"
                 % (BRIDGE_HOST, _bridge_port, type(exc).__name__, exc),
        "_hint": "Bridge not reachable on %s:%d - start it with: python3 server.py --port %d "
                 "(from the fusion-bridge repo), then retry. Nothing was sent to Fusion."
                 % (BRIDGE_HOST, _bridge_port, _bridge_port),
    }


def _bridge_get(path, timeout=10.0):
    """GET a bridge endpoint. Never raises: transport failures come back as an error dict."""
    try:
        request = urllib.request.Request(_bridge_url(path), method="GET")
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return _http_error(exc)
    except (urllib.error.URLError, OSError) as exc:
        return _unreachable(getattr(exc, "reason", exc))
    try:
        return json.loads(raw)
    except ValueError:
        return {"success": False, "errorCode": "bridge_bad_response",
                "error": "Bridge answered %s with non-JSON: %r" % (path, raw[:200])}


def _http_error(exc):
    body = ""
    try:
        body = exc.read().decode("utf-8", "replace")
    except Exception:
        pass
    try:
        payload = json.loads(body)
        if isinstance(payload, dict):
            return payload
    except ValueError:
        pass
    return {"success": False, "errorCode": "bridge_http_error",
            "error": "Bridge answered HTTP %s: %s" % (exc.code, (body or exc.reason))[:500]}


def _bridge_command(verb, args=None, timeout=None):
    """POST one verb to the bridge. Never raises: returns the bridge payload or an error dict."""
    bare = verb[7:] if verb.startswith("fusion_") else verb  # server strips it too (Issue #327)
    payload = json.dumps({"command": bare, "args": args or {}}).encode("utf-8")
    limit = float(timeout) if timeout else _verb_timeout(verb)
    request = urllib.request.Request(
        _bridge_url("/command"), data=payload, method="POST",
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=limit) as response:
            raw = response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return _explain(_http_error(exc))
    except (TimeoutError, OSError) as exc:
        reason = getattr(exc, "reason", exc)
        if "timeout" in str(reason).lower() or isinstance(exc, TimeoutError):
            return {"success": False, "errorCode": "bridge_timeout",
                    "error": "No answer for %s within %.0fs." % (verb, limit),
                    "_hint": "The verb may still be running inside Fusion. Poll with "
                             "fusion_get_app_state or fusion_describe rather than re-issuing a "
                             "mutating verb blindly."}
        return _unreachable(reason)
    try:
        result = json.loads(raw)
    except ValueError:
        return {"success": False, "errorCode": "bridge_bad_response",
                "error": "Bridge answered /command with non-JSON: %r" % raw[:200]}
    return _explain(result)


_KNOWN_REFUSALS = {
    "addin_restart_required": (
        "KNOWN STATE, NOT A CRASH. Fusion is running but the AdomBridge add-in is not serving - "
        "Fusion only loads add-ins at launch. Do not report this as a bridge failure: show the "
        "userMessage (and openDocuments/unsavedDocuments when documentsKnown), ask before "
        "restarting if the user has work open, then call fusion_restart_for_addin with save=true."
    ),
    "addin_restarting": (
        "The bridge is restarting Fusion for the user right now (documents saved first). Poll "
        "fusion_status until ready is true (~60-90 s), then retry the same call."
    ),
    "fusion_not_installed": (
        "Fusion 360 is not installed on this machine. Offer to install it (fusion-onboarding "
        "skill) instead of asking the user to do it."
    ),
}


def _explain(result):
    """Annotate known refusals so a model reads them as the deliberate states they are."""
    if not isinstance(result, dict):
        return result
    note = _KNOWN_REFUSALS.get(str(result.get("errorCode") or ""))
    if note:
        result = dict(result)
        result["mcpNote"] = note
    return result


def _command_text(verb, args=None, timeout=None):
    """Run a verb and render its payload as the JSON string every tool returns."""
    return _as_text(_bridge_command(verb, args, timeout))


def _as_text(payload):
    return json.dumps(payload, indent=2, ensure_ascii=False, default=str)


# --------------------------------------------------------------------------------------
# Verb metadata (describe)
# --------------------------------------------------------------------------------------


def _load_describe():
    """Import describe.py from this repo (cheap, stdlib-only) so verb timeouts work offline."""
    global _DESCRIBE_MODULE, _DESCRIBE_TRIED
    if _DESCRIBE_TRIED:
        return _DESCRIBE_MODULE
    _DESCRIBE_TRIED = True
    try:
        if str(REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(REPO_ROOT))
        import describe  # noqa: E402 - local module, imported lazily on purpose
        _DESCRIBE_MODULE = describe
    except Exception:
        _DESCRIBE_MODULE = False
    return _DESCRIBE_MODULE


def _verb_timeouts():
    global _VERB_TIMEOUTS
    if _VERB_TIMEOUTS is not None:
        return _VERB_TIMEOUTS
    table = {}
    module = _load_describe()
    if module:
        try:
            for entry in module._verbs():
                table[entry["name"]] = entry.get("timeoutSeconds")
        except Exception:
            table = {}
    _VERB_TIMEOUTS = table
    return table


def _verb_timeout(verb):
    """Per-verb timeout from its describe entry, fallback 60 s, capped."""
    value = _verb_timeouts().get(verb)
    if not isinstance(value, (int, float)) or value <= 0:
        return TIMEOUT_FALLBACK
    return min(float(value), TIMEOUT_CAP)


def describe_verbs():
    """The verb vocabulary: the live bridge's describe when reachable, else describe.py."""
    result = _bridge_command("describe", {}, timeout=20.0)
    verbs = result.get("verbs") if isinstance(result, dict) else None
    if isinstance(verbs, list) and verbs:
        return verbs
    module = _load_describe()
    if module:
        try:
            return module._verbs()
        except Exception:
            pass
    return []


# --------------------------------------------------------------------------------------
# Curated tools - one table, registered in a loop
# --------------------------------------------------------------------------------------
# Each param: (python_name, wire_name, type, required, description).
# python_name is what the MCP client sees; wire_name is the bridge's own arg name.

_PARAM_TYPES = {
    "str": str,
    "int": int,
    "float": float,
    "bool": bool,
    "list[str]": list[str],
    "dict": dict,
}

_TOOLS = [
    # ---- status / info ---------------------------------------------------------
    {
        "name": "fusion_status",
        "verb": "fusion_status",
        "title": "Bridge + Fusion readiness",
        "summary": "Fast readiness check (does not launch Fusion): is Fusion installed and "
                   "running, is the add-in serving, is the bridge ready to drive? Poll this "
                   "while a long verb runs, or after a restart.",
        "params": [],
    },
    {
        "name": "fusion_get_app_state",
        "verb": "fusion_get_app_state",
        "title": "Current app state",
        "summary": "Current app state: running, active document, workspace, whether the "
                   "active document is electronics. The cheap poll verb for long calls.",
        "params": [],
    },
    {
        "name": "fusion_get_design_info",
        "verb": "fusion_get_design_info",
        "title": "Design summary",
        "summary": "Design-level summary of the active design: components, bodies, sketches, "
                   "units, root name.",
        "params": [],
    },
    {
        "name": "fusion_document_info",
        "verb": "fusion_document_info",
        "title": "Active document info",
        "summary": "The active document: name, kind (project, schematic, board, library, "
                   "model), saved/unsaved-changes flags, cloud location, version and project. "
                   "Read only; opens nothing.",
        "params": [],
    },
    {
        "name": "fusion_get_parameters",
        "verb": "fusion_get_parameters",
        "title": "List parameters",
        "summary": "All user and model parameters of the active design (name, expression, "
                   "value, unit).",
        "params": [],
    },
    {
        "name": "fusion_set_parameter",
        "verb": "fusion_set_parameter",
        "title": "Set parameter expression",
        "summary": "Set one parameter's expression by name in the ACTIVE 3D design (user "
                   "parameters first, then model parameters). Mutates the design and "
                   "regenerates it.",
        "params": [
            ("name", "name", "str", True,
             "Parameter name as listed by fusion_get_parameters."),
            ("expression", "expression", "str", True,
             'New expression, e.g. "10 mm".'),
        ],
    },
    # ---- documents -------------------------------------------------------------
    {
        "name": "fusion_open_cloud_file",
        "verb": "fusion_open_cloud_file",
        "title": "Open a cloud file",
        "summary": "Open a CLOUD file by name. For electronics, open the DESIGN document "
                   "(EcadDesignProductType), never a child schematic/.brd/3D file, then switch "
                   "views with fusion_show_schematic / fusion_show_2d_board.",
        "params": [
            ("file_name", "fileName", "str", True, "File name to open."),
            ("project_name", "projectName", "str", False,
             "Project to look in; defaults to the active project."),
            ("folder_path", "folderPath", "str", False, "Subfolder inside the project."),
        ],
    },
    {
        "name": "fusion_open_design",
        "verb": "fusion_open_design",
        "title": "Open a local design file",
        "summary": "Open a LOCAL design file (.f3d/.step, a Windows path). Prefer the cloud "
                   "verbs for team designs.",
        "params": [
            ("file_path", "filePath", "str", True, "Windows path of the local file."),
            ("intent", "intent", "str", False, '"mechanical" or "electronics".'),
        ],
    },
    {
        "name": "fusion_save_document",
        "verb": "fusion_save_document",
        "title": "Save the open design in place",
        "summary": "Save the open electronics design IN PLACE as a new version (no Save As), "
                   "then read back each changed document's version.",
        "params": [
            ("document_name", "documentName", "str", False,
             "The design's name; defaults to the active document."),
            ("comment", "comment", "str", False, "Version comment."),
        ],
    },
    {
        "name": "fusion_close_document",
        "verb": "fusion_close_document",
        "title": "Close a document (no modal)",
        "summary": "Close ONE document without raising a modal save dialog. A saved cloud "
                   "document with unsaved changes is refused rather than prompted.",
        "params": [
            ("name", "name", "str", False, "Document name; defaults to the active document."),
            ("save", "save", "bool", False, "Save a version first, then close."),
            ("save_to_close", "saveToClose", "bool", False,
             "Save a version only because the close needs it."),
        ],
    },
    {
        "name": "fusion_list_cloud_projects",
        "verb": "fusion_list_cloud_projects",
        "title": "List cloud projects",
        "summary": "List the cloud PROJECTS visible to the signed-in Fusion session (a full "
                   "hub walk - slow on big teams; prefer fusion_aps_search to find a file).",
        "params": [],
    },
    {
        "name": "fusion_list_cloud_files",
        "verb": "fusion_list_cloud_files",
        "title": "List files in a project",
        "summary": "List the files and folders of a cloud project, optionally inside a "
                   "subfolder.",
        "params": [
            ("project_name", "projectName", "str", False,
             "Project name; defaults to the active project."),
            ("folder_path", "folderPath", "str", False, "Subfolder path."),
        ],
    },
    # ---- export ----------------------------------------------------------------
    {
        "name": "fusion_export_step",
        "verb": "fusion_export_step",
        "title": "Export STEP",
        "summary": "Export the active design to STEP (needs the 3D view).",
        "params": [
            ("output_path", "outputPath", "str", True, "Destination .step path."),
        ],
    },
    {
        "name": "fusion_export_stl",
        "verb": "fusion_export_stl",
        "title": "Export STL",
        "summary": "Export the active design to an STL mesh.",
        "params": [
            ("output_path", "outputPath", "str", True, "Destination .stl path."),
        ],
    },
    {
        "name": "fusion_export_3mf",
        "verb": "fusion_export_3mf",
        "title": "Export 3MF",
        "summary": "Export the active design to a 3MF mesh.",
        "params": [
            ("output_path", "outputPath", "str", True, "Destination .3mf path."),
        ],
    },
    {
        "name": "fusion_export_iges",
        "verb": "fusion_export_iges",
        "title": "Export IGES",
        "summary": "Export the active design to IGES.",
        "params": [
            ("output_path", "outputPath", "str", True, "Destination .iges path."),
        ],
    },
    {
        "name": "fusion_export_dxf",
        "verb": "fusion_export_dxf",
        "title": "Export DXF (board layers or mechanical)",
        "summary": "Export the board to DXF, one file per requested layer (Edge.Cuts, copper, "
                   "silkscreen, fab), or the mechanical drawing to a single DXF via "
                   "output_path.",
        "params": [
            ("layers", "layers", "list[str]", False,
             "Board form: layers to write (Edge.Cuts, F.Cu, In1.Cu, B.Cu, F.SilkS, B.SilkS, "
             "F.Fab, B.Fab, Dwgs.User or Fusion layer numbers)."),
            ("output_dir", "outputDir", "str", False, "Board form: output folder."),
            ("output_path", "outputPath", "str", False, "Mechanical form: the .dxf path."),
            ("units", "units", "str", False, "mm (default) or in."),
            ("contours", "contours", "bool", False, "Write the outline as closed polylines."),
            ("expect_document", "expectDocument", "str", False,
             "Board form: required - the board's exact name, so a wrong tab is caught."),
        ],
    },
    {
        "name": "fusion_export_pdf",
        "verb": "fusion_export_pdf",
        "title": "Export PDF",
        "summary": "Every sheet of the design as one PDF; board=true adds the board as a "
                   "second PDF.",
        "params": [
            ("output", "output", "str", False, "The .pdf path; defaults to Documents/<design>.pdf."),
            ("expect_document", "expectDocument", "str", False, "The design's exact name."),
            ("board", "board", "bool", False, "Also export the board."),
        ],
    },
    {
        "name": "fusion_export_bom",
        "verb": "fusion_export_bom",
        "title": "Export BOM CSV",
        "summary": "Export a Bill of Materials CSV from the open board: Comment, Designator, "
                   "Footprint, Quantity, Library plus each part's MPN and distributor numbers.",
        "params": [
            ("output_path", "outputPath", "str", False, "Destination .csv path."),
            ("grouped", "grouped", "bool", False,
             "Group by value/footprint/library (default true), else one line per part."),
        ],
    },
    # ---- modeling --------------------------------------------------------------
    {
        "name": "fusion_run_modeling_script",
        "verb": "fusion_run_modeling_script",
        "title": "Run a modeling script",
        "summary": "Run an adsk.fusion modeling script in the live session (sketches, "
                   "extrudes, parameters). Globals available: adsk, app, ui, design.",
        "params": [
            ("script", "script", "str", True, "Python source using the adsk API."),
        ],
    },
    {
        "name": "fusion_inspect_bodies",
        "verb": "fusion_inspect_bodies",
        "title": "Inspect bodies (mm)",
        "summary": "Geometry read-back in MILLIMETRES: per body bbox, size, volume, area, face "
                   "counts, appearance, material. Use it to verify your own modelling.",
        "params": [
            ("world_space", "worldSpace", "bool", False,
             "Resolve through occurrence transforms (default false = root component only)."),
            ("occurrence", "occurrence", "str", False,
             "Substring filter on occurrence name (worldSpace only)."),
        ],
    },
    {
        "name": "fusion_bodies",
        "verb": "fusion_bodies",
        "title": "Board part bodies",
        "summary": "Each part's 3D body as a box from the design's 3D PCB: footprint rectangle "
                   "in the board frame (mm) and its height range, plus the parts with no 3D "
                   "model.",
        "params": [
            ("expect_document", "expectDocument", "str", True,
             "The board's exact name."),
            ("board_id", "boardId", "str", False, "Optional document id guard."),
        ],
    },
    {
        "name": "fusion_physical_properties",
        "verb": "fusion_physical_properties",
        "title": "Physical properties",
        "summary": "Per-component physical properties (volume cm3, mass kg, density, area, "
                   "center of mass) for the active design.",
        "params": [
            ("names", "names", "list[str]", False,
             "Component names to measure; default all."),
            ("accuracy", "accuracy", "str", False,
             "low (default), medium, high or veryhigh."),
        ],
    },
    # ---- view / screenshot -----------------------------------------------------
    {
        "name": "fusion_take_screenshot",
        "verb": "fusion_take_screenshot",
        "title": "Screenshot the Fusion window",
        "summary": "Screenshot the Fusion window in the BACKGROUND (no focus steal). Use it to "
                   "verify your own work: geometry at the wrong Z, an empty view or a "
                   "placeholder body only shows up in the pixels.",
        "params": [
            ("output_path", "outputPath", "str", False, "Where to write the image."),
        ],
    },
    {
        "name": "fusion_view",
        "verb": "fusion_view",
        "title": "Move the camera",
        "summary": "First-class camera motion in the 3D viewport: fit, home, iso, top, front, "
                   "right, back, left, bottom, orbit, zoom, pan, style.",
        "params": [
            ("action", "action", "str", True,
             "fit | home | iso | top | front | right | back | left | bottom | orbit | zoom | "
             "pan | style."),
            ("space", "space", "str", False, "'3d' (default) or '2d'."),
            ("degrees", "degrees", "float", False, "orbit: angle, default 40."),
            ("factor", "factor", "float", False, "zoom: >1 zooms in."),
            ("dx", "dx", "float", False, "pan: horizontal."),
            ("dy", "dy", "float", False, "pan: vertical."),
            ("style", "style", "str", False,
             "style: shaded | shaded-edges | wireframe | hidden-edges."),
            ("smooth", "smooth", "bool", False, "Animate the move (default true)."),
            ("fit", "fit", "bool", False, "Fit after an orientation change (default true)."),
            ("steps", "steps", "int", False, "2D: number of frames."),
            ("x1", "x1", "float", False, "2D pan: start x."),
            ("y1", "y1", "float", False, "2D pan: start y."),
            ("x2", "x2", "float", False, "2D pan: end x."),
            ("y2", "y2", "float", False, "2D pan: end y."),
        ],
    },
    # ---- APS (cloud, no Fusion needed) -----------------------------------------
    {
        "name": "fusion_aps_status",
        "verb": "fusion_aps_status",
        "title": "APS status",
        "summary": "APS state: configured, signed in, token live, never-charge meter. When "
                   "configured is false, read the fusion-aps-org-setup skill before telling "
                   "anyone to register an app.",
        "params": [],
    },
    {
        "name": "fusion_aps_search",
        "verb": "fusion_aps_search",
        "title": "APS server-indexed search",
        "summary": "Server-indexed search across the whole team hub (20-30 s hub-wide, a few "
                   "seconds with project_id). A zero from the server index is not absence: the "
                   "reply then carries data.fallback.",
        "params": [
            ("query", "query", "str", True, "Search text."),
            ("limit", "limit", "int", False, "Max results, default 50."),
            ("hub_id", "hubId", "str", False, "Restrict to one hub."),
            ("project_id", "projectId", "str", False, "Restrict to one project (much faster)."),
        ],
    },
    {
        "name": "fusion_aps_recent",
        "verb": "fusion_aps_recent",
        "title": "Recently modified cloud designs",
        "summary": "Most-recently-modified designs across the team hub, newest first.",
        "params": [
            ("limit", "limit", "int", False, "How many, default 25."),
        ],
    },
    {
        "name": "fusion_aps_browse",
        "verb": "fusion_aps_browse",
        "title": "Browse the hub tree",
        "summary": "Browse hubs, projects and folders without launching Fusion.",
        "params": [
            ("hub_id", "hubId", "str", False, "Hub to browse; default the first."),
            ("project_id", "projectId", "str", False, "Project to browse."),
            ("folder_id", "folderId", "str", False, "Folder to browse."),
        ],
    },
    # ---- electronics -----------------------------------------------------------
    {
        "name": "fusion_show_schematic",
        "verb": "fusion_show_schematic",
        "title": "Show the schematic view",
        "summary": "Switch the OPEN electronics design to its schematic view. Pass "
                   "document_name whenever more than one tab is open.",
        "params": [
            ("document_name", "documentName", "str", False,
             "The open document to show; the bridge activates it first."),
            ("capture", "capture", "bool", False,
             "Settle the surface and return a proof screenshot inline."),
        ],
    },
    {
        "name": "fusion_show_2d_board",
        "verb": "fusion_show_2d_board",
        "title": "Show the 2D board view",
        "summary": "Switch the OPEN electronics design to its 2D PCB editor (board layout). A "
                   "VIEW of the project, not a file to open.",
        "params": [
            ("document_name", "documentName", "str", False,
             "The open document to show; the bridge activates it first."),
            ("capture", "capture", "bool", False,
             "Settle the surface and return a proof screenshot inline."),
        ],
    },
    {
        "name": "fusion_show_3d_board",
        "verb": "fusion_show_3d_board",
        "title": "Show the 3D PCB view",
        "summary": "Switch the OPEN electronics design to its 3D PCB view (the populated "
                   "board). A VIEW of the project, not a standalone file.",
        "params": [
            ("document_name", "documentName", "str", False,
             "The open document to show; the bridge activates it first."),
            ("capture", "capture", "bool", False,
             "Settle the surface and return a proof screenshot inline."),
        ],
    },
    {
        "name": "fusion_read_schematic",
        "verb": "fusion_read_schematic",
        "title": "Read the schematic",
        "summary": "Components of the open design's schematic: designator, libReference (the "
                   "deviceset), comment (the value), device, library and the part's "
                   "parameters.",
        "params": [
            ("expect_document", "expectDocument", "str", False, "The design's exact name."),
            ("params", "params", "bool", False, "Include each part's parameters (default true)."),
            ("limit", "limit", "int", False, "Max parts, default 500."),
        ],
    },
    {
        "name": "fusion_schematic_data",
        "verb": "fusion_schematic_data",
        "title": "Schematic data per part",
        "summary": "Parts on the schematic with their library part, value, description, gates "
                   "with position, pads and parameters - the full per-part read.",
        "params": [
            ("expect_document", "expectDocument", "str", False,
             "The design's exact name; default the one in front."),
            ("sheet", "sheet", "int", False, "A sheet number; default all."),
            ("designator", "designator", "str", False, "One part, e.g. U1."),
            ("pins", "pins", "bool", False, "Include pads (default true)."),
            ("params", "params", "bool", False, "Include parameters."),
            ("param_names_only", "paramNamesOnly", "bool", False,
             "Return parameter names without values."),
            ("limit", "limit", "int", False, "Max parts, default 500."),
        ],
    },
    {
        "name": "fusion_board_info",
        "verb": "fusion_board_info",
        "title": "Raw board geometry",
        "summary": "Raw board geometry XML (copper, arcs, lines) for the open board.",
        "params": [],
    },
    {
        "name": "fusion_netlist",
        "verb": "fusion_netlist",
        "title": "Netlist",
        "summary": "The compiled netlist: every net with its pads as Designator.Pad (U1.3), "
                   "from the live board, or from a schematic FILE without Fusion (sch_path); "
                   "expect compares it pin by pin.",
        "params": [
            ("expect_document", "expectDocument", "str", False,
             "The active board's exact name (live read)."),
            ("sch_path", "schPath", "str", False,
             "An EAGLE .sch or Fusion .fsch on the Fusion machine (file read)."),
            ("net", "net", "str", False, "One net only (case-insensitive)."),
            ("expect", "expect", "dict", False,
             'Compare pin by pin: {"net": ["U1.3", ...]}.'),
        ],
    },
    {
        "name": "fusion_list_nets",
        "verb": "fusion_list_nets",
        "title": "List board nets",
        "summary": "Every net on the live board with its pad, track, arc, via and pour counts "
                   "and its net class.",
        "params": [
            ("expect_document", "expectDocument", "str", True, "The board's exact name."),
            ("board_id", "boardId", "str", False, "Optional document id guard."),
        ],
    },
    {
        "name": "fusion_routing_state",
        "verb": "fusion_routing_state",
        "title": "Board model (EDA standard)",
        "summary": "The live board as the EDA Bridge Standard's board model, from a fresh "
                   "background export: tracks, arcs, vias, pads with nets, parts, pours, "
                   "outline and unconnected/splitNets.",
        "params": [
            ("expect_document", "expectDocument", "str", True, "The board's exact name."),
            ("board_id", "boardId", "str", False, "Optional document id guard."),
        ],
    },
    {
        "name": "fusion_run_drc",
        "verb": "fusion_run_drc",
        "title": "Run DRC",
        "summary": "Run Fusion's own design rule check on the live board in the background and "
                   "read every violation back: rule text, error or warning, layer, position, "
                   "approved flag, counts by rule.",
        "params": [
            ("expect_document", "expectDocument", "str", True, "The board's exact name."),
            ("board_id", "boardId", "str", False, "Optional document id guard."),
            ("limit", "limit", "int", False, "Max violations, default 200."),
        ],
    },
    {
        "name": "fusion_electron_run",
        "verb": "fusion_electron_run",
        "title": "Run an EAGLE command",
        "summary": "Run an EAGLE/Electron command in the open board (e.g. 'RUN <ulp>'). The "
                   "generic electronics extension point; see the fusion-eagle-commands skill "
                   "for which commands block.",
        "params": [
            ("command", "command", "str", True, "The EAGLE command line."),
        ],
    },
    {
        "name": "fusion_export_gerbers",
        "verb": "fusion_export_gerbers",
        "title": "Export gerbers + drill",
        "summary": "Run the bundled JLCPCB CAM job (auto 2/4-layer) to produce the gerber and "
                   "drill ZIP.",
        "params": [
            ("output_dir", "outputDir", "str", False, "Output folder; default the bridge's temp dir."),
        ],
    },
    # ---- recovery --------------------------------------------------------------
    {
        "name": "fusion_restart_for_addin",
        "verb": "fusion_restart_for_addin",
        "title": "Restart Fusion for the add-in",
        "summary": "ASYNC remedy for addin_restart_required: saves the user's open documents, "
                   "restarts Fusion in the background and returns at once (started:true). Ask "
                   "the user first when documents are open; poll fusion_status afterwards.",
        "params": [
            ("save", "save", "bool", False, "Save open documents first (default true)."),
        ],
    },
    {
        "name": "fusion_describe",
        "verb": "fusion_describe",
        "title": "List the bridge verbs",
        "summary": "Self-describe the bridge's verb vocabulary. By default returns just the "
                   "verb names (about 375); set names_only=false for the full per-verb "
                   "summary/input/output/timeout records (a very large payload).",
        "params": [
            ("names_only", "namesOnly", "bool", False,
             "Return only the verb names (default true). False returns the full describe "
             "records for every verb, roughly 1.2 MB."),
        ],
    },
]


def _tool_function(entry, field):
    """Build one tool callable whose MCP schema comes from the table entry."""
    verb = entry["verb"]
    params = entry["params"]
    wire_names = {python_name: wire for python_name, wire, _t, _r, _d in params}
    trims_describe = entry["name"] == "fusion_describe"

    def tool(**kwargs):
        args = {}
        for python_name, value in kwargs.items():
            if value is None:
                continue
            args[wire_names[python_name]] = value
        if trims_describe:
            # The bridge's describe has no namesOnly flag: trim its reply here so the
            # default call does not push ~1.2 MB of schemas into the client's context.
            if args.pop("namesOnly", True):
                result = _bridge_command(verb, {})
                verbs = result.get("verbs") if isinstance(result, dict) else None
                if isinstance(verbs, list):
                    names = [item.get("name") for item in verbs if isinstance(item, dict)]
                    return _as_text({"success": True, "verbCount": len(names), "verbs": names})
                return _as_text(result)
        return _command_text(verb, args)

    signature = []
    for python_name, _wire, type_code, required, description in params:
        base = _PARAM_TYPES[type_code]
        keyword = inspect.Parameter.POSITIONAL_OR_KEYWORD
        if required:
            signature.append(inspect.Parameter(
                python_name, keyword, annotation=Annotated[base, field(description)]))
        else:
            signature.append(inspect.Parameter(
                python_name, keyword,
                annotation=Annotated[Optional[base], field(description)], default=None))
    tool.__signature__ = inspect.Signature(signature)
    tool.__name__ = entry["name"]
    tool.__doc__ = entry["summary"]
    return tool


def fusion_call(verb: str, args: Optional[dict] = None) -> str:
    """Use this for any of the 375 verbs not exposed as a dedicated tool. Get the full list with fusion_describe. Example: fusion_call('fusion_routing_state', {'expectDocument': 'my-board'})

    Forwards one bridge verb verbatim: `verb` is the fusion_* name (the fusion_
    prefix is stripped on the wire, as the bridge does), `args` is that verb's own
    argument object, exactly as the bridge documents it. Returns the bridge reply as
    a JSON string.
    """
    if not verb or not str(verb).strip():
        return _as_text({"success": False, "errorCode": "missing_verb",
                         "error": "fusion_call needs a verb name.",
                         "_hint": "Call fusion_describe for the list of verbs, then pass one "
                                  "as `verb`."})
    if args is not None and not isinstance(args, dict):
        return _as_text({"success": False, "errorCode": "invalid_args",
                         "error": "args must be an object (dict) of the verb's own arguments, "
                                  "or omitted."})
    return _command_text(str(verb).strip(), args or {})


_FUSION_CALL_DESCRIPTION = (
    "Use this for any of the 375 verbs not exposed as a dedicated tool. Get the full list "
    "with fusion_describe. Example: fusion_call('fusion_routing_state', "
    "{'expectDocument': 'my-board'})"
)


def describe_fusion_call(field):
    """Give the escape hatch the same typed schema as the generated tools (returns the fn)."""
    fusion_call.__signature__ = inspect.Signature([
        inspect.Parameter(
            "verb", inspect.Parameter.POSITIONAL_OR_KEYWORD,
            annotation=Annotated[str, field(
                "The bridge verb to run, e.g. 'fusion_routing_state' (the fusion_ prefix is "
                "stripped on the wire).")]),
        inspect.Parameter(
            "args", inspect.Parameter.POSITIONAL_OR_KEYWORD,
            annotation=Annotated[Optional[dict], field(
                "That verb's own argument object, exactly as fusion_describe documents it, "
                "or omit it for a verb that takes none.")], default=None),
    ])
    return fusion_call


# --------------------------------------------------------------------------------------
# Skills - dynamic discovery, exposed as resources and prompts
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Skill:
    """One bundled skill: its directory name, SKILL.md, frontmatter description, extras."""

    name: str
    path: Path
    description: str
    extras: tuple = ()  # ((uri_suffix, path), ...) for the .md files shipped beside it


def _split_frontmatter(text):
    """Return (frontmatter, body) for a markdown file with a leading YAML block."""
    if not text.startswith("---"):
        return "", text
    end = text.find("\n---", 3)
    if end == -1:
        return "", text
    return text[3:end], text[end + 4:]


def _unquote(value):
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1]
    return value


def _frontmatter_description(frontmatter):
    """Read the top-level `description:` key, folding YAML block scalars (>-, |, >, |)."""
    lines = frontmatter.splitlines()
    for index, line in enumerate(lines):
        if not line.startswith("description:"):
            continue
        value = line[len("description:"):].strip()
        if value in (">", ">-", ">+", "|", "|-", "|+"):
            block = []
            for follow in lines[index + 1:]:
                if not follow.strip():
                    continue
                if not follow.startswith((" ", "\t")):
                    break
                block.append(follow.strip())
            return " ".join(block).strip()
        return _unquote(value).strip()
    return ""


def _first_heading(body):
    for line in body.splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            return stripped.lstrip("#").strip()
    return ""


def _extra_markdown(skill_dir):
    """Every other .md a skill ships, as (uri suffix, path) - e.g. LIBRARY_FINDINGS."""
    extras = []
    for path in sorted(skill_dir.rglob("*.md")):
        if path.name == "SKILL.md" and path.parent == skill_dir:
            continue
        relative = path.relative_to(skill_dir).with_suffix("")
        extras.append((relative.as_posix(), path))
    return tuple(extras)


def discover_skills(skills_dir=SKILLS_DIR):
    """Scan skills/<name>/SKILL.md. Adding a directory adds a resource and a prompt."""
    if not skills_dir.is_dir():
        return []
    skills = []
    for skill_dir in sorted(skills_dir.iterdir()):
        skill_md = skill_dir / "SKILL.md"
        if not skill_dir.is_dir() or not skill_md.is_file():
            continue
        try:
            text = skill_md.read_text(encoding="utf-8")
        except OSError:
            continue
        frontmatter, body = _split_frontmatter(text)
        description = _frontmatter_description(frontmatter) or _first_heading(body)
        skills.append(Skill(name=skill_dir.name, path=skill_md,
                            description=description, extras=_extra_markdown(skill_dir)))
    return skills


def _read_text(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        return "# Unreadable\n\nCould not read %s: %s" % (path, exc)


def _skill_index(skills):
    lines = ["# Fusion bridge skills", "",
             "Every bundled skill, as `skill://<name>`. Invoke one as a prompt with the same "
             "name, or read the resource for the full text.", ""]
    for skill in skills:
        summary = " ".join(skill.description.split())
        lines.append("- `skill://%s` - %s" % (skill.name, summary or "(no description)"))
    extras = [(skill.name, suffix) for skill in skills for suffix, _path in skill.extras]
    if extras:
        lines += ["", "## Extra documents", ""]
        lines += ["- `skill://%s/%s`" % (name, suffix) for name, suffix in extras]
    return "\n".join(lines) + "\n"


def _reader(path):
    def read():
        return _read_text(path)
    return read


# --------------------------------------------------------------------------------------
# Server assembly
# --------------------------------------------------------------------------------------


def build_server(name="fusion-bridge", skills_dir=SKILLS_DIR):
    """Build the MCPServer with every tool, resource and prompt registered.

    Imports the `mcp` SDK here (not at module level) so the CLI's offline
    subcommands keep working on a box without it.
    """
    from mcp.server.mcpserver import MCPServer
    from pydantic import Field

    def field(description):
        return Field(description=description)

    server = MCPServer(
        name,
        title="Fusion Bridge",
        instructions=(
            "Drive Autodesk Fusion 360 through the Fusion bridge. Tools map one-to-one onto "
            "the bridge's fusion_* verbs and return the bridge's own JSON reply as text. "
            "fusion_call reaches any verb not exposed here, and fusion_describe lists them "
            "all. Read the skills (skill://index, skill://<name>) and the fusion-driving / "
            "fusion-no-modal-dialogs skills before driving a long automation."
        ),
    )

    for entry in _TOOLS:
        server.add_tool(
            _tool_function(entry, field),
            name=entry["name"],
            title=entry["title"],
            description=entry["summary"],
            structured_output=False,
        )
    server.add_tool(
        describe_fusion_call(field),
        name="fusion_call",
        title="Call any bridge verb",
        description=_FUSION_CALL_DESCRIPTION,
        structured_output=False,
    )

    skills = discover_skills(skills_dir)
    server.resource(
        uri="skill://index",
        name="skill-index",
        title="Fusion bridge skills index",
        description="Every bundled skill with its description, as skill://<name> resources.",
        mime_type="text/markdown",
    )(_string_reader(_skill_index(skills)))
    for skill in skills:
        server.resource(
            uri="skill://%s" % skill.name,
            name=skill.name,
            title=skill.name,
            description=skill.description or skill.name,
            mime_type="text/markdown",
        )(_reader(skill.path))
        for suffix, path in skill.extras:
            server.resource(
                uri="skill://%s/%s" % (skill.name, suffix),
                name="%s/%s" % (skill.name, suffix),
                title="%s %s" % (skill.name, suffix),
                description="Supporting document of the %s skill." % skill.name,
                mime_type="text/markdown",
            )(_reader(path))
        server.prompt(
            name=skill.name,
            title=skill.name,
            description=skill.description or skill.name,
        )(_reader(skill.path))

    return server


def _string_reader(text):
    def read():
        return text
    return read


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def _print_status():
    payload = _bridge_get("/status", timeout=10.0)
    print(json.dumps(payload, indent=2, ensure_ascii=False, default=str))
    if not isinstance(payload, dict):
        return 1
    return 0 if not payload.get("errorCode") else 1


def _print_skills():
    skills = discover_skills()
    print("%d skill(s) in %s" % (len(skills), SKILLS_DIR))
    for skill in skills:
        summary = " ".join(skill.description.split())
        extras = ", ".join("skill://%s/%s" % (skill.name, suffix) for suffix, _p in skill.extras)
        line = "  skill://%-32s %s" % (skill.name, summary[:110] or "(no description)")
        print(line + ("   [extras: %s]" % extras if extras else ""))
    return 0


def _print_verbs():
    verbs = describe_verbs()
    if not verbs:
        print("No verbs: the bridge on %s:%d is down and describe.py could not be loaded."
              % (BRIDGE_HOST, _bridge_port), file=sys.stderr)
        return 1
    print("%d verb(s)" % len(verbs))
    for entry in verbs:
        if not isinstance(entry, dict):
            continue
        summary = " ".join((entry.get("summary") or "").split())
        print("  %-40s %s" % (entry.get("name"), summary[:100]))
    return 0


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(
        prog="mcp_server.py",
        description="MCP server for the Fusion bridge (tools, skills as resources and prompts).")
    parser.add_argument("command", nargs="?", default="serve",
                        choices=["serve", "status", "list-skills", "list-verbs"],
                        help="serve (default), status, list-skills, list-verbs")
    parser.add_argument("--bridge-port", type=int, default=None,
                        help="Bridge HTTP port (default %d or $%s)."
                             % (DEFAULT_BRIDGE_PORT, BRIDGE_PORT_ENV))
    args = parser.parse_args(argv)
    if args.bridge_port:
        set_bridge_port(args.bridge_port)

    if args.command == "status":
        return _print_status()
    if args.command == "list-skills":
        return _print_skills()
    if args.command == "list-verbs":
        return _print_verbs()
    asyncio.run(build_server().run_stdio_async())
    return 0


if __name__ == "__main__":
    sys.exit(main())
