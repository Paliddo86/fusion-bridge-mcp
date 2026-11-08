"""When a write runs Fusion's full recompute (Design.computeAll), following Fusion's own lead.

Fusion recomputes what an edit touches as the edit is made ("Typically Fusion computes everything
automatically as you edit a parametric design", Autodesk's Compute All help) and keeps Compute All for
checkpoints: major milestones, significant changes, edits upstream in the timeline that later features
reference (deletes above all), and a check before the design leaves. Fusion computes on its main thread
in both cases, so nothing runs in the background; the only choice is when. So every write reads the
timeline's health after Fusion's own compute, and the full recompute runs at a checkpoint:

- the writes since the last full recompute add up to CHECKPOINT (WEIGHTS: deletes and assembly
  relationships count most, edits of existing history next, appended features least, metadata nothing);
- the writes move on to another top-level occurrence, so the previous part's edits are done;
- before an export leaves Fusion;
- whenever a caller asks: health with recompute:true.

The counters and the last health a write verified are per document, for this add-in session.

A write is held to the faults the model carried before it (its baseline), not to a perfect model: a
production design can carry old timeline faults that nobody is about to fix. The write may not add a
fault, change one (its health state or message), or leave a fault on what it edits in place; faults it
clears are reported. With a healthy baseline this is the old rule: any fault refuses."""
from collections import Counter
import time

from . import core
from .core import MechanicalError, health, same

CHECKPOINT = 6.0
# a move to another part checkpoints edits worth one: an edit of existing history, a joint, a delete, two appends
FOCUS_MIN = 2.0
WEIGHTS = {
    # appended at the end of the timeline: nothing later depends on it yet
    "sketch_create": 0.5, "sketch_geometry": 0.5, "sketch_constraint": 0.5, "sketch_dimension": 0.5,
    "sketch_project": 0.5, "sketch_offset": 0.5, "plane_create": 0.5, "construction_axis": 0.5,
    "joint_origin_create": 0.5, "feature_create": 1.0, "component_create": 1.0, "occurrence_insert": 1.0,
    "cad_import": 1.0, "occurrence_update": 1.0,
    # edits of existing history, which later features may reference
    "parameter_set": 1.5, "feature_edit": 2.0, "sketch_edit": 2.0, "sketch_curve_edit": 2.0, "joint_origin_edit": 2.0,
    # assembly relationships
    "joint_create": 2.0, "joint_edit": 2.0, "regular_joint_create": 2.0, "regular_joint_edit": 2.0,
    "rigid_group_create": 2.0, "rigid_group_edit": 2.0, "motion_link_create": 2.0, "motion_link_edit": 2.0,
    # deletes of something later features or joints may reference
    "entity_delete": 3.0, "occurrence_delete": 3.0, "joint_delete": 3.0, "joint_origin_delete": 3.0,
    "rigid_group_delete": 3.0, "motion_link_delete": 3.0,
    # no geometry
    "parameter_create": 0.0, "component_metadata": 0.0, "material_assign": 0.0,
}
BATCH = {"create": 0.0, "set": WEIGHTS["parameter_set"], "delete": 3.0}

_ledgers = {}


def ledger(document):
    return _ledgers.setdefault(document, {"significance": 0.0, "writes": 0, "focus": None, "last": None, "verified": None})


def reset():
    _ledgers.clear()


def weight(op, args):
    if op == "parameter_batch":
        return min(CHECKPOINT, sum(BATCH.get(change.get("action"), 1.0) for change in args.get("changes") or []))
    return WEIGHTS.get(op, 1.0)


def focus_of(args):
    """The top-level occurrence a write works in: the first segment of the first occurrence path among
    its references, in argument order. None for root-level work, which never counts as a move to
    another part."""
    pending = [value for key, value in reversed(list(args.items())) if key != "target"]
    while pending:
        item = pending.pop()
        if isinstance(item, dict):
            path = item.get("occurrencePath")
            if isinstance(path, str) and path:
                return path.split("+")[0]
            pending.extend(reversed(list(item.values())))
        elif isinstance(item, list):
            pending.extend(reversed(item))
    return None


