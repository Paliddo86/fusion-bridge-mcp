"""Photoreal pictures of a design: Fusion's local renderer from a chosen view, lighting environment and
backdrop, waited on, with the camera and every render and scene setting it touched put back. Journaled,
since a high-quality render of a big assembly runs for minutes.

The image is rendered into the bridge's own staging folder and moved to `filename` only when the render
finished in time, so a failed or timed-out render leaves the destination as it was. A render that runs
out of time is cancelled (render_process ends its RenderProcess.exe, since Fusion's API has no cancel),
so it writes nothing; one that cannot be cancelled (still queued) loses its staging folder, which Fusion does
not recreate, so it writes nothing either."""
import os
from pathlib import Path
import shutil
import tempfile
import threading
import time
import uuid

import adsk.core
import adsk.fusion

from . import render_process
from .core import SESSION_ID, MechanicalError, require, target
from .glyphs import Glyphs

VIEWS = {"iso": "IsoTopRightViewOrientation", "iso-left": "IsoTopLeftViewOrientation", "front": "FrontViewOrientation",
         "back": "BackViewOrientation", "top": "TopViewOrientation", "bottom": "BottomViewOrientation",
         "left": "LeftViewOrientation", "right": "RightViewOrientation"}
# custom environment files Fusion's renderer reads, by their first bytes (Radiance HDR, OpenEXR)
ENVIRONMENT_FILES = {".hdr": b"#?", ".exr": b"v/1\x01"}
BACKGROUND_HELP = 'background is "environment" or a colour "#rrggbb".'
STAGING = "adom-fusion-render"
# a staging folder older than this belongs to no running render of this bridge
STALE_SECONDS = 3600


def staging_root():
    return Path(tempfile.gettempdir()) / STAGING


def sweep(root, now=None):
    """Remove staging folders an earlier call left behind. A backstop: each call removes its own, and Fusion
    did not recreate a removed folder for a late image (measured)."""
    now = time.time() if now is None else now
    if not root.is_dir():
        return
    for folder in root.iterdir():
        try:
            if folder.is_dir() and now - folder.stat().st_mtime > STALE_SECONDS:
                shutil.rmtree(folder, ignore_errors=True)
        except OSError:
            pass


def publish(staged, destination, overwrite):
    """Move the finished image to the destination in one step. Without overwrite, a file that appeared
    there during the render is kept and the call is refused."""
    partial = destination.with_name(".%s.%s.partial" % (destination.name, uuid.uuid4().hex[:8]))
    try:
        try:
            os.replace(str(staged), str(partial))
        except OSError:
            # the staging folder is on another volume
            shutil.copyfile(str(staged), str(partial))
        if overwrite:
            os.replace(str(partial), str(destination))
            return
        try:
            os.link(str(partial), str(destination))
        except FileExistsError:
            raise MechanicalError("destination_exists", "A file appeared at filename during the render; pass overwrite:true to replace it.")
        except OSError:
            # no hard links on this volume (FAT, some shares): check, then rename
            require(not destination.exists(), "destination_exists", "A file appeared at filename during the render; pass overwrite:true to replace it.")
            os.replace(str(partial), str(destination))
    finally:
        if partial.exists():
            partial.unlink()


def parse_background(value):
    """None (the document's own backdrop), "environment", or an (r, g, b) colour from "#rrggbb"."""
    if value is None or value == "environment":
        return value
    rgb = None
    if isinstance(value, str) and len(value) == 7 and value[0] == "#":
        try:
            rgb = tuple(int(value[i:i + 2], 16) for i in (1, 3, 5))
        except ValueError:
            rgb = None
    require(rgb is not None, "invalid_input", BACKGROUND_HELP)
    return rgb


def library(manager):
    """Fusion's own environments (the Scene Settings library), without its 'Custom...' placeholder."""
    environments = manager.renderEnvironments
    rows = [environments.item(i) for i in range(environments.count)]
    return [row for row in rows if not row.isCustomEnvironment]


def find(rows, name):
    wanted = name.strip().lower()
    return next((row for row in rows if row.id.lower() == wanted or row.name.lower() == wanted), None)


