---
name: fusion-molecule-export
description: >
  Turn a Fusion 360 electronics board into a real entry in the Adom molecule system (adom-molecule /
  hydrogen) end to end. Covers the FIVE required Fusion exports the bridge already ships
  (.usdz, .f3d, EAGLE .brd, EAGLE .sch, silkscreen top .png), the exact multipart field names the
  hydrogen create API expects, and the session-cookie auth workaround for the adom-molecule CLI's auth
  gap. Trigger words: fusion board to molecule, create molecule from fusion, adom-molecule create
  fusion, upload fusion board, molecule from the board adonis made, save a molecule component page,
  export brd sch f3d from fusion, fusion molecule pipeline.
---

# Fusion 360 board -> Adom molecule (the whole pipeline)

You have an electronics design open in Fusion (a PROJECT, opened per `fusion-electronics`) and you
want it in the **adom-molecule** system so it gets a molecule datasheet page with native symbol /
footprint / 3D viewers. This skill is the durable record of doing it live (John, 2026-07-14); every
gotcha below cost real time to find.

## The bridge ALREADY exports every file the molecule system needs

Do NOT conclude "the bridge can't export .brd/.sch/.f3d" and fall back to a hand-built wiki page.
It can. The verbs exist today:

| File | Bridge verb | Which editor must be active |
|---|---|---|
| `.usdz` (3D, PBR colors) | `fusion_export_usdz {outputPath}` | 3D PCB (`fusion_show_3d_board`) |
| `.f3d` (Fusion archive) | `fusion_export_f3d {outputPath}` | 3D / a Design product (`fusion_show_3d_board` first) |
| EAGLE `.brd` (board) | `fusion_export_eagle_source {outputPath:...brd}` | Board editor (`fusion_show_2d_board`) |
| EAGLE `.sch` (schematic) | `fusion_export_eagle_source {outputPath:...sch}` | Schematic editor (`fusion_show_schematic`) |
| silkscreen top `.png` | any top-down board render, or `fusion_export_gerbers` | 3D or board |

Notes learned the hard way:
- `fusion_export_eagle_source` picks board vs schematic from the **active editor**, so switch first
  (`fusion_show_2d_board` -> export `.brd`; `fusion_show_schematic` -> export `.sch`). Since 1.10.35 the export verbs switch to the matching editor themselves, so from the board editor a `.sch` export just works. Tested: 840 KB
  `.brd` + 623 KB `.sch` off a ~110-part board.
- Do NOT try raw EAGLE `WRITE 'path'` via `fusion_electron_run`: the bridge BLOCKS it because on cloud
  documents `WRITE` opens a blocking "Version Description" save dialog and hangs. `fusion_export_eagle_source`
  is the supported path (it drives the export without the dialog).
- `.f3d` / `.usdz` need a **Design product** active; from the 2D board or schematic editor they error
  with "3D exports require a Design product" -> call `fusion_show_3d_board` first.
- `fusion_export_obj` is NOT supported for the electronics 3D PCB (only STL / USDZ). Use USDZ for color.

After exporting on the box, `pull_file {filePaths:[...]}` them into the container so you can upload.

## The molecule create API (what actually works)

`adom-molecule create` is the intended CLI, but **its auth is broken in the shipped build**: it sends
no credential, so every `create` returns `401 Unauthorized - provide a session_token cookie or X-Api-Key
header`. Until that's fixed (see the hand-off prompt in `fusion-molecule-export/adom-lbr-style-prompt`
below), POST to hydrogen directly with your wiki session cookie:

```bash
TOKEN=$(python3 -c "import json;print(json.load(open('$HOME/.adom/auth.json'))['token'])")   # valid hydrogen session_token
curl -s -H "Cookie: session_token=$TOKEN" \
  -F "name=BQ25792_Charger" -F "username=adonis" -F "sourceType=fusion" \
  -F "usdzFile=@bq25792.usdz" \
  -F "f3dFile=@bq25792.f3d" \
  -F "brdFile=@bq25792.brd" \
  -F "schFile=@bq25792.sch" \
  -F "silkscreenTopFile=@bq25792_top.png" \
  https://hydrogen.adom.inc/api/molecules/create
# -> {"success":true,"message":"Molecule 'BQ25792_Charger' version 'v1' created with 5 provided file(s)."}
```

Field-name gotchas (these are the exact multipart names the server parses; guessing wastes turns):
- The owner field is **`username`**, NOT `owner`.
- The source field is **`sourceType`**, NOT `source`. Value `fusion` (lowercase).
- File fields are **`usdzFile`, `f3dFile`, `brdFile`, `schFile`, `silkscreenTopFile`**
  (KiCad equivalents: `glbFile`, `kicadPcbFile`, `kicadSchFile`, `kicadProFile`; EasyEDA: `objFile`;
  optional `silkscreenBottomFile`).
- **Fusion source requires ALL FIVE** `.usdz`+`.f3d`+`.brd`+`.sch`+silk-top. The CLI's help table claims
  only usdz + silk-top are required; the SERVER rejects anything short of all five with
  `"Fusion requires .usdz, .f3d, .brd, .sch, and silkscreen top .png files"`. That message is static
  (it always lists all five), so it does NOT tell you which one is missing - supply the whole set.

The page then lives at `https://hydrogen.adom.inc/{username}/molecules/{Name}` (login-gated) and the
USDZ->GLB optimization is queued automatically (ready in a few minutes).

## The pretty 3D: `fusion_export_optimized_glb` (do NOT ship a raw STEP dump)

