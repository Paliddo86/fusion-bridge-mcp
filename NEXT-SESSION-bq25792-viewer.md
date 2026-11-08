# HANDOFF: BQ25792 charger viewer — Fusion-source rebuild (do this FIRST next session)

Live page: https://wiki.adom.inc/adom/bq25792-charger  (readme viewer at v0.1.49)
Working dir: /tmp/bq25792-charger  (readme.html + the group GLB PNGs)

## The pivot John demanded (2026-07-16)

STOP reverse-engineering the flattened/join-merged GLB. Build the viewer's layers from
FUSION SOURCE TRUTH instead. The merged GLB is why components can't be identified and why
the inner copper is faked from gerber raster PNGs.

## Decisions already locked (do NOT re-ask)

- Machine-pins layer = ALL 14 (MP1..MP14), by NAME from Fusion. Anchored, never animated.
- Everything else (C*/R*/U*/L*/J*/D*/Q*...) -> "Components" layer (the animated top layer).
- Inner copper L2/L15 must be REAL VECTORS from Fusion, same PBR copper material as outer,
  NOT gerber PNG planes.
- Fusion is NOT open on the board -> must open by URN first (Main project, doc v222).

## PROGRESS 2026-07-16 (started, then hit context limit + relay flap)

- ADOMGPU: Fusion NOT installed (fusion_readiness). Use AdomLapper instead.
- AdomLapper: Fusion INSTALLED. `fusion_start` SUCCEEDED (addinReady:true). Active doc = "Untitled"
  (board NOT open yet).
- BLOCKER hit: locating the board in the cloud. `fusion_search_cloud_files {query:"BQ25792"}` and
  {query:"BQ",project:"Main"} both returned 0 files (searched only 1 folder — search scope/project
  arg may be wrong). `fusion_walk_cloud_tree` then returned a stale "call fusion_start" hint = relay
  FLAP (Fusion was running). Retry the walk/search after re-confirming fusion_get_app_state; may
  need the exact project name or a cached URN. The .f3d is ALSO already on the wiki page
  (bq25792-charger.f3d in /tmp/bq25792-charger) — could fusion_aps_upload + open that, or open the
  local f3d directly, instead of hunting the cloud original.

## Exact plan (one source pass fixes BOTH components + copper)

1. On AdomLapper: Fusion already startable. Open BQ25792 — either find the cloud URN (retry
   search/walk after app_state re-confirm) OR just open the local .f3d from the wiki page
   (/tmp/bq25792-charger/bq25792-charger.f3d) which is the same v222 design.
2. `fusion_run_modeling_script` — ONE script that:
   a. Walks the occurrence tree; export each occurrence's mesh tagged by designator.
      MP1..MP14 -> machine-pins group; all others -> components group.
   b. Pulls each copper layer's polygons (top/L2/L15/bottom) as geometry.
      Two options: (i) the Electronics board API trace/polygon geometry, or
      (ii) `fusion_export_dxf` per copper layer (DXF = vectors) then triangulate closed polys.
      NOTE the earlier gerber2glb bug: gerber coords are MILLI-UNITS; I mis-scaled (÷1e6 vs
      the real viewBox units) which blew the mesh up to a speck. DXF avoids that; if reusing
      gerber2glb (/tmp/gerber2glb.mjs), fix the unit scale against the viewBox first.
3. Rebuild the viewer layers from that source data. The engine already works — only the layer
   SOURCES change. Keep: cascade-up animation, layers panel (live z + visibility + hover-
   highlight), realtime order check, Play/Pause + scrub (2-min resume), draggable floaties,
   Click-pivot toggle (camera.setClickToSetRotationPoint), Ground toggle, deterministic
   framing (radius 0.115 at board center 25.2,13.2,18mm — computed diag blows up, don't use it).
4. Verify on AdomLapper (session bqview, hwnd in /tmp/bqview_hwnd.txt), flash, show John.

## What already works at v0.1.49 (don't regress)

Native Adom Babylon9 viewer + ViewCube; cascading UPWARD explode (nothing below ground);
layers panel; order check; Play/Pause + scrub; Click-pivot + Ground toggles; Fullscreen;
draggable floaties; unified PBR copper (albedo 0.95,0.60,0.42 / metallic 1 / roughness 0.28 /
envIntensity 1.2); transparent ground (alpha 0.2) + every-frame shadow refresh.

## Key infra facts

- readme CSP: connect-src 'none' -> models ride as lossless PNGs in the repo (bytes in RGB,
  4-byte LE length header), decoded via CORS canvas (loadPngBytes). ~4-5MB readme cap.
- Storage shim in <head> (sandboxed docs throw on localStorage -> aborts the bundle's rig).
- Blob->dataURI shim in the module (img-src has no blob:).
- Group GLBs: optimize with --instance false; do NOT quantize (bounds lie); images -> data URIs.
- Publish loop: repo push + `adom-wiki pkg publish --org adom --public -y`, bump version in
  BOTH package.json and page.json.
- Platform asks on adom/wiki #47 (CSP, size cap, /blob content-types, iframe height,
  allowfullscreen, deployed-bundle sandbox rig, native exploded-animation mode).

## Skill draft (private, NOT yet shipped — too hacky per John)

/home/adom/project/wiki-readme-3d-skill/SKILL.md — full recipe. Ship to the wiki skillpack
ONLY once the Fusion-source approach replaces the hacks.