def parse_environment(manager, value):
    """A library environment by name or id (any case), or an absolute .hdr or .exr file, checked but not
    loaded yet. Returns (library RenderEnvironment or None, description for the reply)."""
    require(isinstance(value, str) and value.strip(), "invalid_input", "environment must name an environment or an .hdr/.exr file.")
    path = Path(value.strip())
    if path.suffix.lower() in ENVIRONMENT_FILES or "/" in value or "\\" in value:
        suffix = path.suffix.lower()
        require(path.is_absolute() and suffix in ENVIRONMENT_FILES, "invalid_environment",
                "A custom environment is an absolute path to an .hdr or .exr file.")
        require(path.is_file(), "environment_not_found", "There is no environment file at %s." % path)
        with path.open("rb") as handle:
            head = handle.read(len(ENVIRONMENT_FILES[suffix]))
        # never hand Fusion a file its loader would choke on
        require(head == ENVIRONMENT_FILES[suffix], "invalid_environment", "%s is not a Radiance HDR or OpenEXR image." % path.name)
        return None, {"name": path.name, "file": str(path), "custom": True}
    rows = library(manager)
    environment = find(rows, value)
    require(environment is not None, "unknown_environment", "Fusion has no environment named %s." % value.strip(),
            available=[row.name for row in rows])
    return environment, {"name": environment.name, "id": environment.id, "custom": False}


def load_environment(described):
    """Fusion's environment for a checked custom file. Fusion keeps one custom environment per design, so
    this replaces the design's own custom one once it is assigned."""
    try:
        # Fusion raises (3 : File does not exist) rather than returning None when it cannot load one
        environment = adsk.fusion.RenderEnvironment.loadCustomEnvironment(described["file"])
    except Exception as error:
        raise MechanicalError("environment_load_failed", "Fusion could not load %s as an environment: %s" % (described["name"], str(error)[:200]))
    require(environment is not None, "environment_load_failed", "Fusion could not load %s as an environment." % described["name"])
    return environment


def color(rgb):
    return adsk.core.Color.create(rgb[0], rgb[1], rgb[2], 255)


def scene_state(scene):
    """The document's backdrop: the environment shown behind the model (which also lights it), or a solid
    colour. Behind a solid colour the API cannot read which environment lights the scene."""
    if scene.backgroundType == adsk.fusion.RenderSceneBackgroundTypes.EnvironmentRenderSceneBackgroundType:
        environment = scene.backgroundEnvironment
        return {"background": "environment", "environment": environment, "id": environment.id, "custom": environment.isCustomEnvironment}
    shade = scene.backgroundSolidColor
    return {"background": "color", "color": (shade.red, shade.green, shade.blue)}


def same_scene(a, b):
    keys = ("background", "id") if a["background"] == "environment" else ("background", "color")
    return all(a.get(k) == b.get(k) for k in keys)


def lighting_environment(app, manager):
    """The environment lighting a scene behind a solid backdrop, or None. backgroundEnvironment
    refuses to answer there, so it is asked of the Render workspace (Render.GetEnvironment, a text command
    that only answers in it), and the workspace that was active comes back."""
    ui = app.userInterface
    previous = ui.activeWorkspace
    try:
        if not manager.isRenderWorkspaceActive:
            require(manager.activateRenderWorkspace(), "scene_unreadable", "Fusion did not open the Render workspace.")
        name = app.executeTextCommand("Render.GetEnvironment")
    except MechanicalError:
        raise
    except Exception as error:
        raise MechanicalError("scene_unreadable", "Fusion did not name the scene's environment: %s" % str(error)[:200])
    finally:
        if previous is not None and ui.activeWorkspace is not None and ui.activeWorkspace.id != previous.id:
            previous.activate()
    if not (isinstance(name, str) and name.strip()):
        return None
    # a custom environment is named after the library's 'Custom...' slot, which holds the design's own file
    environments = manager.renderEnvironments
    return find([environments.item(i) for i in range(environments.count)], name)


def save_settings(rendering):
    names = ("aspectRatio", "resolution", "resolutionWidth", "resolutionHeight", "renderQuality", "isBackgroundTransparent")
    return {name: getattr(rendering, name) for name in names}


def same_settings(before, after):
    """The document's render settings as two fresh reads saw them. Under the current-viewport aspect ratio the
    size follows the viewport (in physical pixels), so a window resized meanwhile is not a change."""
    viewport = adsk.fusion.RenderAspectRatios.CurrentViewportRenderAspectRatio
    keys = [k for k in before if not (before["aspectRatio"] == after["aspectRatio"] == viewport and k in ("resolutionWidth", "resolutionHeight"))]
    return {k: [before[k], after[k]] for k in keys if before[k] != after[k]}


