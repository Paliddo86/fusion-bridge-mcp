#!/usr/bin/env python3
"""End-to-end verb test runner for the fusion bridge (dev-skills/fusion-verb-tests).

Runs IN THE CONTAINER, through the real relay, exactly the way a user's AI calls verbs:
    adom-bridge --target <box> --ai-thread "fusion-verb-tests" <verb> '<json>'

Tiers:
  daily    read-only + reversible state (safe every day, leaves the box as found)
  weekly   heavy flows (library builds, cloud writes into the test workspace, exports)
  manual   human-present only (sign-in, uninstall-for-real, notify, install, demo)
  negative always run: verbs that MUST refuse (deprecated verbs, the document guard)

Usage:
  python3 tests/run_verb_tests.py --target AdomLapper [--tier daily] [--only verb1,verb2]

Every described verb MUST appear in PLAN. If describe.py grows a verb the plan does not
know, the run FAILS with 'untriaged' - catalog drift becomes a failing test, not a gap.
"""
import argparse, datetime, json, os, pathlib, subprocess, sys, time

REPO = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))
import describe  # noqa: E402
CATALOG_TIMEOUTS = {v[0]: v[4] for v in describe._T}

THREAD = "fusion-verb-tests"
# user-neutral: the suite runs on any box (it said C:/Users/john until arav-rog joined, 2026-09-20)
SCRATCH = "C:/tmp/fusion-verb-tests"
LAST_RESULTS: dict = {}

def call(target, verb, args=None, timeout=90):
    cmd = ["adom-bridge", "--target", target, "--ai-thread", THREAD, verb, json.dumps(args or {})]
    t0 = time.time()
    try:
        for _attempt in range(3):
            p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
            raw = p.stdout.strip() or p.stderr.strip()
            d = json.loads(raw[raw.index("{"):]) if "{" in raw else {"error": raw[:200]}
            if d.get("errorCode") in ("main_thread_busy", "fusion_addin_busy", "bridge_restarting") and _attempt < 2:
                time.sleep(15); continue
            break
    except subprocess.TimeoutExpired:
        return {"_testTimeout": True}, time.time() - t0
    except Exception as e:
        return {"error": f"runner: {e}"}, time.time() - t0
    # unwrap the nested output envelope when present
    out = d.get("output")
    if isinstance(out, str) and out.strip().startswith("{"):
        try:
            inner = json.loads(out)
            merged = dict(d); merged.update(inner if not isinstance(inner.get("data"), dict) else inner["data"])
            if isinstance(inner.get("data"), dict): merged.update(inner["data"])
            d = merged
        except Exception:
            pass
    if isinstance(d.get("data"), dict):
        merged = dict(d); merged.update(d["data"]); d = merged
    return d, time.time() - t0

def expect(d, spec):
    """spec: {'ok':True} needs success/status ok; {'fields':[...]} need present;
    {'errorCode':'x'} exact; {'pred':name} named predicate."""
    errs = []
    if spec.get("okOrCode"):
        # success, or the one honest refusal that depends on the box's history (not on the verb)
        if not (d.get("success") is True or d.get("status") == "ok" or d.get("errorCode") == spec["okOrCode"]):
            errs.append("expected success or errorCode=%s, got error=%s code=%s" % (spec["okOrCode"], str(d.get("error"))[:80], d.get("errorCode")))
    if spec.get("ok"):
        if not (d.get("success") is True or d.get("status") == "ok" or d.get("ok") is True):
            errs.append("expected success, got error=%s code=%s" % (str(d.get("error"))[:80], d.get("errorCode")))
    for f in spec.get("fields", []):
        if f not in d: errs.append("missing field %r" % f)
    if "errorCode" in spec and d.get("errorCode") != spec["errorCode"]:
        errs.append("expected errorCode=%s got %s (error=%s)" % (spec["errorCode"], d.get("errorCode"), str(d.get("error"))[:60]))
    if spec.get("pred") == "count_gt_0" and not (d.get("count", 0) > 0 or len(d.get("results") or []) > 0):
        errs.append("expected >0 results")
    if spec.get("pred") == "verbs_match_plan":
        got = {v["name"] if isinstance(v, dict) else v for v in (d.get("verbs") or [])}
        planned = {k.split("#")[0] for k in PLAN}
        if got and (got - planned): errs.append("verbs described but UNTRIAGED in test plan: %s" % sorted(got - planned)[:6])
    if spec.get("pred") == "must_refuse" and not (d.get("success") is False or d.get("error")):
        errs.append("expected a refusal, got success")
    if spec.get("pred") == "ok_or_budget":
        ok = (d.get("success") is True and all(f in d for f in ("parts", "collapsedUnits")))
        budget = d.get("errorCode") == "bom_walk_budget_exceeded"
        no_design = "No active Fusion Design" in str(d.get("error"))  # honest refusal when the open document is not a Design
        if not (ok or budget or no_design):
            errs.append("expected a BOM or the budget refusal, got error=%s code=%s" % (str(d.get("error"))[:60], d.get("errorCode")))
    if spec.get("pred") == "board_model_ok":
        # the EDA Bridge Standard board model (1.12.0): every pad sized (custom shapes as their
        # bounding rectangle, never 0 x 0), copper layers by KiCad's names, unconnected computed
        pads = d.get("pads") or []
        if d.get("model") != "eda-bridge-standard/board 0.1": errs.append("model is %r, not the standard board model" % d.get("model"))
        if not pads: errs.append("no pads")
        if any(not (p.get("sizeX", 0) > 0 and p.get("sizeY", 0) > 0) for p in pads): errs.append("a pad has no size")
        if "F.Cu" not in (d.get("copperLayers") or []): errs.append("copper layers not named F.Cu..B.Cu")
        if not isinstance(d.get("unconnected"), int): errs.append("no unconnected count")
    if spec.get("pred") == "bom_has_part_numbers":
        data = d.get("data") or {}
        head = (d.get("csv") or "").splitlines()[:1]
        if not head or not all(c in head[0] for c in ("MPN", "LCSC")): errs.append("BOM has no MPN/LCSC columns")
        if not data.get("withLcsc") and not data.get("withMpn"): errs.append("no part carries an MPN or LCSC number")
    if d.get("_testTimeout"): errs.append("timed out")
    return errs


def call_polled(target, verb, args, doc, poll_sec):
    """Fire a long open, then poll get_app_state until the doc is active (the bridge's own
    _timeoutHint prescribes exactly this instead of trusting the relay timeout)."""
    args = {k: v for k, v in args.items() if not k.startswith("_")}
    d, dt = call(target, verb, args, timeout=70)
    ok = (d.get("success") is True or d.get("status") == "ok")
    t0 = time.time()
    while time.time() - t0 < poll_sec:
        st, _ = call(target, "fusion_get_app_state", {}, timeout=45)
        if st.get("activeDocument") == doc:
            return {"success": True, "activeDocument": doc}, dt + (time.time() - t0)
        time.sleep(8)
    return (d if not ok else {"success": False, "error": "document never became active"}), dt + (time.time() - t0)

# tier, args, expectation, [cleanup verb, cleanup args]
P = {}
def t(verb, tier, args=None, exp=None, note=""):
    P[verb] = {"tier": tier, "args": args or {}, "exp": exp or {"ok": True}, "note": note}

# ── stage 0: bridge + catalog ────────────────────────────────────────────────
t("fusion_readiness", "daily", {}, {"ok": True, "fields": ["bridgeVersion"]})
t("fusion_prewarm", "daily", {}, {"ok": True, "fields": ["steps", "fusionStarted", "ready"]}, "never starts Fusion; idempotent")
t("fusion_account", "daily", {}, {"ok": True, "fields": ["holdsSeat", "running"]}, "read-only: never starts Fusion or signs anyone in")
t("fusion_claim_seat", "manual", note="moves the Autodesk seat between machines; suspends another box's Fusion")
t("fusion_release_seat", "manual", note="closes Fusion on the box to free the seat")
t("fusion_describe", "daily", {}, {"ok": True, "pred": "verbs_match_plan"})
t("fusion_get_app_state", "daily", {}, {})  # shape only; content depends on Fusion state
# must-refuse (deprecated, hard-blocked):
t("fusion_search_cloud_files", "negative", {"query": "x"}, {"pred": "must_refuse"}, "hard-blocked: must refuse")
t("fusion_walk_cloud_tree", "negative", {}, {}, "expect refusal text")

# the mechanical layer's exe-side verbs: answered from the exe's own files and validation, so they
# never wait on Fusion and never change a document (the typed writes run in the mechanical drill)
t("fusion_mech_capabilities", "daily", {}, {"ok": True, "fields": ["operations", "idempotency", "sessionId"]}, "read-only: schemas and the live add-in session, from the exe's files")
t("fusion_mech_operation_status", "daily", {"sessionId": "verb-test", "operationId": "verb-test-unknown"}, {"errorCode": "operation_unknown"}, "read-only: an unknown operation is answered from the journal, never by asking Fusion")
t("fusion_mechanical", "negative", {"op": "no_such_operation"}, {"errorCode": "unsupported_operation"}, "refused by the exe's validation before Fusion is touched; typed writes are covered by the mechanical drill")

# ── stage 1: APS, no Fusion needed ───────────────────────────────────────────
t("fusion_aps_status", "daily", {}, {"ok": True})
t("fusion_aps_get_browser", "daily", {}, {"ok": True})
t("fusion_aps_search", "daily", {"query": "BQ25792", "limit": 5}, {"ok": True, "pred": "count_gt_0"})
t("fusion_aps_recent", "daily", {"limit": 5}, {"ok": True, "pred": "count_gt_0"})
t("fusion_aps_browse", "daily", {}, {"ok": True, "fields": ["hubs"]})
t("fusion_aps_file_info", "daily", {"query": "BQ25792"}, {"ok": True, "fields": ["versionCount"]})
t("fusion_aps_versions", "weekly", {"query": "BQ25792"}, {"ok": True, "fields": ["versionCount"]})
t("fusion_aps_download", "weekly", {"query": "BQ25792", "saveDir": SCRATCH}, {"ok": True})
t("fusion_aps_get", "daily", {"path": "/project/v1/hubs"}, {"ok": True})
t("fusion_uninstall", "daily", {"dryRun": True}, {"ok": True, "fields": ["removed", "skipped", "dryRun"]})
t("fusion_set_seat_takeover", "daily", {}, {"ok": True, "fields": ["autoTakeOverSeat"]}, "no-args form is a pure pref read")
# APS mutations: only into the AI workspace, weekly, with cleanup
t("fusion_aps_create_folder", "manual", note="needs projectId/parentFolderId of the test workspace")
t("fusion_aps_upload", "manual", note="cloud write; exercised via build flows weekly")
t("fusion_aps_set_client_id", "manual", note="would clobber org auth config")
t("fusion_aps_set_browser", "manual"); t("fusion_aps_forget_browser", "manual")
t("fusion_aps_signin", "manual", note="opens a browser on the user's machine")

# ── stage 2: Fusion lifecycle + prefs + MCP ─────────────────────────────────
t("fusion_start", "daily", {}, {"ok": True}, "runner restores prior running-state at the end")
t("fusion_get_preferences", "daily", {}, {"ok": True, "fields": ["current"]})
t("fusion_set_preference", "daily", {"theme": "darkblue"}, {"ok": True}, "sets the value it already has")
t("fusion_mcp_status", "daily", {}, {"ok": True, "fields": ["enabled"]})
t("fusion_mcp_tools", "weekly", {}, {"ok": True}, "only meaningful when MCP enabled")
t("fusion_mcp_resources", "weekly", {}, {"ok": True})
t("fusion_mcp_enable", "manual", note="drives the prefs UI, takes foreground")
t("fusion_prefs_open", "manual", note="opens a dialog on the user's screen")
t("fusion_prefs_close", "manual")
t("fusion_set_auto_update", "daily", {}, {"ok": True}, "no args = read-only")
t("fusion_assembly_bom", "daily", {}, {"pred": "ok_or_budget"},
  "either a BOM (small doc) or the fast bom_walk_budget_exceeded refusal (3D PCB) - both are the verb working as designed")
t("fusion_dismiss_blocking_dialogs", "daily", {}, {"ok": True}, "no-op when nothing is blocking")