A raw `fusion_export_step` + a plain `adom-step2glb convert` leaves EVERY solid/pad/via
as its own draw call: a real board comes out ~16 MB / ~25,000 primitives that HALTS the
viewer's GPU on rotation (John hit this live). The bridge verb runs the board through
**service-step2glb molecule mode** instead (the OCCT replacement for Colby's Blender
pipeline: anchor to MP pins -> optional silk bake -> gold pins -> dedup/flatten/JOIN/weld/
prune -> Draco), so the same board comes out **~465 KB / ~31 draw calls, auto-anchored**.
ALWAYS use this for a component page's `component.parts.model_3d` - never a raw STEP->GLB.

Two verbs, because a big board tessellates for minutes (longer than the ab relay's ~60s
request timeout), so it is submit-then-poll, never one long blocking call:

```
# 1) export STEP + submit to the optimizer (returns fast with a jobId; small boards may
#    already come back complete within `wait` seconds)
fusion_export_optimized_glb {"outputPath":"%TEMP%/board.glb", "async":true}
#    board:true (default) also exports the EAGLE .brd and sends it, so the service writes
#    <stem>_footprint.json and <stem>_symbol.json (per-pad wire-attach Z) beside the GLB.
#    gerbers:true (default) writes <stem>-fab.zip (Gerber + Excellon + .gbrjob from the bundled CAM job) and
#    export.json (every source file with sha256 + role, Fusion version, stackup) beside the outputs.
#    compress: quantize|draco|none (optional). pin: fallback only; the size comes from the MP marker names.
#    -> { pending:true, jobId:"...", stepPath, boardPath, resultUrl }   (big board)
#    -> { status:"complete", glbPath, boardPath, fabZipPath, exportRecordPath, footprintPath, symbolPath, webGlbPath, gates }  (small board)

# 2) finish it: re-call until status=complete (each call polls ~15s, well under the relay
#    timeout, and writes the GLB on the box when ready). No Fusion needed for this verb.
fusion_fetch_optimized_glb {"jobId":"...", "outputPath":"%TEMP%/board.glb", "wait":15}
#    -> { status:"complete", glbPath, sizeBytes, footprintPath, symbolPath, gates }   then pull_file them.
```

**Gate before publishing.** `gates` carries the molecule-publish stop signs from the service:
`pinSize` + `pinSizeSource` (expect `markers`), `footprintApplied` (expect true) +
`footprintPins` (expect 4), `footprintRebase` (absent or small), `contactZ`, and `warnings`
(expect none). When one is not met, the reply's `_hint` starts with "Publish gates NOT met" and
names it. `moleculeAnchored:false` means the MP1-MP4 markers are missing from the STEP.

Gotchas baked in from the live bring-up (2026-07-14):
- **The *.adom.cloud edge WAF 403s the default `Python-urllib` User-Agent.** The bridge sets
  a UA on every service call; if you ever call the service yourself from Python, set one too.
- **`/jobs/{id}/result` is ONE-SHOT** - it returns the GLB once, then empty. So fetch exactly
  once per job when complete; do not race multiple overlapping fetches at the same outputPath
  (they truncate each other to 0 bytes).
- Keep `wait` small (<=~25s). A `wait` big enough to exceed the relay timeout loses the response
  even though the box finished - and with a one-shot result, the re-call then gets nothing.
- Silkscreen bake needs the silk PNGs registered to the board XY bbox (Drew's contract); the
  verb captures them best-effort and still returns a great (un-silked) GLB if capture is off.

## A parallel, PUBLIC option: an adom-wiki `component` page

If you also want a browsable PUBLIC page with a live 3D viewer (no molecule login), scaffold an
adom-wiki component page and push a **colored GLB** made from the STEP via `adom-step2glb` (OCCT XCAF
preserves the Fusion material colors). `adom-wiki pkg init <slug> --type component`, set
`component.parts.model_3d` to the `.glb` in `page.json`, `pkg publish --org adom --public`, then
`repo push` the page.json + GLB + images (publish only ships the package layer, NOT the page repo).
The native `/viewer/3d/component/<slug>` iframe renders the GLB in Babylon. Verified live.


## PUBLISH EVERYTHING (John, hard rule, 2026-07-15)

A board/molecule wiki page is about SHARING. Push absolutely every design file you can produce into
the page repo - never just the GLB + images. The full set for a Fusion board:

| File | How |
|---|---|
| `.f3d` (Fusion archive) | `fusion_export_f3d` (3D view active) |
| `.step` | `fusion_export_step` |
| `.usdz` | `fusion_export_usdz` |
| EAGLE `.brd` / `.sch` | `fusion_export_eagle_source` (board / schematic editor active) |
| Fusion `.fbrd` / `.fsch` | `fusion_export_source` (board / schematic editor active) |
| gerbers ZIP | `fusion_export_gerbers` |
| BOM `.csv` | `fusion_export_bom` (board editor) |
| CPL (pick and place) `.csv` | `fusion_export_cpl` (board editor) |
| GLB (assembled + exploded) | the optimized-GLB pipeline in this skill |
| renders (iso/top), schematic PNG | viewport captures |

If a file type exists for the design, it belongs in the repo. List them all in the README Files
section so humans see what is available.

## Related
- `fusion-electronics` / `fusion-eagle-commands` - open the PROJECT and drive the editors first.
- `adom-molecule` (user skill) - the CLI reference (note its auth gap above).
- `fusion-bridge-dev` - if you add a one-call `fusion_export_molecule_bundle` verb, it belongs here.
