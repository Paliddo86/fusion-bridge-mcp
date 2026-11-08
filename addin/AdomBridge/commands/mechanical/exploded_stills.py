"""Step images for assembly instructions. Steps come in assembly order, and each move says where its
part comes from (the way it moves out when exploded). For every step the view is saved twice from one
fixed orthographic camera: with that step's parts hovering where they go, earlier steps seated and
later steps out of view ("before"), then with them seated ("after"). One more image shows the view
before anything is assembled, so "after" of one step is the context of the next and the difference
between two images is exactly the parts that came in. Each moved part's box and centre in both images
come back normalized to the image, for callouts and arrows. Only parts whose pose changes are moved
between images, and every move is reverted at the end."""
import math
from pathlib import Path
import threading

import adsk.core

from .core import SESSION_ID, current_target, require, target
from .exploded_render import MOVED_NOTE, POSE_TOLERANCE_MM, Mover, capture, home_drift, moved_scope
from .exploded_view import compile_steps, request_arguments, sample_plan
from .glyphs import Glyphs

MAX_STEPS = 64
MAX_MOVES = 2048
MAX_INSTANCES = 4096
# bodies read per moved part for its box (a subassembly's own bodies and its children's)
MAX_BOX_BODIES = 200
# later steps' parts are parked this many view extents to the right, well out of frame
PARK_EXTENTS = 4.0
# room around the moved parts when the camera frames them
MARGIN = 1.12
# at most this many occurrences outside the subassembly are moved out of view
MAX_CONTEXT = 1000


def unit(v):
    n = math.sqrt(sum(c * c for c in v))
    require(n > 1e-12, "camera_invalid", "The camera's eye, target and up do not define a view.")
    return [c / n for c in v]


def cross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


class View:
    """Fusion frames an orthographic camera so that the image's shorter side spans viewExtents, centred
    on the target (measured at 1280x720, 960x960 and 720x1280: every point within a pixel). Units mm."""
    def __init__(self, camera, width, height):
        self.eye = [camera.eye.x * 10, camera.eye.y * 10, camera.eye.z * 10]
        self.target = [camera.target.x * 10, camera.target.y * 10, camera.target.z * 10]
        self.up = [camera.upVector.x, camera.upVector.y, camera.upVector.z]
        self.extents = camera.viewExtents * 10
        forward = unit([t - e for t, e in zip(self.target, self.eye)])
        self.right = unit(cross(forward, self.up))
        self.true_up = cross(self.right, forward)
        self.width, self.height = width, height
        self.scale = min(width, height) / self.extents

    def point(self, p):
        d = [a - b for a, b in zip(p, self.target)]
        x = self.width / 2 + sum(a * b for a, b in zip(d, self.right)) * self.scale
        y = self.height / 2 - sum(a * b for a, b in zip(d, self.true_up)) * self.scale
        return [round(x / self.width, 5), round(y / self.height, 5)]

    def box(self, lo, hi):
        corners = [self.point([x, y, z]) for x in (lo[0], hi[0]) for y in (lo[1], hi[1]) for z in (lo[2], hi[2])]
        return [min(c[0] for c in corners), min(c[1] for c in corners), max(c[0] for c in corners), max(c[1] for c in corners)]

    def describe(self):
        return {"eye": self.eye, "target": self.target, "up": self.up, "extentsMm": self.extents, "orthographic": True,
                "rule": "x = width/2 + dot(p - target, right) * min(width, height) / extentsMm; y likewise along up, downwards"}


def axis_of(v):
    """The world axis nearest a direction, as a unit vector."""
    i = max(range(3), key=lambda k: abs(v[k]))
    return [(1.0 if v[i] > 0 else -1.0) if k == i else 0.0 for k in range(3)]


def choose_direction(current, approaches):
    """The isometric view (looking down from one of the four upper corners) that shows the parts coming
    in best: each step's approach direction, weighted by its moves, should cross the screen rather than
    point along the line of sight, and come from the camera's side rather than from behind the parts.
    Ties keep the direction nearest the current view."""
    up = axis_of(current["up"])
    flat = [k for k in range(3) if up[k] == 0.0]
    best = None
    for a in (1.0, -1.0):
        for b in (1.0, -1.0):
            f = [0.0, 0.0, 0.0]
            f[flat[0]], f[flat[1]] = a, b
            f = unit([fi - ui for fi, ui in zip(f, up)])
            score = 0.0
            for d, n in approaches:
                along = sum(x * y for x, y in zip(d, f))
                across = math.sqrt(max(0.0, 1.0 - along * along))
                score += n * across * (1.0 if along <= 0.2 else 0.3)
            near = sum(x * y for x, y in zip(f, current["forward"]))
            key = (round(score, 6), near)
            if best is None or key > best[0]:
                best = (key, f)
    return best[1], up


