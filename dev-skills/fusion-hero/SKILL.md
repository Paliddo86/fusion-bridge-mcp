---
name: fusion-hero
description: Maintainer recipe for the fusion-bridge hero image - content contract, exact layout tokens, the window clamp bypass for any aspect ratio, the pixel-scan framing loop, and the Hero Studio gate traps (double pill, marquee-scrim bake, silent flash)
---

# fusion-hero - making the Fusion bridge's hero image

The exact recipe behind `screenshots/hero.png` on adom/fusion-bridge, iterated with John
across NINE versions (2026-08-09) until every margin passed his review. Use this to
regenerate the fusion hero, and as the reference recipe for any bridge hero built around
a real desktop app window (kicad is the sibling case). Family references: pup's hero
(adom/pup-bridge hero-v9.png) and adom-native-browser's.

Trigger words: fusion hero, hero image, regenerate the hero, bridge hero, hero recipe.

## Content contract (locked with John, never re-guess)

| Field | Value |
|---|---|
| Hero title | **Fusion** - only the single word users type to their AI. The page's dynamic title carries the rest |
| Page title | Fusion - the Fusion 360 Bridge (parallel to "Pup - Puppeteer Bridge") |
| Subtitle | Your real CAD, driven by AI. - ONE line (never wrap it), accent gradient on "driven by AI." |
| Chip | BRIDGE, solid mint pill, top right, the ONLY pill (see the double-pill trap) |
| Vitrine | The REAL Fusion window, native chrome, UNCROPPED, BQ25792 board in 3D PCB view on the dark River Rubicon canvas |
| Not present | No URL line, no prompt pill, no detail line, no word-wrapped subtitle |

## The two laws (violating either got versions rejected)

1. **Never crop the screenshot.** Decide the vitrine box from the margin rules FIRST,
   then make the WINDOW that exact aspect and shoot it. Do not reuse an old capture and
   shuffle the layout around it; every layout change that alters the slot ratio means a
   fresh reshoot at the new ratio. "It's just math" (John): the window can be any pixel
   SIZE, it is the RATIO that must equal the slot's.
2. **Margins like a professional graphic designer leaves.** The approved rhythm at
   1600x1000: title-to-vitrine gutter **130**, pill air above the vitrine ~**115**,
   vitrine bottom edge pinned at **910** (90 from canvas bottom), vitrine right edge
   ALIGNED with the pill's right edge on the 42px margin line. Subtitle sits close
   enough to the title to feel connected (top 405 under a 235px title).

## Sizing the window to ANY ratio (the clamp bypass)

Fusion's ribbon imposes a hard minimum width (1822 physical at 150% DPI), and Windows'
max-track clamps programmatic sizing at roughly screen height, so naive
`desktop_set_window_bounds` cannot reach a tall ratio. Interactive dragging cannot
either (John tried). The bypass: raw `SetWindowPos` with **`SWP_NOSENDCHANGING`**
(0x0400) skips WM_GETMINMAXINFO enforcement, and the window happily exceeds the screen;
background capture (PrintWindow) renders off-screen regions perfectly.

```powershell
Add-Type '...user32 SetWindowPos...'
[WP]::SetWindowPos([IntPtr]$hwnd, [IntPtr]::Zero, 100, 0, 1822, 1793, 0x0414)
# flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING
```

Note the call takes LOGICAL px from a DPI-unaware shell: at 150% the window lands 1.5x
larger in physical px, which is a BONUS (finer chrome, crisper board). Record the
original rect first and restore it after the shoot.

Final approved numbers: slot 751x739 at (807,171), so ratio = 751/739 = **1.0162**,
window sized 1822x1793 logical = 2734x2690 physical, captured at 1568x1543.

## Preparing the shot

1. Dark mode, all three layers, per the `fusion-dark-mode` skill. The one that matters:
   viewport Display Settings > Environment > **River Rubicon**.
2. `fusion_open_cloud_file {"fileName":"BQ25792","projectName":"Main","folderPath":"Molecules/XRP/Power"}`
   then `fusion_show_2d_board` then `fusion_show_3d_board` (3D only reachable from 2D).
3. Size the window to the slot ratio (above).
4. Frame by MEASUREMENT, never eyeball: run `vp.fit(); cam.viewExtents *= K; vp.camera = cam;
   vp.refresh()` via `fusion_run_modeling_script` (expectDocument!), capture, then
   pixel-scan the capture for the board and iterate K until the board sits at ~70-75%
   of the viewport with balanced margins. K landed at **0.855** for the final ratio.
   TRAP: green-pixel scans match Fusion's green TOOLBAR ICONS; scan the viewport
   interior only (exclude ~15% top chrome, ~7% bottom).
