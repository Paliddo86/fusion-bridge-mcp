"""Definition identity, nested instance quantities, purchased collapse and CSV."""
import csv
from pathlib import Path
import tempfile
import time
import adsk.core
import adsk.fusion

from . import bom_views
from .assembly_state import metadata, native_properties
from .core import identity, items, occurrence_suppression, ref, require, target
from .exporting import publish_staged
from .configurations import provenance, snapshot as configuration_snapshot, source_link


def stock_length_mm(comp):
    """A linear part's cut length: the longest edge of its largest body's oriented minimum bounding
    box (the axis-aligned box when Fusion has no oriented one), in mm."""
    best = None
    for body in items(comp.bRepBodies):
        box = getattr(body, "orientedMinimumBoundingBox", None)
        if box is not None:
            extent = max(box.length, box.width, box.height)
        else:
            low, high = body.boundingBox.minPoint, body.boundingBox.maxPoint
            extent = max(high.x - low.x, high.y - low.y, high.z - low.z)
        best = extent if best is None else max(best, extent)
    return round(best * 10, 2) if best is not None else None


def wire_length_mm(comp):
    """A wire's length: the volume of its longest body over the area of one of its two planar end caps,
    exact for any constant section swept along any path (the oriented box undercounts a routed wire)."""
    best = None
    for body in items(comp.bRepBodies):
        caps = [face for face in items(body.faces) if face.geometry.objectType == adsk.core.Plane.classType()]
        if len(caps) == 2 and caps[0].area > 0 and abs(caps[0].area - caps[1].area) <= 0.01 * caps[0].area:
            length = body.volume / caps[0].area
            best = length if best is None else max(best, length)
    return round(best * 10, 1) if best is not None else stock_length_mm(comp)