def restore_settings(rendering, saved):
    """Width and height set the resolution to custom, and a preset resolution sets width, height and the
    aspect ratio; so the size goes back first and a preset after it. (Fusion gives each caller its own
    Rendering object, so this only tidies render_image's own; it guards against that ever changing.)"""
    rendering.aspectRatio = saved["aspectRatio"]
    rendering.resolutionWidth = saved["resolutionWidth"]
    rendering.resolutionHeight = saved["resolutionHeight"]
    if saved["resolution"] != adsk.fusion.RenderResolutions.CustomRenderResolution:
        rendering.resolution = saved["resolution"]
    rendering.renderQuality = saved["renderQuality"]
    rendering.isBackgroundTransparent = saved["isBackgroundTransparent"]


def set_scene(scene, before, environment, background, lighting):
    """The lighting environment and the backdrop asked for. Setting an environment also puts it behind
    the model, and setting a colour keeps the environment lighting the scene."""
    try:
        if environment is not None:
            scene.backgroundEnvironment = environment
        if isinstance(background, tuple):
            scene.backgroundSolidColor = color(background)
        elif background == "environment" and environment is None and before["background"] == "color":
            scene.backgroundEnvironment = lighting
        elif background is None and environment is not None and before["background"] == "color":
            # the document's own solid backdrop stays
            scene.backgroundSolidColor = color(before["color"])
    except Exception as error:
        raise MechanicalError("environment_unavailable", "Fusion refused the environment or backdrop: %s" % str(error)[:300])


def put_back(rendering, saved, scene, saved_ground, before, lighting, viewport, camera):
    """Every setting the render changed, each step on its own so one failure cannot keep the camera or
    the scene from coming back. Returns the steps that failed."""
    def scene_back():
        if before["background"] == "environment":
            scene.backgroundEnvironment = before["environment"]
            return
        if lighting is not None:
            scene.backgroundEnvironment = lighting
        scene.backgroundSolidColor = color(before["color"])

    def camera_back():
        camera.isSmoothTransition = False
        viewport.camera = camera
        viewport.refresh()

    failed = []
    for name, step in (("settings", lambda: restore_settings(rendering, saved)),
                       ("ground", lambda: setattr(scene, "isGroundDisplayed", saved_ground)),
                       ("scene", scene_back), ("camera", camera_back)):
        try:
            step()
        except Exception as error:
            failed.append({"step": name, "error": str(error)[:200]})
    return failed


def timeout_message(seconds, report):
    if report["cancelled"]:
        return ("The render did not finish in %g s and was cancelled: its render process was ended and Fusion dropped the job, "
                "so it wrote nothing. Lower quality or size, or allow more time." % seconds)
    return ("The render did not finish in %g s and could not be cancelled (%s). Its staging folder is gone, so it cannot "
            "write its image anywhere, and never to filename." % (seconds, report["reason"]))


