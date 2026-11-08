"""Design efficiency: what makes a design slow to recompute, open and edit, read from the model and
held against the playbook (skills/fusion-design-efficiency). It never changes the model and needs no
revision digest; a time budget stops the costly passes early and says so.
findings() is pure, so every rule is tested offline."""
import re
import time

import adsk.core
import adsk.fusion

from .core import components_by_occurrence, current_target, identity, require, target
from .bom_views import GAUGE, WIRING_HARDWARE

# limits people cite (Autodesk's large-assembly article, AU classes, forum benchmarks); see the playbook
LIMITS = {"occurrences": 1000, "components": 1000, "timeline": 500, "sketchCurves": 200, "baseFeatures": 50,
          "facesPerDefinition": 20000, "wires": 20}
COPY_NAME = re.compile(r" \(\d+\)$")


def size(things):
    """Count a Fusion collection (.count) or vector (len): allJoints and friends return vectors."""
    count = getattr(things, "count", None)
    return count if isinstance(count, int) else len(things)


def _kind(entity):
    return entity.objectType.split("::")[-1] if entity is not None else "None"


def measure(app, design, budget_seconds=45.0):
    """The numbers the playbook's rules read, cheapest first; passes past the budget are skipped."""
    start = time.perf_counter()
    skipped = []

    def spare():
        return time.perf_counter() - start < budget_seconds

    root = design.rootComponent
    parametric = design.designType == adsk.fusion.DesignTypes.ParametricDesignType
    intents = {}
    if hasattr(adsk.fusion, "DesignIntentTypes"):
        for name in ("PartDesignIntentType", "AssemblyDesignIntentType", "HybridDesignIntentType"):
            if hasattr(adsk.fusion.DesignIntentTypes, name):
                intents[getattr(adsk.fusion.DesignIntentTypes, name)] = name[:-len("DesignIntentType")].lower()
    m = {"designType": "parametric" if parametric else "direct",
         "designIntent": intents.get(getattr(design, "designIntent", None))}

    timeline = {"count": 0, "groups": 0, "byType": {}, "warnings": 0, "errors": 0, "suppressed": 0}
    if parametric:
        tl = design.timeline
        timeline["count"] = tl.count
        for i in range(tl.count):
            item = tl.item(i)
            if item.isGroup:
                timeline["groups"] += 1
                continue
            try:
                kind = _kind(item.entity)
            except Exception:
                kind = "unreadable"
            timeline["byType"][kind] = timeline["byType"].get(kind, 0) + 1
            state = getattr(item, "healthState", 0)
            timeline["warnings"] += state == adsk.fusion.FeatureHealthStates.WarningFeatureHealthState
            timeline["errors"] += state == adsk.fusion.FeatureHealthStates.ErrorFeatureHealthState
            timeline["suppressed"] += bool(getattr(item, "isSuppressed", False))
    m["timeline"] = timeline
    snapshots = getattr(design, "snapshots", None)
    m["snapshots"] = snapshots.count if snapshots is not None else 0
    m["pendingSnapshot"] = bool(snapshots.hasPendingSnapshot) if snapshots is not None else False

    # components through the occurrences (allComponents.item costs about 5 ms each on a big assembly)
    occurrence_list = root.allOccurrences
    all_occurrences = [occurrence_list.item(i) for i in range(occurrence_list.count)]
    every = components_by_occurrence(design, all_occurrences, design.allComponents.count)
    own = [c for c in every if c.parentDesign == design]
    m["components"] = {"all": len(every), "own": len(own), "referenced": len(every) - len(own)}
    m["occurrences"] = len(all_occurrences)
    m["joints"] = {"joints": size(root.allJoints), "asBuilt": size(root.allAsBuiltJoints), "rigidGroups": size(root.allRigidGroups)}

    counts = {"bodies": 0, "faces": 0, "meshBodies": 0, "baseFeatures": 0, "moveFeatures": 0, "sketches": 0,
              "sketchCurves": 0, "modeledThreads": 0, "emptyComponents": 0, "wires": 0, "copyNames": 0}
    big_sketches, patterns, per_definition = [], [], []
    for comp in own:
        bodies = comp.bRepBodies
        faces = sum(bodies.item(i).faces.count for i in range(bodies.count))
        counts["bodies"] += bodies.count
        counts["faces"] += faces
        counts["meshBodies"] += comp.meshBodies.count
        features = comp.features
        counts["baseFeatures"] += features.baseFeatures.count
        counts["moveFeatures"] += features.moveFeatures.count
        for sketch in (comp.sketches.item(i) for i in range(comp.sketches.count)):
            counts["sketches"] += 1
            curves = sketch.sketchCurves.count
            counts["sketchCurves"] += curves
            if curves > LIMITS["sketchCurves"]:
                big_sketches.append({"component": comp.name, "sketch": sketch.name, "curves": curves})
        threads = features.threadFeatures
        counts["modeledThreads"] += sum(1 for i in range(threads.count) if threads.item(i).isModeled)
        for group in (features.rectangularPatternFeatures, features.circularPatternFeatures):
            for i in range(group.count):
                pattern = group.item(i)
                option = getattr(pattern, "patternComputeOption", None)
                names = {adsk.fusion.PatternComputeOptions.AdjustPatternCompute: "adjust",
                         adsk.fusion.PatternComputeOptions.IdenticalPatternCompute: "identical",
                         adsk.fusion.PatternComputeOptions.OptimizedPatternCompute: "optimized"}
                # a component pattern has no compute option to choose (Fusion reports adjust), so it is not a finding
                kinds = getattr(adsk.fusion, "PatternEntityTypes", None)
                try:
                    of_occurrences = kinds is not None and pattern.patternEntityType == kinds.OccurrencesPatternType
                except (AttributeError, RuntimeError):
                    of_occurrences = False
                patterns.append({"component": comp.name, "name": pattern.name,
                                 "computeOption": None if of_occurrences else names.get(option), "ofComponents": of_occurrences})
        if not bodies.count and not comp.occurrences.count and not comp.sketches.count and not comp.meshBodies.count:
            counts["emptyComponents"] += 1
        if GAUGE.search(comp.name) and not WIRING_HARDWARE.search(comp.name):
            counts["wires"] += 1
        counts["copyNames"] += bool(COPY_NAME.search(comp.name))
        per_definition.append((comp, faces))
    m.update(counts)
    m["bigSketches"] = big_sketches[:20]
    m["patterns"] = patterns[:50]

    # costly passes: instance-weighted faces and repeated definitions
    heavy, signatures = [], {}
    # instances per definition in one walk (allOccurrencesByComponent rescans every occurrence per call)
    instance_counts = {}
    for occurrence in all_occurrences:
        if not spare():
            break
        key = identity(occurrence.component)
        instance_counts[key] = instance_counts.get(key, 0) + 1
    for comp, faces in per_definition:
        if not spare():
            skipped.append("heavy geometry and duplicate definitions")
            break
        instances = instance_counts.get(identity(comp), 0)
        if faces * max(instances, 1) > LIMITS["facesPerDefinition"]:
            heavy.append({"component": comp.name, "faces": faces, "instances": instances, "weightedFaces": faces * max(instances, 1)})
        bodies = comp.bRepBodies
        if 0 < bodies.count <= 5:
            key = []
            for i in range(bodies.count):
                body = bodies.item(i)
                box = body.boundingBox
                key.append((body.faces.count, round((box.maxPoint.x - box.minPoint.x) * 1000), round((box.maxPoint.y - box.minPoint.y) * 1000),
                            round((box.maxPoint.z - box.minPoint.z) * 1000)))
            signatures.setdefault(tuple(sorted(key)), []).append(comp.name)
    heavy.sort(key=lambda h: -h["weightedFaces"])
    m["heavyDefinitions"] = heavy[:15]
    groups = sorted((names for names in signatures.values() if len(names) > 1), key=len, reverse=True)
    m["duplicateDefinitions"] = {"groups": len(groups), "extraDefinitions": sum(len(g) - 1 for g in groups),
                                 "largest": [{"count": len(g), "names": g[:5]} for g in groups[:10]]}
    m["skipped"] = skipped
    m["seconds"] = round(time.perf_counter() - start, 2)
    return m