def generate(design, value, args, started=None, budget_seconds=None):
    """budget_seconds, counted from started: past it, part numbers and descriptions not read yet are left
    for the next run (each is a cloud read of about 0.5 s on a cloud assembly: 280 of them took 282 s)."""
    deadline = started + budget_seconds if started is not None and budget_seconds else None
    pending = []
    configuration = configuration_snapshot(design, strict=True)
    table = configuration["table"]
    columns = {column["id"]: column for column in table["columns"]} if table else {}
    suppressed = [columns[cell["columnId"]]["source"] for cell in table["cells"]
                  if cell["rowId"] == table["activeRowId"] and cell["type"] == "ConfigurationSuppressCell"
                  and cell["isEffectivelySuppressed"]] if table else []
    def suppression(occurrence):
        if occurrence_suppression(occurrence) is True:
            return "suppressed"
        for source in suppressed:
            if source["objectType"].endswith("::Occurrence"):
                candidate = source_link(design, occurrence)
                if candidate["entityId"] == source["entityId"] and (source["occurrenceId"] is None or candidate["occurrenceId"] == source["occurrenceId"]):
                    return "configuration_suppressed"
        return None
    parts, instances, excluded, hidden_counted = {}, [], [], []
    physical = {"mass": 0.0, "volume": 0.0, "bodyInstances": 0}
    accuracy = adsk.fusion.CalculationAccuracy.HighCalculationAccuracy
    budget = {"visited": 0}
    # what the walks read about a definition, read once per definition instead of once per instance
    # (a 4,579-occurrence assembly has 1,249 definitions): every instance of a definition weighs the same
    counts, physicals, metas, kits, refs = {}, {}, {}, {}, {}
    def body_count(comp, key):
        if key not in counts:
            counts[key] = comp.bRepBodies.count
        return counts[key]
    def direct_physical(comp, key):
        if key not in physicals:
            props = [b.getPhysicalProperties(accuracy) for b in items(comp.bRepBodies)]
            physicals[key] = (sum(p.mass for p in props), sum(p.volume * 1000 for p in props), len(props))
        return physicals[key]
    def visit(comp, depth, ancestry):
        budget["visited"] += 1
        require(depth <= 100 and budget["visited"] <= 20000, "bom_budget", "Assembly exceeds combined purchasing/physical traversal budget.")
        key = identity(comp)
        require(key not in ancestry, "cyclic_assembly", "Assembly contains cyclic component containment.")
        return key
    # Physical totals are independent of purchasing collapse. Count direct bodies
    # once per physical instance, including root bodies and repeated children.
    def physical_walk(comp, occurrence=None, depth=0, ancestry=()):
        key = visit(comp, depth, ancestry)
        mass, volume, bodies = direct_physical(comp, key)
        physical["mass"] += mass
        physical["volume"] += volume
        physical["bodyInstances"] += bodies
        children = items(occurrence.childOccurrences if occurrence else comp.occurrences)
        for child in children:
            if suppression(child) is None:
                physical_walk(child.component, child, depth + 1, ancestry + (key,))

    def walk(comp, occurrence=None, parent_path=None, depth=0, ancestry=(), hidden=False):
        key = visit(comp, depth, ancestry)
        path = occurrence.fullPathName if occurrence else "<root>"
        reason = suppression(occurrence) if occurrence else None
        if reason:
            excluded.append({"path": path, "reason": reason})
            return
        if occurrence and bom_views.is_library(comp.name, body_count(comp, key)) and not args.get("includeLibraries", False):
            # a palette of parts kept to copy from: counting it adds one of everything it holds
            excluded.append({"path": path, "reason": "library_group"})
            return
        if key not in metas:
            native = args.get("nativeProperties", True)
            if native and deadline is not None and time.perf_counter() > deadline and native_properties(comp, read=False) is None:
                native = False
                pending.append(key)
            metas[key] = metadata(comp, native)
            if key in pending:
                metas[key]["nativePending"] = True
        data = dict(metas[key])
        if data.get("excludeFromBom", False):
            excluded.append({"path": path, "reason": "excludeFromBom"})
            return
        if occurrence and not hidden and not occurrence.isLightBulbOn:
            hidden = True
            if len(hidden_counted) < 50:
                hidden_counted.append({"path": path, "name": comp.name})
        children = items(occurrence.childOccurrences if occurrence else comp.occurrences)
        # an imported vendor model, a circuit board or a modeled bought product is one bought line unless
        # someone said otherwise: without this a 3,000-occurrence cell listed 90 resistors and every screw
        # inside a machine's vendor model
        explicit = "asPurchased" in data or data.get("makeBuy") == "make"
        if not (explicit or not occurrence or not children or not args.get("inferKits", True)) and key not in kits:
            kits[key] = bom_views.inferred_kit(comp.name, [child.component.name for child in children])
        kit_reason = None if explicit or not occurrence or not children or not args.get("inferKits", True) else kits[key]
        if kit_reason:
            data = {**data, "kitReason": kit_reason}
        purchased = (bool(data.get("asPurchased", False)) or bool(kit_reason)) and not args.get("explodePurchased", False)
        if key not in refs:
            refs[key] = ref(comp, value)
        row = {"path": path, "parentPath": parent_path, "depth": depth, "componentId": key, "metadata": data,
               "bodyCount": body_count(comp, key), "childCount": len(children), "asPurchased": purchased,
               "componentRef": dict(refs[key]), "occurrenceRef": ref(occurrence, value) if occurrence else None}
        instances.append(row)
        if body_count(comp, key) or purchased:
            if key not in parts:
                props = comp.getPhysicalProperties(accuracy) if purchased else None
                direct_mass, direct_volume, _ = (0.0, 0.0, 0) if purchased else direct_physical(comp, key)
                kind = None if purchased else bom_views.category(data)
                parts[key] = {"componentId": key, **data, "quantity": 0, "occurrencePaths": [],
                              "scope": "component_subtree" if purchased else "direct_component_bodies",
                              "bodyCount": body_count(comp, key),
                              "lengthMm": stock_length_mm(comp) if kind in bom_views.LINEAR else wire_length_mm(comp) if kind == "wire" else None,
                              "unitMass": props.mass if props else direct_mass,
                              "unitVolume": props.volume * 1000 if props else direct_volume}
            parts[key]["quantity"] += 1
            parts[key]["occurrencePaths"].append(path)
        if not purchased:
            for child in children:
                walk(child.component, child, path, depth + 1, ancestry + (key,), hidden)

    walk(design.rootComponent)
    if args.get("physicalTotals", True):
        physical_walk(design.rootComponent)
    rows = list(parts.values())
    # the views a buyer and a builder read: indented with item numbers, the purchasing list, the
    # cut list (and a stock plan when a stock length is given), fasteners, totals and warnings
    flat = bom_views.flat(rows, {r["componentId"]: r.get("lengthMm") for r in rows}, args.get("currency", "USD"))
    cuts = bom_views.cut_list(flat)
    warnings = bom_views.warnings(flat) + [
        {"code": "hidden_counted", "component": h["name"], "message": "Hidden in the model but counted: set excludeFromBom if it is not part of the build."}
        for h in hidden_counted]
    views = {"indented": bom_views.indented(instances, rows), "flat": flat, "cutList": cuts,
             "wireList": bom_views.wire_list(flat), "orderList": bom_views.order_list(flat),
             "fasteners": bom_views.fastener_rollup(flat), "totals": bom_views.totals(flat), "warnings": warnings}
    if args.get("stockLengthMm"):
        views["stockPlan"] = bom_views.stock_plan(cuts, args["stockLengthMm"], args.get("kerfMm", 3.0))
    return {"target": value, "complete": True, "configuration": provenance(configuration), **views,
            "configurationSuppression": [cell for cell in table["cells"] if cell["rowId"] == table["activeRowId"] and cell["type"] == "ConfigurationSuppressCell"] if table else [],
            "parts": rows, "instances": instances, "excluded": excluded, "hiddenCounted": hidden_counted,
            "partCount": len(rows), "totalPurchasingInstances": sum(r["quantity"] for r in rows),
            "physical": {**physical, "scope": "all unsuppressed direct-body instances; BOM exclusions do not remove physical mass",
                         "materialBasis": "Fusion assigned/default material; supplier provenance does not certify modeled material density"},
            "units": {"mass": "kg", "volume": "mm3"}, "calculationAccuracy": "high",
            "identityScope": "running add-in session; supplier metadata persists in native archives",
            **({"nativePending": len(pending),
                "nativeHint": "%d part numbers and descriptions were left unread (each is a cloud read of about 0.5 s): run bom "
                              "again and it continues from what this run read, or pass budgetSeconds to wait longer, or "
                              "nativeProperties:false to skip them." % len(pending)} if pending else {})}


