"""Exploded-view animation frames for operators: play an exploded_view_plan on the real occurrences,
save one viewport image per frame from a fixed camera, then revert every move and prove each instance
is back home. Fusion has no video export in its API, so the frames are joined into MP4 outside Fusion
(ffmpeg), and the steps' titles and instructions come back as timed captions."""
import math
from pathlib import Path
import threading

import adsk.core
import adsk.fusion

from .core import SESSION_ID, concrete, current_target, digest, identity, ref, require, resolve, target
from .exploded_view import MAX_TRACKED, SPLIT_HINT, compile_steps, matrix, request_arguments, sample_plan, text
from .glyphs import Glyphs

MAX_FRAMES = 3600
MOVED_NOTE = ("Fusion marks a document modified when a part moves, even after the move is reverted; the parts are back where "
              "they were (restoredWithinMm), so there is nothing to save.")
# how far a part may sit from its planned pose before it counts as held back (mm, and matrix terms)
POSE_TOLERANCE_MM = 0.01


def to_matrix(pose):
    """A plan pose (row-major, translation in mm) as a Fusion matrix (cm)."""
    values = list(pose)
    for index in (3, 7, 11):
        values[index] /= 10
    matrix = adsk.core.Matrix3D.create()
    require(matrix.setWithArray(values), "plan_frame_invalid", "Fusion refused a sampled pose.")
    return matrix


def moved_scope(design, value, steps, limit=MAX_TRACKED):
    """Full paths of the occurrences the steps move, and of their ancestors (for inherited motion).
    Children of a moved occurrence follow it by themselves, so they are not tracked."""
    scope = set()
    for step in steps:
        for move in step.get("moves") or []:
            path = text(resolve(design, value, move["occurrence"], ("Occurrence",)).fullPathName)
            parts = path.split("+")
            scope.update("+".join(parts[:n]) for n in range(1, len(parts) + 1))
    require(len(scope) <= limit, "plan_budget", "The steps move more than %d occurrences (with their parents)." % limit,
            hint=SPLIT_HINT.replace("plans", "runs"), tracked=len(scope), limit=limit)
    return scope


def capture(design, value, scope):
    """The plan's instance rows for just the occurrences in scope, parents first. A big assembly
    (thousands of occurrences) only pays for the parts an animation moves."""
    found = {}
    every = design.rootComponent.allOccurrences
    for i in range(every.count):
        occurrence = concrete(every.item(i))
        path = occurrence.fullPathName
        if path in scope:
            require(path not in found, "ambiguous_context", "Every root-context occurrence needs a unique full path.")
            found[path] = occurrence
    require(set(found) == scope, "invalid_context", "A moved occurrence or one of its parents is no longer in the design.")
    rows, handles, ids = [], {}, {}
    for path in sorted(scope, key=lambda p: (p.count("+"), p)):
        occurrence = found[path]
        parent_path = path.rsplit("+", 1)[0] if "+" in path else None
        comp = concrete(occurrence.component)
        comp_id = identity(comp)
        instance_id = digest([SESSION_ID, value["documentId"], path, comp_id])
        rows.append({"instanceId": instance_id, "path": path, "occurrence": ref(occurrence, value),
                     "parentInstanceId": ids.get(parent_path), "componentId": comp_id,
                     "componentName": text(comp.name, 256), "homeMatrix": matrix(occurrence.transform2)})
        ids[path] = instance_id  # parents sort first, so a child finds its parent here (a dict, not a scan of the rows)
        handles[instance_id] = occurrence
    return {"instances": rows, "rootBodyGroup": {}}, handles