def frame(forward, up, corners, width, height):
    """Target, true up and extents (mm) that fit every corner, the image's shorter side spanning the
    extents (how Fusion frames an orthographic camera)."""
    right = unit(cross(forward, up))
    true_up = cross(right, forward)
    xs = [sum(a * b for a, b in zip(c, right)) for c in corners]
    ys = [sum(a * b for a, b in zip(c, true_up)) for c in corners]
    span_x, span_y = max(xs) - min(xs), max(ys) - min(ys)
    short, long_ = (span_y, span_x) if width >= height else (span_x, span_y)
    extents = MARGIN * max(short, long_ * min(width, height) / max(width, height), 1.0)
    lo = [min(c[k] for c in corners) for k in range(3)]
    hi = [max(c[k] for c in corners) for k in range(3)]
    middle = [(a + b) / 2 for a, b in zip(lo, hi)]
    # centre on what the camera sees: the middle of the projected extents, not of the 3D box
    dx = (max(xs) + min(xs)) / 2 - sum(a * b for a, b in zip(middle, right))
    dy = (max(ys) + min(ys)) / 2 - sum(a * b for a, b in zip(middle, true_up))
    center = [m + dx * r + dy * u for m, r, u in zip(middle, right, true_up)]
    return center, true_up, extents, math.dist(lo, hi)


def context_paths(design, moved):
    """When every moved part sits in one subassembly, the occurrences outside it (the siblings along
    its path, still visible); empty when the parts sit at the root."""
    split = [p.split("+") for p in moved]
    common = []
    for names in zip(*split):
        if any(n != names[0] for n in names):
            break
        common.append(names[0])
    if not common or any(len(s) <= len(common) for s in split):
        return []
    out, level = [], design.rootComponent.occurrences
    for depth, name in enumerate(common):
        keep = None
        for i in range(level.count):
            occ = level.item(i)
            if occ.name == name:
                keep = occ
            elif occ.isLightBulbOn:
                out.append(occ.fullPathName)
        if keep is None:
            return []
        level = keep.childOccurrences
    return out if len(out) <= MAX_CONTEXT else []


def world_box(occurrence):
    """The occurrence's box in world mm from its body proxies (a referenced design's own box reads 0),
    falling back to its children's bodies; None when it holds no bodies."""
    lo, hi, read = None, None, 0
    pending = [occurrence]
    while pending and read < MAX_BOX_BODIES:
        occ = pending.pop(0)
        bodies = occ.bRepBodies
        for i in range(bodies.count):
            if read >= MAX_BOX_BODIES:
                break
            box = bodies.item(i).boundingBox
            read += 1
            a = [box.minPoint.x * 10, box.minPoint.y * 10, box.minPoint.z * 10]
            b = [box.maxPoint.x * 10, box.maxPoint.y * 10, box.maxPoint.z * 10]
            lo = a if lo is None else [min(u, v) for u, v in zip(lo, a)]
            hi = b if hi is None else [max(u, v) for u, v in zip(hi, b)]
        children = occ.childOccurrences
        pending.extend(children.item(i) for i in range(children.count))
    return (lo, hi) if lo is not None else None