def bom(app, args):
    started = time.perf_counter()
    _, design, value = target(app, args)
    return generate(design, value, args, started, args.get("budgetSeconds", 40))


def bom_export(app, args):
    started = time.perf_counter()
    _, design, value = target(app, args, True, model_write=False)
    destination = Path(args["filename"])
    require(destination.is_absolute() and destination.suffix.lower() == ".csv" and not destination.is_symlink() and not destination.is_dir(), "invalid_destination", "BOM destination must be an absolute regular .csv path.")
    require(args.get("overwrite", False) or not destination.exists(), "destination_exists", "Pass overwrite:true to replace an existing CSV.")
    # journaled, but its first answer still has ab's 60 s, and Fusion stays busy while it reads: the same
    # budget as bom, and a CSV written past it says how many part numbers it left blank (export again)
    result = generate(design, value, args, started, args.get("budgetSeconds", 40))
    destination.parent.mkdir(parents=True, exist_ok=True)
    published = {}
    try:
        with tempfile.TemporaryDirectory(prefix=".adom-bom-", dir=str(destination.parent)) as folder:
            staged = Path(folder) / "bom.csv"
            mode = args.get("mode", "flat")
            # flat: the purchasing list; indented: the structure with item numbers and quantities
            # per parent; cutlist and fasteners: what to cut and what hardware to buy
            keys = ["partNumber", "name", "description", "quantity", "makeBuy", "makeBuyReason", "category", "process", "finish",
                    "material", "stock", "lengthMm", "supplier", "supplierPartNumber", "supplierInferred", "sourceUrl",
                    "leadTimeDays", "unitCost", "currency", "extendedCost", "unitMassKg", "totalMassKg", "asPurchased",
                    "kitReason", "bodyCount", "notes", "componentId"]
            rows = result["flat"]
            if mode == "structured":
                keys = ["path", "parentPath", "depth", "componentId", "name", "partNumber", "description", "quantity", "bodyCount", "childCount", "asPurchased", "supplier", "supplierPartNumber", "sourceUrl", "sha256"]
                rows = [{**row["metadata"], **row, "quantity": 1} for row in result["instances"]]
            elif mode == "indented":
                keys = ["itemNumber", "level", "name", "partNumber", "description", "kind", "makeBuy", "category", "qtyPerParent", "totalQty", "componentId"]
                rows = result["indented"]
            elif mode == "cutlist":
                keys = ["profile", "category", "lengthMm", "quantity", "components"]
                rows = [{**c, "components": "; ".join(c["components"])} for c in result["cutList"]]
            elif mode == "fasteners":
                keys = ["spec", "quantity", "supplierPartNumber", "names"]
                rows = [{**f, "names": "; ".join(f["names"])} for f in result["fasteners"]]
            elif mode == "wires":
                keys = ["wireId", "gauge", "color", "lengthMm", "quantity", "name"]
                rows = [{**w, "gauge": g["gauge"], "color": g["color"]} for g in result["wireList"] for w in g["wires"]]
            elif mode == "order":
                keys = ["supplier", "orderNumber", "quantity", "unitCost", "currency", "extendedCost", "inferred", "lines", "names"]
                rows = [{**o, "names": "; ".join(o["names"])} for o in result["orderList"]]
            configuration = result["configuration"]
            context = {"documentId": value["documentId"], "revision": value["revision"],
                       "configurationTableId": configuration["tableId"], "configurationRowId": configuration["rowId"],
                       "configurationRowName": configuration["rowName"]}
            keys += list(context)
            rows = [{**row, **context} for row in rows]
            with staged.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.writer(stream)
                writer.writerow(keys)
                for part in rows:
                    values = [part.get(key, "") for key in keys]
                    writer.writerow(["'" + v if isinstance(v, str) and v[:1] in ("=", "+", "-", "@") else v for v in values])
            published.update(publish_staged(staged, destination, args.get("overwrite", False)))
    except Exception as exc:
        if not published.get("published"):
            raise
        published["cleanupWarning"] = str(exc)
    return {"target": value, "partCount": result["partCount"], "mode": args.get("mode", "flat"), "rowCount": len(rows),
            **{k: result[k] for k in ("nativePending", "nativeHint") if k in result},
            "totals": result["totals"], "warningCount": len(result["warnings"]),
            "excludedCount": len(result["excluded"]),
            "calculationAccuracy": result["calculationAccuracy"], "configuration": result["configuration"], **published}


OPERATIONS = {"bom": bom, "bom_export": bom_export}
WRITES = {"bom_export"}