class Mover:
    """Puts the tracked instances at the poses asked for, moving only those whose pose changes: a
    transform2 set costs about 44 ms on a 4,579-occurrence assembly, and most frames move only one
    step's parts. Plans translate from home, so an instance's pose is its home plus an offset; Fusion
    carries tracked children along when their parent moves, and their applied offsets follow."""
    def __init__(self, snapshot, handles):
        self.rows, self.handles, self.moves = snapshot["instances"], handles, 0
        self.home = {row["instanceId"]: row["homeMatrix"] for row in self.rows}
        self.applied = {key: [0.0, 0.0, 0.0] for key in self.home}
        self.below = {key: [] for key in self.home}
        parent_of = {row["instanceId"]: row.get("parentInstanceId") for row in self.rows}
        for row in self.rows:
            parent = row.get("parentInstanceId")
            while parent is not None:
                self.below[parent].append(row["instanceId"])
                parent = parent_of[parent]

    def offset(self, key, pose):
        home = self.home[key]
        return [pose[3] - home[3], pose[7] - home[7], pose[11] - home[11]]

    def apply(self, poses):
        """poses: instanceId -> row-major matrix in mm. Parents are set before their children."""
        for row in self.rows:
            key = row["instanceId"]
            if key not in poses:
                continue
            want = self.offset(key, poses[key])
            delta = [w - a for w, a in zip(want, self.applied[key])]
            if max(abs(d) for d in delta) <= 1e-9:
                continue
            self.handles[key].transform2 = to_matrix(poses[key])
            self.moves += 1
            self.applied[key] = want
            for child in self.below[key]:
                self.applied[child] = [a + d for a, d in zip(self.applied[child], delta)]


def held_back(snapshot, poses, handles):
    """Instances Fusion did not put where the plan says (a joint, a pattern or a constraint held them):
    the frames would show them standing still, so the caller is told which ones."""
    paths = {row["instanceId"]: row["path"] for row in snapshot["instances"]}
    out = []
    for pose in poses:
        actual = matrix(handles[pose["instanceId"]].transform2)
        gap = max(abs(a - b) * (1 if index in (3, 7, 11) else 1000) for index, (a, b) in enumerate(zip(actual, pose["matrix"])))
        if gap > POSE_TOLERANCE_MM:
            # offByMm: translation in mm, or a rotation difference as the shift it makes 1 m from the origin
            out.append({"path": paths[pose["instanceId"]], "offByMm": round(gap, 3)})
    return out


def home_drift(before, after):
    """The largest difference, in mm (rotation terms at a 1 m lever arm), between two captures' home poses;
    None when an instance appeared or vanished. Exact equality is wrong here: reverting a big assembly
    leaves float noise far below any real tolerance."""
    a = {row["path"]: row["homeMatrix"] for row in before["instances"]}
    b = {row["path"]: row["homeMatrix"] for row in after["instances"]}
    if set(a) != set(b):
        return None
    return max([abs(x - y) * (1 if i in (3, 7, 11) else 1000) for path in a for i, (x, y) in enumerate(zip(a[path], b[path]))] or [0.0])


def captions(steps, total, playback):
    """Each step's title and instruction with its time range in the video (reversed for assemble)."""
    out = []
    for step in steps:
        start, end = step["startSeconds"], step["endSeconds"]
        if playback == "assemble":
            start, end = total - end, total - start
        out.append({"index": step["index"], "startSeconds": round(start, 3), "endSeconds": round(end, 3),
                    "title": step["title"], "instruction": step["instruction"]})
    return sorted(out, key=lambda c: c["startSeconds"])


def frame_times(total, fps, hold):
    """Times to sample: every 1/fps through the plan, then the last pose held for hold seconds."""
    count = int(math.floor(total * fps + 1e-9)) + 1
    times = [min(k / fps, total) for k in range(count)]
    if times[-1] < total:
        times.append(total)
    return times + [total] * int(round(hold * fps))


