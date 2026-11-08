---
name: fusion-publishable-lbr
description: How to generate a PUBLISHABLE .lbr for a component wiki page from its KiCad sources (.kicad_sym + .kicad_mod), and the four gate checks that must pass before it ships. Covers the platform constraint (adom-lbr is a Linux binary, so this NEVER runs inside the bridge - it runs in WSL/a container at authoring time), the exact `adom-lbr generate` invocation, why `adom-lbr validate` is necessary but NOT sufficient, the bundled lbrgate.py that catches dangling connects and account-scoped package3d urns, the pinmap.json sidecar for parts whose pin numbers disagree with pad names, and what a component wiki repo must carry. Read this before publishing any .lbr to a wiki page, before trusting a generated library, or when fusion_create_library returns adom_lbr_unavailable. Trigger words - publishable lbr, generate lbr from kicad, kicad_sym to lbr, adom-lbr generate, adom-lbr validate, adom_lbr_unavailable, validate lbr, dangling connect, connect pad missing, pinmap.json, pin map sidecar, wip_urn in published lbr, component wiki repo files, component linter, lbr gate, lbr publish checklist.
---

# Generating a publishable `.lbr` from a component page's KiCad sources

Parent skill: [fusion-libraries](../fusion-libraries/SKILL.md)

The authoring-time half of [fusion-libraries](../fusion-libraries/SKILL.md). That skill covers
getting a library *into Fusion on this box*. This one covers producing a `.lbr` that is correct
for **everyone else**, and proving it before it ships.

Written 2026-08-21 from a ralph test of web control's library creation across 13 Adom demo
components. 12 came out clean; the 13th exposed a whole failure class (see step 4).

## Why this cannot run in the bridge

```
a bridge          runs on Windows or macOS
adom-lbr          is a Linux binary, and always will be
a browser         runs no binaries at all
```

`fusion_create_library` shape B (`kicadSymbolPath` + `kicadFootprintPath` + `stepPath`) shells out
to `adom-lbr` and returns **`adom_lbr_unavailable`** on every Windows box. That is not a bug to fix
in the bridge - there will never be a Windows build. So conversion happens **at authoring time**
(here) or **on the wiki** (Linux), and the bridge only ever consumes a finished `.lbr`.

Tracking: adom/adom-bridge#92 (the file spec), adom/component-widgets#1 (a wiki-side generator),
adom/fusion-bridge#43 (which shape the bridge should keep).

## Step 0 - run adom-lbr in WSL, not Windows

```
wsl_exec { distro: "Adom-Workspace", user: "adom", scriptB64: <base64 of the script> }
```

Windows paths bridge in as `/mnt/c/...`, so stage sources under `C:\tmp\partaudit` and read them
from the container without copying. `adom-lbr` lives at `/home/adom/.local/bin/adom-lbr` (2.7.1
at time of writing). See the `adom-binaries-run-in-wsl` skill.

## Step 1 - pull the component page's KiCad sources

```bash
adom-wiki repo clone adom/<mpn> --dir /c/tmp/partaudit/<mpn>
```

You need `<MPN>.kicad_sym` and `<MPN>.kicad_mod`. If either is missing the page is not ready and
no amount of conversion will fix it - report the gap rather than synthesizing one.

## Step 2 - generate

```bash
adom-lbr generate --sym "$p.kicad_sym" --fp "$p.kicad_mod" -o "$OUT/$p.lbr"
```

`generate` binds the symbol pin's **NUMBER** to the footprint pad's **NAME**. That is the KiCad
convention and it holds for the overwhelming majority of parts, including a 94-pin BGA. Step 5 is
about the minority where it does not.

Batch form: `genscript.sh` in this folder.

## Step 3 - `adom-lbr validate` (necessary, NOT sufficient)

```bash
adom-lbr validate "$OUT/$p.lbr"
  -> OK: ... is a valid EAGLE .lbr with complete deviceset
```