# ── stage 3: document flows on the canonical board ──────────────────────────
# ORDER IS THE TEST: 2D-context verbs (Electron.*, gerbers/BOM/CPL, stackup,
# detect_layers) run while the 2D board view is ACTIVE, before show_3d_board
# switches away. The 30-fail weekly round of 2026-08-14 was mostly this.
# The battery runs on fusion-lane's COPY of BQ25792 in Default Project/fusion-lane-tests, never on the
# owner's design (Colby, 2026-10-28: "leave those designs to owner ... only make new stuff or make
# copies of existing stuff to mess with"). Refresh the copy with fusion_save_to_cloud {asCopy}.
DESIGN = "lane-BQ25792-copy"
OPEN = {"fileName": DESIGN, "projectName": "Default Project", "folderPath": "fusion-lane-tests"}
t("fusion_open_cloud_file", "daily", dict(OPEN, **{"_pollDoc": DESIGN, "_pollSec": 180}), {"ok": True}, "fire-and-poll: cloud opens outlive the relay timeout")
t("fusion_show_schematic", "daily", {}, {"ok": True})
t("fusion_show_2d_board", "daily", {}, {"ok": True})
t("fusion_board_info", "daily", {}, {"ok": True, "fields": ["rawXml"]}, "needs the 2D board view ACTIVE")
t("fusion_open_electronics", "daily", {}, {"ok": True, "fields": ["isElectronics"]}, "no-args form is a pure workspace read; 2D board view active")
# 1.12.0: the EDA Bridge Standard reads, on the canonical board, 2D view active. All read-only:
# they export the board in the background and change nothing (run_drc, repour and the saves write,
# so they stay out of the daily run on a shared cloud design).
B = {"expectDocument": DESIGN}
t("fusion_routing_state", "daily", B, {"ok": True, "pred": "board_model_ok"}, "the standard board model: pads sized, copper layers named, unconnected from the copper")
t("fusion_layer_stack", "daily", B, {"ok": True, "fields": ["layers", "totalUm", "copperLayers"]})
t("fusion_netlist", "daily", B, {"ok": True, "fields": ["nets", "netCount"]})
t("fusion_pour_state", "daily", B, {"ok": True, "fields": ["zones", "filledAreaByLayerMm2"]})
t("fusion_component_data", "daily", dict(B, designator="IC1"), {"ok": True, "fields": ["component"]})
t("fusion_component_pins", "daily", dict(B, designator="IC1"), {"ok": True, "fields": ["pins", "count"]})
t("fusion_list_nets", "daily", B, {"ok": True, "fields": ["nets", "count"]})
t("fusion_net_connections", "daily", dict(B, net="VBUS"), {"ok": True, "fields": ["pads", "nets"]})
t("fusion_list_designators", "daily", B, {"ok": True, "fields": ["designators", "count"]})
t("fusion_layers", "daily", B, {"ok": True, "fields": ["layers"]})
t("fusion_copper_layers", "daily", B, {"ok": True, "fields": ["layers"]})
t("fusion_layers_in_use", "daily", B, {"ok": True, "fields": ["copperLayersInUse"]})
t("fusion_read_rules", "daily", B, {"ok": True, "fields": ["rules", "ruleSet"]})
t("fusion_rules_by_kind", "daily", B, {"ok": True, "fields": ["rulesByKind"]})
t("fusion_silk_state", "daily", B, {"ok": True, "fields": ["components", "texts"]})
t("fusion_net_classes", "daily", B, {"ok": True, "fields": ["classes"]})
t("fusion_board_summary", "daily", {}, {"ok": True, "fields": ["aliasOf"]}, "EDA Bridge Standard alias of fusion_board_info")
t("fusion_zoom_to", "daily", dict(B, ref="IC1"), {"ok": True, "fields": ["rect"]}, "frames the view; changes nothing")
t("fusion_select_net", "daily", dict(B, net="VBUS", zoom=True), {"ok": True, "fields": ["selected", "byKind", "zoom"]}, "highlights the net (SHOW left waiting for a click) and frames it; changes nothing")
t("fusion_set_layer_visibility#hide", "daily", dict(B, layerId="B.Cu", displayed=False), {"ok": True, "fields": ["displayed"]}, "read back from the board's layer flags")
t("fusion_set_layer_visibility#show", "daily", dict(B, layerId="B.Cu", displayed=True), {"ok": True, "fields": ["displayed"]})
t("fusion_board_outline", "daily", B, {"ok": True, "fields": ["alreadyThere", "outlineClosed"]}, "Fusion's edge is the Dimension layer: reports it, draws nothing")
t("fusion_run_erc", "daily", B, {"ok": True, "fields": ["messages", "messageCount", "byClass"]}, "brings the design's schematic tab forward for the ERC and puts the board back")
t("fusion_status", "daily", {}, {"ok": True, "fields": ["aliasOf"]}, "EDA Bridge Standard alias of fusion_readiness")
t("fusion_check_dialogs", "daily", {}, {"ok": True, "fields": ["dialogs", "count"]}, "UI Automation read; never touches the add-in")
for v, own in (("fusion_dialog_log", "fusion_action_ledger"), ("fusion_signin_status", "fusion_account"), ("fusion_license_status", "fusion_account")):
    t(v, "daily", {}, {"ok": True, "fields": ["aliasOf"]}, "EDA Bridge Standard alias of " + own)
t("fusion_list_commands", "daily", {"filter": "Electron"}, {"ok": True, "fields": ["aliasOf", "commands"]}, "alias of fusion_list_text_commands")
for v in ("fusion_save", "fusion_run_script", "fusion_run_command", "fusion_open_document", "fusion_show_3d_chip"):
    t(v, "manual", note="EDA Bridge Standard alias: the same verb as its target, which carries the tests")
t("fusion_sch_items", "daily", B, {"ok": True, "fields": ["wires", "labels", "junctions", "counts"]}, "the design's schematic, exported in the background from the board tab")
t("fusion_sch_pins", "daily", dict(B, references=["IC1"]), {"ok": True, "fields": ["pins", "count"]})
t("fusion_sch_trace_point", "daily", dict(B, at=[40.64, 132.08]), {"ok": True, "fields": ["items", "nets"]}, "a VBUS junction on BQ25792")
t("fusion_sch_page", "daily", B, {"ok": True, "fields": ["titleBlock"]})
t("fusion_sch_hierarchy", "daily", B, {"ok": True, "fields": ["tree", "modules"]})
t("fusion_sch_sheet_pins", "daily", dict(B, validate=True), {"ok": True, "fields": ["pins", "problems"]})
t("fusion_sch_connect_pins#dry", "daily", dict(B, pairs=[{"from": "IC1.1", "to": "IC1.2"}], dryRun=True), {"ok": True, "fields": ["routes", "commands"]}, "plan only: changes nothing")
t("fusion_sch_fix_connectivity", "daily", B, {"ok": True, "fields": ["moves", "unresolved"]}, "dryRun is its default")
t("fusion_render_schematic", "daily", dict(B, name="verbtest", width=1200), {"ok": True, "fields": ["png", "width", "dpi"]}, "EXPORT IMAGE of sheet 1, no window")
t("fusion_sch_update_from_library#refuse", "negative", B, {"pred": "must_refuse"}, "Fusion's Library Manager asks per part: refused with the reason")
t("fusion_sch_batch#dry", "daily", dict(B, dryRun=True, ops=[{"op": "sch_text", "texts": [{"text": "FL BATCH", "at": [5, 5]}]}]), {"ok": True, "fields": ["results", "revision"]}, "the standard's sch_batch, planned only")
t("fusion_sch_batch#rollback", "daily", dict(B, ops=[{"op": "sch_text", "texts": [{"text": "FL BATCH", "at": [5, 5]}]}, {"op": "sch_text", "texts": [{"text": ""}]}]), {"errorCode": "batch_failed"}, "op 0 writes a text, op 1 is refused: the batch undoes op 0 (the copy ends at its revision before)")
for v in ("fusion_sch_text", "fusion_sch_junction", "fusion_sch_delete_wire", "fusion_sch_connect_pins", "fusion_sch_split_wire", "fusion_sch_bus",
          "fusion_sch_edit_label", "fusion_sch_move_region", "fusion_sch_bus_entry", "fusion_sch_connect_to_bus", "fusion_sch_reset_fields", "fusion_sch_create"):
    t(v, "manual", note="schematic edit: tested live on a scratch import of BQ25792 (never the cloud design)")
t("fusion_update_board_from_eagle#guard", "negative", dict(B, brdPath="C:/nonexistent/none.brd"), {"pred": "must_refuse"}, "a missing file is refused before anything changes")
t("fusion_update_board_from_eagle", "manual", note="writes a saved design: tested live on fl-sync-test in Default Project / fusion-lane-tests (#125 item 4)")
t("fusion_export_dxf#board", "daily", dict(B, layers=["Edge.Cuts", "F.Cu", "F.SilkS"], outputDir=f"{SCRATCH}/dxf"), {"ok": True, "fields": ["files"]}, "board layers to DXF (the standard's export_dxf)")
t("fusion_plan_route#standard", "daily", dict(B, net="N$15", layers=["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"], gridMm=0.25, **{"from": "SW1.2", "to": "JP5.P1"}), {"ok": True, "fields": ["found", "routeNet", "planner", "engine"]}, "the shared engine plans on all four of BQ25792's layers (read only)")
t("fusion_length_tune", "daily", dict(B, net="SCL", targetMm=55.6), {"ok": True, "fields": ["remove", "routeNet", "engine"]}, "a plan only (D_P has no room on this board; SCL does)")
t("fusion_route_pair", "manual", note="a plan; BQ25792's D pair is routed, so a pair plan meets its own copper")
t("fusion_render_board_3d", "daily", dict(B, side="iso", width=800, height=500, outputPath=f"{SCRATCH}/board-iso.png"), {"ok": True, "fields": ["path"]}, "shows the 3D PCB itself when it is not open, then puts the board tab back")
t("fusion_remove_graphics#dry", "daily", dict(B, layer="F.SilkS", kinds=["text"], dryRun=True), {"ok": True, "fields": ["removed", "count"]}, "lists ids, changes nothing")
t("fusion_align_parts#dry", "daily", dict(B, refs=["C19", "C20", "C21"], mode="top", dryRun=True), {"ok": True, "fields": ["targets"]})
t("fusion_remove_parts#dry", "daily", dict(B, refs=["R2"], dryRun=True), {"ok": True, "fields": ["removed"]}, "plan only")
t("fusion_edit_parts#dry", "daily", dict(B, edits=[{"ref": "R2", "value": "0.01"}], dryRun=True), {"ok": True, "fields": ["plan"]}, "plan only")
t("fusion_generate_body#engine", "daily", {"family": "qfn", "dimensionsMm": {"pins": 32, "D": 5.0, "E": 5.0, "A": 0.9, "e": 0.5, "D2": 3.3, "E2": 3.3}, "outputPath": "C:/Users/olive/adom-tests/verb-tests/QFN32.step"}, {"ok": True, "fields": ["stepPath", "bboxMm", "engine"]}, "the shared writer: no Fusion, writes a STEP")
for v in ("fusion_edit_parts", "fusion_align_parts", "fusion_remove_graphics", "fusion_remove_parts", "fusion_generate_body", "fusion_library_views"):
    t(v, "manual", note="edits a design, writes files or closes documents: tested live on scratch imports and fl-sync-test")
