# Mechanical design bridge

Mechanical operations use three public verbs: `fusion_mech_capabilities`, `fusion_mechanical` and `fusion_mech_operation_status`. Discover the live schema before constructing a request. Each operation returns `success`; inspect that field after unwrapping the Bridge transport envelope.

## Start a session

1. Call `fusion_mech_capabilities {}`. Keep its `sessionId` and operation schemas. The bridge starts Fusion first when no add-in session is live; `sessionId` stays null until the add-in has loaded.
2. Call `fusion_mechanical {"op":"inspect"}`. Keep the returned `data.target` and entity references.
3. Before transactional sketch, feature, assembly, supplier-import, material or atomic parameter mutations on an existing document, call `transaction_probe` with the session ID, a new operation ID and the current target. It proves commit/abort on this Fusion session using a temporary parameter, removes the probe and returns a fresh target. `new_design` performs this probe automatically when needed.
4. Use the exact fresh target for the next write. Fetch references again after edits; old revisions are deliberately rejected. Pass reference objects unchanged, including the opaque `rootComponentId` used for root-component references.

To create a separate design, call `new_design` with the session ID and a unique operation ID. The optional `box` accepts width/height/depth in mm as a simple parametric fixture. A requested name is metadata; the scratch document remains unnamed until an explicit save to avoid Fusion's save dialogs.

```json
{
  "op": "new_design",
  "sessionId": "<from capabilities>",
  "operationId": "part-create-001",
  "name": "Mounting bracket"
}
```

Existing-document mutations also require `target: {"documentId":"...","revision":"..."}` from inspection. Never fabricate or cache these values across an add-in restart.

## Big assemblies

The mechanical layer is measured on a 4,579-occurrence, 1,249-component production assembly built from referenced designs:

| Call | Time |
|---|---|
| revision digest (every guarded call pays it) | 5 s |
| `inspect` with `detail:"target"` | 5 s |
| `inspect` (full) | 13 s |
| resolving one occurrence reference | 6 ms |
| `bom` (part numbers cached) | 15 s |
| timeline health walk (689 items) | 14 s |
| recovery witness, bodies already read | 7 s |
| `parameter_create` (a model write without a full recompute) | 46 s |
| `Design.computeAll` | 66 s |