def render_image(app, args):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Rendering requires the main thread.")
    require(args.get("sessionId") == SESSION_ID, "session_mismatch", "Rendering requires the current session.")
    destination = Path(args["filename"])
    require(destination.is_absolute() and destination.suffix.lower() in (".png", ".jpg", ".jpeg"), "invalid_destination",
            "filename must be an absolute .png or .jpg path.")
    transparent = bool(args.get("transparent", False))
    require(not (transparent and destination.suffix.lower() != ".png"), "invalid_destination",
            "A transparent background needs a .png filename; JPEG has no transparency.")
    background = parse_background(args.get("background"))
    require(not (transparent and background is not None), "invalid_input",
            "transparent leaves nothing behind the model; drop background or transparent.")
    overwrite = bool(args.get("overwrite", False))
    require(overwrite or not destination.exists(), "destination_exists", "Pass overwrite:true to replace the image.")
    document, design, value = target(app, args)
    manager = design.renderManager
    environment, described = parse_environment(manager, args["environment"]) if "environment" in args else (None, None)
    flags = (document.isSaved, document.isModified)
    # every renderManager.rendering is a separate object (measured): render_image sets up its own, and a fresh
    # one shows what the document holds, before and after
    document_settings = save_settings(manager.rendering)
    rendering, scene, viewport = manager.rendering, manager.sceneSettings, app.activeViewport
    saved, saved_ground, before = save_settings(rendering), scene.isGroundDisplayed, scene_state(scene)
    # behind a solid backdrop the lighting environment is unreadable; it is looked up only when it changes
    lighting = None
    if before["background"] == "color" and (described is not None or background == "environment"):
        lighting = lighting_environment(app, manager)
        require(lighting is not None, "scene_unreadable",
                "Fusion did not name the environment lighting the document's scene, so it could not be shown or put back; "
                "render without environment.")
    own_custom = before.get("custom") or (lighting is not None and lighting.isCustomEnvironment)
    require(not (own_custom and described is not None and described["custom"]), "custom_environment_in_use",
            "The document uses a custom environment of its own, and Fusion keeps one custom environment per design, so "
            "loading another file would lose it. Use a library environment, or render without environment.")
    if described is not None and described["custom"]:
        environment = load_environment(described)
    camera = viewport.camera
    glyphs = Glyphs(design)
    destination.parent.mkdir(parents=True, exist_ok=True)
    sweep(staging_root())
    stage = staging_root() / ("render-" + uuid.uuid4().hex)
    stage.mkdir(parents=True)
    staged = stage / ("render" + destination.suffix.lower())
    started = time.perf_counter()
    failed = []
    try:
        try:
            rendering.aspectRatio = adsk.fusion.RenderAspectRatios.CustomRenderAspectRatio
            rendering.resolutionWidth = int(args.get("width", 1920))
            rendering.resolutionHeight = int(args.get("height", 1080))
            rendering.renderQuality = int(args.get("quality", 75))
            rendering.isBackgroundTransparent = transparent
            if "ground" in args:
                scene.isGroundDisplayed = bool(args["ground"])
            set_scene(scene, before, environment, background, lighting)
            view = args.get("view", "iso")
            if view != "current":
                # the renderer never draws joint, sketch or construction glyphs, but the fit counts them:
                # hidden while fitting, the view frames the parts, and they are back before the render
                glyphs.hide()
                framed = viewport.camera
                framed.isSmoothTransition = False
                framed.viewOrientation = getattr(adsk.core.ViewOrientations, VIEWS[view])
                viewport.camera = framed
                viewport.fit()
                glyphs.restore()
                viewport.refresh()
            wall = time.time()
            future = rendering.startLocalRender(str(staged))
            require(future is not None, "render_failed", "Fusion did not start the render.")
            timeout = float(args.get("timeoutSeconds", 600))
            states = adsk.fusion.LocalRenderStates
            state = render_process.state_of(future)
            while state not in (states.FinishedLocalRenderState, states.FailedLocalRenderState):
                require(state is not None, "render_failed", "Fusion dropped the render job before it finished (its render process ended).")
                if time.perf_counter() >= started + timeout:
                    report = render_process.cancel(future, wall, states, pump=adsk.doEvents)
                    raise MechanicalError("render_timeout", timeout_message(timeout, report), cancel=report)
                adsk.doEvents()
                time.sleep(0.2)
                state = render_process.state_of(future)
            require(state == states.FinishedLocalRenderState, "render_failed", "Fusion's renderer reported a failure.")
        finally:
            glyphs.restore()
            failed = put_back(rendering, saved, scene, saved_ground, before, lighting, viewport, camera)
        require(glyphs.restored, "restore_failed", "Object Visibility switches did not come back after rendering: %s." % ", ".join(glyphs.failed))
        require(staged.is_file() and staged.stat().st_size > 0, "render_failed", "The rendered image was not written.")
        publish(staged, destination, overwrite)
    except MechanicalError as error:
        if failed:
            error.details["restoreFailed"] = failed
        raise
    finally:
        shutil.rmtree(str(stage), ignore_errors=True)
    changed = same_settings(document_settings, save_settings(manager.rendering))
    if scene.isGroundDisplayed != saved_ground:
        changed["isGroundDisplayed"] = [saved_ground, scene.isGroundDisplayed]
    out = {"target": value, "filename": str(destination), "bytes": destination.stat().st_size,
           "size": [future.imageWidth, future.imageHeight], "seconds": round(time.perf_counter() - started, 1),
           "view": args.get("view", "iso"), "quality": int(args.get("quality", 75)),
           "environment": described or "document", "background": args.get("background", "document"),
           "settingsRestored": not changed, "sceneRestored": same_scene(before, scene_state(scene)) and not failed,
           "glyphsHiddenForFit": glyphs.report()["hidden"], "documentFlagsUnchanged": (document.isSaved, document.isModified) == flags}
    if changed:
        out["settingsChanged"] = changed
    if failed:
        out["restoreFailed"] = failed
    return out


OPERATIONS = {"render_image": render_image}
WRITES = {"render_image"}