This checks the deviceset has a symbol, a package, and connects. **It does not check that the
connects point at anything real.** Do not stop here. (Filed as adom-lbr#9.)

## Step 4 - the publish gate (`lbrgate.py`, bundled here)

```bash
python lbrgate.py out/*.lbr        # exit 0 = every file publishable
```

Four checks, and **every one of them is a failure that actually happened**, not a hypothetical:

**1. `<packages>` non-empty.** `adom/cr0603-fx-5601elf` shipped a `.lbr` with zero packages while
its `.kicad_mod` had two perfectly good SMD pads. Downstream it looked like a mysterious Fusion
failure; it was an empty library.

**2. Every `<connect>` resolves** - its `pin` exists in the referenced gate's symbol and its `pad`
exists in the device's package. `adom/type-c-31-m-12` generated:

```xml
<connect gate="G$1" pin="SHIELD" pad="S1"/>   <!-- package pads are A1..B12, SH -->
```

The shield connects to nothing. **This is invisible to every check a human would make**: symbol
renders, footprint renders, 3D renders, a four-frame Fusion evidence set passes 3/3. On a USB-C
receptacle it is an EMI and retention defect that only shows up in hardware. The gate names the
unreferenced pad, which is the useful half - `SH` is obviously the intended target:

```
FAIL  type-c-31-m-12.lbr
        USB_C_Receptacle_USB2.0_16P: pin 'SHIELD' -> pad 'S1', no such pad in
        package 'USB_C_Receptacle_USB2.0_16P' (unused pads: SH)
```

Worth knowing: **that part is broken in KiCad too**, for the same reason - KiCad also binds pin
number to pad name. So the gate catches genuine source-data defects, not conversion artifacts.

**3. No `<package3d>` / `wip_urn` in a published `.lbr`.** A Fusion `package3d` is an
**account-scoped pointer** into the minting user's Autodesk cloud storage, not embedded geometry.
Measured: a bound `.lbr` is 7,340 bytes for a 120,696-byte STEP. Publishing one hands everybody
else a dead link. Ship the `.step` **beside** the `.lbr` and let each viewer's Fusion mint its own
(that is what `fusion_attach_3d_package` is for - see
[fusion-libraries](../fusion-libraries/SKILL.md)).

**4. Pin coverage** - every symbol pin appears in a connect. A silently dropped pin is the
dangerous case for exactly the reason in check 2.

## Step 5 - when the convention does not hold: `pinmap.json`

`adom-lbr generate` has **no pin-map input** (`--sym --fp -o --name --symbol-art
--symbol-art-scale`), so a diverging part cannot currently be converted correctly by the generator
alone. Two ways forward:

- publish a `pinmap.json` sidecar on the component page - `{"S1": "SH"}` - which
  `fusion_create_library` accepts as `pinMap` and reports back as `pinMapApplied`; or
- fix the source: rename the pad in the `.kicad_mod`, or the pin number in the `.kicad_sym`, so
  they agree. This is the better fix when you own the part, because it also fixes KiCad.

Frequency, measured: **1 of 13 parts**. Do not add a pin map by default - run the gate and let it
tell you.

## Step 6 - the hub JSON (optional but recommended)

```bash
adom-lbr import-kicad --sym "$p.kicad_sym" --fp "$p.kicad_mod" --mpn "$p" -o "$p.adom-lbr.json"
```

The canonical cross-EDA record: symbol + footprint + 3D ref + an explicit `connect: [{pin, pad}]`
map. Worth publishing because it makes the pin correspondence **inspectable** without parsing
EAGLE XML.

## Step 7 - publish

```bash
adom-wiki repo clone adom/<mpn> --dir <dir>
cp out/<MPN>.lbr <dir>/
adom-wiki repo commit <dir> -m "add validated .lbr"
adom-wiki repo push adom/<mpn>
```

Run the gate one more time on exactly the bytes you are about to push.

## What a component wiki repo must carry

| file | required | why |
|---|---|---|
| `<MPN>.kicad_sym` | yes | symbol, the source of truth |
| `<MPN>.kicad_mod` | yes | footprint, the source of truth |
| `<MPN>.step` | yes | 3D. Never optional, because of gate check 3 |
| `<MPN>.lbr` | yes | gate-passing, no `package3d` |
| `<MPN>.adom-lbr.json` | recommended | inspectable pin/pad map |
| `pinmap.json` | only when the gate says so | `{pinNumber: padName}` |

## Result of the 2026-08-21 run

13 parts regenerated from their own KiCad sources with `adom-lbr 2.7.1`. All 13 pass the gate.
12 needed nothing beyond `generate`; `type-c-31-m-12` needed the shield pad corrected. One part
(`cr0603-fx-5601elf`) had shipped an empty-package `.lbr` that `validate` had passed.
