# Creating a Fusion 360 library of Adom basic parts (the full tool-chain tutorial)

How to build ONE Fusion 360 `.lbr` library that holds a whole sampler of popular Adom parts -
resistors, capacitors, inductors, LEDs, microcontrollers, op-amps, ADCs, sensors - each with a
schematic symbol, a real footprint, AND the real 3D model attached. This is also the recipe behind
the **montage demo video** on this page, so it doubles as a tutorial: every Adom tool used, in order,
and how each one was shown on camera.

> Companion guides in the **`fusion-libraries`** skill: `MAKING_LIBRARIES.md` (one part, end to end) and
> `LIBRARY_FINDINGS.md` (the hard-won findings). This file is the *many-part,
> all-the-tools* version. **The per-package 3D method is in the
> [fusion-multipart-libraries skill](skills/fusion-multipart-libraries/SKILL.md) - read that first.**

## ✅ THE SOLVED METHOD (2026-06-29) - programmatic, zero GUI dialogs, every part renders

Getting a distinct, RENDERING 3D model onto each package in one `.lbr` is solved, and it does NOT use
the GUI Package3D generator (that path is trapped behind unbeatable CEF dialogs).

**⚡ The whole library in ONE call (v1.6.0+):** `fusion_build_library_3d {lbrPath:"C:/.../Combined.lbr",
parts:[{package,lbrPath,modelPath}, ...]}` runs the entire pipeline - per-part footprint+chip `.f3d`
packages, **all** bindings injected in one pass, the finished library opened once - and returns each
part's `wip_urn` plus a BEFORE (footprint in the 3D viewer) and AFTER (chip placed) screenshot.
`projectId`/`folderId` default to the MAIN-project upload folder. This is the verb to use; the per-part
steps below are what it does internally (read them to debug).

Per part (the manual equivalent):

1. `fusion_make_3d_package {lbrPath:"C:/.../PART.lbr", modelPath:"C:/.../PART.step"}` - loads the
   package FOOTPRINT, imports the STEP onto it, orients flat, saves an `.f3d`, uploads it, and returns a
   `wip_urn`. **One verb, no dialogs.** (projectId/folderId default to the MAIN-project upload folder.)
2. Hand-write that `wip_urn` into the combined `.lbr` (a `<package3d>` in `<packages3d>` + a
   `<package3dinstance>` in the device - exact markup in the skill). `fusion_build_library_3d` does this
   for ALL parts in one deterministic pass.
3. `fusion_open_lbr` the merged library -> every chip RENDERS, flat on its footprint; save to cloud.

⛔ **Use an f3d, not a raw STEP:** a raw STEP upload binds but shows "Thumbnail download failed" (it is
not a renderable Fusion asset); `fusion_make_3d_package` produces an f3d, which renders. Proven on a
10-part Adom basic-parts library (R/C/L/LED/nRF54L15), every package rendering. Full recipe + the dead
ends to avoid: the [fusion-multipart-libraries skill](skills/fusion-multipart-libraries/SKILL.md).

## ⛔ THE HARD REQUIREMENT - the symbol must carry the 3D chip outline

**Every schematic symbol you send into Fusion MUST have the part's 3D chip outline drawn inside the
symbol body.** Not optional. A symbol that is just pins-and-a-box is a FAIL.

- The outline is a vector line-drawing of the actual chip (from **adom-symbol**, the `*-3d-outline-*.svg`).
- It gets into the symbol via **`adom-lbr generate --symbol-art <outline.svg>`**, which draws the SVG
  as raw `<wire>` lines centred in the symbol body (size via `--symbol-art-scale`, default 0.38).
- **Verify it in Fusion**: open the library, look at the symbol - the chip outline must be visible
  behind/within the pins. If it is not there, you used the wrong file or skipped `--symbol-art`. Fix it
  before continuing; do not ship a symbol without the outline.

## The tools (each has a live app - open it in a pup tab for the video)

| Tool | What it does here | Show it via |
|---|---|---|
| **chip-fetcher** | downloads the manufacturer symbol + footprint + STEP bundle for an MPN; the curated `library/<MPN>/` is the content source | `chip-fetcher serve` / `app` viewer + dashboard; `chip-fetcher fetch <MPN>` |
| **chip-fetcher-lite** (`adom/chip-fetcher-lite`) | wiki-first staging for **Adom BASIC parts** (R/C/L/LED). Add by MPN or a description like `100nF 0402 capacitor`; per-row KiCad/Altium/Fusion/JLC status boxes, grouped by category; files them into your libs | the wiki app (Node, `public/index.html`) - open the page in a pup tab |
| **adom-symbol** | builds + renders the schematic symbol AND the **3D-outline SVG** that becomes the symbol-art | `adom-symbol serve` (live app) + `adom-symbol ui ...` to drive it; `render` / `embed` |
| **adom-footprint** | builds + renders the footprint (`.kicad_mod`) with layer / paste-dot views | `adom-footprint serve` + `ui`; `embed` |
| **adom-lbr** | assembles the EAGLE `.lbr` (symbol + footprint + deviceset), embeds the symbol-art, writes the 3D sidecar | `adom-lbr generate ...`; `adom-lbr serve --file X.lbr` (symbol \| footprint \| 3D + pin-map viewer) |
| **fusion-bridge** (this repo) | opens the `.lbr` in Fusion and attaches the 3D | `fusion_open_lbr`, `fusion_attach_3d_package` |
| **the Adom Basic Parts wiki** (`adom/basic-parts-library`) | the canonical catalog: 79 resistors, 40 MLCCs, 6 inductors, 11 LEDs, ferrite beads | open the page in a pup tab (it has a fun interactive viewer) |