def exploded_view_render(app, args):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Rendering requires the main thread.")
    require(args.get("sessionId") == SESSION_ID, "session_mismatch", "Rendering requires the current session.")
    folder = Path(args["folder"])
    require(folder.is_absolute(), "invalid_destination", "folder must be an absolute path.")
    folder.mkdir(parents=True, exist_ok=True)
    require(args.get("overwrite", False) or not any(folder.glob("frame_*.png")), "destination_exists",
            "folder already holds frames; pass overwrite:true to replace them.")
    for old in folder.glob("frame_*.png"):
        old.unlink()
    fps, width, height = int(args.get("fps", 24)), int(args.get("width", 1280)), int(args.get("height", 720))
    total, _, playback = request_arguments({**args, "sampleTimes": []})
    times = frame_times(total, fps, float(args.get("holdSeconds", 1.0)))
    require(len(times) <= MAX_FRAMES, "plan_budget", "The animation exceeds %d frames; lower fps or shorten the steps." % MAX_FRAMES)

    document, design, value = target(app, args)
    snapshots = design.snapshots
    # rendering ends by reverting every uncaptured move, so it never starts on someone else's
    require(not snapshots.hasPendingSnapshot, "pending_snapshot",
            "The design has an uncaptured move; capture or revert it first (rendering reverts every move it makes).")
    flags = (document.isSaved, document.isModified)
    scope = moved_scope(design, value, args["steps"])
    snapshot, handles = capture(design, value, scope)
    steps = compile_steps(design, value, snapshot, handles, args["steps"])
    viewport = app.activeViewport
    original_camera = viewport.camera

    mover = Mover(snapshot, handles)
    glyphs = Glyphs(design, args.get("glyphs", False))

    def pose_at(seconds):
        poses = sample_plan(snapshot, steps, [seconds], total, playback)[0]["poses"]
        mover.apply({pose["instanceId"]: pose["matrix"] for pose in poses})
        return poses

    written = 0
    try:
        # joint, sketch and construction glyphs off first, so the fit frames the parts and no frame shows them
        glyphs.hide()
        # frame the most exploded state so every part stays in view, then hold that camera; check there
        # that every part reached its planned pose
        held = held_back(snapshot, pose_at(total if playback == "explode" else 0.0), handles)
        if args.get("fit", True):
            viewport.fit()
        camera = viewport.camera
        camera.isSmoothTransition = False
        for index, seconds in enumerate(times):
            pose_at(seconds)
            viewport.camera = camera
            viewport.refresh()
            adsk.doEvents()
            path = folder / ("frame_%05d.png" % index)
            require(viewport.saveAsImageFile(str(path), width, height), "render_failed", "Fusion could not save frame %d." % index)
            written += 1
    finally:
        glyphs.restore()
        if snapshots.hasPendingSnapshot:
            snapshots.revertPendingSnapshot()
        original_camera.isSmoothTransition = False
        viewport.camera = original_camera
        viewport.refresh()
    require(glyphs.restored, "restore_failed", "Object Visibility switches did not come back after rendering: %s." % ", ".join(glyphs.failed))
    after, _ = capture(design, value, scope)
    drift = home_drift(snapshot, after)
    require(drift is not None and drift <= POSE_TOLERANCE_MM, "restore_failed",
            "Instances did not return to their home poses after rendering (off by %s mm)." % (round(drift, 4) if drift is not None else "a missing instance"))
    # isModified is not compared: any move sets it and nothing in the API clears it (measured on a saved design)
    require(document.isSaved == flags[0] and not snapshots.hasPendingSnapshot, "restore_failed",
            "Document state changed during rendering.")
    modified = bool(document.isModified and not flags[1])
    # the revision digest reads exact transforms and revision ids, which moving and reverting can change:
    # hand back the target as it is now, so the caller's next write is not refused as stale
    return {"target": current_target(app), "sourceTarget": value, "folder": str(folder), "frames": written, "fps": fps, "size": [width, height],
            "documentModified": modified, **({"documentModifiedNote": MOVED_NOTE} if modified else {}),
            "durationSeconds": round(len(times) / fps, 3), "playback": playback, "captions": captions(steps, total, playback),
            "pattern": "frame_%05d.png", "restored": True, "restoredWithinMm": round(drift, 6), "heldBack": held, "moves": mover.moves,
            "glyphs": glyphs.report(),
            "encode": "ffmpeg -framerate %d -i frame_%%05d.png -c:v libx264 -pix_fmt yuv420p -crf 18 exploded.mp4" % fps}


OPERATIONS = {"exploded_view_render": exploded_view_render}
WRITES = {"exploded_view_render"}