def exploded_view_stills(app, args):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Rendering requires the main thread.")
    require(args.get("sessionId") == SESSION_ID, "session_mismatch", "Rendering requires the current session.")
    folder = Path(args["folder"])
    require(folder.is_absolute(), "invalid_destination", "folder must be an absolute path.")
    folder.mkdir(parents=True, exist_ok=True)
    require(args.get("overwrite", False) or not any(folder.glob("step_*.png")), "destination_exists",
            "folder already holds step images; pass overwrite:true to replace them.")
    for old in folder.glob("step_*.png"):
        old.unlink()
    width, height = int(args.get("width", 1280)), int(args.get("height", 720))
    # assembly order in, explode order inside: the plan explodes the last step first
    listed = [{"durationSeconds": 1.0, **{k: v for k, v in step.items() if k in ("title", "instruction", "moves")}} for step in args["steps"]]
    explode = list(reversed(listed))
    total, _, _ = request_arguments({"unit": args.get("unit"), "steps": explode, "playback": "assemble", "sampleTimes": []}, MAX_STEPS, MAX_MOVES)

    document, design, value = target(app, args)
    snapshots = design.snapshots
    require(not snapshots.hasPendingSnapshot, "pending_snapshot",
            "The design has an uncaptured move; capture or revert it first (rendering reverts every move it makes).")
    flags = (document.isSaved, document.isModified)
    context = args.get("context", "auto")
    require(context in ("auto", "keep"), "invalid_input", "context must be auto or keep.")
    scope = moved_scope(design, value, explode, MAX_INSTANCES)
    snapshot, handles = capture(design, value, scope)
    compiled = compile_steps(design, value, snapshot, handles, explode)
    count = len(listed)
    # which assembly step each moved instance belongs to (1-based), from its own moves
    owner = {}
    for step in compiled:
        for move in step["moves"]:
            owner.setdefault(move["instanceId"], count - step["index"])
    # what lies outside the moved parts' subassembly is moved out of view with the later steps' parts
    hidden_paths = set()
    if context == "auto":
        moved_paths = [row["path"] for row in snapshot["instances"] if row["instanceId"] in owner]
        hidden_paths = set(context_paths(design, moved_paths)) - scope
        if hidden_paths:
            extra, extra_handles = capture(design, value, hidden_paths)
            snapshot = dict(snapshot, instances=snapshot["instances"] + extra["instances"])
            handles = dict(handles, **extra_handles)
    rows = snapshot["instances"]
    parent = {row["instanceId"]: row.get("parentInstanceId") for row in rows}
    paths = {row["instanceId"]: row["path"] for row in rows}
    hidden = {row["instanceId"] for row in rows if row["path"] in hidden_paths}
    names = {row["instanceId"]: row.get("componentName") for row in rows}
    viewport = app.activeViewport
    original_camera = viewport.camera
    mover = Mover(snapshot, handles)
    glyphs = Glyphs(design, args.get("glyphs", False))
    boxes = {key: world_box(handles[key]) for key in owner}  # read at home, before anything moves
    mode = args.get("camera", "auto")
    require(mode in ("auto", "fit", "current"), "invalid_input", "camera must be auto, fit or current.")

    def state(k, seated):
        """Poses with steps before k seated, step k hovering (or seated) and later steps parked."""
        when = float(k) if seated else float(k - 1)
        poses = {p["instanceId"]: list(p["matrix"]) for p in sample_plan(snapshot, compiled, [when], total, "assemble")[0]["poses"]}
        shift, parked = {}, {}
        for row in rows:  # parents first, so a parked parent carries its tracked children
            key = row["instanceId"]
            own = park if owner.get(key, 0) > k or key in hidden else [0.0, 0.0, 0.0]
            up = parked.get(parent[key], [0.0, 0.0, 0.0])
            parked[key] = [a + b for a, b in zip(own, up)]
            for axis, index in enumerate((3, 7, 11)):
                poses[key][index] += parked[key][axis]
            shift[key] = [poses[key][3] - row["homeMatrix"][3], poses[key][7] - row["homeMatrix"][7], poses[key][11] - row["homeMatrix"][11]]
        return poses, shift

    images, steps_out, written = {}, [], 0
    try:
        glyphs.hide()  # no joint, sketch or construction glyphs in the step images
        camera = viewport.camera
        camera.isSmoothTransition = False
        camera.cameraType = adsk.core.CameraTypes.OrthographicCameraType
        park = [0.0, 0.0, 0.0]
        if mode != "current":
            # frame the moved parts, seated and exploded, then hold that camera for every image
            exploded = {p["instanceId"]: p["translationFromHome"] for p in sample_plan(snapshot, compiled, [0.0], total, "assemble")[0]["poses"]}
            corners = []
            for key, box in boxes.items():
                if box is None:
                    continue
                for shift in ([0.0, 0.0, 0.0], exploded[key]):
                    corners.extend([x + shift[0], y + shift[1], z + shift[2]]
                                   for x in (box[0][0], box[1][0]) for y in (box[0][1], box[1][1]) for z in (box[0][2], box[1][2]))
            require(corners, "plan_identity_unavailable", "None of the moved parts has a body to frame.")
            eye, target_point = camera.eye, camera.target
            forward = unit([(target_point.x - eye.x), (target_point.y - eye.y), (target_point.z - eye.z)])
            current = {"forward": forward, "up": [camera.upVector.x, camera.upVector.y, camera.upVector.z]}
            if mode == "auto":
                approaches = [(unit(move["worldTranslation"]), 1) for step in compiled for move in step["moves"]]
                forward, up = choose_direction(current, approaches)
            else:
                up = current["up"]
            center, true_up, extents, diagonal = frame(forward, up, corners, width, height)
            back = diagonal + 2000.0  # mm behind the target, clear of everything framed
            camera.target = adsk.core.Point3D.create(*[c / 10 for c in center])
            camera.eye = adsk.core.Point3D.create(*[(c - f * back) / 10 for c, f in zip(center, forward)])
            camera.upVector = adsk.core.Vector3D.create(*true_up)
            camera.viewExtents = extents / 10
        viewport.camera = camera
        camera = viewport.camera
        camera.isSmoothTransition = False
        view = View(camera, width, height)
        park = [c * view.extents * PARK_EXTENTS for c in view.right]

        def shoot(name):
            nonlocal written
            viewport.camera = camera
            viewport.refresh()
            adsk.doEvents()
            path = folder / name
            require(viewport.saveAsImageFile(str(path), width, height), "render_failed", "Fusion could not save " + name + ".")
            written += 1
            return name

        poses, _ = state(0, True)
        mover.apply(poses)
        images["context"] = shoot("step_000_after.png")
        for k in range(1, count + 1):
            before_poses, before_shift = state(k, False)
            mover.apply(before_poses)
            before = shoot("step_%03d_before.png" % k)
            after_poses, after_shift = state(k, True)
            mover.apply(after_poses)
            after = shoot("step_%03d_after.png" % k)
            instances = []
            for key in sorted((key for key, step in owner.items() if step == k), key=lambda key: paths[key]):
                row = {"path": paths[key], "componentName": names[key]}
                box = boxes.get(key)
                for label, shift in (("before", before_shift), ("after", after_shift)):
                    if box is None:
                        row[label] = None
                        continue
                    lo = [a + s for a, s in zip(box[0], shift[key])]
                    hi = [a + s for a, s in zip(box[1], shift[key])]
                    row[label] = {"point": view.point([(a + b) / 2 for a, b in zip(lo, hi)]), "box": view.box(lo, hi)}
                instances.append(row)
            step_in = listed[k - 1]
            steps_out.append({"order": k, "title": step_in.get("title", "Step %d" % k), "instruction": step_in.get("instruction", ""),
                              "before": before, "after": after, "context": images["context"] if k == 1 else steps_out[-1]["after"],
                              "instances": instances})
    finally:
        glyphs.restore()
        if snapshots.hasPendingSnapshot:
            snapshots.revertPendingSnapshot()
        original_camera.isSmoothTransition = False
        viewport.camera = original_camera
        viewport.refresh()
    require(glyphs.restored, "restore_failed", "Object Visibility switches did not come back after rendering: %s." % ", ".join(glyphs.failed))
    after_capture, _ = capture(design, value, scope | hidden_paths)
    drift = home_drift(snapshot, after_capture)
    require(drift is not None and drift <= POSE_TOLERANCE_MM, "restore_failed",
            "Instances did not return to their home poses after rendering (off by %s mm)." % (round(drift, 4) if drift is not None else "a missing instance"))
    # isModified is not compared: any move sets it and nothing in the API clears it (measured on a saved design)
    require(document.isSaved == flags[0] and not snapshots.hasPendingSnapshot, "restore_failed",
            "Document state changed during rendering.")
    modified = bool(document.isModified and not flags[1])
    # the target as it is now (moving and reverting can change the revision digest), and the one the steps were bound to
    return {"target": current_target(app), "sourceTarget": value, "folder": str(folder), "size": [width, height], "camera": view.describe(), "images": written,
            "documentModified": modified, **({"documentModifiedNote": MOVED_NOTE} if modified else {}),
            "steps": steps_out, "moves": mover.moves, "hiddenContext": len(hidden), "restored": True, "restoredWithinMm": round(drift, 6),
            "glyphs": glyphs.report(),
            "coordinates": "normalized to the image: x right, y down, 0..1"}


OPERATIONS = {"exploded_view_stills": exploded_view_stills}
WRITES = {"exploded_view_stills"}