## Build recipe (per part)

1. **Get the curated CAD.** `chip-fetcher/library/<MPN>/` carries the Fusion variant set:
   `<MPN>.fusion.kicad_sym`, `<MPN>.fusion.kicad_mod`, `<MPN>-named.step` (3D with the part number
   embossed), and `<MPN>-3d-outline-bold.svg` (**the symbol-art**). For Adom basic passives, the STEP /
   footprint / symbol already exist on the wiki - **do not invent geometry**. Stage them with
   **chip-fetcher-lite** (resolve a description like `100nF 0402 capacitor` to the real MPN page and pull
   its Fusion files), then run the STEP through **service-step2glb** (`adom-step2glb`) exactly like the
   chip-fetcher tools do, to produce the GLB + the named/outline assets. Then feed those to adom-symbol
   / adom-lbr the same way as an IC.
2. **Generate the part `.lbr` WITH the outline + the model:**
   ```bash
   adom-lbr generate \
     --sym  <MPN>.fusion.kicad_sym \
     --fp   <MPN>.fusion.kicad_mod \
     --name <MPN> \
     --symbol-art <MPN>-3d-outline-bold.svg \   # ⛔ the 3D outline in the symbol - REQUIRED
     --model      <MPN>-named.step \            # writes the <out>.fusion3d.json 3D handoff sidecar
     -o <MPN>.lbr
   ```
   The `.lbr` stays 2D (a valid Fusion 3D ref needs an Autodesk cloud URN only Fusion can mint); the
   sidecar tells the bridge which model to attach and how to align it.
3. **Merge all parts into ONE library.** An EAGLE `.lbr` is a library of many devicesets, so combine
   the per-part files: keep one file's `<layers>` skeleton, then concatenate every part's `<package>`,
   `<symbol>`, `<deviceset>` (and `<package3d>` if pre-bound) children. Names are MPN-unique, so no
   collisions. Validate: `adom-lbr validate combined.lbr`.
4. **Into Fusion.** Stage `combined.lbr` (+ each `<MPN>.step`) to Windows (no container→Windows push
   verb - serve + `Invoke-WebRequest`), `fusion_open_lbr`, then `fusion_attach_3d_package` **once per
   part** (pass `packageName`). Save the library to the cloud, **close, reopen** - that forces the 3D
   previews to refresh from gray placeholder to the real chip.

## The montage video recipe (how this page's video was made)

- Open EVERY tool app + the basic-parts wiki pages as **tabs in ONE maximized pup window**, and
  **swap between tabs on camera** - that swapping IS the montage.
- Record each tool *doing its thing* as a short pup snippet: chip-fetcher fetching a bundle,
  chip-fetcher-lite's category status boxes lighting up, **adom-symbol showing the symbol WITH the 3D
  outline**, adom-footprint's layers, the adom-lbr viewer (symbol \| footprint \| 3D), the fun
  basic-parts wiki page.
- Record **Fusion** (WGC, `desktop_record_window_start` on the main "Autodesk Fusion" window) opening
  the combined library and attaching the 3D.
- Speed the snippets up and stitch them in **video-post** (`adom-video-post`): a "what it takes"
  montage with burned-in captions + a TTS voiceover. Publish to this page and show it in pup.

## Part selection (the sampler - cover every category)

Ten popular parts spanning the categories: **MCUs** (ESP32-S3-WROOM-1, ATSAMD51J20A), **op-amp**
(LM358), **ADC** (ADS1115), **MOSFET** (SI2302), **sensor** (BME688), **resistor** (WSL2010), **LED**
(VLMS1500), **capacitor** (0603 100nF), **inductor** (0805 10µH). Pick parts that have the full curated
set so every symbol gets its outline and every package gets real 3D.

## Gotchas (each one bit us - see the `fusion-libraries` skill's LIBRARY_FINDINGS.md for detail)

- **Stale `.lbr` = wrong footprint.** Regenerate with the CURRENT `adom-lbr`. The pup viewer renders the
  footprint from the SVG, so a `.lbr` can LOOK right while its pads are an old 2-column placeholder.
  Verify the pad layout: a 4-sided LQFP has many distinct X **and** Y, not 2 columns of pads.
- **No symbol-art = requirement failure.** If the symbol has no outline, you skipped `--symbol-art` or
  pointed it at the wrong SVG.
- **One bad part poisons the WHOLE merged library.** Fusion rejects the entire `.lbr` with a generic
  "has errors and cannot be opened" if ANY device has an invalid `<connect>` (a pin mapped to a pad the
  package doesn't define). This bit us on WSL2010 (chip-fetcher gave it a 3-pin symbol but a 2-pad
  footprint, so pin 3 → pad 3 which doesn't exist). **Before merging, validate every part: for each
  device, every `<connect pad="N">` must reference an `<smd name="N">`/`<pad name="N">` that exists in
  its package.** A single-device `.lbr` may still open, masking the bug - it only blows up once merged.
  (adom-lbr's lint did NOT catch this - file it.) Passives are the usual offenders: their symbol pin
  count can disagree with the footprint pad count.
- **CAPTURE the error dialog.** When `fusion_open_lbr` returns `success:false`, grab the modal error
  with `desktop_screenshot_window` (it returns owned popups) AT open time - don't just log the false.
- **3D preview lags after attach** (gray placeholder) - save → close → reopen the library to refresh.
- **Record the MAIN Fusion window** (title contains "Autodesk Fusion"), not a stray palette window.
- **Caches.** The wiki/CDN caches files for ~24h; publish a new video under a fresh filename or cache-bust.