def due(document, w, focus):
    """Why this write runs the full recompute, or None."""
    led = ledger(document)
    if led["significance"] + w >= CHECKPOINT:
        return "significance"
    if focus is not None and led["focus"] is not None and focus != led["focus"] and led["significance"] >= FOCUS_MIN:
        return "focus_change"
    return None


def status(document):
    led = ledger(document)
    return {"significance": round(led["significance"], 2), "checkpointAt": CHECKPOINT,
            "writesSinceFullCompute": led["writes"], "focus": led["focus"], "lastFullCompute": led["last"]}


FIRST = 5  # faults a reply lists; the count covers the rest


def fault_rows(state):
    """A health read's faults, then the items whose health could not be read."""
    return list(state.get("faults") or ()) + list(state.get("unknown") or ())


def fault_key(row):
    """A fault as writes compare it: the item's name, health state and message. Not its timeline index,
    which moves when an earlier item is deleted."""
    if "error" in row:
        return ("unreadable", row.get("name"), row.get("error"))
    return ("fault", row.get("name"), row.get("state"), row.get("message"))


def unmatched(rows, others):
    """The rows the other read does not account for, counting repeats: two faults with the same name and
    message are two. Matching runs in timeline order, so the later of two lookalikes is the one left over."""
    budget = Counter(fault_key(row) for row in others)
    left = []
    for row in rows:
        key = fault_key(row)
        if budget[key] > 0:
            budget[key] -= 1
        else:
            left.append(row)
    return left


def summary(baseline, state=None):
    """The faults the model carried before the write, for its reply: how many, the first few, and the ones
    the write cleared (deleted, suppressed or repaired)."""
    rows = fault_rows(baseline)
    result = {"count": len(rows), "faults": rows[:FIRST]}
    if len(rows) > FIRST:
        result["more"] = len(rows) - FIRST
    if state is not None:
        cleared = unmatched(rows, fault_rows(state))
        if cleared:
            result["cleared"] = {"count": len(cleared), "faults": cleared[:FIRST]}
    return result


def _native(entity):
    try:
        return getattr(entity, "nativeObject", None) or entity
    except Exception:
        return entity


# What a write edits in place, by argument. A fault the model already carried there that the write did not
# clear is the write's own result. Inputs it only reads (a profile, an axis, joint geometry) are not, and a
# new item it creates is held to "no new fault".
SUBJECTS = {"feature_edit": "feature", "occurrence_update": "occurrence", "joint_edit": "joint",
            "regular_joint_edit": "joint", "rigid_group_edit": "group", "joint_origin_edit": "origin",
            "motion_link_edit": "link", **{op: "sketch" for op in (
                "sketch_geometry", "sketch_constraint", "sketch_dimension", "sketch_project", "sketch_edit",
                "sketch_curve_edit", "sketch_offset")}}


def owner_of(design, name):
    """The feature (or sketch dimension, or joint) that owns a model parameter, as a list; a user parameter has none."""
    try:
        parameter = design.allParameters.itemByName(name)
        owner = getattr(parameter, "createdBy", None) if parameter is not None else None
    except (AttributeError, RuntimeError):
        return []
    return [owner] if owner is not None else []


def subjects(design, value, args):
    """The entities a write edits in place (SUBJECTS; a parameter batch edits the owners of the parameters it
    sets), read before the edit. A reference that does not resolve is left for the operation to refuse."""
    found = []
    key = SUBJECTS.get(args.get("op"))
    if key and isinstance(args.get(key), dict):
        try:
            found.append(core.resolve(design, value, args[key]))
        except (MechanicalError, AttributeError, RuntimeError):
            pass
    if args.get("op") == "parameter_batch":
        for change in args.get("changes") or ():
            if isinstance(change, dict) and change.get("action") == "set":
                found.extend(owner_of(design, change.get("name")))
    return found