def findings(m):
    """Each rule the design breaks, with what was measured, the limit and the fix, worst first."""
    out = []

    def add(rule, severity, measured, limit, fix):
        out.append({"rule": rule, "severity": severity, "measured": measured, "limit": limit, "fix": fix})

    if m.get("snapshots"):
        add("placement-snapshots", "high" if m["snapshots"] > 10 else "medium", m["snapshots"], 0,
            "Place each occurrence where it belongs when inserting it (occurrence_insert transform), and move it with "
            "occurrence_update, which edits its initial position when Fusion allows: neither writes a Position snapshot.")
    if m.get("pendingSnapshot"):
        add("pending-snapshot", "medium", True, False, "A moved occurrence is waiting to be captured: capture or revert it before the next recompute.")
    if m.get("moveFeatures"):
        add("move-features", "medium", m["moveFeatures"], 0, "Model bodies in place, or move their occurrence instead of adding Move features.")
    dups = (m.get("duplicateDefinitions") or {}).get("extraDefinitions", 0)
    if dups:
        add("duplicate-definitions", "high" if dups > 20 else "medium", dups, 0,
            "Copies made as new components (Paste New): insert instances of one component so it is computed once and counted right.")
    size = max(m.get("occurrences", 0), (m.get("components") or {}).get("all", 0))
    if size > LIMITS["occurrences"]:
        add("design-size", "high", size, LIMITS["occurrences"],
            "Split the design into sub-designs inserted by reference: each computes on its own and opens fast.")
    if (m.get("timeline") or {}).get("count", 0) > LIMITS["timeline"]:
        add("timeline-length", "medium", m["timeline"]["count"], LIMITS["timeline"],
            "Shorten the timeline: placement at creation, instances instead of copies, sub-designs by reference.")
    if m.get("designType") == "parametric" and m.get("baseFeatures", 0) > LIMITS["baseFeatures"]:
        add("imported-history", "medium", m["baseFeatures"], LIMITS["baseFeatures"],
            "Keep imported and generated geometry in direct-mode designs inserted by reference, out of this timeline.")
    if m.get("designType") == "parametric" and m.get("wires", 0) > LIMITS["wires"]:
        add("wires-in-assembly", "high", m["wires"], LIMITS["wires"],
            "Build the wiring in its own direct-mode design and insert it by reference.")
    heavy = m.get("heavyDefinitions") or []
    if heavy:
        add("heavy-geometry", "medium", [h["component"] for h in heavy[:5]], LIMITS["facesPerDefinition"],
            "Defeature these models (delete small faces, replace detail with primitives) or keep a simplified copy.")
    if m.get("bigSketches"):
        add("sketch-size", "medium", len(m["bigSketches"]), LIMITS["sketchCurves"],
            "Split big sketches; pattern the 3D feature instead of repeating sketch entities.")
    adjust = [p for p in m.get("patterns") or [] if p.get("computeOption") == "adjust"]
    if adjust:
        add("pattern-compute", "medium", len(adjust), 0, "Set these patterns' compute option to identical or optimized.")
    timeline = m.get("timeline") or {}
    if timeline.get("errors") or timeline.get("warnings"):
        add("timeline-health", "high" if timeline.get("errors") else "low", {"errors": timeline.get("errors", 0), "warnings": timeline.get("warnings", 0)}, 0,
            "Fix the timeline's errors and warnings: each one costs time on every recompute.")
    if m.get("modeledThreads"):
        add("modeled-threads", "medium", m["modeledThreads"], 0, "Use cosmetic threads and unthreaded vendor models.")
    if m.get("meshBodies"):
        add("mesh-bodies", "low", m["meshBodies"], 0, "Convert or replace mesh bodies; they are heavy to display and cannot be edited.")
    if m.get("emptyComponents"):
        add("empty-components", "low", m["emptyComponents"], 0, "Delete empty components; each still takes part in the compute.")
    order = {"high": 0, "medium": 1, "low": 2}
    return sorted(out, key=lambda f: order[f["severity"]])


def efficiency_report(app, args):
    # a full target (with its revision), like every other read: a caller keeps the last one it got, and a
    # target without a revision made the next write refuse
    _, design, value = target(app, args)
    metrics = measure(app, design, float(args.get("budgetSeconds", 120)))
    result = {"target": value, "complete": not metrics["skipped"], "metrics": metrics, "findings": findings(metrics),
              "playbook": "skills/fusion-design-efficiency/SKILL.md"}
    if args.get("computeTiming", False):
        # the one number that sums everything up, and the slowest to get: opt in
        start = time.perf_counter()
        design.computeAll()
        result["computeAllSeconds"] = round(time.perf_counter() - start, 2)
        result["target"] = current_target(app)
    return result


# journaled like bom_export: model-read-only, but a big design's report outlives the relay's 60 s
OPERATIONS = {"efficiency_report": efficiency_report}
WRITES = {"efficiency_report"}