5. `desktop_screenshot_window {hwnd}` in the background; use the auto-pulled `.full.png`.

## Layout tokens (1600x1000 canvas, render @2x, deliver 2000x1250)

| Element | Spec |
|---|---|
| Background | `radial-gradient(1100px 750px at 92% -8%, rgba(0,184,177,.16), transparent 60%), linear-gradient(155deg,#0a0e14,#0d1117 48%,#0a1216)` |
| ADOM wordmark | left 64, top 51, height 40, white SVG |
| Title | Familjen Grotesk 700, 235px, letter-spacing -7px, #eef3f8, box at (50,128) so the F's 14px side bearing lands the INK at 64 |
| Subtitle | Satoshi 400, 35px, box at (64,405), one line, accent `linear-gradient(100deg,#39b8ff,#5ea0f2 60%,#8c6bf7)` text-clipped |
| BRIDGE pill | top 42, right 42, bg #7ce0d3, radius 999, padding 10x22, Familjen 700 16px ls 2.4, color #0b2723 |
| Vitrine | left 807, top 171, width 751, height 739 (img width:100%, NO object-fit), radius 12, shadow `0 30px 90px rgba(0,0,0,.66), 0 0 0 1px rgba(255,255,255,.08)` |

Render with the `adom-wiki-hero-image` machinery (render.js, bundled woff2 fonts) at
deviceScaleFactor 2, then PIL-resize to 2000x1250 LANCZOS, save optimize=True. LOOK at
every render before shipping; the margin failures were all invisible in code.

## The Hero Studio gate (a human generates, never you)

1. `hero-studio pose adom/fusion-bridge --surface pup --port <free>` on a parallel port
   if another thread's gate is open. The pup leg is broken post-rename (hero-studio#559):
   pose exits 3 AFTER arming the gate server-side, so open/reload the studio proxy URL
   in your own pup session and poll GET /state for `approved` yourself.
2. Stage the PNG: `hero-studio vitrine --file screenshots/hero.png --full --port <p>`.
   Verify by sha: state.vitrine.src bytes == local file. Restage after every revision;
   the open gate updates live.
3. **Before the human clicks Generate, force ALL THREE:**
   `hero-studio set layout=contained marqueePreview=false overlay.pill=false --port <p>`
   - **layout=contained is the critical one.** The default `showcase` layout composites
     its gradient SCRIM over a full-hero import and BAKES it into the export: the first
     approved export shipped with dimmed title/wordmark/subtitle while the vitrine stayed
     bright. Cost a full human re-review round to root-cause (hero-studio bug).
   - overlay.pill=true bakes the studio's type pill next to our BRIDGE chip = two pills.
   - marqueePreview off is hygiene; it was NOT the dim culprit (it was already off when
     the dim export happened).
   Then PROVE fidelity before wasting a human click: `POST /generate {"renderer":"local"}`
   is sanctioned for testing; pixel-diff the test export against the staged PNG
   (mean abs diff under ~1.0 and equal title-zone brightness = faithful).
4. If the page title changed since pose: `POST /api/pages` upsert, or the marquee band
   previews stale data.
5. Signal the human by flashing: ab's `desktop_flash_window {hwnd}` directly.
   `pup_alert_window` silently no-ops once the window title changed.
6. After Generate: verify the export is FAITHFUL before applying (compare geometry AND
   brightness against the staged PNG; the stamp does not guarantee fidelity). Read
   `hero-studio changes` and sync any gate-time field edits into page.json.

## Publish (the hero-unlink trap)

```bash
adom-wiki repo push adom/fusion-bridge --files screenshots/hero.png -m "hero"
adom-wiki page hero adom/fusion-bridge --image screenshots/hero.png
adom-wiki repo push adom/fusion-bridge --files page.json -m "page.json LAST"
```

Same `screenshots/hero.png` path every revision so the URL stays stable. Always push a
COMPLETE page.json LAST; a stale one clobbers the page title (done once, restored).

## Optical alignment (measure ink, not boxes)

Left-align the wordmark, title, and subtitle by their INK pixels, not their CSS boxes:
glyph side bearings differ per font and size (the 235px Familjen F carries ~14px). After
every render, scan each element's leftmost ink column at export scale and adjust the
boxes until all three land within ~2px. The approved hero: all three at 80px (2000-wide).