def edited(natives, edits):
    """The faults among natives ((row, timeline item) pairs) that sit on an entity the write edits in place:
    a fault left there means the write's own result is not healthy."""
    if not edits:
        return []
    mine = [_native(entity) for entity in edits if entity is not None]
    found = []
    for row, item in natives:
        try:
            entity = item.entity
        except Exception:
            continue
        if entity is None:
            continue
        entity = _native(entity)
        try:
            if any(same(entity, other) for other in mine):
                found.append(row)
        except Exception:
            continue
    return found


def judge(state, baseline, natives=(), edits=(), **details):
    """Refuse (unhealthy_model) a health read a write may not leave behind: a fault the baseline lacks (a
    new one, or an existing one whose state or message changed), a fault left on an entity the write edits
    in place, or a full recompute that failed. (Design.computeAll returns true on a model that carries
    faults: measured with 15 lost-reference warnings and with a failed extrude.)"""
    added = unmatched(fault_rows(state), fault_rows(baseline))
    own = [] if added else edited(natives, edits)
    failed = state.get("computeSucceeded") is False
    if not (added or own or failed):
        return
    if added:
        message = "The write added a fault, or changed one the model already had."
    elif own:
        message = "The write left a fault on an entity it edited."
    else:
        message = "Recompute failed."
    raise MechanicalError("unhealthy_model", message, health=state, newFaults=added, editedFaults=own,
                          preExistingFaults=summary(baseline), **details)


def health_before(design, document, revision):
    """The model's health before a write, the baseline the write is held to: what the last write verified
    when nothing changed since (same revision), otherwise a walk of the timeline. Faults the model already
    carries do not refuse the write; settle refuses one that adds a fault or changes one."""
    cached = ledger(document)["verified"]
    if cached is not None and cached["revision"] == revision:
        return dict(cached["health"])
    return health(design, False)


def settle(design, document, args, baseline, edits=()):
    """After a write's edit and before it commits. Fusion's own compute has run; the full recompute runs
    first when a checkpoint is due. Refuses a result that judge refuses against the baseline; edits are
    the entities the write edits in place (subjects)."""
    w = weight(args.get("op"), args)
    focus = focus_of(args)
    reason = due(document, w, focus)
    started = time.perf_counter()
    natives = []
    state = health(design, reason is not None, natives)
    info = {"full": reason is not None, "reason": reason, "seconds": round(time.perf_counter() - started, 2), "weight": w, "focus": focus}
    judge(state, baseline, natives, edits, compute=info)
    return state, info


def midway(design):
    """Inside a write, before it settles (Fusion solves a joint as it is added): the model's health now,
    held to the write's baseline."""
    baseline = core.write_baseline()
    state = health(design, False)
    judge(state, baseline if baseline is not None else {})
    return state


def restored(design, baseline):
    """After a failed write is undone, or the transaction probe: the full recompute, and no fault the
    model did not carry before."""
    state = health(design, True)
    judge(state, baseline)
    return state


def committed(document, revision, state, info):
    """Count a committed write, remember the health it verified, and report the ledger."""
    led = ledger(document)
    if info["full"]:
        led.update(significance=0.0, writes=0, last={"reason": info["reason"], "seconds": info["seconds"], "at": round(time.time())})
    else:
        led["significance"] += info["weight"]
        led["writes"] += 1
    if info["focus"] is not None:
        led["focus"] = info["focus"]
    # stored as a plain walk reports it, so a later witness compares equal
    led["verified"] = {"revision": revision, "health": dict(state, computeSucceeded=None)}
    return dict(status(document), full=info["full"], reason=info["reason"], seconds=info["seconds"])


def checkpoint(design, document, reason):
    """Run the full recompute now (an export, or a caller asking) and start the counters again."""
    started = time.perf_counter()
    state = health(design, True)
    led = ledger(document)
    led.update(significance=0.0, writes=0, verified=None,
               last={"reason": reason, "seconds": round(time.perf_counter() - started, 2), "at": round(time.time())})
    return state


def pending(document):
    led = ledger(document)
    return led["writes"] > 0 or led["significance"] > 0