- On a design with more than 200 components, `inspect` leaves out each body's physical properties and bounds (`bodyMetrics:true` reads them; `bodyMetricsRead` says which you got). With more than 200 timeline items it leaves out the timeline health walk, which reads every item at about 23 ms apiece there (`health:true` reads it, or call `health`).
- The first guarded call after the add-in loads registers every entity once: on that assembly, the first revision digest takes about 45 s instead of 5 s.
- Writes keep their guards: a health check before, Fusion's own compute and a health check after, and the recovery witness. The full recompute runs at checkpoints ([Recompute](#recompute)). The witness reads again only the bodies whose revision changed since the last one.
- The witness reads every body the first time, 2,642 bodies in about 300 s on that assembly. A full recompute changes the revision of every body the design computes itself (1,591 there; bodies of referenced designs keep theirs), so the next witness reads those again (about 220 s). The write after a checkpoint, and `transaction_probe`, which recomputes in full and then takes the witness, pay that: the probe there would take about 5 minutes.
- That assembly carries 15 old timeline faults (warnings) and three sketches holding relationships the graph reader cannot read (rectangular pattern constraints, and midpoint constraints linked outside their sketch). Writes are held to the faults already there, and only the sketches a write names must read completely ([Designs that already carry faults](#designs-that-already-carry-faults)).

## Recompute

Fusion recomputes what an edit touches as the edit is made. Autodesk keeps Compute All for checkpoints: major milestones, significant changes, edits upstream in the timeline that later features reference, and a check before the design leaves Fusion. Writes follow that lead. Each write reads the timeline's health after Fusion's own compute and refuses (and rolls back) a result that adds a fault or changes one the model already had ([Designs that already carry faults](#designs-that-already-carry-faults)). `Design.computeAll` runs at a checkpoint:

- **Significance.** Writes since the last full recompute add up their weights (`compute.py`):
  - 0: metadata, materials and new parameters;
  - 0.5: appended sketches, planes and axes;
  - 1: features, components, inserts and occurrence moves;
  - 1.5: parameter changes;
  - 2: edits of existing features, sketches and joint origins, and joints, rigid groups and motion links;
  - 3: deletes.

  The write that brings the total to 6 runs the full recompute before it commits.
- **A move to another part.** A write whose references sit under a different top-level occurrence than the last write's checkpoints the previous part's edits first, once they add up to at least 2 (an edit of existing history, a joint, a delete, or two appended features).
- **Export.** `export` runs the full recompute first when anything changed since the last one.
- **On request.** `health {recompute:true}` runs it and starts the counters again: at a milestone, or before saving a long run of edits.

Fusion computes on its main thread either way, so none of this runs in the background; the policy only chooses when. Failure recovery still recomputes in full. While the revision is unchanged, a write's pre-check reuses the health the previous write verified, so consecutive writes walk the timeline once each. Every write reply and `health` carry `compute`: `full`, `reason`, `seconds`, `significance`, `checkpointAt`, `writesSinceFullCompute`, `focus` and `lastFullCompute`. The counters are per document, for the add-in session.

## Designs that already carry faults

A production design can carry timeline faults that nobody is about to fix, such as a plane or sketch whose face was deleted long ago. A write is held to the faults the model carried before it (its baseline), not to a perfect model:

- The baseline is the health the previous write verified at the same revision, otherwise a walk of the timeline: the same walk as before, so it costs no extra walk. Comparing 15 faults takes under 15 µs.
- A fault is the same fault when its timeline item's name, health state and message are the same. Its index can move when an earlier item is deleted.
- The write succeeds when its result adds no fault, changes none, and leaves no fault on what it edits in place: the feature of `feature_edit`, the feature that owns a parameter it sets, the sketch of a sketch edit, or the occurrence, joint, rigid group, joint origin or motion link it edits. Inputs it only reads, such as a profile, an axis or joint geometry, do not count.
- Otherwise it refuses with `unhealthy_model` and rolls back as before. `newFaults` lists the faults it added or changed, and `editedFaults` the faults left on what it edited. Recovery is verified against the same baseline.
- The replies of model writes (the transactional operations, `parameter_set` and `parameter_create`), `transaction_probe` and `export` carry `preExistingFaults`: `count`, the first five `faults`, `more` when there are more, and `cleared` for the faults the write removed (deleted, suppressed or repaired). While old faults remain, `health.healthy` stays false on a write that succeeded.
- A full recompute at a checkpoint is held to the same baseline (`Design.computeAll` succeeds on a model that carries faults; a failed one still refuses). On the 4,579-occurrence assembly the first full recompute after opening showed the same 15 faults. Should one ever show a fault Fusion had not computed since the design opened, `health {recompute:true}` computes the design and the next write takes its baseline from that.
- `transaction_probe` and `export` follow the same rule. With a healthy baseline the rule is the old one: any fault refuses.

Sketches: the recovery witness records every sketch's constraint graph. The sketches a write names, or whose curve, point, constraint or dimension it names, must read completely, or the write refuses with `unsupported_constraint_graph` before it changes anything. A profile's sketch does not count: a feature reads only the profile. Every other sketch enters the witness with what can be read, and a relationship type the reader does not support (a polygon constraint, for example) enters as its type, so it no longer blocks writes elsewhere in the design. `inspect_entity` on such a sketch lists those relationships in `constraintGraph.unreadable` (`complete: false`).

## Plan exploded assembly steps

`exploded_view_plan` returns an immutable source-bound instance/home-pose plan,
ordered translation steps, component-linked labels and finite sampled matrices.
Use current session/target, exact occurrence references, explicit mm distances and
home-fixed `world`, `parent` or `local` directions. Parent motion is inherited once;
explicit child offsets add. `playback: "assemble"` reverses the same timeline and
returns exact home poses at its endpoint. Optional titles and instructions accompany
the step/progress sequence. See [the contract and limits](EXPLODED-VIEWS.md).

A design over 256 occurrences gets a plan of just the occurrences the steps move and their
parents (`instanceScope: "moved"`), each found by name level by level, never by walking the whole
tree; other occurrences keep their home pose and children follow their parent. Components inserted
by reference are planned too. Every bound refuses with `plan_budget` and a `hint`, and the native
reads stop at 40 s, inside the relay's 60 s for a read. Measured: 128 moves in a 4,590-occurrence
design planned in 4.5 s, most of it the revision checks every guarded operation runs.
`exploded_view_render` now tracks up to 4,096 moved parts with their parents, like
`exploded_view_stills` (it stopped at 256).

This operation performs native reads and pure pose calculations only. It does not
move occurrences, suppress joints, create graphics, change BOM quantities, create
a storyboard, render images or export video. Native module-function qualification
passed six instances across 60 poses, source/joint/BOM preservation and F3D reopening;
installed public routing remains untested. See [the evidence](build-2026-10-06-exploded-plans.md).
A [finite exploded-preview controller candidate](EXPLODED-PREVIEWS.md) now exists in source (61 typed operations): independent copied graphics, explicit finite samples/show/dispose, and session-owned presentation leases. Its native backend and installed public routing remain unqualified; the first backend attempt stopped before graphics creation. Camera framing and visible-output acceptance are separate future gates.

## Render an exploded-view animation

`exploded_view_render` takes the same `unit`, `steps` and `playback` as `exploded_view_plan`, plus an absolute `folder`, `fps` (default 24), `width` and `height` (default 1280 x 720) and `holdSeconds` (default 1). It plays the steps on the real occurrences and saves one viewport image per frame, moving only the parts whose pose changes from one frame to the next (`moves` in the reply; each move costs about 44 ms on a 4,579-occurrence assembly). The camera is fitted to the most exploded state unless `fit:false` keeps the current view. Afterwards it reverts every move and verifies that each instance is home and that the document's saved flag is unchanged. Fusion marks a document modified whenever a part moves, even after the move is reverted, and its API cannot clear that flag (measured on a saved design), so the reply's `documentModified` says when the render set it; the parts are back where they were and there is nothing to save. It refuses a design that already has an uncaptured move, since the revert would discard it. It is journaled. The reply lists the frames, timed `captions` from the steps' titles and instructions, and the ffmpeg line that joins the frames into MP4; Fusion's API has no video export. A three-step check (screws out, lid off, housing slid off the base) rendered 133 frames, in both explode and assemble order, and was joined into captioned MP4. At the most exploded pose it checks that every part reached its planned pose and names any that did not in `heldBack` (with how far off), so a part a constraint holds never just stands still in the video; live, a rigidly jointed post, component-pattern copies, a nested child moved in its parent's frame and a rotated part moved along its own axis all reached their poses and came back home. Frames show the parts without joint, sketch or construction glyphs unless `glyphs:true` (see [Clean captures](#clean-captures)).

## Render step images for assembly instructions

`exploded_view_stills` makes the pictures for step-by-step assembly instructions. Its `steps` come in **assembly order**, and each move gives the direction and distance its part comes from (the way it moves out when exploded); `title` and `instruction` are optional. For every step it saves two images from one fixed orthographic camera:

- `step_NNN_before.png`: that step's parts hovering where they go, earlier steps seated and later steps' parts moved well out of frame;
- `step_NNN_after.png`: the same with the step's parts seated.

`step_000_after.png` shows the view before anything is assembled, so each image's predecessor is its context: the difference between `step_003_before.png` and `step_002_after.png` is exactly the parts step 3 brings in, which is how a viewer highlights them. The reply lists, per step, every moved part's `point` (its centre) and `box` in the before and after images, normalized to the image (x right, y down, 0 to 1), for callouts and insertion arrows, plus the `camera` (eye, target, up, `extentsMm`) and the rule that projects a point.

`camera` chooses the view:

- `auto` (the default): of the four isometric views from above, the one that best shows the parts coming in: each step's approach direction should cross the screen rather than point along the line of sight, and come from the camera's side. It frames the moved parts, seated and exploded, so a subassembly inside a big assembly fills the picture.
- `fit`: the current view direction, framing the moved parts the same way.
- `current`: the view as it is, made orthographic.

`context` chooses what else shows. `auto` (the default): when every moved part sits in one subassembly, the occurrences outside it that are visible move out of view in every image, so a panel inside a big assembly appears on its own; parts at the root hide nothing, so a base part always stays. `keep` shows the whole design.

Fusion frames an orthographic image so that its shorter side spans the camera's `viewExtents`, centred on the target (measured at 1280 x 720, 960 x 960 and 720 x 1280, every point within a pixel); the projections in the reply follow that rule. It moves only the parts whose pose changes between images, about four moves per moved part in all. It takes up to 64 steps and 2048 moves (an animation takes 32 and 128) and tracks up to 4,096 moved parts with their parents. Like the animation, the images leave out joint, sketch and construction glyphs unless `glyphs:true` (see [Clean captures](#clean-captures)). Afterwards it reverts every move, restores the camera, verifies every part is home and reports `documentModified` like the animation. It is journaled. Measured on a 4,579-occurrence assembly: an electronics panel of 4 steps and 92 parts gave 9 images in 40 s with 368 moves.

## Plan an assembly order: layout and rays

Two reads give an assembly planner what it needs from the design. Neither changes anything.

`assembly_layout` lists one scope's parts: every visible occurrence directly under `scope`, a full occurrence path such as `Cell:1+Panel:1` (the root's when `scope` is omitted). Each part has its `path`, `name`, `component` name, `grounded`, the number of its `children` and its `box`: axis-aligned in root space, in mm, from the part's bodies and its children's, `null` for a part without bodies. The box is read breadth first from at most `maxBodies` bodies (default 200; a body's box costs about 26 ms on a 4,579-occurrence assembly), and `boxCapped` marks a part whose walk stopped there. The reply also lists the scope component's unsuppressed joints and as-built joints as `[partOne, partTwo, "joint" | "asBuilt"]` and its rigid groups, both as part paths (`""` is the scope's own geometry), with `frameBodies`, the bodies of the scope component itself. It is journaled like `efficiency_report` (`sessionId`, `operationId`): on a big assembly the first guarded read after the add-in loads registers every entity, about 45 s, so follow it with `fusion_mech_operation_status` once the relay's 60 s pass. `target` is optional, as for `inspect`, and the reply carries the current target.

`ray_cast` casts rays against named occurrences. `occurrences` lists full occurrence paths; each ray has an `origin` and a `direction` in root space (mm), a `length`, and `against`, indexes into `occurrences`. Each ray is cast against each occurrence it names in that occurrence's own component coordinates (the inverse of its root-space transform), hidden bodies included: about a millisecond on a part, where a ray against the component of a big subassembly (thousands of bodies, in a 4,579-occurrence assembly) took a quarter of a second. `hits` has a row per ray answered: `[index, distance]` for each occurrence the ray meets within `length` (its nearest hit), nearest first, distances in mm rounded to 0.01. A ray that starts inside a body meets that body's exit face. `missing` lists the indexes with no occurrence at their path, and `failed` counts the casts Fusion refused.

`ray_cast` is not journaled. A plan casts thousands of rays, and the journal keeps every request and result and never evicts (10,000 records, 64 MiB), so journaled batches would fill it within tens of plans. Instead a call stops at `budgetSeconds` (default 30, at most 50) or before its reply passes 1 MiB, answers the first `cast` rays and says `complete: false` and why (`stopped`); send the rest in the next call. Every call answers at least one ray. The `adom-bridge` CLI passes the arguments on its command line, which Linux caps at 128 KiB, so a big batch goes in several calls of a few hundred rays.

The guard is the usual one: the active document must be the target's, and with a `revision` the model must still be at it (`stale_revision` otherwise). Without a revision `ray_cast` computes no revision digest (about 5 s a call on a 4,579-occurrence assembly) and its reply's target carries none. A planner reads the layout, which returns the full target, casts with `{documentId}` alone, and checks the revision once at the end with a `ray_cast` that has the full target and no rays.

Measured through the relay: a `ray_cast` call answers in about 0.85 s round trip on a small design. A planner read a 15-part and an 11-part testbed (850 and 760 distinct rays) in five calls each; its whole plan took 7.1 to 7.8 s, where the same reads as modeling scripts took 11.3 to 11.7 s, with identical results.

## Render a picture

`render_image` renders a photoreal image with Fusion's local renderer to an absolute `.png` or `.jpg` `filename`. The `view` is `iso` by default, or `iso-left`, `front`, `back`, `top`, `bottom`, `left` or `right`, each fitted to the design; `current` keeps the view as it is. It takes `width` and `height` (1920 x 1080 by default), `quality` from 25 to 100 (75 by default; Fusion refuses less than 25), `transparent` and `ground`. Fusion's renderer never draws joint, sketch or construction glyphs, but fitting a standard view counts them, so they are hidden while the view is fitted and back before the render starts (`glyphsHiddenForFit` in the reply; see [Clean captures](#clean-captures)). It is journaled, since a large high-quality render runs for minutes. A 1280 x 720 iso render at quality 60 took 28 s.

`environment` chooses the environment that lights the scene: one of Fusion's library environments by name or id, in any case (Sharp Highlights, Rim Highlights, Cool Light, Warm Light, Soft Light, Grid Light, Crossroads, Dry lake bed, Field, Photobooth, Plaza, Skylight, Snow Field; `render_scene_inspect` lists them with their ids), or an absolute path to an `.hdr` or `.exr` image, whose header is checked before Fusion sees it. `background` chooses what shows behind the model: `"environment"` (the environment image) or a colour `"#rrggbb"`. By default the document's own backdrop stays, so behind a solid backdrop a new environment changes only the light and the reflections. `transparent` (PNG only) leaves nothing behind the model and does not combine with `background`. An unknown name is refused with the library's names in `available` (`unknown_environment`). Fusion fetches some library environments the first time each is used in a session (Crossroads 2.4 s, Field 1.5 s, Photobooth 4.0 s, Skylight 2.0 s, Snow Field 2.5 s); after that, and for the studio ones, setting an environment takes under 10 ms.

Fusion's API can choose the environment (earlier notes here said it could not): `SceneSettings.backgroundEnvironment` sets the scene's environment, which both lights the model and shows behind it, and `backgroundSolidColor` then makes the backdrop solid while that environment keeps lighting the scene (measured: Plaza and Grid Light behind the same white backdrop give their own reflections). `RenderEnvironment.loadCustomEnvironment` loads a file; it raises for a file it cannot load. The size, quality and transparency are set on render_image's own `Rendering` object: Fusion hands every caller a separate one that starts from the document's settings (measured: a change made through one was invisible through the next), so the document's render settings never change, and `settingsRestored` confirms it from fresh reads before and after (`settingsChanged` would name a difference; under the current-viewport aspect ratio the size follows the viewport and is not compared). Afterwards render_image puts back the camera and the document's scene, each step on its own so that one failure cannot keep the others from coming back, then reads the scene again for `sceneRestored` (a failed step is listed in `restoreFailed`). It also reports whether the document's saved and modified flags are unchanged. A document whose backdrop is an environment gets that environment back by assigning the same object, which brings back a custom file too (measured: the restored render matched the original file's to a mean of 0.03 levels). Behind a solid backdrop the API cannot read which environment lights the scene (the getter refuses), so when a render changes it, render_image asks the Render workspace (`Render.GetEnvironment`, a text command that answers only there) and returns to the workspace that was active: 0.9 s in and 0.1 s back, with no dialog. Fusion keeps one custom environment per design: loading another file into a design replaces its own (measured: re-assigning the original object then gave the new file), while a file loaded in another document leaves it alone. So a design that has a custom environment of its own takes library environments (its own comes back, also from behind a solid backdrop, where Fusion names it `Custom...`), and a second custom file is refused (`custom_environment_in_use`) rather than lose it.

The render goes to the bridge's staging folder (`%TEMP%\adom-fusion-render`) and the image is moved to `filename` only when the render finished in time, so a failed or timed-out render leaves an existing file as it was. Fusion's API cannot cancel a local render: `RenderFuture` has no method for it and no text command stops one. Each local render runs in its own `RenderProcess.exe`, a child of Fusion that exits once the image is written, one render at a time (later ones wait in a queue). At `timeoutSeconds` render_image ends that process, and the `render_timeout` reply carries `cancel` (`cancelled`, `renderState`, `reason`). Ending it takes about 0.3 s: Fusion drops the job (reading its state then raises "render job doesn't exist"), writes no image, raises no dialog, logs no error, adds nothing to the rendering gallery and leaves no process or temporary file behind; the next render runs normally and a render queued behind it starts. Measured with a render that takes 24.9 s left alone: ended at about 6 s, and 55 s after it started (twice its own time) no image had appeared. Only a render that is processing can be cancelled this way. One still queued behind another render has no process yet, and another render's process is never touched: the process ended must be a `RenderProcess.exe` child of this Fusion created after this render started, and the only one. A render that could not be cancelled loses its staging folder when the call returns, and Fusion does not recreate it, so its image is written nowhere (measured: the render reported finished and no file or folder appeared); a later render also sweeps staging folders older than an hour. It never reaches `filename`.

The viewport frames of `exploded_view_render` and `exploded_view_stills` do not use the render scene: two frames captured with different scene environments were identical, so those operations take no `environment`.

## Clean captures

A viewport image shows what the graphics window shows. So `exploded_view_render` and `exploded_view_stills` hide joint and joint origin glyphs, sketches, origin planes, axes and points, and construction planes, axes and points for every image. They turn these off in the design's Object Visibility (Display Settings > Object Visibility in the navigation bar; `Design.objectVisibility` in the API), then turn back on exactly the switches they turned off, after a finished capture, a failed image or a timeout alike. The animation's `fit` happens after the glyphs hide, so it frames the parts: on a test design with a sketch line and planes beside the parts, the fitted view spanned 12.2 cm instead of 23.3 cm. Pass `glyphs: true` to keep everything as the design shows it. The reply's `glyphs` lists the kinds that were hidden (`joints`, `jointOrigins`, `jointOriginAxes`, `sketches`, `originPlanes`, `originAxes`, `originPoints`, `constructionPlanes`, `constructionAxes`, `constructionPoints`) and says every switch is back; a switch that does not come back fails the operation with `restore_failed`, after the parts and the camera are home.

Fusion's local renderer never draws these glyphs (a render with every glyph showing matched one with them hidden to within a pixel), so `render_image` hides them only while it fits a standard view, and they are back before the render starts.

Object Visibility is a view setting. On a clean new design, all ten switches off and back on left the document unmodified, so hiding glyphs adds nothing to `documentModified`. It costs ten setters, about 0.3 ms, whatever the size of the design. The folder light bulbs on each component (`isJointsFolderLightBulbOn`, `isSketchFolderLightBulbOn` and the rest) hide the same pixels, but one toggle marks the document modified, turning it back does not clear that, and the API cannot clear it. They also cost a native call per folder per component: 48 toggles at 6.9 ms each on a 23-component design, and a light-bulb toggle costs about 26 ms on a 4,579-occurrence assembly. `Design.setLightBulbOn` refuses joints. Object Visibility has no switch for canvases, decals or section analyses, so those stay as the design shows them.

## Inspect render settings and prepare a detached camera

`render_scene_inspect` is a guarded read. Pass the current `sessionId` and exact
`target` from inspection. It reads the current viewport camera, native render and
scene settings, runtime API availability, and a bounded environment list. An
absent viewport camera or false native validity flag stays an
explicit field error; useful settings are still returned as `status: "PARTIAL"`.

```json
{
  "op": "render_scene_inspect",
  "sessionId": "<from capabilities>",
  "target": {"documentId": "<from inspect>", "revision": "<from inspect>"},
  "maxEnvironments": 16,
  "camera": {
    "type": "orthographic", "space": "world", "unit": "mm",
    "eye": [0, 0, 100], "target": [0, 0, 0], "upVector": [0, 1, 0],
    "extents": [100, 75]
  }
}
```

Omit `camera` for observations alone. A perspective descriptor replaces `extents`
with `"perspectiveAngle": 45, "angleUnit": "deg"` and uses `"type": "perspective"`.
Eye and target must differ; `upVector` must be a unit vector perpendicular to the
view direction. Extents are positive width and height in mm. Descriptor positions
and extents are bounded to 1e9 mm, and perspective angles lie strictly between
0 and 180 degrees. Unknown fields and other units/frames refuse before construction.

The optional `detachedCamera` result describes a temporary `Camera.create()` object
with verified geometry and lens readback. It is separate from the observed current
viewport and `scene.cameraType`/focal length. The bridge never assigns this camera
to a viewport, focuses a window, activates a workspace, or changes scene settings.
A requested camera that cannot be constructed or retain its values refuses; a
successful detached camera does not fill missing current-camera observations.

Autodesk tracks false transient `Camera.isValid` results as
[FUS-263280](https://forums.autodesk.com/t5/fusion-api-and-scripts-forum/camera-isvalid/td-p/14202647).
For a newly created detached camera only, the bridge verifies the exact native
Camera wrapper/type and every requested geometry/lens value independently, and
preserves the Boolean flags in `detachedCamera.nativeValidity.before/after`.
These flags are never rewritten as true. `renderEligibility` remains
`NOT_ESTABLISHED`; numeric readback is not proof of rendering. Other native
object validity guards, including current-viewport observation, remain unchanged.

Native lengths are converted from cm to public mm, angular values from radians
to degrees, and scene `cameraFocalLength` stays native mm. Every value carries its
unit where applicable. Conditional background/DOF/ground properties are read only
in the modes where the API supports them. `maxEnvironments` is an integer 0–32;
the default is 16, and truncation prevents `observationComplete`. Two bounded reads
bracket detached-camera preparation, with a fresh target check and exact comparison
of the returned observations and document saved/modified flags. A detected change
refuses without attempting to restore user state. `observedStateUnchanged` refers
only to the fields available in these observations; missing fields are unverified.
The complete result is limited to 128 KiB, native strings to 256 characters, and
per-field error text to 512 characters.

`observationComplete` covers this bounded read only. `sceneWitnessComplete` and
`renderQualified` are always false: appearances, textures, materials, light and
occurrence-visibility graphs, custom-environment contents, native queue idleness,
settings ownership and revision-to-pixels consistency are unqualified. No render
is started and no render job/cancellation API is introduced. Offline tests do not
establish native or installed-public-operation acceptance.

Primary API contracts: [Camera](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_Camera.htm),
[Camera.create](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_Camera_create.htm),
[Camera.setExtents](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_Camera_setExtents.htm),
and [API units](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/Units_UM.htm).

## Inspect native configurations

Development checkpoint 7 adds `configuration_inspect` as a read-only operation:

```json
{"op":"configuration_inspect","target":{"documentId":"<from inspect>","revision":"<from inspect>"}}
```

The result contains the loaded design's ordered top table, ordinary rows, a separate `table.testRow`
when the native reserved test row exists, column source relationships, all supported cells and the
active ordinary row. Cells identify their `ordinary` or `test` scope; a native column's row count
includes both. The reader does not assume a default column count. Table/row/column IDs identify native objects;
localized names and titles are presentation fields. Numeric cells return their original expression,
an explicitly labeled engineering value, and a separate database-unit diagnostic. Text cells use
the native text getter. Numeric units are limited to length, angle, mass, area, volume and unitless;
the reader checks a positive conversion basis before calculating signed values, because native
conversion errors can return the otherwise valid numeric value `-1`. Other unit families are
explicitly incomplete. Model-parameter ownership uses its actual component, while user parameters
are identified as design-scoped. Suppression retains both explicit and effective flags.
[Autodesk configuration table API](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_ConfigurationTopTable.htm)

`configuration.complete:false` reports unsupported columns, theme tables, preview hierarchies,
inconsistent test-row membership, unreadable fields or externally inserted configurations. Each
unreadable source or cell field is recorded separately while remaining cells are returned. The
observed native unsaved/offline configured fixture cannot read its property-cell values, and therefore
remains incomplete even though row and property-owner identities are available. Such diagnostics cannot serve
as a recovery witness or authorize a typed model mutation. Even fully read configured designs remain
read-only for model edits until native configuration Abort and archive gates pass; this includes
parameter/feature edits that could implicitly modify an active-row cell. No configuration conversion,
activation, cell edit or row/column lifecycle operation is advertised. Standard unconfigured designs
retain the existing modeling workflow. Legacy script endpoints retain their own independent contract.

Generated single-configuration documents (`Design.isConfiguration`) are reported with their native row ID, but remain incomplete and unsupported for model writes, complete BOMs and exports. Their source-table identity and durable lifecycle have not been verified. Missing or contradictory configuration flags/row IDs also refuse strict operations; row identity participates in stale-target checks.

This development reader checks preview-only custom/test-row and hierarchy getters to establish
that no deferred data is hidden. Some hierarchy getters were introduced in July 2026 and
`ConfigurationTopTable.testRow` in September 2026. If an installed build lacks these getters,
inspection still returns stable ordinary rows, columns and cells, with field-specific read errors;
it does not infer that preview state is absent or claim a complete witness. These preview guard
dependencies must be reevaluated before a production release.
[Autodesk test-row API status](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_ConfigurationTopTable_testRow.htm)

Supported configuration graphs, including inactive and reserved test cells and exact controlled-source links, are part
of revision and recovery witnesses. Changing an inactive expression invalidates a previously inspected
target even if current geometry is identical. Incomplete graphs refuse recovery comparison rather than
comparing equal unknown placeholders.

`bom` reports provenance for the currently active row: `tableId`, `rowId`, `rowName`, and its normal
document/revision target. Both flat and structured CSV data rows include that same provenance.
Configuration suppression affects represented instances; visibility alone never removes quantity.
BOM generation refuses incomplete configuration scope. Export remains an external file effect and
does not prove that F3D preserved a full configuration table; inspect the reopened archive before
claiming persistence. Multi-row activation/export is outside this slice.

See [CP7 native acceptance](mechanical-verification/CP7-ACCEPTANCE.md) for the finite readback gate.

## Build a parametric part

Create a sketch with `sketch_create` on `xy`, `xz`, `yz`, a construction plane or a planar face. Use `sketch_geometry` for points, lines, rectangles, circles, arcs, polygons, slots and fitted splines. Geometry coordinates are sketch-space mm; arc sweeps and polygon angles are degrees. Use `sketch_constraint` and `sketch_dimension` to express design intent, then inspect the sketch for fully-constrained state, curves, points, dimension parameters and profile references.

Pass current profile references to `feature_create`. The schema enumerates extrusion, revolve, fillet, chamfer, shell, hole, loft, sweep, combine, split, move, mirror, rectangular/circular pattern, draft and thread inputs. Creation targets a native component definition. Assembly-context proxy geometry is for inspection; obtain native references when authoring a definition. Cut/intersect operations require explicit participant bodies. Join requires an unambiguous single body; use combine for explicit multiple-body selection.

A rectangular or circular pattern repeats exactly one kind of entity: `bodies`, `features`, `faces` or `occurrences`. Pattern a feature or its faces rather than drawing every instance in a sketch: one hole cut once and patterned is far cheaper to recompute than a sketch full of circles. `occurrences` makes a component pattern. It is built in the occurrences' shared parent component, so the part stays one definition and the BOM counts every instance; mixed inputs (`mixed_pattern_inputs`) and occurrences with different parents (`mixed_parents`) are refused. `computeOption` sets Fusion's pattern compute option for bodies, features and faces: `identical` (the default) repeats the first result, `optimized` is fastest but can drop instances that touch other geometry, and `adjust` recomputes every instance and is the slowest. Component patterns have no compute option. The reply's `pattern` says what was patterned, the option Fusion reports and the number of instances.

Use `feature_edit` to rename or suppress a feature or change the expressions of parameters owned by that feature. `entity_delete` handles supported native deletable entities. `parameter_batch` groups user-parameter creation/deletion and user/model-parameter expression edits in one measured model transaction. Generic mixed operations with file/cloud rollback are outside this contract.

## Select, measure and edit everyday geometry

Development checkpoint 5 adds six typed operations. Their exact schemas are returned by
`fusion_mech_capabilities`; [the native acceptance protocol](mechanical-verification/CP5-ACCEPTANCE.md)
separates implemented behavior from live verification.

`geometry_query` searches one body's faces, edges or vertices. Supply `space:"root"` or
`space:"component"`. Filters can specify an exact native geometry type, inclusive area/length/radius
ranges, a nearby point and tolerance, or an oriented planar normal/linear tangent. Units are mm and
mm²; angular tolerance is degrees. `selection:"one"` is the default and refuses zero or multiple
matches. `selection:"many"` explicitly permits an empty result; exceeding the result budget fails
instead of truncating. Component-space filters use the definition's coordinates, while returned
references retain the exact input body's occurrence context. Root-space queries require that context
for geometry in a child component. No “first face” ordering is promised.

`measure` uses native minimum distance or angle calculations in `space:"root"`, returning a value in
mm/degrees and witness positions in root-space mm. It accepts two distance entities, two linear/planar
angle entities, or three points with the second point as the apex. Refuse a native child-definition
reference and use its occurrence-context reference. Missing or inconsistent witness positions fail.
For valid parallel line/plane angles, Fusion can return three zero witness points. These are preserved;
`witnessValidation` identifies the native geometry directions used to verify parallelism instead.
Degenerate three-point angles still fail. [Autodesk measurement API](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_MeasureResults.htm)

`sketch_curve_edit` accepts `action:"trim"|"extend"|"split"|"break"`, a native sketch/curve reference
and a sketch-space mm `pick`. Split requires an interior point on the curve; extend chooses the nearer
endpoint. Trim can replace/delete the original, so consume the returned replacement references after
every edit. An in-place extension can return the original curve with a fresh reference, after its
geometry change survives recompute. Removing a whole curve requires `allowDelete:true`, which applies
only to trim. Empty break/split and unchanged edits fail and abort. Linked/derived or deferred-compute sketches are
refused. [Autodesk curve-edit API](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_SketchCurve.htm)

`sketch_offset` accepts an ordered, connected chain of native 2D sketch `curves` and a nonzero signed
`distance` number or expression. Curve order is preserved. Positive offsets go right relative to
chain flow; negative offsets go left. For a single circle or ellipse, positive means outward.
`topologyMatched` defaults to true. The operation uses `createOffsetInput`/`addOffset2`, returns the
controlling parameter and child curves, and checks the requested signed value after recompute.
[Autodesk signed offset input](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_GeometricConstraints_createOffsetInput.htm)

`construction_axis` accepts `type:"edge"|"circular_face"|"two_planes"|"two_points"` and exact
`entities`. Native references in the target component need no context. Referencing another component
requires root-context geometry; a non-root target also requires `creationOccurrence` identifying the
specific target instance. The result reports actual parametric state, native geometry and health.
It does not use direct-only infinite-line or preview snap-point APIs.
[Autodesk axis input](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_ConstructionAxisInput.htm)

`sketch_diagnostics` reads sketch health, constraint flags and driving dimensions. It reports native
fully-constrained/fixed/linked/reference observations; it does not invent solver degrees of freedom or
a conflicting-constraint explanation, and it does not recompute or alter the sketch.

Model transaction recovery records sketch constraint and dimension links into their native curve/point
inventory, including offset relationships and curve endpoint links. In a sketch the write names (or whose
curve, point, constraint or dimension it names), unsupported relationship classes, foreign links or
unreadable graph properties fail closed before mutation; other sketches enter the witness with what can be
read ([Designs that already carry faults](#designs-that-already-carry-faults)). A reordered inventory can
conservatively leave recovery unverified; equal current coordinates alone never establish equal design
intent. Readback does not compare entity-token strings to establish these relationships.

## Retry and recovery

Give each typed write a unique `operationId`. After a transport timeout, query `fusion_mech_operation_status` with that ID and its recorded session ID. Repeating the exact canonical request in the same session returns the recorded terminal result or a pending/reconciliation response; it never dispatches a duplicate. Changed arguments under the same ID refuse. Bridge-owned `caller` and `_callerBudgetMs` metadata are excluded from the canonical digest.

The bridge exe validates every request against `schema.json` and records each write in an app-owned SQLite journal before the add-in sees it (`rust/bridge/src/mech.rs`); the add-in only executes the Fusion API work. Its main-thread document observation captures the exact live document identity, with creation/data-file IDs as additional evidence; native handlers still enforce the original target/revision guards. History retains request digest, operation, source version, declared target, observed document, timestamps and the original terminal result. The `dispatched` phase means a dispatch generation was durably reserved before the add-in was asked, not that Fusion accepted or began it. `executing` means the add-in claimed the dispatch. Terminal completion is acknowledged only after its result commits to the journal.

A lost reply cannot cancel an already-started operation. Each write's dispatch has a claim file beside the journal: the add-in creates it, exclusively, before invoking the typed handler, and the exe creates it only to expire a dispatch whose reply did not arrive. Whoever creates it first wins, so `not_executed` is recorded only when native execution never claimed the operation, and a queued or late event then cannot start it. The add-in also leaves its exact result in a file, so a result whose reply was lost is recorded on the next status query or write. An executing or completed operation retains its pending or terminal evidence. A bare admission without a durable dispatch claim requires reconciliation; it is never automatically dispatched after an acknowledgement failure. Delivery errors cannot overwrite a faster native completion. A persistence failure after execution returns `operation_reconciliation_required`, `outcome:unknown`, and available observed-result evidence with `terminalRecorded:false`; it makes no rollback claim. `nativeExecutionStarted` refers to handler execution, not proof that a requested model mutation succeeded. Current-session observed result payloads are capped at 64 KiB each and 4 MiB in aggregate; omitted payloads are explicitly labeled, with a digest when canonicalization within the record limit succeeds.

After restart, old session IDs can query history but cannot execute or rebind its entity references. Historical nonterminal records are `interrupted` with unknown outcome. A recorded historical terminal result describes its original document/session, not the current model. Inspect current native state before deciding whether a new operation is appropriate. Missing or unreadable history never proves that an operation did not run. The exe reads history from the journal itself, outside lifecycle repair, window etiquette and the CAD queue, so status answers while Fusion is busy, hung or closed. It never starts/restarts Fusion, handles dialogs, or changes preferences; querying history does not reopen Fusion. Bounded SQLite lock waits can still delay it.

Storage is fixed to `%LOCALAPPDATA%/Adom/FusionBridge/mechanical/operations-v1.sqlite3` on Windows, `~/Library/Application Support/Adom/FusionBridge/mechanical/operations-v1.sqlite3` on macOS, or `~/.local/state/adom/fusion-bridge/mechanical/operations-v1.sqlite3` on Linux. Public verbs accept no storage path. Storage version 1 uses FULL synchronous commits, a maximum 10,000 retained operations, 64 MiB database, 4 MiB per record and 1 MiB canonical request. SQLite rollback files may require another database-sized allocation. Corruption, unsupported storage versions, missing SQLite, unsafe paths, permission failures and capacity exhaustion refuse admission; history is never silently reset, migrated or evicted. Looking up absent history creates no files. Opening existing history permits SQLite to recover a hot rollback journal after a process crash.

These guarantees apply to the typed synchronous mechanical dispatcher. They do not add asynchronous jobs, cancellation, automatic replay, rendering, or durable recovery to legacy script verbs. File publication and native transaction recovery retain their existing independent boundaries.

A failed transactional typed model operation reports its abort result and a semantic recovery witness. Standalone `parameter_set` and `parameter_create` instead use measured compensating recovery and report `transactional:false`. `recovery.verified:false` means restoration was not proved. Successful publication followed by temporary-file cleanup failure remains a success with a warning. An operation committed successfully but failing subsequent inspection returns `refreshRequired`; read the model again before continuing.

## Export and save

`export` writes to a temporary sibling location, verifies nonempty output, then publishes to the requested path. Overwrite is explicit. Runtime format support is checked; unsupported requests preserve existing destination files. Results include byte count and SHA-256. STL coordinates are mm. Native F3D preserves Fusion data more fully than neutral geometry formats; never assume supplier attributes survive a STEP roundtrip.

`fusion_save_to_cloud` handles an explicit cloud project/folder/name. The typed export operation creates a local archive or exchange file and does not itself create a Fusion Team version.

## 2D drawings

`drawing_create` makes a drawing of the active design with Fusion's automatic drawing (`adsk.drawing.DrawingManager`, a preview API since July 2026). Fusion lays out the sheets: for a part, one sheet with a base view (front), its projected views (top, right), an isometric view, center marks, a title block filled with the title, author and date, and dimensions by the chosen strategy (`overall` by default). For an assembly it also makes the main assembly and subassembly sheets, with a parts list on the isometric sheet. Options: `standard` (iso or asme), `units`, `sheetSize` (A4 to A0 with iso, A to E with asme; a size of the other standard is refused, since Fusion would ignore it silently), `orientation`, `views`, `style`, `tangentEdges`, `dimensions` (`none`, `overall`, `automatic`, `baseline`, `chain`, `ordinate`, the symmetric variants), `holeNotes`, `datum`, `content`, `sheets`, `omit`, `configuration`, `name` and `folderId`.

The design is drawn from its cloud file, so it must be saved, with no unsaved changes (`useSavedVersion:true` draws the last saved version anyway). Fusion puts the drawing (an `.f2d` file) in the design's folder; `folderId` moves it elsewhere, and a project's root folder is refused either way. The API cannot place a view or set a view's scale: the layout picks the scale for the sheet size (an 80 x 50 x 40 mm part came out at 1:1 on A3). Measured on Fusion 2705: 24.7 to 33.6 s per drawing of a small part, in a drawing engine (`AcCoreConsole.exe`) that Fusion starts and stops; the drawing is not opened.

`drawing_export` writes PDF, DWG and DXF of a drawing `drawing_create` made, staged and published like `export` (byte count, SHA-256, explicit `overwrite`). Two traps found live each blocked Fusion's main thread for every caller, so the operation is built around them:

- **Never export in the call that opens a drawing.** A drawing that a script opens finishes loading only after the script returns, and opening an automatically created drawing starts Fusion's interactive Edit Auto Dimensions command in the drawing engine. An export sent while that command waits for input is fed to its prompt and rejected, and `DrawingExportManager.execute` holds the main thread until its 600 s timeout; PDF, DWG and DXF each waited the full 600 s.
- **Never open a drawing invisibly.** `Documents.open(file, False)` opened the drawing in 0.8 s by Fusion's log, but no drawing engine started for it and the call did not return.

So the first `drawing_export` call opens the drawing (visibly; 9 s for the test part) and answers `drawing_not_ready` with `retryable` and `retryAfterSeconds`; call again with a new `operationId`. A later call ends Edit Auto Dimensions on that drawing if it is running (and answers `drawing_not_ready` again), and exports only when the sheets read, Fusion runs no command, the drawing engine that loaded the drawing has no unfinished command and the engine's log (`FusionDrawing.log`) has been quiet for 5 s. A drawing opened by someone else, or before the add-in restarted, is refused (`drawing_open_elsewhere`): its engine's state is unknown. DXF holds the active sheet. `close:true` closes the drawing, and the design its open brought in when that design was not open before and is unmodified (opening a drawing also opens the design it references), then puts the previously active document back in front. `drawing_status` reports the same readiness without changing anything.

Sheets report their paper (`A3`), width and height (420 x 297 on an mm drawing) and their views. In Fusion 2705, `View.type` raises `InternalValidationError: pAsset` for every view of a fully opened, idle drawing (Fusion's log: no base model asset in the drawing document), so such views are listed as unreadable and do not hold up an export. With `dimensions:"none"` Fusion does not start Edit Auto Dimensions on open.

Between formats in one call, the operation reads the engine's log again and answers `drawing_not_ready` with the files written so far if the engine is still busy. With a DXF, the reply's `scale` is the scale the layout chose, read from the DXF's linear dimensions (sheet length over measured value, `1:1` for the test part on A3); it is null for a drawing without dimensions. `dimensions:"none"` plus `Sheet.autoDimension` later gives the same dimensions without Edit Auto Dimensions ever starting (3.8 s for the test part).

Measured on Fusion 2705 through the gated flow, for the 80 x 50 x 40 mm bracket: create 18 to 22 s, visible open 5 to 9 s, ready 0 to 14 s after the open (with automatic dimensions, one `terminateActiveCommand` of Edit Auto Dimensions took 0.45 s), then PDF 0.3 to 0.9 s, DWG 0.1 s (AutoCAD 2013 format) and DXF 0.2 s (AutoCAD 2013, mm, real DIMENSION entities): about 45 s from the call to three files.

End to end through a deployed bridge: `drawing_create` 18.5 s, then three `drawing_export` calls: the first opened the drawing (8.3 s, `drawing_not_ready`), the second ended Edit Auto Dimensions (3.2 s, `drawing_not_ready`), the third wrote PDF, DWG and DXF and closed the drawing (3.8 s). That was 41.6 s from `drawing_create` to three files, and every SHA-256 in the reply matched its file.

## Coverage and verification

See [build handoff](build-2026-10-02.md) for the verified checkpoint and live host. See the [feature matrix](mechanical-comparison/FEATURE-MATRIX.md) for each surveyed external capability, its bridge route and remaining gaps. A discoverable native API or arbitrary Python execution does not establish that a dedicated bridge operation is implemented or live verified.

## Assemble parts

`component_create` creates an internal definition and its first occurrence. These operations require a Hybrid design; they never silently change the intent of an existing Part or Assembly design. `occurrence_insert` reuses a definition, or uses `independentCopy:true` to create a separate definition. `parent` is a component-definition reference, so adding children to it affects every instance of that definition.

Create/insert transforms use parent-component coordinates. Creation, insertion and CAD import apply the requested pose to every currently placed parent instance, capture pending positions, and verify the resulting root-context matrices after recompute; `affectedOccurrences` lists these instances and `placementScope` is `all_current_parent_instances`. The definition-local base transform can differ from the computed position after a capture. Occurrence updates use the selected instance's assembly context. The transform input uses `position:[x,y,z]` in mm and `axis:[x,y,z]`, `angle` in degrees; it specifies an absolute pose. `occurrence_update` can ground/unground explicitly. A new transform edits the occurrence's initial position when Fusion allows it (a root-level occurrence without joints or patterns; earlier occurrences are edited with the timeline marker rolled back and restored), so nothing joins the timeline; otherwise the position is captured as a snapshot. `method` (`auto`, `initial_position`, `capture`) chooses, and the reply's `placement` says which was used. Constrained or grounded placements refuse changes that cannot be applied and measured.

`joint_create` creates an **as-built** rigid, revolute, slider, cylindrical, pin-slot, planar or ball joint. First place the occurrences, then supply exact occurrence references and joint geometry for non-rigid motion. Joint axes are in joint coordinates. Read the returned unit metadata: joint-geometry transform matrices retain explicitly labeled native cm translations, while motion values use mm/degrees. `joint_edit` reads/sets enabled limits, rest positions and motion values in mm/degrees, and rejects out-of-limit driving. Rigid groups have separate create/edit/delete operations. Joint and group suppression changes capture pending native state before recompute; responses report `positionCaptured`. Full motion-study authoring remains a comparison gap.

`interference` analyzes selected occurrence instances or root-context body proxies and returns overlap volumes without adding interference bodies to the model. `assembly_properties` reads whole-assembly, component, occurrence or body mass, volume, area and center of mass with explicit units and coordinate scope. Material values reflect Fusion's assigned/default materials; they are not a supplier certification.

## Design efficiency

`efficiency_report` reads what makes a design slow to recompute, open and edit: timeline make-up and health, Position snapshots, Move features, size, repeated definitions made by Paste New, instance-weighted faces, imported history and wires in a parametric design, big sketches, pattern compute options, modeled threads, meshes and empty components. It returns `metrics` and `findings` (rule, severity, measured, limit, fix), worst first. It never changes the model and needs no revision digest; it is journaled like `bom_export`, so a long report on a big design is followed with `fusion_mech_operation_status`. Passes past `budgetSeconds` (default 120) are skipped and listed with `complete:false`; `computeTiming:true` also times a full `computeAll`. The rules and the benchmarks behind them are in `skills/fusion-design-efficiency/SKILL.md`.

## CAM: toolpaths and G-code

`cam_tools` lists the tools of one of Fusion's tool libraries, with diameter, corner radius and flute length in mm. `library` is `milling` or `hole` (Fusion's metric samples; `milling` by default), any sample or local library by its name (`Milling Tools (Inch)`, ...), or `document` (the tools the document's CAM already uses). It filters by `type`, `description` (a substring) and `minDiameterMm` / `maxDiameterMm`, and every reply lists the `libraries` a job can name.

`cam_job` makes a milling job in one journaled call.

**Setup.** A setup on the given `bodies`.
- `stock` is Fusion's relative box unless it says otherwise: `{"mode": "relative_box", "offsetMm": {"sides": 2, "top": 1, "bottom": 0}}` adds stock round the model, `{"mode": "fixed_box", "sizeMm": [110, 70, 15]}` sets a fixed box, and `{"mode": "solid", "bodies": [...]}` machines from stock bodies.
- `wcs` puts the origin at a box point of the stock (`stock_point`) or of the model (`model_point`), `top center` by default. `top 1` to `top 4` are the top corners at x-min y-min, x-max y-min, x-min y-max and x-max y-max; Fusion also has `top side 1..4`, `center`, `middle ...` and `bottom ...`. `model_origin` keeps the model's origin. The axes stay the model's.
- The reply's `setup` reads back what Fusion built: the stock mode, size and box and the WCS origin and axes, in mm and model coordinates. Fusion keeps the stock box relative to the WCS (`stockXLow` and so on, in cm) and `Setup.workCoordinateSystem` in mm; the reply turns both into model coordinates.

**Operations**, in order, each a `strategy`:
- `face`;
- `adaptive2d` and `pocket2d`: the pocket floors given as `faces`, else the setup bodies' pocket floors (upward planar faces below each body's top). Fusion's own pocket recognition found the pocket on a test plate but generated an empty toolpath, so it is not used;
- `contour2d`: the `loops` (`outside` by default, `inside` or `all`) of the given `faces`, else of the setup bodies' silhouette;
- `drill`: the bodies' holes of the drill's diameter unless `holeFaces` are given, with `cycle` `drill` (Fusion's drilling, G81 on FANUC), `peck` (deep drilling, G83) or `chip_break` (G73), and `peckDepthMm` (Fusion's `peckingDepth`, which those cycles enable; the older `incrementalDepth` stays disabled);
- `adaptive` (3D adaptive clearing), `pocket_clearing`, `contour` (3D contour), `parallel`, `scallop`.

Each tool comes from a library by `type` (a flat end mill by default, a drill for `drill`), an optional `description` substring and `flutes`, and the nearest `diameterMm` (a tie goes to the smaller tool); `tool.library` names any library `cam_tools` lists. `parameters` override Fusion's operation parameters by name, and a value Fusion flags is refused rather than kept. Each operation in the reply names its tool and library and where its geometry came from (`faces`, the bodies' pocket floors or silhouette, or the holes found, with counts).

**Toolpaths.** The job generates them, gives an operation Fusion left untouched (no toolpath, no warning, no error) one more try on its own (`generated.retried`), and reports every operation's `status`: `generated` (a valid toolpath), `empty` (generation ran and produced nothing), `error`, `outdated` (the model has changed since), `suppressed` or `not_generated`, with Fusion's `warnings` and `errors` line by line and its machining time. `generated` counts the operations with a toolpath and says whether the job is `complete`. `machining` is Fusion's estimate split into feed, rapid and tool change time. On Fusion 2705 `getMachiningTime` ignores the feed scale and rapid rate it is given (a face pass timed the same with feed scale 1 or 100 and rapids of 500 or 8.33 cm/s) and times rapids at its own rate, which the reply reports as `rapidMmPerMin` (30,000 seen). The job used to pass 500 for the rapid rate, meaning mm/min / 10, while the reference gives cm/s.

**Post.** With `post` (`machine`, `folder`, `program`, `comment`) the job posts G-code through a post from Fusion's library matched by name (`FANUC`, `Grbl`, `HAAS - Next Generation Control`, ...) into `folder`, as `<program>.nc` numbered `program`, with the comment after the number. FANUC and HAAS take a number from 1 to 9999. The job does not post while any operation lacks a valid toolpath, because Fusion's post leaves such an operation out of the program without a word: the reply says `posted: false` and names the operations. A posted reply carries the file, its first lines, `programInHeader` (the header's program number is the one asked for), the operations posted, and `ncProgramKept` (the name and folder the NC program held when it posted).

A failed job deletes its setup and the tools it brought into the document's tool library, and a failed post deletes its NC program, so nothing half-made is left. A job that runs but leaves an operation empty or failed keeps its setup, so a person can open it in Fusion.

**The Manufacture workspace.** A document has no CAM product until the Manufacture workspace has been opened in it once: on Fusion 2705 `Products.itemByProductType("CAMProductType")` raises `3 : failed to find product`, and `Products` has no way to add one. So the first job in a document opens Manufacture for the moment Fusion takes to create the product (0.6 to 1.6 s measured), then makes the previous workspace active again, also when that fails, and reports the round trip as `workspace` (`switched`, `from`, `restored`, `now`). The switch shows only in the target document, which is the active one; afterwards the active product is the design again. Later jobs in that document, `cam_inspect` and `cam_tools` never change the workspace.

`cam_inspect` reads the document's setups (stock box, WCS, and every operation's status, warnings, errors and tool) and its NC programs (post, program, file name, folder).

**The program number and the output folder.** When an NC program's operations come from one setup, Fusion copies the setup's `job_programName` and `job_programComment` into the program's `nc_program_name` and `nc_program_comment` (`NCProgramInput.operations` in Fusion's API reference). The job used to set the name before the operations, so every FANUC program came out as O1001, the setup's default. The name and comment now go on the setup as well, and on the NC program after its operations and again after its post is chosen. The output folder was ignored because of its backslashes: a folder written `C:\Users\...` reads back as unspecified and the file lands in Documents/Fusion 360/NC Programs, while the same folder written `C:/Users/...` is used, set on the NC program input or on the program after its post. The job writes it with forward slashes. If Fusion still writes a file to its default NC folder, the job moves it into `post.folder` and says `movedFromFusionFolder`; a file of the same name already in `post.folder` is replaced (`replacedExisting`). Probed side by side on Fusion 2705: the name set before the operations gave O1001, after them O2002, the setup's `job_programName` O2003; the backslash folder landed in the default folder, both forward-slash variants in the requested one.

Live on Fusion 2705, on a 100 x 60 x 12 mm plate with a 40 x 30 x 5 mm pocket and four 5 mm through holes (scratch documents, closed afterwards):
- a 2D job (relative stock with 2 mm sides and 1 mm top, WCS at the stock's top front-left corner; face, 2D adaptive on the pocket floor it found, 2D contour round the silhouette, peck drilling) generated all four toolpaths in 2.7 s, one of them on the retry, and posted a 959-line FANUC program headed `O2001 (CAM CHECK 2D)` into the requested folder, with `G98 G83 X12. Y12. Z-13. R4. Q2.` at the holes, measured from the stock corner. Fusion estimates 206.8 s;
- a 3D job (fixed 104 x 64 x 14 mm stock, WCS at the model origin; 3D adaptive, 2D pocket on the given floor, 3D contour, and parallel and scallop with a 6 mm ball end mill) generated five toolpaths in 3.2 s and posted an 8,537-line HAAS program headed `O03002 (CAM CHECK 3D)`;
- 3D pocket clearing generated; a 2D contour of loops the face does not have came back `empty` with Fusion's warning, and that job did not post; a 20 mm drill on a part with 5 mm holes was refused and left neither a setup nor its drill behind.

## McMaster and purchased CAD

`mcmaster_discover` reports availability of Fusion's native Insert McMaster-Carr command. `mcmaster_open` launches its interactive supplier picker in the targeted Hybrid design. Its recorded result is a launch request, not proof that a part was inserted. Inspect again after insertion or cancellation. This operation does not place orders.

For an already downloaded model, use `cad_import` with an absolute host-local STEP/SAT filename, optional expected `sha256`, and provenance:

```json
{
  "supplier": "McMaster-Carr",
  "supplierPartNumber": "3597N17",
  "sourceUrl": "https://www.mcmaster.com/3597N17/",
  "description": "Supplier CAD downloaded for this part"
}
```

The importer hashes the actual source file, imports into a new wrapper component, and stores supplier/part number/URL/source path/checksum in native attributes. Caller-supplied supplier labels do not authenticate the download; retain the original supplier file/receipt. Imports are treated as purchased subassemblies by default. Use `component_metadata` for part number, description and BOM policy; `material_list` and `material_assign` use loaded library/material IDs.

## Generate a mechanical BOM

`bom` returns grouped purchasing parts plus a structured instance tree. It distinguishes component definitions from repeated occurrences, includes root-owned bodies, and keeps distinct same-named components separate. A purchased subassembly counts as one purchased item unless `explodePurchased:true`. `excludeFromBom` removes that purchasing subtree; physical totals still include its unsuppressed bodies. Typed BOM JSON and CSV publication responses declare `calculationAccuracy:"high"`, matching the high-accuracy physical-property workflow.

`bom` also returns the views a buyer and a builder read:

- `indented`: the structure with item numbers (1, 1.2, 1.2.1), quantity per parent and the total rolled up through every level. Each line is a part, an assembly, a kit (bought, not expanded) or a phantom (a body-less grouping that is counted through, never stocked).
- `flat`: one line per component definition with make or buy, `makeBuyReason`, category, material, stock, length, supplier, cost and mass.
- `cutList`: extrusions grouped by profile and cut length. Pass `stockLengthMm` (and `kerfMm`) for `stockPlan`, a first-fit-decreasing plan of bars per profile that reports any cut longer than the stock.
- `wireList`: cut wires grouped by gauge and colour, each measured as body volume over end-cap area. That is exact for any constant section swept along any path, where the bounding box undercounts a routed wire.
- `orderList`: bought lines merged by supplier part number, so repeated models of one catalog item become one line.
- `fasteners`, `totals` (cost per currency, cost coverage, mass, and how many lines were inferred from names) and `warnings` (default names, missing part numbers, costs or materials, short "extrusions", made parts with many bodies, hidden groups that were counted).

Metadata always wins: `makeBuy`, `asPurchased`, `category`, `supplier`, `supplierPartNumber`, `unitCost` and the other `component_metadata` fields. When nobody set them, the BOM reads the name and says so in `makeBuyReason`:

- A name that says made here (fabricated, machined, 3D printed, cut from) is made.
- A wire (a gauge in the name) or an extrusion is cut from stock.
- A McMaster-Carr number in the name becomes the supplier part number, marked `supplierInferred`.
- Fasteners, electronics and hardware are bought, and so is a name with a vendor note ("(vendor STEP)", "(modelled)", "(as installed)") or a maker's name, unless it is an accessory such as a mount or plate.
- An assembly that reads as a bought product (an imported .stp or STEP-translator model, a vendor note, a maker, a McMaster number) or whose children are a circuit board's footprints counts as one bought line. `inferKits:false` turns that off.
- A body-less group named as a library, a palette of parts kept to copy from, is excluded with reason `library_group` unless `includeLibraries:true`.

`nativeProperties:false` skips Fusion's part number and description, which are cloud reads of about 0.6 s per component on a cloud document (cached per component revision after the first read). So that `bom` answers inside ab's 60 s budget, it stops those reads once the call has run `budgetSeconds` (default 40): the components not read yet come back with `nativePending`, the response carries `nativePending` (a count) and `nativeHint`, and the next `bom` continues from what this one read. `bom_export` is journaled and reads them all unless given `budgetSeconds`. `physicalTotals:false` skips the physical walk.

`bom_export` writes CSV to an absolute `filename`, with explicit overwrite and staged publication. The modes are `flat` (the default), `structured` (one row per instance), `indented`, `cutlist`, `fasteners`, `wires` and `order`. Rows retain supplier provenance. Quantities do not depend on component display names. Save an F3D archive to retain Fusion metadata; STEP metadata persistence is not promised. Traversal budgets fail explicitly rather than presenting a truncated BOM as complete.

## Regular assemblies (checkpoint 6)

`regular_joint_create` solves geometry alignment between two guarded occurrence-context attachments or joint origins. `regular_joint_edit` redefines root-owned joint geometry, motion, offset, angle, flip or lock and restores the timeline marker even on error. Existing `joint_edit` and `joint_delete` remain the drive/limits/suppression and delete interfaces for both joint families. Rigid joints have implicit `locked:false`: requests for `locked:true` refuse before mutation, and the unsupported native rigid lock setter is never called. Redefining a locked moving joint as rigid clears its native lock; the resulting false value is verified after recompute.

`joint_origin_create/edit/delete` manage native component-definition origins. Child-instance references appear as `jointOriginRefs` on occurrence inspection. Origins accept analytical face/edge/point attachments and optional straight orientation references in the same definition.

Native origin `transform` is component-local; an occurrence-context origin returns its root-space proxy transform. The nested `geometry` frame is separately labeled `native_joint_geometry_api`: Fusion can expose an origin's defining geometry through a placed-instance proxy even for a native origin. Definition edits verify the exact native source and owner; regular-joint attachments additionally require the exact selected occurrence.

A moving-to-rigid redefinition can make an existing Capture Position snapshot empty and unhealthy in Fusion. Such an edit refuses with `causeCode: unhealthy_model`; the response reports whether `PTransaction.Abort` verified restoration. Existing snapshot history is retained. Successful family changes and geometry reattachments require resulting history with no new or changed fault; the bridge does not retarget or discard snapshots automatically.

`motion_link_create/edit/delete` use the native November 2025+ API, with explicit joint degrees of freedom and positive mm/deg ratio magnitudes. `reversed` controls opposite motion. Missing native support fails explicitly. Motion link membership changes require recreating the link.

Coupling-ratio edits run with the timeline at its end, where the link remains evaluated; they do not roll before the link as regular-joint redefinition does. Changing a ratio or reversal can re-solve member positions. Inspect the resulting assembly poses and base subsequent drive deltas on that post-edit state; an edit does not promise to preserve either member's prior position. A same-joint link can expose either null or the first joint again in native `jointTwo`. Readback preserves that native field and adds `sameJoint`; identity validation accepts only those equivalent representations, never an unrelated second joint.

The [CP6 native acceptance cases](mechanical-verification/CP6-ACCEPTANCE.md) define solver/frame, repeated-instance, grounding, timeline, motion-link and F3D persistence gates. Inferred joints, multi-plane/profile/tangent attachments, custom joint motion axes and definition-owned nested joint redefinition are outside this checkpoint. Public origins and regular joints carry attachment and parameter readback; solver placement reports root-space occurrence transforms in mm. Native joint API frame matrices are labeled separately from root-space placement evidence.

Regular-joint creation refuses pre-existing pending positions. Initial solve checks native joint-transform anchor distance against offset magnitude, offset axiality, and zero-angle Z-axis collinearity before applying drives. Native joint matrices identify their API source and root-space anchor origins; their axes differ from transient `JointGeometry` axes and do not independently prove physical angle/flip sense. The native acceptance campaign verifies that sense using independently known occurrence geometry and poses. Attachment source references are captured before solving and resolved afresh afterward: recompute can invalidate saved faces and transient geometry even when `JointGeometry.isValid` remains true. Driven `joint_edit` also verifies that active root motion links preserve their settings and propagate the specified motion ratio.