t("fusion_copper_balance", "daily", B, {"ok": True, "fields": ["pairs", "engine"]}, "the shared checks engine (adom-eda-checks, vendored)")
t("fusion_return_path", "daily", B, {"ok": True, "fields": ["engine"]})
t("fusion_impedance", "daily", dict(B, z0=50), {"ok": True, "fields": ["layers"]})
t("fusion_current_capacity", "daily", dict(B, nets={"VBUS": 3}), {"ok": True, "fields": ["nets"]})
t("fusion_fab_check", "daily", dict(B, vendor="jlcpcb"), {"ok": True, "fields": ["measured", "outsideLimits", "fab"]}, "fetches JLCPCB's record from adom/pcb-design-rules")
t("fusion_estimate_cost", "daily", dict(B, quantities=[5, 30], assembly=True), {"ok": True, "fields": ["prices", "source", "engine"]})
t("fusion_read_board", "daily", dict(B, sections="components,nets,rules,outline", limit=5), {"ok": True, "fields": ["data", "counts"]})
t("fusion_diff_pairs", "daily", dict(B), {"ok": True, "fields": ["pairs"]}, "BQ25792 pairs D_P/D_N by name")
t("fusion_auto_classes", "daily", dict(B), {"ok": True, "fields": ["proposal", "applyArgs", "engine"]}, "a proposal: nothing is written")
t("fusion_silk_designators", "daily", dict(B, dryRun=True), {"ok": True, "fields": ["planned", "applyArgs", "engine"]}, "dryRun: a plan only")
t("fusion_net_lengths", "daily", dict(B), {"ok": True, "fields": ["nets", "pairs", "engine"]})
t("fusion_assembly_check", "daily", dict(B), {"ok": True, "fields": ["economic", "standard", "engine"]})
t("fusion_review_board", "daily", dict(B), {"ok": True, "fields": ["counts", "engine"]})
t("fusion_board_report", "daily", dict(B, outputDir="C:/Users/olive/adom-tests/report"), {"ok": True, "fields": ["path", "engine"]}, "writes one HTML page; Fusion stores its DRC result in the board")
t("fusion_jlcpcb_order", "daily", dict(B, outputDir="C:/Users/olive/adom-tests/jlc-order"), {"ok": True, "fields": ["ready", "issues", "gerberZip", "bomFiles", "orderForm"]}, "writes the order folder; BQ25792 is not ready (13 BOM lines without LCSC)")
t("fusion_sch_zoom", "daily", dict(B, refs=["IC1", "C11"]), {"ok": True, "fields": ["found", "rectMm", "image"]}, "SHOW + WINDOW + EXPORT IMAGE WINDOW: nothing changes")
t("fusion_export_spice", "daily", dict(B), {"ok": True, "fields": ["path", "elements", "unmapped"]}, "EAGLE's EXPORT SPICENETLIST: BQ25792's ICs have no SPICE mapping")
t("fusion_simulate", "daily", dict(B, analysis={"type": "op"}), {"ok": True, "fields": ["signals", "notSimulated", "engine"]}, "BQ25792 is partly modelled: it runs and names the 89 parts it left out (standard 0.21.1)")
t("fusion_simulate#noise", "daily", {"netlistPath": "C:/Users/olive/Documents/fusion-lane-tests/rc.cir", "analysis": {"type": "noise", "source": "V1", "output": "OUT", "sweep": "dec", "points": 10, "start": 10, "stop": 1e6}}, {"ok": True, "fields": ["signals", "totals"]}, "the RC's output noise: sqrt(4kTR) at low frequency, about sqrt(kT/C) integrated")
t("fusion_simulate#montecarlo", "daily", {"netlistPath": "C:/Users/olive/Documents/fusion-lane-tests/rc.cir", "analysis": {"type": "ac", "sweep": "dec", "points": 20, "start": 10, "stop": 1e6}, "probes": [{"net": "OUT"}], "monteCarlo": {"runs": 3, "distribution": "gaussian", "seed": 7, "tolerances": {"resistor": 1, "capacitor": 10}}}, {"ok": True, "fields": ["signals", "runs"]}, "3 seeded Monte Carlo runs, the drawn values in each signal's step")
t("fusion_simulate#sweep", "daily", {"netlistPath": "C:/Users/olive/Documents/fusion-lane-tests/rc.cir", "analysis": {"type": "ac", "sweep": "dec", "points": 20, "start": 10, "stop": 1e6}, "probes": [{"net": "OUT"}], "sweep": {"parameter": "R1", "start": 1000, "stop": 4000, "step": 1000}, "temperatures": {"start": 25, "stop": 85, "step": 60}}, {"ok": True, "fields": ["signals", "runs"]}, "an RC low-pass: R1 x temperature, 8 runs, each its own signal with step")
for v in ("fusion_sch_place_part", "fusion_sch_connect", "fusion_sch_swap_part"):
    t(v, "manual", note="edits the schematic (and the board with it): tested live on lane-BQ25792-copy with a part placed and deleted again")
t("fusion_add_rule", "manual", note="edits the board's net classes: tested live on lane-BQ25792-copy and put back")
t("fusion_height_check", "daily", dict(B, maxTopMm=8, minGapMm=0.1), {"ok": True, "fields": ["heights", "noBody", "engine"]}, "reads fusion_bodies (switches to the 3D PCB and back)")
t("fusion_placement_check", "daily", dict(B, marginMm=0.1), {"ok": True, "fields": ["parts", "courtyardOverlaps", "engine"]})
t("fusion_find_space", "daily", dict(B, near="IC1", sizeMm=[1, 0.5], count=3, radiusMm=25), {"ok": True, "fields": ["spots", "engine"]}, "a 2 x 1.25 mm spot has none within 10 mm of IC1 on this dense board")
t("fusion_check_orientation", "daily", dict(B), {"ok": True, "fields": ["engine"]})
t("fusion_check_clearance", "daily", dict(B, mode="pads", thresholdMm=0.2), {"ok": True, "fields": ["engine"]})
t("fusion_place_decoupling", "daily", dict(B, ic="IC1"), {"ok": True, "fields": ["engine"]}, "a plan only: nothing moves")
t("fusion_plan_fanout", "daily", dict(B, ref="IC1"), {"ok": True, "fields": ["calls", "conflicts", "engine"]}, "a plan only: nothing is drawn")
t("fusion_export_odb", "daily", dict(B, outputDir=f"{SCRATCH}/odb"), {"ok": True, "fields": ["path", "layers", "nets"]}, "Fusion's own ODB++ tree, found by its matrix and zipped")
t("fusion_export_ipc356", "daily", dict(B, outputDir=f"{SCRATCH}/ipc356"), {"ok": True, "fields": ["path", "records"]})
t("fusion_export_gencad", "daily", dict(B, outputDir=f"{SCRATCH}/gencad"), {"ok": True, "fields": ["path", "components", "signals"]})
t("fusion_export_ipc2581", "daily", dict(B, outputDir=f"{SCRATCH}/ipc2581"), {"ok": True, "fields": ["path", "components", "pourIslands"]})
t("fusion_read_footprint", "daily", dict(B, libId="Capacitor:CAPC2012X110"), {"ok": True, "fields": ["pads", "courtyardMm", "layers"]})
t("fusion_project_info", "daily", dict(B), {"ok": True, "fields": ["files", "board", "sheets"]})
t("fusion_schematic_data", "daily", dict(B, designator="C11", params=True), {"ok": True, "fields": ["components", "count"]})
t("fusion_part_parameters", "daily", dict(B), {"ok": True, "fields": ["names"]})
t("fusion_cross_check", "daily", dict(B), {"ok": True, "fields": ["matched", "onSheetNotOnBoard", "onBoardNotOnSheet"]})
t("fusion_change_vias#guard", "negative", {"expectDocument": "ZZZ_NoSuchDoc_TEST", "filter": {"drill": 0.2}, "set": {"drill": 0.3}}, {"pred": "must_refuse"}, "same guards as the other edits")
for v in ("fusion_sch_label", "fusion_sch_wire", "fusion_edit_tracks", "fusion_place_parts", "fusion_change_vias"):
    t(v, "manual", note="edits a design: run live by tests/sch_drill.py (place_parts and a track's end on a board with no schematic by hand)")
