"""Structured, kit-aware mechanical assembly BOM (mirrors Fusion's Manage -> BOM).

The electronics `export_bom` only works on an open PCB board. This is its mechanical
counterpart: it walks the active Design assembly and returns a purchasing BOM that
respects the component hierarchy. It recurses through organizational subassemblies but
counts a physical part or a purchased kit/unit ONCE, so hardware modeled INSIDE a kit
(for example a "... with fasteners" bracket, or a bearing/pulley unit) is not
double-counted the way a flat allOccurrences walk would count it.
"""

import csv
import time

import adsk.core
import adsk.fusion


def _material_name(comp):
    try:
        if comp.material:
            return comp.material.name
    except Exception:
        pass
    try:
        if comp.bRepBodies.count > 0 and comp.bRepBodies.item(0).material:
            return comp.bRepBodies.item(0).material.name
    except Exception:
        pass
    return ""


def _attr(comp, name):
    try:
        v = getattr(comp, name, "") or ""
        return v.strip()
    except Exception:
        return ""


def handle_assembly_bom(app: adsk.core.Application, args: dict) -> dict:
    """Return a structured, kit-aware parts list for the active mechanical design.

    args:
      treatAsUnit  optional list of name substrings whose subassemblies are counted once
                   and NOT exploded (default: ["with fasteners"]).
      exclude      optional list of top-level component-name prefixes to skip entirely.
      includePhysicalProperties  optional bool -- add volume_cm3 / mass_kg per line.
      outputPath   optional str -- also write a CSV to this Windows path.
    """
    design = adsk.fusion.Design.cast(app.activeProduct)
    if not design:
        return {"success": False, "error": "No active Fusion Design.",
                "_hint": "Open a mechanical design in the Design workspace, then retry."}

    args = args or {}
    # Walk budget (issue #659, found by the daily verb suite): an electronics 3D PCB carries an
    # occurrence per pad, so the walk that is instant on mechanical assemblies grinds for minutes,
    # blows the relay window, and blocks the add-in main thread for every later verb. Refuse FAST
    # with a redirect instead: the mechanical walk is the wrong tool for a board anyway.
    budget = float(args.get("maxSeconds", 20))
    deadline = time.time() + budget
    try:
        occ_total = design.rootComponent.allOccurrences.count
    except Exception:
        occ_total = -1
    if occ_total > 3000:
        return {"success": False, "errorCode": "bom_walk_budget_exceeded",
                "occurrences": occ_total,
                "error": "This design has %d occurrences - walking it would block Fusion for minutes." % occ_total,
                "_hint": "This looks like an electronics 3D PCB (an occurrence per pad), and the mechanical "
                         "BOM walk is the wrong tool for a board: use fusion_export_bom for the electronics "
                         "BOM. For a genuinely huge MECHANICAL assembly, re-run with maxSeconds raised "
                         "(e.g. {\"maxSeconds\": 120}) and expect a long wait."}
    kit = [k.lower() for k in (args.get("treatAsUnit") or ["with fasteners", "with fastener"])]
    exclude = tuple(args.get("exclude") or [])
    include_pp = bool(args.get("includePhysicalProperties", False))

    def kit_rule(comp):
        n = comp.name.lower()
        for k in kit:
            if k in n:
                return k
        return None

    def is_leaf(comp):
        if kit_rule(comp) is not None:
            return True
        return comp.bRepBodies.count > 0  # a physical part is a leaf

    parts = {}
    # Transparency (issue #25, Oliver + John's call): every heuristic collapse is REPORTED so a
    # name-match on someone's own "bearing test jig" is a visible, correctable decision instead of
    # silently vanishing its internals from the BOM. collapsed = {unitName: {rule, quantity}}.
    collapsed = {}
    # Attribution for the inverse advisory: which DESCENDED subassembly each leaf came from, so a
    # purchased-looking unit that was exploded (240 identical bearing balls) can be flagged.
    sub_stats = {}   # subassembly name -> {part name -> instances}
    sub_stack = []

    class _BudgetExceeded(Exception):
        pass

    def walk(occs):
        for i in range(occs.count):
            if time.time() > deadline:
                raise _BudgetExceeded()
            occ = occs.item(i)
            comp = occ.component
            name = comp.name
            if any(name.startswith(e) for e in exclude):
                continue
            children = occ.childOccurrences
            has_children = children is not None and children.count > 0
            if is_leaf(comp):
                rule = kit_rule(comp)
                if rule is not None and has_children:
                    c = collapsed.setdefault(name, {"unit": name, "matchedRule": rule, "quantity": 0})
                    c["quantity"] += 1
                if sub_stack and rule is None:
                    stats = sub_stats.setdefault(sub_stack[-1], {})
                    stats[name] = stats.get(name, 0) + 1
                row = parts.get(name)
                if row is None:
                    row = {
                        "componentName": name,
                        "partNumber": _attr(comp, "partNumber"),
                        "description": _attr(comp, "description"),
                        "material": _material_name(comp),
                        "quantity": 1,
                        "bodies": comp.bRepBodies.count,
                    }
                    if include_pp:
                        try:
                            pp = comp.getPhysicalProperties(
                                adsk.fusion.CalculationAccuracy.LowCalculationAccuracy)
                            row["volume_cm3"] = round(pp.volume, 4)
                            row["mass_kg"] = round(pp.mass, 6)
                        except Exception:
                            row["volume_cm3"] = None
                            row["mass_kg"] = None
                    parts[name] = row
                else:
                    row["quantity"] += 1
            elif has_children:
                sub_stack.append(name)
                walk(children)
                sub_stack.pop()

    try:
        walk(design.rootComponent.occurrences)
    except _BudgetExceeded:
        return {"success": False, "errorCode": "bom_walk_budget_exceeded",
                "occurrences": occ_total, "budgetSeconds": budget,
                "partialParts": len(parts),
                "error": "BOM walk exceeded its %ds budget (%d part types found so far)." % (budget, len(parts)),
                "_hint": "On an electronics board use fusion_export_bom instead; on a huge mechanical "
                         "assembly re-run with a larger maxSeconds."}
    rows = list(parts.values())

    doc_name = design.rootComponent.name
    try:
        if design.parentDocument:
            doc_name = design.parentDocument.name
    except Exception:
        pass

    # Inverse advisory (Oliver, PR #38 intent): a DESCENDED subassembly dominated by many
    # identical small parts walks and quacks like a purchased unit that should be one BOM line.
    UNDER_COLLAPSE_MIN = 12
    possible_under = []
    for sub, stats in sub_stats.items():
        part, n = max(stats.items(), key=lambda kv: kv[1])
        if n >= UNDER_COLLAPSE_MIN:
            possible_under.append({
                "subassembly": sub, "dominantPart": part, "instances": n,
                "why": "descended subassembly dominated by %d identical parts; if it is a purchased unit, add a matching substring to treatAsUnit and it becomes one line" % n})

    out = {
        "success": True,
        "design": doc_name,
        "partCount": len(rows),
        "totalInstances": sum(r["quantity"] for r in rows),
        "treatAsUnit": kit,
        "collapsedUnits": sorted(collapsed.values(), key=lambda c: -c["quantity"]),
        "possibleUnderCollapse": sorted(possible_under, key=lambda a: -a["instances"]),
        "parts": rows,
    }
    if collapsed or possible_under:
        bits = []
        if collapsed:
            bits.append("%d subassembly type(s) were COLLAPSED to one line each by treatAsUnit name-matching (see collapsedUnits); if any is one of YOUR designs rather than a purchased unit, re-run with an explicit treatAsUnit list (or []) to explode it" % len(collapsed))
        if possible_under:
            bits.append("%d descended subassembly(ies) look like purchased units (see possibleUnderCollapse); add a matching substring to treatAsUnit to collapse them" % len(possible_under))
        out["_hint"] = ". ".join(bits) + "."


    outpath = args.get("outputPath")
    if outpath:
        try:
            cols = ["Part Number", "Part Name", "Description", "Material", "Quantity"]
            if include_pp:
                cols += ["Volume cm3", "Mass kg"]
            with open(outpath, "w", newline="", encoding="utf-8") as f:
                w = csv.writer(f)
                w.writerow(cols)
                for r in rows:
                    line = [r["partNumber"], r["componentName"], r["description"],
                            r["material"], r["quantity"]]
                    if include_pp:
                        line += [r.get("volume_cm3"), r.get("mass_kg")]
                    w.writerow(line)
            out["outputPath"] = outpath
        except Exception as e:
            out["csvError"] = str(e)

    return out