t("fusion_model_check", "daily", dict(B), {"ok": True, "fields": ["parts", "checked", "ok"]}, "does each part's cloud 3D model open, read only")
t("fusion_extensions", "daily", {}, {"ok": True, "fields": ["extensions", "count"]}, "the add-ins installed for this user")
t("fusion_update_pcb", "daily", dict(B), {"ok": True, "fields": ["executed", "preview", "changeOrder"]}, "the standard's update_pcb: the preview, nothing to execute")
t("fusion_preferences", "daily", {}, {"ok": True}, "the standard's preferences, read only here")
t("fusion_select_items", "negative", {}, {"errorCode": "bad_args"}, "refs or clear is required")
t("fusion_schematic_review", "daily", dict(B), {"ok": True, "fields": ["engine"]})
t("fusion_pin_pad_check", "daily", dict(B), {"ok": True, "fields": ["engine"]})
t("fusion_doc_params", "daily", dict(B), {"ok": True, "fields": ["params"]})
t("fusion_sheet_params", "daily", dict(B, designators=["C11"], params={"ADOM_TEST": "1"}, dryRun=True), {"ok": True})
t("fusion_sch_edit", "daily", dict(B, edits=[{"designator": "C11", "action": "rotate", "degrees": 90}], dryRun=True), {"ok": True})
t("fusion_annotate", "daily", dict(B, dryRun=True), {"ok": True, "fields": ["planned"]})
t("fusion_thermal_vias", "daily", dict(B, ref="L2", pad="2", dryRun=True), {"ok": True, "fields": ["planned", "apply", "engine"]})
t("fusion_via_fence", "daily", dict(B, nets=["D_P", "D_N"], viaNet="PGND", dryRun=True), {"ok": True, "fields": ["planned", "apply", "engine"]})
t("fusion_board_features", "daily", dict(B, stitching={"net": "PGND", "pitch": 4}, dryRun=True), {"ok": True, "fields": ["planned", "results"]})
t("fusion_thermal_vias", "negative", dict(B, ref="IC1"), {"errorCode": "pad_too_small"}, "IC1 has no exposed pad: refused before anything is drawn")
t("fusion_footprint_primitives", "daily", dict(B, designator="C11"), {"ok": True, "fields": ["primitives", "pattern"]})
t("fusion_export_pdf", "daily", dict(B, output=f"{SCRATCH}/pdf/design.pdf"), {"ok": True, "fields": ["pdfPath", "sheets"]})
t("fusion_project_rename", "negative", {"project": "Main/Molecules/BQ25792", "newName": "X"}, {"errorCode": "not_supported"}, "a cloud design is renamed in Fusion's data panel, not by path")
t("fusion_project_create", "daily", {"parentDir": "C:/Users/olive/AppData/Local/Temp/fusion-lane", "name": "lane-new", "layers": 4, "dryRun": True}, {"ok": True, "fields": ["plan"]})
t("fusion_project_snapshot", "daily", dict(B, label="daily", outputDir=f"{SCRATCH}/snapshots"), {"ok": True, "fields": ["folder", "files", "pdfs"]})
t("fusion_cross_probe", "daily", dict(B, ref="C11"), {"ok": True, "fields": ["matches", "symbolWithoutPart"]})
t("fusion_set_layer_count", "daily", dict(B, count=6, dryRun=True), {"ok": True, "fields": ["stackup", "to"]})
t("fusion_set_layer_count", "negative", dict(B, count=2), {"errorCode": "layers_in_use"}, "BQ25792's inner layers carry pours and tracks")
t("fusion_copy_routing", "daily", dict(B, box=[20, 13, 30, 16], offset=[0, -10], dryRun=True), {"ok": True, "fields": ["copied", "skipped"]})
t("fusion_copy_routing", "negative", dict(B, box=[20, 13, 30, 16], offset=[0, -10], keepUnmappedNets=True), {"errorCode": "destination_conflict"}, "the copy would land on other nets' pads: refused before drawing")
t("fusion_import_svg", "daily", dict(B, svg='<svg xmlns="http://www.w3.org/2000/svg" width="4mm" height="2mm" viewBox="0 0 40 20"><rect width="40" height="20"/></svg>', at=[2, 24], dryRun=True), {"ok": True, "fields": ["polygons", "sizeMm"]})
t("fusion_import_svg", "negative", dict(B, svg='<svg xmlns="http://www.w3.org/2000/svg" width="4mm" height="2mm" viewBox="0 0 40 20"><rect width="40" height="20"/></svg>', at=[2, 24], layer="F.Cu"), {"errorCode": "not_supported"}, "copper is refused before any command runs (no Signal Name modal)")
t("fusion_read_footprint", "negative", {"libraryPath": "C:/nope/missing.lbr", "name": "X"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file")
t("fusion_export_bom#parts", "daily", {"outputPath": f"{SCRATCH}/bom-parts.csv"}, {"ok": True, "pred": "bom_has_part_numbers"}, "1.12.0: MPN/LCSC columns from the part attributes (page issue #105)")
t("fusion_list_text_commands", "daily", {"filter": "Electron"}, {"ok": True, "fields": ["commands"]}, "TextCommands.List needs the electronics editor context")
t("fusion_export_source", "weekly", {"outputPath": f"{SCRATCH}/verbtest.fbrd"}, {"ok": True, "fields": ["savedTo"]}, ".fbrd needs the 2D board view ACTIVE (Document.CopyToDesktop)")
t("fusion_export_eagle_source", "weekly", {"outputPath": f"{SCRATCH}/verbtest.brd"}, {"ok": True, "fields": ["savedTo"]}, ".brd needs the 2D board view ACTIVE; extracts the EAGLE XML from the .fbrd container")
t("fusion_export_board_image", "weekly", {"outputPath": f"{SCRATCH}/verbtest-assembly-top.png", "preset": "assembly_top"}, {"ok": True, "fields": ["outputPath"]}, "EXPORT IMAGE needs the 2D board view ACTIVE")
t("fusion_execute_text_command", "daily", {"command": "Electron.ImmediateRepaint /full", "expectDocument": DESIGN}, {"ok": True},
  "Electron.* needs the ELECTRONICS editor context - must run in the 2D stage")
t("fusion_electron_run", "weekly", {"command": "WINDOW FIT;"}, {"ok": True}, "2D board view active")
t("fusion_electron_zoom", "weekly", {"factor": 1.0}, {"ok": True}, "2D board view active")
t("fusion_electron_pan", "weekly", {"x1": 0, "y1": 0, "x2": 5, "y2": 5}, {"ok": True}, "board-mm rectangle, 2D view active")
t("fusion_detect_layers", "weekly", {}, {"ok": True}, "2D board view active")
t("fusion_export_gerbers", "weekly", {"outputDir": SCRATCH}, {"ok": True}, "2D board view active")
t("fusion_export_bom", "weekly", {"outputPath": f"{SCRATCH}/bom.csv"}, {"ok": True}, "2D board view active")
t("fusion_export_cpl", "weekly", {"outputPath": f"{SCRATCH}/cpl.csv"}, {"ok": True}, "2D board view active")
t("fusion_board_stackup", "weekly", {}, {"ok": True}, "exports the .brd - needs the board context")
t("fusion_bodies", "daily", {"expectDocument": DESIGN}, {"ok": True, "fields": ["bodies", "noBody", "suspect"]}, "switches to the 3D PCB and back to the 2D board")
t("fusion_show_3d_board", "daily", {}, {"ok": True})
t("fusion_inspect_bodies", "daily", {}, {"ok": True, "fields": ["bodies", "units"]})
t("fusion_mcp_call", "weekly", {"tool": "fusion_mcp_read", "arguments": {"queryType": "document"}}, {"ok": True},
  "MCP reads the ACTIVE document - must run after the board opens; queryType is a schema enum (document|projects|...)")
t("fusion_assembly_bom#realboard", "weekly", {}, {"pred": "ok_or_budget"},
  "on the open BQ25792 3D PCB the budget guard must refuse FAST - the grind was issue #659")
t("fusion_physical_properties", "daily", {}, {"ok": True})
t("fusion_get_design_info", "daily", {}, {"ok": True})
t("fusion_get_parameters", "daily", {}, {"ok": True})
t("fusion_document_info", "daily", {}, {"ok": True})
t("fusion_list_cloud_projects", "weekly", {}, {"ok": True}, "slow hub walk")
t("fusion_list_cloud_files", "weekly", {"projectName": "Main"}, {"ok": True})
t("fusion_check_recovery", "weekly", dict(OPEN), {"ok": True, "fields": ["hasRecovery"]}, "best-effort API read on the canonical board; never mutates")
t("fusion_delete_cloud_file#guard", "negative", {}, {"pred": "must_refuse"}, "fileName or fileId required - refusing a blanket delete IS the safety rail")
t("fusion_delete_cloud_file", "manual", note="destructive cloud delete; only against files a rehearsal created")
t("fusion_create_cloud_folder", "manual", note="cloud write into the team hub")
t("fusion_set_parameter#guard", "negative", {"name": "ZZZ_NoSuchParam_TEST", "expression": "1 mm"}, {"pred": "must_refuse"}, "unknown parameter must refuse (nothing mutated)")
t("fusion_set_parameter", "manual", note="mutates the active design's parameters")
# THE GUARD (issue #289): mismatch must refuse - on a READ verb too
t("fusion_run_modeling_script#guard", "negative",
  {"script": "result=1", "expectDocument": "ZZZ_NoSuchDoc_TEST"}, {"errorCode": "wrong_document"})
t("fusion_assembly_bom#guard", "negative",
  {"expectDocument": "ZZZ_NoSuchDoc_TEST"}, {"errorCode": "wrong_document"})
t("fusion_run_modeling_script", "daily",
  {"script": "result = {'ok': True}", "expectDocument": DESIGN}, {"ok": True})
t("fusion_electron_select", "manual", note="selection state is user-visible")
t("fusion_open_by_urn", "weekly", note="URN fetched from aps_search at runtime", args={"_dynamic": "urn_from_search"})
t("fusion_aps_open", "weekly", {"query": "BQ25792", "wait": False}, {"ok": True})
t("fusion_open_design", "manual", note="local-file open; superseded by cloud flows")
t("fusion_open_board", "manual", note="local .brd open; needs a .brd on the box (make_test_project exercises the same newDesignFromLocal path)")
t("fusion_open_schematic", "manual", note="local .sch open; needs a .sch on the box (make_test_project exercises the same newDesignFromLocal path)")
t("fusion_import_electronics", "manual", note="needs a Fusion-native .fsch/.fbrd/.flbr on the box; creates a NEW unsaved document")
t("fusion_close_window#noargs", "negative", {}, {"pred": "must_refuse"}, "dialog closer; hwnd required - refusal IS the pass")
t("fusion_close_document", "manual"); t("fusion_close_all_documents", "manual", note="would close the user's own tabs")

# ── stage 4: 3D-design exports (local writes to scratch; 3D PCB doc active) ──
t("fusion_export_step", "weekly", {"outputPath": f"{SCRATCH}/verbtest.step"}, {"ok": True})
t("fusion_export_stl", "weekly", {"outputPath": f"{SCRATCH}/verbtest.stl"}, {"ok": True})
t("fusion_export_3mf", "weekly", {"outputPath": f"{SCRATCH}/verbtest.3mf"}, {"ok": True})
t("fusion_export_f3d", "weekly", {"outputPath": f"{SCRATCH}/verbtest.f3d"}, {"ok": True})
t("fusion_export_iges", "weekly", {"outputPath": f"{SCRATCH}/verbtest.iges"}, {"ok": True})
t("fusion_export_sat", "weekly", {"outputPath": f"{SCRATCH}/verbtest.sat"}, {"ok": True})
t("fusion_export_cloud_file", "weekly", {"outputPath": f"{SCRATCH}/verbtest-cloud.step", "format": "step"}, {"ok": True, "fields": ["outputPath"]}, "exports the ACTIVE design despite the name; 3D PCB doc active")
for fmt in ("obj", "fbx", "usdz", "dxf", "dwg", "skp"):
    t(f"fusion_export_{fmt}", "manual", note="build/doc-type dependent (OBJ refused on the 2026-08-14 box); verify quarterly")
t("fusion_export_optimized_glb", "weekly", {"outputPath": f"{SCRATCH}/board.glb", "async": True}, {"ok": True, "fields": ["exportId"]},
  "async form: sync outlived ab's 60s relay window on a cold session (2026-08-14)")
t("fusion_fetch_optimized_glb", "weekly", {"_dynamic": "glb_job"}, {"ok": True, "fields": ["glbPath"]},
  "polls the async exportId to completion - the final status must carry the GLB")
t("fusion_set_design_rules", "manual", note="mutates the board's DRC rules")

# ── stage 5: libraries (weekly) + the show verbs (issue #29) ────────────────
t("fusion_open_lbr", "weekly", {"_dynamic": "staged_lbr"}, {"ok": True})
t("fusion_export_lbr", "weekly", {"outputPath": f"{SCRATCH}/verbtest.scr"}, {"ok": True, "fields": ["outputPath"]}, "EXPORT SCRIPT of the library open_lbr just put in front")
t("fusion_capture_library_views", "weekly", {"packages": ["ADS8588SIPM"]}, {"ok": True}, "after open_lbr; packages arg required")
t("fusion_show_library", "weekly", {"name": "ADS8588SIPM"}, {"ok": True}, "already-open case right after open_lbr")
t("fusion_show_symbol", "weekly", {"name": "ADS8588SIPM"}, {"ok": True}, "library already open from open_lbr")
t("fusion_show_footprint", "weekly", {"name": "ADS8588SIPM"}, {"ok": True})
t("fusion_show_symbol#wrap", "weekly",
  {"symbolXml": '<symbol name="ADOMTESTSYM"><wire x1="-5.08" y1="5.08" x2="5.08" y2="5.08" width="0.254" layer="94"/>'
                '<wire x1="5.08" y1="5.08" x2="5.08" y2="-5.08" width="0.254" layer="94"/>'
                '<wire x1="5.08" y1="-5.08" x2="-5.08" y2="-5.08" width="0.254" layer="94"/>'
                '<wire x1="-5.08" y1="-5.08" x2="-5.08" y2="5.08" width="0.254" layer="94"/>'
                '<pin name="IN" x="-10.16" y="2.54" length="middle"/>'
                '<pin name="OUT" x="10.16" y="-2.54" length="middle" rot="R180"/>'
                '<text x="-5.08" y="5.842" size="1.778" layer="95">&gt;NAME</text></symbol>'},
  {"ok": True}, "bare-symbol wrapping: the bridge generates the library around the fragment (John, 2026-08-14)")
t("fusion_generate_package", "weekly", {"type": "chip", "params": {"D": 1.6, "E": 0.8, "A": 0.45}}, {"ok": True})
t("fusion_show_3d_package", "manual", note="enters Package3DEnvironment (exit needs Package3DStop + dialog); tour covers it; attach path in release rehearsals")
t("fusion_bind_library_3d", "manual", note="cloud f3d mint per run; release rehearsals (needs signed-in session)")
t("fusion_attach_3d_package", "manual", note="modal Save dialog needs a human eye")
t("fusion_make_3d_package", "manual", note="cloud upload; run in release rehearsals")
t("fusion_build_library_3d", "manual", note="long multi-part build; release rehearsals")
t("fusion_cleanup_cloud_files#guard", "negative", {"dryRun": True}, {"pred": "must_refuse"},
  "fileIds required - refusing a blanket cleanup IS the safety rail")
t("fusion_cleanup_cloud_files", "manual", note="real cleanup takes explicit lineage urns; used by weekly build rehearsals")
t("fusion_new_electronics_from_eagle", "manual", note="drives native dialogs")

# ── stage 6: doc-creating flows LAST (they change the active document) ──────
t("fusion_import_step", "weekly", {"_dynamic": "staged_step"}, {"ok": True},
  "creates a NEW design doc - must run after every active-doc-dependent test")
t("fusion_make_test_project", "weekly", {"_dynamic": "staged_lbr_mtp"}, {"ok": True, "fields": ["schPath", "brdPath"]},
  "from-scratch project generation around the staged sample part (John's no-BQ25792 rule)")
t("fusion_screenshot_fusion", "daily", {}, {"ok": True, "fields": ["savedTo"]}, "background main-window capture")
t("fusion_state", "daily", {}, {"ok": True, "fields": ["running"]},
  "the render-me verb (#28/#30): window+dialog+busy state + background screenshot; must answer even mid-wedge")
t("fusion_show_project", "weekly", {}, {"ok": True, "fields": ["document"]},
  "no-args form: activates the generated test project from make_test_project just above")
t("fusion_demo#tour", "weekly", {"mode": "tour", "includeDesign": False, "_pollTour": 300}, {"ok": True},
  "the six-content-type showcase (issue #29): async start + progress polls until done")
t("fusion_demo_progress#unknown", "negative", {"demoId": "tour-nope"}, {"pred": "must_refuse"})

# ── never in automation ──────────────────────────────────────────────────────
for v in ("fusion_stop", "fusion_kill", "fusion_signin", "fusion_signin_2fa", "fusion_demo",
          "fusion_install_fusion", "fusion_notify_owner", "fusion_open_by_urn_note"):
    t(v, "manual", note="lifecycle/human-wall verbs; manual tier")

# ── triage of 2026-09-03 (the verbs the daily run flagged as untriaged) ──────────────────
# Read-only bridge/state verbs run daily; anything that drives Fusion's GUI or writes to the
# cloud is weekly (exercised by the drills) or manual with the reason recorded.
t("fusion_action_ledger", "daily", {"limit": 5}, {"ok": True})
t("fusion_addin_status", "daily", {}, {"ok": True})
t("fusion_alerts", "daily", {"sinceSec": 300}, {"ok": True, "fields": ["alerts"]})
t("fusion_panel", "daily", {}, {"ok": True, "fields": ["panels"]}, "read-only: Fusion's docked panels (Panels.List)")
t("fusion_panel#browser", "daily", {"name": "Browser"}, {"ok": True, "fields": ["panel", "verified"]}, "Panels.Expose then Panels.ValidateState EXPANDED; Browser is in every editor (Inspector only in the electronics ones)")
t("fusion_tool_update", "daily", {"action": "check"}, {"ok": True, "fields": ["updateOffered", "auto"]}, "read-only: Fusion's update offer, nothing clicked")
t("fusion_export_3d#vrml", "negative", {"format": "vrml", "outputPath": f"{SCRATCH}/board.wrl"}, {"errorCode": "not_supported"}, "a format Fusion has no exporter for is refused by name")
t("fusion_export_3d", "daily", dict(B, format="stl", outputPath=f"{SCRATCH}/board-3d.stl"), {"ok": True, "fields": ["path", "bytes"]}, "the design's 3D PCB as STL through the one export_3d verb")
t("fusion_edit_pours", "manual", note="writes pours: tested live on lane-BQ25792-copy (thermals and rank set, read back, set back)")
t("fusion_board_summary", "daily", B, {"ok": True, "fields": ["summary"]}, "the standard's summary shaped from read_board")
t("fusion_read_schematic", "daily", B, {"ok": True, "fields": ["components", "designators"]}, "the standard's read_schematic from schematic_data")
t("fusion_list_library", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/plate/plate-test.lbr", "kind": "pcb"}, {"ok": True, "fields": ["data", "kind"]}, "offline .lbr names")
t("fusion_export_pos", "daily", dict(B, fab="jlcpcb", outputDir=f"{SCRATCH}/pos"), {"ok": True, "fields": ["files", "parts"]}, "pick-and-place per side plus JLCPCB's CPL")
t("fusion_export_pos#gerber", "negative", dict(B, format="gerber"), {"errorCode": "not_supported"}, "no Gerber position file in Fusion: refused by name")
t("fusion_window_info", "daily", {}, {"ok": True, "fields": ["windows", "dialogs", "hasModalDialogs"]}, "read-only: windows and dialogs, no screenshots")
t("fusion_bom", "daily", B, {"ok": True, "fields": ["lines", "checks", "fittedParts"]}, "the standard's bom from board_bom, schematic_data and variants")
t("fusion_render_board_layers", "daily", dict(B, preset="top_copper", outputPath=f"{SCRATCH}/layers/top_copper.png"), {"ok": True, "fields": ["path", "widthPx"]}, "EXPORT IMAGE with the standard's preset")
t("fusion_render_board_layers#svg", "negative", dict(B, format="svg"), {"errorCode": "not_supported"}, "Fusion writes no svg: refused by name")
t("fusion_render_board_3d#framed", "daily", dict(B, side="top", refs=["IC1"], rotate="-45,0,45", outputPath=f"{SCRATCH}/render/framed.png"), {"ok": True, "fields": ["path", "framed"]}, "framed on a part and turned")
t("fusion_pcb_report", "daily", {"path": "C:/Users/olive/Documents/fusion-lane-tests/plate/plate-test.lbr"}, {"ok": True, "fields": ["data"]}, "offline .lbr report")
t("fusion_pcb_report#board", "daily", dict(B, want="board"), {"ok": True, "fields": ["data"]}, "the open board's report from read_board")
t("fusion_export_manufacturing", "manual", note="silkscreen prep moves designators on the board: tested live on lane-BQ25792-copy")
t("fusion_log_tail", "daily", {"sinceOffset": 0, "maxBytes": 20000, "errorsOnly": True}, {"ok": True, "fields": ["entries", "next", "truncated"]}, "read-only: Fusion's own log since an offset")
t("fusion_verb_times", "daily", {"limit": 20}, {"ok": True, "fields": ["verbs", "since"]})
t("fusion_live_feed", "daily", {"action": "poll"}, {"ok": True}, "one poll of the feed; no waiting")
t("fusion_prove_result", "daily", {}, {"okOrCode": "no_prove_run"}, "shape only: the last completed run, or an honest 'none yet'")
t("fusion_set_driving_badge", "daily", {}, {"ok": True}, "no-args form is a pure pref read")
t("fusion_set_window_sizing", "daily", {}, {"ok": True}, "no-args form is a pure pref read")
t("fusion_cancel_run", "negative", {"runId": "no-such-run"}, {"errorCode": "unknown_run_id"}, "expect an honest 'unknown runId'")
t("fusion_preflight_lbr", "negative", {"libraryPath": "C:/nope/missing.lbr"}, {"pred": "must_refuse"}, "expect a refusal naming the missing file")
t("fusion_library_parts", "negative", {"libraryPath": "C:/nope/missing.lbr"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file")
LIBNEW = "C:/Users/olive/AppData/Local/Temp/fusion-lane/battery/new.lbr"
t("fusion_create_footprint", "daily", {"libraryPath": LIBNEW, "name": "SOT-23", "pads": "1,-37,-40,24,28,rect,0;2,37,-40,24,28,rect,0;3,0,40,24,28,rect,0", "dryRun": True}, {"ok": True, "fields": ["data"]}, "dry run: a new library is planned and preflighted, nothing written")
BOARDLIB = "C:/Users/olive/Documents/fusion-lane-tests/boardgen.lbr"   # R0603, SOT23, QFN-16-3X3 (fusion-lane's)
BOARDDIR = "C:/Users/olive/AppData/Local/Temp/fusion-lane/battery"
t("fusion_footprint_from_land_pattern", "daily", {"family": "dual", "name": "SOIC-8", "pins": 8, "pitch": 1.27, "span": 5.4, "padLength": 1.55, "padWidth": 0.6, "body": {"w": 3.9, "l": 4.9}, "dryRun": True}, {"ok": True, "fields": ["padTable", "courtyardMm", "engine"]}, "the engine lays a SOIC-8 out; nothing written")
t("fusion_footprint_from_land_pattern#missing", "negative", {"family": "dual", "name": "SOIC-8", "pins": 8, "pitch": 1.27, "span": 5.4, "padWidth": 0.6, "dryRun": True}, {"pred": "must_refuse"}, "a missing land-pattern number is refused by name")
IMPORT_FP = 'META|SOT-23-5-LANE|SOT-23-5, 0.95 mm pitch\nPAD|1|-1.1|0.95|1.06|0.65|roundrect|0|top|25\nPAD|2|-1.1|0|1.06|0.65|roundrect|0|top|25\nPAD|3|-1.1|-0.95|1.06|0.65|roundrect|0|top|25\nPAD|4|1.1|-0.95|1.06|0.65|roundrect|0|top|25\nPAD|5|1.1|0.95|1.06|0.65|roundrect|0|top|25\nLINE|overlay|-0.3|1.6|0.3|1.6|0.15\nLINE|overlay|-0.3|-1.6|0.3|-1.6|0.15\nLINE|fab|-0.8|1.45|0.8|1.45|0.1\nLINE|fab|0.8|1.45|0.8|-1.45|0.1\nLINE|fab|0.8|-1.45|-0.8|-1.45|0.1\nLINE|fab|-0.8|-1.45|-0.8|1.45|0.1\nLINE|courtyard|-1.9|1.75|1.9|1.75|0.05\nLINE|courtyard|1.9|1.75|1.9|-1.75|0.05\nLINE|courtyard|1.9|-1.75|-1.9|-1.75|0.05\nLINE|courtyard|-1.9|-1.75|-1.9|1.75|0.05\nDISC|overlay|-1.95|1.5|0.15\n'
IMPORT_SYM = 'META|LDO-LANE|300 mA LDO (fusion-lane test)|U\nMODE|0|std\nRECT|0|0|-400|800|0\nPIN|0|1|IN|0|-100|left|300||power_in\nPIN|0|3|EN|0|-200|left|300||input\nPIN|0|2|GND|400|-400|down|300||power_in\nPIN|0|5|OUT|800|-100|right|300||power_out\nPIN|0|4|NC|800|-300|right|300||no_connect\n'
t("fusion_import_part", "daily", {"libraryPath": BOARDDIR + "/import.lbr", "footprintSpec": IMPORT_FP, "symbolSpec": IMPORT_SYM, "params": {"MPN": "LANE-LDO-TEST"}, "dryRun": True}, {"ok": True, "fields": ["footprint", "symbol", "steps"]}, "Altium-format specs into one .lbr: footprint, symbol, connects, params (dry run, nothing written)")
t("fusion_import_part#nothing", "negative", {"libraryPath": BOARDDIR + "/import.lbr"}, {"pred": "must_refuse"}, "nothing to import is not success")
t("fusion_not_supported", "daily", {}, {"ok": True, "fields": ["verbs"]}, "the Altium-only standard verbs and what Fusion does instead")
t("fusion_not_supported#a365", "negative", {"verb": "a365_status"}, {"pred": "must_refuse"}, "an Altium-only verb answers not_supported")
t("fusion_set_models", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr", "symbolName": "LDO-LANE", "footprint": "SOT-23-5-LANE", "dryRun": True}, {"ok": True, "fields": ["data"]}, "one footprint kept (dry run)")
t("fusion_add_models", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr", "symbolName": "LDO-LANE", "models": [{"name": "SOT-23-5-LANE"}], "dryRun": True}, {"ok": True, "fields": ["data"]}, "an attached footprint is skipped, not stacked (dry run)")
t("fusion_swap_footprint", "daily", {"libraryPath": BOARDLIB, "footprintName": "SOT23", "pcbPath": BOARDDIR + "/test.brd", "designator": "U1", "assigns": "1=IN;2=GND;3=OUT", "open": False}, {"ok": True, "fields": ["swap"]}, "U1 on the battery's test board takes SOT23 (offline)")
t("fusion_teardrops", "daily", dict(B, dryRun=True), {"ok": True, "fields": ["teardrops", "skipped", "engine"]}, "the engine's teardrops for BQ25792 (dry run: 87 planned, 5 skipped with why)")
t("fusion_panelize", "daily", dict(B, rows=2, cols=2, separation="vscore", dryRun=True), {"ok": True, "fields": ["panelMm", "copyShift", "jlcpcb", "engine"]}, "a 2 x 2 V-score panel plan with JLCPCB's rules (dry run)")
t("fusion_replicate_layout", "daily", dict(B, source=["C11", "C12"], targets=[["C13", "C14"]], dryRun=True), {"ok": True, "fields": ["moves", "footprintMismatches"]}, "a channel's placement planned onto another (dry run on BQ25792)")
t("fusion_symbol_prims", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr", "symbolName": "LDO-LANE"}, {"ok": True, "fields": ["pins", "bodyRectMil", "models"]}, "the imported LDO's drawing counted")
t("fusion_set_visibility", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr", "symbolName": "LDO-LANE", "parameter": "MPN", "visible": True, "dryRun": True}, {"ok": True, "fields": ["data"]}, "show MPN on the sheet (dry run)")
t("fusion_sch_update_from_library", "daily", {"library": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr"}, {"ok": True, "fields": ["parts", "differs"]}, "read-only plan on the open design's schematic")
t("fusion_plate_vias", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/plate/plate-test.lbr", "dryRun": True}, {"ok": True, "fields": ["data"]}, "offline .lbr: holes become plated pads (dryRun writes nothing)")
t("fusion_cleanup_libs", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/plate/plate-test.lbr", "dryRun": True}, {"ok": True, "fields": ["data"]}, "offline .lbr: empty unused items listed (dryRun writes nothing)")
t("fusion_variants", "daily", dict(B), {"ok": True, "fields": ["variants"]}, "the design's assembly variants (BQ25792 has none)")
t("fusion_variants#edit", "negative", dict(B, name="FL-TEST", notFitted=["C11"]), {"pred": "must_refuse"}, "Fusion's VARIANT text command changes nothing, so an edit is refused not_supported")
t("fusion_silkscreen_prep", "daily", dict(B, dryRun=True), {"ok": True, "fields": ["before", "after", "silkOffCopper"]}, "designators on pads counted and a move planned (dry run)")
t("fusion_diagnostics", "daily", {}, {"ok": True, "fields": ["bridge", "installs", "running", "addin", "env"]}, "bug-report facts, no Fusion driving; credential values never shown")
t("fusion_instances", "daily", {}, {"ok": True, "fields": ["count", "instances"]}, "Fusion's processes")
t("fusion_progress", "daily", {}, {"ok": True, "fields": ["active", "verbTimes"]}, "the bridge's timing book")
t("fusion_foreground", "manual", {"reason": "battery check of the foreground verb"}, {"ok": True}, "brings Fusion in front: manual only, never in an unattended battery")
t("fusion_foreground#noreason", "negative", {}, {"pred": "must_refuse"}, "a foreground with no reason is refused")
t("fusion_list_installed_libraries", "daily", {"projectName": "Default Project", "depth": 2}, {"ok": True, "fields": ["libraries", "complete"]}, "the hub's .flbr libraries in one project")
t("fusion_add_symbol_art", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/art/art-test.lbr", "symbolName": "TPS54202DDCR", "kicadSym": "C:/Users/olive/Documents/fusion-lane-tests/art/tps54202ddcr-outline.kicad_sym", "dryRun": True}, {"ok": True, "fields": ["data"]}, "the TPS54202 chip outline fitted to the symbol body (dry run)")
t("fusion_lint_library", "daily", {"libraryPath": "C:/Users/olive/Documents/fusion-lane-tests/import/lane-import.lbr"}, {"ok": True, "fields": ["findings", "checked", "engine"]}, "the imported LDO library judged by the engine")
t("fusion_update_footprints_from_library", "daily", dict(B, libraryPath="C:/Users/olive/Documents/fusion-lane-tests/boardgen.lbr"), {"ok": True, "fields": ["plan", "footprints", "differs"]}, "BQ25792's footprints against a test library: mostly library_not_found, a plan either way")
t("fusion_update_footprints_from_library#apply", "negative", dict(B, libraryPath="C:/Users/olive/Documents/fusion-lane-tests/boardgen.lbr", apply=True), {"pred": "must_refuse"}, "apply is refused: Fusion's UPDATE asks about each part in a dialog")
t("fusion_build_board", "daily", {"libraryPath": BOARDLIB, "pcbPath": BOARDDIR + "/build.brd", "parts": [{"ref": "R1", "footprint": "R0603", "x": 5, "y": 5}, {"ref": "Q1", "footprint": "SOT23", "x": 12, "y": 5}], "nets": {"GND": ["R1.2", "Q1.2"]}, "outline": {"rect": [0, 0, 18, 10], "cornerRadius": 1}, "open": False}, {"ok": True, "fields": ["pcbPath", "unknownPins"]}, "written offline (open:false keeps the battery from opening a design each run)")
t("fusion_build_board#missing", "negative", {"libraryPath": BOARDLIB, "pcbPath": BOARDDIR + "/bad.brd", "parts": [{"ref": "U1", "footprint": "NO_SUCH_FP", "x": 0, "y": 0}], "open": False}, {"pred": "must_refuse"}, "a footprint missing from the library refuses the whole build")
t("fusion_test_board", "daily", {"libraryPath": BOARDLIB, "footprintName": "R0603;SOT23", "pcbPath": BOARDDIR + "/test.brd", "open": False}, {"ok": True, "fields": ["pcb", "placed", "saved"]}, "test.brd is made on the first run and appended to after (designators continue); written offline")
t("fusion_create_symbol", "daily", {"libraryPath": LIBNEW, "name": "LM358", "pins": "1,OUT,right,100;2,IN-,left,100;3,IN+,left,200;8,V+,left,300,0;4,V-,left,400,0", "dryRun": True}, {"ok": True, "fields": ["data"]}, "dry run")
t("fusion_copy_footprint", "negative", {"libraryPath": "C:/nope/missing.lbr", "from": "A", "to": "B"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_rename_footprint", "negative", {"libraryPath": "C:/nope/missing.lbr", "from": "A", "to": "B"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_remove_footprint", "negative", {"libraryPath": "C:/nope/missing.lbr", "footprintName": "A"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_clone_symbol", "negative", {"libraryPath": "C:/nope/missing.lbr", "from": "A", "to": "B"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_rename_symbol", "negative", {"libraryPath": "C:/nope/missing.lbr", "from": "A", "to": "B"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_remove_symbol", "negative", {"libraryPath": "C:/nope/missing.lbr", "symbolName": "A"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_set_params", "negative", {"libraryPath": "C:/nope/missing.lbr", "params": {"MPN": "X"}}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file (live-tested on new files 2026-10-28; golden: authoring_builds_a_part_fusion_will_open)")
t("fusion_xcopy_footprint", "negative", {"fromLib": "C:/nope/missing.lbr", "from": "A", "toLib": LIBNEW, "dryRun": True}, {"errorCode": "edit_refused"}, "a missing source library is refused")
t("fusion_read_symbol", "negative", {"libraryPath": "C:/nope/missing.lbr", "symbolName": "X"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file")
t("fusion_find_symbol", "negative", {"query": "X"}, {"errorCode": "bad_args"}, "libraries or folder are required")
t("fusion_edit_footprint", "negative", {"libraryPath": "C:/nope/missing.lbr", "name": "X", "dryRun": True}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file")
t("fusion_export_library_part", "negative", {"libraryPath": "C:/nope/missing.lbr", "part": "X"}, {"errorCode": "library_unreadable"}, "expect a refusal naming the missing file")
t("fusion_take_screenshot", "weekly", {}, {"ok": True}, "needs Fusion up; hwnd-targeted background capture")
t("fusion_activate_document", "weekly", {"name": "Untitled"}, {}, "needs an open document; shape only")
t("fusion_library_manager", "weekly", {}, {"ok": True}, "drives the GUI (Library Manager), reads nothing destructive")
t("fusion_plan_library_import", "weekly", {"parts": []}, {}, "planner with no parts answers honestly; the drills cover the real thing")
t("fusion_create_library", "manual", note="the five-part drill covers it (fusion_install_library_bundle -> prove)")
t("fusion_install_library_bundle", "manual", note="the five-part drill covers it; needs a bundle zip on the box")
t("fusion_managed_library_create", "manual", note="creates a managed library on the hub (cloud write)")
t("fusion_managed_library_link", "manual", note="drives LIBRARY.IO dialogs against the hub")
t("fusion_managed_library_pull", "manual", note="drives LIBRARY.IO dialogs against the hub")
t("fusion_managed_library_push", "manual", note="drives LIBRARY.IO dialogs; cloud write")
t("fusion_managed_library_unlink", "manual", note="drives LIBRARY.IO dialogs against the hub")
t("fusion_reset_workspace", "manual", note="closes the user's documents")
t("fusion_restart_for_addin", "manual", note="restarts Fusion")
t("fusion_save_to_cloud", "manual", note="cloud write; exercised by the drills")
t("fusion_web_signin", "manual", note="opens a browser on the user's machine")

PLAN = P

# -- Section map ------------------------------------------------------------
# John's high-level product sections (2026-08-14, verbatim intent): the run must
# be readable as "which PRODUCT AREAS are green", not just a flat verb list.
#   1. gui-driving        basic driving of the Fusion GUI incl. all electronics
#                         tabs: proj mgr, schematic, 2d board, 3d board,
#                         library, symbol, footprint
#   2. cad-3d-bom         Fusion CAD 3D including the BOM manager (Oliver's)
#   3. mcp-server         the MCP server inside Fusion
#   4. python-addin       the python add-in = all Fusion python cmds + Electron.Run
#   5. aps                the APS Fusion feature
#   6. auth-tokens        other authenticated Fusion APIs: does auth still work,
#                         did session tokens stay valid
# Plus two buckets his list implies but doesn't name:
#   7. exports            gerbers/BOM/CPL/GLB/stackup off the board
#   8. bridge-lifecycle   readiness/describe/install/uninstall plumbing
SECTION_ORDER = ["gui-driving", "cad-3d-bom", "mcp-server", "python-addin",
                 "aps", "auth-tokens", "exports", "bridge-lifecycle"]
# ── triage of 2026-09-11 (routing verbs from 1.10.0, lifecycle and narration verbs) ───────
t("fusion_routing_state#guard", "negative", {"expectDocument": "ZZZ_NoSuchDoc_TEST"}, {"pred": "must_refuse"},
  "no board named that is active: wrong_document, never a read of whatever is open")
t("fusion_route_net#guard", "negative", {"expectDocument": "ZZZ_NoSuchDoc_TEST", "boardId": "x", "expectedRevision": "x", "net": "NET_1", "layer": 1, "width": 0.25, "paths": [[{"pad": "J1.1"}, {"pad": "J2.1"}]]}, {"pred": "must_refuse"},
  "board/revision guards must refuse before any copper is drawn")
t("fusion_routing_drc#guard", "negative", {"expectDocument": "ZZZ_NoSuchDoc_TEST", "boardId": "x", "expectedRevision": "x"}, {"pred": "must_refuse"}, "same guards as route_net")
t("fusion_plan_route#guard", "negative", {"expectDocument": "ZZZ_NoSuchDoc_TEST", "net": "NET_1"}, {"pred": "must_refuse"}, "planner needs the live board it was asked about")
for v in ["fusion_route_net", "fusion_routing_drc", "fusion_plan_route", "fusion_move_parts", "fusion_place_vias", "fusion_apply_pours", "fusion_board_source", "fusion_ripup_nets", "fusion_schematic_run", "fusion_board_bom", "fusion_change_pours", "fusion_board_seed", "fusion_schematic_plan", "fusion_remove_pours", "fusion_run_drc", "fusion_repour", "fusion_apply_silkscreen", "fusion_set_track_width", "fusion_remove_items", "fusion_set_outline", "fusion_board_shape", "fusion_autoroute"]:
    t(v, "manual", note="live routing on a fixture board: demo/routing/run_demo.py is the weekly proof (needs a board open)")
for v in ["fusion_apply_fusion_update", "fusion_resolve_seat_dialog", "fusion_interactive", "fusion_narrate", "fusion_view", "fusion_electronics_query"]:
    t(v, "manual", note="lifecycle, seat or presentation verbs: need a human-visible Fusion state; manual tier")

t("fusion_schematic#guard", "negative", {}, {"pred": "must_refuse"}, "no action: must refuse with the menu, never open a dialog")
for v in ["fusion_launch", "fusion_export_gerber", "fusion_dismiss_dialogs", "fusion_install"]:
    t(v, "manual", note="EDA Bridge Standard alias of an existing verb (the reply carries aliasOf); tested through the verb it names")
t("fusion_save_document", "manual", note="saves a cloud design in place (a new version): exercised live on a scratch design in a test folder")
t("fusion_read_sheet_file#guard", "negative", {"path": "C:/no/such/file.sch"}, {"pred": "must_refuse"}, "a missing file refuses with file_not_found, never an empty parts list")
t("fusion_schematic#missing", "negative", {"action": "open", "schPath": "C:/nope/missing.sch"}, {"pred": "must_refuse"}, "a missing local .sch must refuse (via open_schematic) with no dialog")
t("fusion_schematic", "manual", note="open a cloud design's schematic / create a blank one: exercised by the weekly tour")
t("fusion_make_showcase_project#guard", "negative", {}, {"pred": "must_refuse"}, "no libraryPath: must refuse")
t("fusion_make_showcase_project", "manual", note="places a library's parts on a fresh sch+brd; exercised on the Adom_ToF library")
t("fusion_eagle_import_status#guard", "negative", {"runId": "no-such-run"}, {"pred": "must_refuse"}, "unknown runId must refuse")
t("fusion_eagle_import_status", "manual", note="status of a background EAGLE import")
t("fusion_show_attributes#guard", "negative", {}, {"pred": "must_refuse"}, "no deviceset: must refuse")
t("fusion_show_attributes", "manual", note="opens the device editor for a deviceset and returns its metadata")
_SECTION_BY_VERB = {
    "fusion_action_ledger": "bridge-lifecycle", "fusion_addin_status": "python-addin", "fusion_alerts": "gui-driving",
    "fusion_verb_times": "bridge-lifecycle", "fusion_live_feed": "bridge-lifecycle", "fusion_account": "auth-tokens", "fusion_claim_seat": "auth-tokens", "fusion_release_seat": "auth-tokens", "fusion_prewarm": "bridge-lifecycle", "fusion_prove_result": "cad-3d-bom",
    "fusion_set_driving_badge": "bridge-lifecycle", "fusion_set_window_sizing": "gui-driving", "fusion_cancel_run": "cad-3d-bom",
    "fusion_preflight_lbr": "cad-3d-bom", "fusion_library_parts": "cad-3d-bom", "fusion_export_library_part": "cad-3d-bom", "fusion_edit_footprint": "cad-3d-bom", "fusion_create_footprint": "cad-3d-bom", "fusion_create_symbol": "cad-3d-bom", "fusion_read_symbol": "cad-3d-bom", "fusion_find_symbol": "cad-3d-bom", "fusion_copy_footprint": "cad-3d-bom", "fusion_rename_footprint": "cad-3d-bom", "fusion_remove_footprint": "cad-3d-bom", "fusion_xcopy_footprint": "cad-3d-bom", "fusion_clone_symbol": "cad-3d-bom", "fusion_rename_symbol": "cad-3d-bom", "fusion_remove_symbol": "cad-3d-bom", "fusion_set_params": "cad-3d-bom", "fusion_read_footprint": "cad-3d-bom", "fusion_cross_probe": "routing", "fusion_project_info": "routing", "fusion_schematic_data": "routing", "fusion_part_parameters": "routing", "fusion_cross_check": "routing", "fusion_schematic_review": "routing", "fusion_pin_pad_check": "routing", "fusion_doc_params": "routing", "fusion_sheet_params": "routing", "fusion_sch_edit": "routing", "fusion_annotate": "routing", "fusion_footprint_primitives": "routing", "fusion_thermal_vias": "routing", "fusion_via_fence": "routing", "fusion_board_features": "routing", "fusion_export_pdf": "routing", "fusion_project_create": "routing", "fusion_project_rename": "routing", "fusion_project_snapshot": "routing", "fusion_import_svg": "routing", "fusion_copy_routing": "routing", "fusion_set_layer_count": "routing", "fusion_take_screenshot": "gui-driving",
    "fusion_schematic": "gui-driving", "fusion_show_attributes": "cad-3d-bom", "fusion_make_showcase_project": "cad-3d-bom", "fusion_eagle_import_status": "cad-3d-bom", "fusion_routing_state": "routing", "fusion_route_net": "routing", "fusion_routing_drc": "routing", "fusion_plan_route": "routing", "fusion_move_parts": "routing", "fusion_place_vias": "routing", "fusion_apply_pours": "routing", "fusion_pour_state": "routing", "fusion_board_source": "routing", "fusion_ripup_nets": "routing", "fusion_schematic_run": "routing", "fusion_board_bom": "routing", "fusion_change_pours": "routing", "fusion_board_seed": "routing", "fusion_schematic_plan": "routing", "fusion_remove_pours": "routing", "fusion_layer_stack": "routing", "fusion_netlist": "routing", "fusion_read_sheet_file": "routing", "fusion_bodies": "routing", "fusion_save_document": "aps", "fusion_apply_silkscreen": "routing", "fusion_component_data": "routing", "fusion_component_pins": "routing", "fusion_list_nets": "routing", "fusion_net_connections": "routing", "fusion_list_designators": "routing", "fusion_layers": "routing", "fusion_copper_layers": "routing", "fusion_layers_in_use": "routing", "fusion_read_rules": "routing", "fusion_rules_by_kind": "routing", "fusion_silk_state": "routing", "fusion_net_classes": "routing", "fusion_run_drc": "routing", "fusion_repour": "routing", "fusion_zoom_to": "routing", "fusion_launch": "bridge-lifecycle", "fusion_export_gerber": "exports", "fusion_dismiss_dialogs": "gui-driving", "fusion_board_summary": "routing", "fusion_install": "bridge-lifecycle", "fusion_sch_items": "routing", "fusion_sch_pins": "routing", "fusion_sch_trace_point": "routing", "fusion_sch_page": "routing", "fusion_sch_hierarchy": "routing", "fusion_sch_sheet_pins": "routing", "fusion_sch_connect_pins": "routing", "fusion_sch_fix_connectivity": "routing", "fusion_render_schematic": "routing", "fusion_sch_update_from_library": "routing", "fusion_sch_text": "routing", "fusion_sch_junction": "routing", "fusion_sch_delete_wire": "routing", "fusion_sch_split_wire": "routing", "fusion_sch_bus": "routing", "fusion_sch_edit_label": "routing", "fusion_sch_move_region": "routing", "fusion_sch_bus_entry": "routing", "fusion_sch_connect_to_bus": "routing", "fusion_sch_reset_fields": "routing", "fusion_sch_create": "routing", "fusion_update_board_from_eagle": "routing", "fusion_render_board_3d": "cad-3d-bom", "fusion_generate_body": "cad-3d-bom", "fusion_library_views": "gui-driving", "fusion_edit_parts": "routing", "fusion_align_parts": "routing", "fusion_remove_graphics": "routing", "fusion_remove_parts": "routing", "fusion_copper_balance": "routing", "fusion_return_path": "routing", "fusion_impedance": "routing", "fusion_current_capacity": "routing", "fusion_fab_check": "routing", "fusion_estimate_cost": "routing", "fusion_read_board": "routing", "fusion_diff_pairs": "routing", "fusion_auto_classes": "routing", "fusion_silk_designators": "routing", "fusion_net_lengths": "routing", "fusion_assembly_check": "routing", "fusion_review_board": "routing", "fusion_board_report": "exports", "fusion_jlcpcb_order": "exports", "fusion_sch_place_part": "routing", "fusion_export_spice": "routing", "fusion_sch_zoom": "routing", "fusion_simulate": "routing", "fusion_sch_connect": "routing", "fusion_sch_swap_part": "routing", "fusion_add_rule": "routing", "fusion_height_check": "routing", "fusion_placement_check": "routing", "fusion_find_space": "routing", "fusion_check_orientation": "routing", "fusion_check_clearance": "routing", "fusion_place_decoupling": "routing", "fusion_plan_fanout": "routing", "fusion_route_pair": "routing", "fusion_length_tune": "routing", "fusion_export_odb": "exports", "fusion_export_ipc356": "exports", "fusion_export_gencad": "exports", "fusion_export_ipc2581": "exports", "fusion_select_net": "routing", "fusion_set_layer_visibility": "routing", "fusion_board_outline": "routing", "fusion_run_erc": "routing", "fusion_set_track_width": "routing", "fusion_remove_items": "routing", "fusion_set_outline": "routing", "fusion_board_shape": "routing", "fusion_autoroute": "routing", "fusion_check_dialogs": "gui-driving", "fusion_status": "bridge-lifecycle", "fusion_save": "aps", "fusion_run_script": "gui-driving", "fusion_run_command": "gui-driving", "fusion_list_commands": "gui-driving", "fusion_dialog_log": "gui-driving", "fusion_signin_status": "bridge-lifecycle", "fusion_license_status": "bridge-lifecycle", "fusion_open_document": "cad-3d-bom", "fusion_show_3d_chip": "cad-3d-bom",
    "fusion_apply_fusion_update": "bridge-lifecycle", "fusion_resolve_seat_dialog": "bridge-lifecycle", "fusion_interactive": "gui-driving", "fusion_narrate": "gui-driving", "fusion_view": "gui-driving", "fusion_electronics_query": "cad-3d-bom",
    "fusion_activate_document": "gui-driving", "fusion_library_manager": "gui-driving", "fusion_plan_library_import": "cad-3d-bom",
    "fusion_create_library": "cad-3d-bom", "fusion_install_library_bundle": "cad-3d-bom",
    "fusion_managed_library_create": "aps", "fusion_managed_library_link": "aps", "fusion_managed_library_pull": "aps",
    "fusion_managed_library_push": "aps", "fusion_managed_library_unlink": "aps",
    "fusion_reset_workspace": "gui-driving", "fusion_restart_for_addin": "bridge-lifecycle",
    "fusion_save_to_cloud": "aps", "fusion_web_signin": "auth-tokens",
    "fusion_start": "gui-driving", "fusion_stop": "gui-driving", "fusion_kill": "gui-driving",
    "fusion_get_app_state": "gui-driving", "fusion_dismiss_blocking_dialogs": "gui-driving",
    "fusion_open_cloud_file": "gui-driving", "fusion_open_design": "gui-driving",
    "fusion_show_schematic": "gui-driving", "fusion_show_2d_board": "gui-driving",
    "fusion_show_3d_board": "gui-driving", "fusion_board_info": "gui-driving",
    "fusion_close_window": "gui-driving", "fusion_close_document": "gui-driving",
    "fusion_close_all_documents": "gui-driving", "fusion_get_preferences": "gui-driving",
    "fusion_set_preference": "gui-driving", "fusion_prefs_open": "gui-driving",
    "fusion_prefs_close": "gui-driving", "fusion_open_lbr": "gui-driving",
    "fusion_capture_library_views": "gui-driving", "fusion_generate_package": "gui-driving",
    "fusion_attach_3d_package": "gui-driving", "fusion_make_3d_package": "gui-driving",
    "fusion_build_library_3d": "gui-driving", "fusion_detect_layers": "gui-driving",
    "fusion_new_electronics_from_eagle": "gui-driving", "fusion_update_libraries": "gui-driving",
    "fusion_signin": "gui-driving", "fusion_demo": "gui-driving", "fusion_notify": "gui-driving",
    "fusion_state": "gui-driving",
    "focus_invariance": "gui-driving",
    "fusion_inspect_bodies": "cad-3d-bom", "fusion_physical_properties": "cad-3d-bom",
    "fusion_assembly_bom": "cad-3d-bom", "fusion_get_design_info": "cad-3d-bom",
    "fusion_get_parameters": "cad-3d-bom", "fusion_document_info": "cad-3d-bom",
    "fusion_import_step": "cad-3d-bom",
    "fusion_set_parameter": "cad-3d-bom", "fusion_export_cloud_file": "exports",
    "fusion_export_source": "exports", "fusion_export_eagle_source": "exports",
    "fusion_export_lbr": "exports", "fusion_export_board_image": "exports",
    "fusion_open_board": "gui-driving", "fusion_open_schematic": "gui-driving",
    "fusion_import_electronics": "gui-driving", "fusion_open_electronics": "python-addin",
    "fusion_list_text_commands": "python-addin",
    "fusion_create_cloud_folder": "aps", "fusion_delete_cloud_file": "aps", "fusion_check_recovery": "aps",
    "fusion_mcp_status": "mcp-server", "fusion_mcp_tools": "mcp-server",
    "fusion_mcp_call": "mcp-server", "fusion_mcp_resources": "mcp-server",
    "fusion_mcp_enable": "mcp-server",
    "fusion_run_modeling_script": "python-addin", "fusion_execute_text_command": "python-addin",
    "fusion_electron_run": "python-addin", "fusion_electron_zoom": "python-addin",
    "fusion_electron_pan": "python-addin", "fusion_electron_select": "python-addin",
    "fusion_aps_status": "auth-tokens", "fusion_aps_get": "auth-tokens",
    "fusion_aps_signin": "auth-tokens", "fusion_aps_set_client_id": "auth-tokens",
    "fusion_export_gerbers": "exports", "fusion_export_bom": "exports",
    "fusion_export_cpl": "exports", "fusion_export_optimized_glb": "exports",
    "fusion_fetch_optimized_glb": "exports", "fusion_board_stackup": "exports",
    "fusion_web_export": "exports",
    "fusion_set_design_rules": "exports",
    "fusion_readiness": "bridge-lifecycle", "fusion_describe": "bridge-lifecycle",
    "fusion_uninstall": "bridge-lifecycle", "fusion_set_seat_takeover": "bridge-lifecycle", "fusion_set_auto_update": "bridge-lifecycle",
}

def call_tour(target, verb, spec_args):
    """fusion_demo tour: async start, then poll fusion_demo_progress until done."""
    budget = int(spec_args.get("_pollTour", 300))
    args = {k: v for k, v in spec_args.items() if not k.startswith("_")}
    t0 = time.time()
    d, _ = call(target, verb, args, timeout=90)
    demo_id = d.get("demoId")
    if not demo_id:
        return d, time.time() - t0
    last = {}
    while time.time() - t0 < budget:
        time.sleep(5)
        last, _ = call(target, "fusion_demo_progress", {"demoId": demo_id, "imagesAs": "paths"}, timeout=30)
        if last.get("done"):
            break
    ok = bool(last.get("done")) and not last.get("failed")
    out = {"success": ok, "demoId": demo_id, "percent": last.get("percent"),
           "shots": len(last.get("shots") or []), "step": last.get("step")}
    if not ok:
        out["error"] = last.get("error") or ("tour not done after %ss (step=%s)" % (budget, last.get("stepLabel")))
    return out, time.time() - t0


def stage_dynamic(target, kind):
    """Stage a dynamic fixture on the box at call time.

    Returns (args, None) on success, (None, why) for a FAIL, or (None, "SKIP: why")
    when the fixture is legitimately absent this run (not a failure).
    Fixtures travel via send_files (the real container->desktop transfer verb;
    write_file-in-argv blew Errno 7 on the 245KB STEP, 2026-08-14).
    """
    try:
        if kind == "staged_lbr_mtp":
            fname = "ADS8588SIPM.fusion.lbr"
            src = str(REPO / "examples" / fname)
            r, _ = call(target, "send_files", {"filePaths": [src], "dest": SCRATCH}, timeout=180)
            if not (r.get("success") or r.get("status") == "ok"):
                return None, "send_files: " + str(r.get("error"))[:80]
            return {"libraryPath": SCRATCH + "/" + fname}, None
        if kind in ("staged_step", "staged_lbr"):
            fname = "ADS8588SIPM.step" if kind == "staged_step" else "ADS8588SIPM.fusion.lbr"
            src = str(REPO / "examples" / fname)
            r, _ = call(target, "send_files", {"filePaths": [src], "dest": SCRATCH}, timeout=180)
            if not (r.get("success") or r.get("status") == "ok"):
                return None, "send_files: " + str(r.get("error"))[:80]
            return {"filePath": SCRATCH + "/" + fname}, None
        if kind == "glb_job":
            prev = LAST_RESULTS.get("fusion_export_optimized_glb") or {}
            eid = prev.get("exportId")
            if not eid:
                return None, "export_optimized_glb returned no exportId (async start failed)"
            deadline = time.time() + 420
            while time.time() < deadline:
                st, _ = call(target, "fusion_fetch_optimized_glb", {"exportId": eid}, timeout=45)
                if st.get("done"):
                    break
                time.sleep(10)
            return {"exportId": eid}, None
        if kind == "urn_from_search":
            r, _ = call(target, "fusion_aps_search", {"query": "BQ25792", "limit": 5}, timeout=90)
            for item in (r.get("results") or r.get("items") or []):
                urn = item.get("urn") or item.get("id")
                if urn and str(urn).startswith("urn:"):
                    return {"urn": urn, "wait": False}, None
            return None, "no URN in aps_search results"
        return None, "unknown fixture kind " + kind
    except Exception as e:
        return None, str(e)[:100]


def section_of(name):
    base = name.split("#")[0]
    if base in _SECTION_BY_VERB:
        return _SECTION_BY_VERB[base]
    if base.startswith("fusion_aps_") or "cloud" in base:
        return "aps"
    return "bridge-lifecycle"


def other_batteries(target):
    """Other lanes' verb batteries (any run_verb_tests.py) driving the same machine right now: every
    lane runs in this distro, so /proc sees them. Ours and our parents are not counted."""
    mine = {os.getpid(), os.getppid()}
    found = []
    for d in os.listdir("/proc"):
        if not d.isdigit() or int(d) in mine:
            continue
        try:
            argv = open("/proc/%s/cmdline" % d, "rb").read().split(b"\0")
        except OSError:
            continue
        argv = [x.decode("utf-8", "replace") for x in argv if x]
        if not any(x.endswith("run_verb_tests.py") for x in argv):
            continue
        t = next((argv[i + 1] for i, x in enumerate(argv[:-1]) if x == "--target"), None) or next((x.split("=", 1)[1] for x in argv if x.startswith("--target=")), None)
        if t == target:
            found.append("%s (pid %s)" % (next(x for x in argv if x.endswith("run_verb_tests.py")), d))
    return found


def wait_for_the_machine(target, limit=1800, step=20):
    """kicad-lane's rule for the shared laptop (2026-10-29): never overlap another lane's battery on
    the same machine, since apps raising windows steal each other's focus. Wait up to 30 min, then refuse."""
    t0 = time.time()
    while True:
        busy = other_batteries(target)
        if not busy:
            return
        if time.time() - t0 >= limit:
            print("REFUSED: another lane's battery is still driving %s after %d min: %s" % (target, limit // 60, "; ".join(busy)))
            sys.exit(3)
        print("waiting: another lane's battery is driving %s (%s)" % (target, "; ".join(busy)), flush=True)
        time.sleep(step)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    ap.add_argument("--tier", default="daily", choices=["daily", "weekly", "all"])
    ap.add_argument("--only", default="")
    # Fusion's daily tier drives Fusion in the background (no verb in it takes the foreground), so it
    # may share the laptop with another lane's battery when that lane has been told (Colby 2026-10-31)
    ap.add_argument("--alongside", action="store_true", help="skip the shared-laptop wait (background-only tier; tell the other lane first)")
    a = ap.parse_args()
    tiers = {"daily": {"daily", "negative"}, "weekly": {"daily", "weekly", "negative"},
             "all": {"daily", "weekly", "negative"}}[a.tier]
    only = set(x.strip() for x in a.only.split(",") if x.strip())
    if not a.alongside: wait_for_the_machine(a.target)

    print("warmup: waking the bridge (unscored)...")
    call(a.target, "fusion_get_app_state", {}, timeout=45)
    pre, _ = call(a.target, "fusion_get_app_state", {}, timeout=45)
    pre = pre or {}
    # Cleanup contract (John, 2026-08-14): close what we opened. Snapshot whether Fusion
    # was running BEFORE the run; if the run started it, the run closes it at the end.
    # errorCode fusion_not_running is the definitive "was closed" signal; any successful
    # app-state read means it was already up (and stays up - it is the user's session).
    fusion_was_running = pre.get("errorCode") != "fusion_not_running"

    global LAST_RESULTS
    described = {v[0] for v in describe._T}
    planned = {k.split("#")[0] for k in PLAN} - {"focus_invariance"}
    results, t0 = [], time.time()
    # Catalog drift fails the run in BOTH directions: a described verb nobody triaged,
    # AND a planned verb missing from describe (fusion_import_step shipped undescribed
    # for weeks until this reverse check was added, 2026-08-14).
    for name in sorted(described - planned):
        results.append({"verb": name, "status": "FAIL", "why": "described but untriaged in tests/run_verb_tests.py"})
    for name in sorted(planned - described):
        results.append({"verb": name, "status": "FAIL", "why": "in the test plan but MISSING from describe.py - undescribed verbs are invisible to callers"})

    for name, spec in PLAN.items():
        if only:
            # An explicit --only list overrides the tier gates entirely - naming a
            # manual/weekly test IS the operator's consent to run it now.
            if name not in only:
                continue
        elif spec["tier"] == "manual" or spec["tier"] not in tiers:
            if spec["tier"] == "manual":
                results.append({"verb": name, "status": "MANUAL", "why": spec.get("note", "")})
            continue
        run_args = spec["args"]
        if "_dynamic" in run_args:
            run_args, stage_err = stage_dynamic(a.target, run_args["_dynamic"])
            if run_args is None:
                if str(stage_err).startswith("SKIP:"):
                    results.append({"verb": name, "status": "SKIP", "why": stage_err[5:].strip()})
                    print("  %-42s SKIP - %s" % (name, stage_err[5:].strip()))
                else:
                    results.append({"verb": name, "status": "FAIL", "why": "fixture staging failed: " + stage_err})
                    print("  %-42s FAIL - fixture staging failed: %s" % (name, stage_err))
                continue
        verb = name.split("#")[0]
        vt = max(90, CATALOG_TIMEOUTS.get(verb, 60) + 30)
        if "_pollTour" in run_args:
            d, dt = call_tour(a.target, verb, run_args)
        elif "_pollDoc" in run_args:
            d, dt = call_polled(a.target, verb, run_args, run_args["_pollDoc"], run_args["_pollSec"])
        else:
            d, dt = call(a.target, verb, run_args, timeout=vt)
        # A busy main thread right after a heavy op (cloud open, export) is congestion,
        # not a verb failure - a real caller retries per the _hint. One retry, like they would.
        if d.get("errorCode") == "main_thread_busy" or str(d.get("error")) == "addin_command_timeout":
            time.sleep(12)
            d2, dt2 = call(a.target, verb, run_args, timeout=vt)
            if not (d2.get("errorCode") == "main_thread_busy" or str(d2.get("error")) == "addin_command_timeout"):
                d, dt = d2, dt + dt2
        LAST_RESULTS[verb] = d
        if str(d.get("error") or "") == "target_not_found":
            flaps = getattr(sys.modules[__name__], "_FLAPS", 0) + 1
            setattr(sys.modules[__name__], "_FLAPS", flaps)
            if flaps >= 4:
                print("\nABORT: the relay lost target %s %d times in a row - the box is offline or its ab is mid-update. Re-run when `adom-bridge targets` shows it stable; these are not verb failures." % (a.target, flaps))
                results.append({"verb": name, "status": "FAIL", "why": "relay lost the target (run aborted; not a verb failure)"})
                break
        else:
            setattr(sys.modules[__name__], "_FLAPS", 0)
        errs = expect(d, spec["exp"])
        if name.startswith("fusion_search_cloud_files") or name.startswith("fusion_walk_cloud_tree"):
            # must refuse: success:true here is the FAILURE
            errs = [] if (d.get("success") is False or "disabled" in str(d.get("error", "")).lower()
                          or "DISABLED" in str(d.get("error", ""))) else ["deprecated verb did NOT refuse"]
        results.append({"verb": name, "status": "PASS" if not errs else "FAIL",
                        "why": "; ".join(errs), "sec": round(dt, 1)})
        print("  %-42s %s %s" % (name, "PASS" if not errs else "FAIL", ("- " + "; ".join(errs)) if errs else ""))

    # Focus invariance (fusion-background-etiquette): the whole run must leave the user's
    # foreground alone. z=0 belonging to a Fusion-family window at the end = a steal happened.
    fg, _ = call(a.target, "desktop_list_windows", {}, timeout=45)
    try:
        wins = fg.get("windows") or []
        top = min(wins, key=lambda w: w.get("z", 999)) if wins else {}
        stolen = "Autodesk Fusion" in str(top.get("title", "")) or str(top.get("title", "")).strip() == "Fusion360"
        results.append({"verb": "focus_invariance", "status": "FAIL" if stolen else "PASS",
                        "why": ("foreground is %r after the run" % top.get("title")) if stolen else ""})
        print("  %-42s %s" % ("focus_invariance", "FAIL" if stolen else "PASS"))
    except Exception as e:
        results.append({"verb": "focus_invariance", "status": "SKIP", "why": str(e)})

    call(a.target, "shell_execute",
         {"command": 'cmd /c if exist "%TEMP%\\fusion-verb-tests" rd /s /q "%TEMP%\\fusion-verb-tests"',
          "reason": "verb-test cleanup: wipe the scratch dir"}, timeout=60)
    if not fusion_was_running:
        print("cleanup: this run started Fusion - closing it (fusion_stop)...")
        stop_resp, _ = call(a.target, "fusion_stop",
                            {"reason": "verb-test cleanup: run started Fusion, run closes it"},
                            timeout=120)
        stopped = (stop_resp or {}).get("success")
        print("  fusion_stop:", "ok" if stopped else "FAILED - close it by hand")
    else:
        print("cleanup: Fusion was already running before the run - leaving it (user's session).")

    passed = sum(1 for r in results if r["status"] == "PASS")
    failed = [r for r in results if r["status"] == "FAIL"]
    for r in results:
        r["section"] = section_of(r["verb"])
    stamp = datetime.datetime.now().strftime("%Y-%m-%d_%H%M")
    rollup = {}
    for r in results:
        s = rollup.setdefault(r["section"], {"PASS": 0, "FAIL": 0, "MANUAL": 0, "SKIP": 0})
        s[r["status"]] += 1
    print("\nSection coverage (John's product map, 2026-08-14):")
    for sec in SECTION_ORDER:
        s = rollup.get(sec)
        if not s:
            continue
        badge = "RED" if s["FAIL"] else ("green" if s["PASS"] else "untested-this-tier")
        print("  %-38s %d pass / %d fail (%d manual, %d skip)  %s"
              % (sec, s["PASS"], s["FAIL"], s["MANUAL"], s["SKIP"], badge))
    report = {"date": stamp, "target": a.target, "tier": a.tier,
              "pass": passed, "fail": len(failed), "sections": rollup, "results": results,
              "durationSec": round(time.time() - t0, 1)}
    out = REPO / "tests" / "reports" / f"verb-test-{stamp}.json"
    out.write_text(json.dumps(report, indent=1))
    print(f"\n{passed} PASS, {len(failed)} FAIL "
          f"({sum(1 for r in results if r['status']=='MANUAL')} manual-tier, "
          f"{sum(1 for r in results if r['status']=='SKIP')} skipped) -> {out}")
    for r in failed: print("  FAIL:", r["verb"], "-", r["why"])
    sys.exit(1 if failed else 0)

if __name__ == "__main__":
    main()
