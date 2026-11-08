---
name: fusion-part-sourcing
description: Find or create the component to place BEFORE placing it — search the available libraries offline, and when a part does not exist, ASK THE USER for a library path or for permission to create the footprint and symbol. Covers the standard through-hole footprints (LED 5mm, axial resistor, 2.54mm headers) so a THT board is buildable even when every bundled library is SMD-only. Trigger words - part not found, no such symbol, which library, create a footprint, THT through hole LED 5mm, axial resistor footprint, 2.54 header, library missing, place a part, cannot find component.
---

# Source the part before you place it

`fusion_sch_place_part` needs a `.lbr` path AND a `symbol` (deviceset) that exists in it. A wrong
guess costs a failed call and a confused user. Resolve the part first, in this order.

## Step 1 - search what is already reachable (offline, no Fusion needed)

```
fusion_find_symbol {query: "LED_RADIAL"}                    # no path needed: reads Fusion's catalogue
fusion_find_symbol {query: "RESAD", kind: "footprint"}     # PCB packages instead of devicesets
fusion_list_library {libraryPath: "<a.lbr>"}                # every deviceset in one library
fusion_list_library {libraryPath: "<a.lbr>", kind: "pcb"}   # every footprint in one library
```

`find_symbol` returns `matches` (each with its `library` path), `best` (exact first, else the
shortest containing name), plus `via` (`catalogue`, `folder`, `catalogue-all`) and `searched` (how
many libraries it looked in). **Always search before asking anybody anything.**

### Where it looks, in order

| # | Source | How |
|---|---|---|
| 1 | the `.lbr` **you** named | `libraries: ["<path>"]` |
| 2 | **`~/Documents/Fusion 360/libraries/`** | `folder: "~/Documents/Fusion 360/libraries"` - the user's own libraries, e.g. the JLCPCB GPLIB set |
| 3 | **Fusion's own installed libraries** | nothing to pass: `find_symbol` reads `…/Electron/<USER_ID>/lbr/libraries.db`, the same index Library Manager shows (934 libraries on a real box) |
| 4 | nothing found | **ASK THE USER** (Step 3) |

A bare `fusion_find_symbol {query: "…"}` with no path already covers 3 and 4, so prefer it.

### The names are FAMILY-based - do not invent them

Search by family prefix, not by a description of the part. Measured on a real machine:

| You want | Search | Real name found | Where |
|---|---|---|---|
| 5 mm through-hole LED | `LED_RADIAL` | `LED_RADIAL`, `LEDRD254W60D565H860B_G` | `Opto-Electronic.lbr`, `Tinkercad.lbr` |
| axial through-hole resistor | `RESAD` (kind=`footprint`) | `RESAD1176W63L850D250B` | `Resistor.lbr` |
| resistor, schematic side | `RESISTOR` | `RESISTOR` | `Tinkercad.lbr` |
| 2-pin header | `1X02` | `PINHD-1X02`, `1X02`, `1X02_LONGPADS` | `Tinkercad.lbr`, `Connector.lbr` |
| SMD LED | `LED_0603` | `LED_0603`, `LED_0402` | `GPLIB_ACTIVE.lbr` |
| SMD resistor | `RES_0805` | `RES_0805` (523 JLCPCB variants) | `GPLIB_PASSIVE.lbr` |

`LED-5MM`, `LED-5MM-THT`, `R-AXIAL-0207`, `5MM_THT` **do not exist** anywhere. Searching them
returns nothing and looks like a missing part when it is a missing *name*. Note the SMD/THT split:
a through-hole part lives in `Opto-Electronic`/`Resistor`/`Tinkercad`, its SMD twin in the GPLIB
libraries.

## Step 2 - if you found it, place it

```
fusion_sch_place_part {
  expectDocument: "<the design>",
  library: "<the .lbr find_symbol reported>",
  symbol: "<the deviceset name>",
  footprint: "<a package name of that deviceset's variants>",
  designator: "D?", comment: "Green 5mm",
  x: 1000, y: 1000
}
```

Pass the `library` that `find_symbol` returned — do not retype a path from memory.

## Step 3 - if it does NOT exist: ASK, never invent

A missing part is a question, not an error to work around. Say what you searched, what was closest,
and give the user exactly two exits:

> Non ho trovato **LED 5mm through-hole**.
> Cercato in: `AdomBasicParts_full.lbr`, `resources/samples/`.
> Il più vicino: `LED-0603-RED`, `LED-0402-GREEN` (entrambi SMD).
>
> 1. **Dammi il path** di una `.lbr` che ce l'ha — la uso subito.
> 2. **Oppure la creo io**: footprint THT 5mm + simbolo, in una libreria nuova. Servono solo conferma
>    e un path dove scriverla.

Do not substitute an SMD part for a through-hole one silently: the user asked for THT because it is
hand-solderable or because they have the physical part. Offer it as a labelled option, never as a
done deal.

## Step 4 - creating the part (offline, no Fusion needed)

`fusion_create_footprint` and `fusion_create_symbol` write the `.lbr` without opening Fusion, so this
works even while Fusion is busy.

### ⛔ ASK FOR THE FOLDER FIRST

Both verbs take `library_path`. **If the user has not given one, ask for a FOLDER before creating
anything.** The bridge deliberately refuses to pick a location: a library written somewhere the user
did not choose is a file they cannot find next week, and it is their disk.

> Dove salvo la libreria con i footprint che creo? Dammi una cartella (o un path `.lbr`), per
esempio `~/Documents/Fusion/libraries/adom-tht.lbr`. Non scrivo nulla finché non me lo dici.

If `library_path` is omitted the server answers `errorCode: library_path_required` with the same
question - pass that to the user verbatim rather than inventing a directory.

Preferred shapes, once they answer:
- they gave a **folder** → append a sensible filename: `<folder>/adom-tht.lbr`
- they gave an **existing `.lbr`** → add to it (check with `fusion_list_library` first, so a part is
  not overwritten)
- they named a **new `.lbr` path** → use it exactly

### Standard through-hole pads (measurements in mil; 1 mm = 39.37 mil)

| Part | Pad | Pitch | Hole | Notes |
|---|---|---|---|---|
| LED 5 mm THT | Ø 70 mil (1.8 mm) | 100 mil (2.54 mm) | 40 mil (1.0 mm) | round pads; flat side = cathode |
| Resistor axial 0207 | Ø 70 mil (1.8 mm) | 400 mil (10.16 mm) | 32 mil (0.8 mm) | body ~6.3 x 2.3 mm on tPlace |
| Resistor axial 0204 | Ø 63 mil (1.6 mm) | 300 mil (7.62 mm) | 32 mil (0.8 mm) | smaller body |
| Header 1x02 2.54 | Ø 63 mil (1.6 mm) | 100 mil (2.54 mm) | 40 mil (1.0 mm) | square pad on pin 1 |
| Header 1x03+ | Ø 63 mil (1.6 mm) | 100 mil (2.54 mm) | 40 mil (1.0 mm) | n pads in a row |
| Electrolytic cap 5 mm | Ø 70 mil (1.8 mm) | 79 mil (2.0 mm) | 40 mil (1.0 mm) | mark the + side |

`fusion_create_footprint` pad syntax:
`des,x_mil,y_mil,w_mil,h_mil,rect|round,hole_mil` joined by `;`, with `hole_mil` = 0 for SMD.

Example — LED 5 mm, anode at 1:

```
fusion_create_footprint {
  libraryPath: "<new or existing .lbr>",
  name: "LED-5MM-THT",
  description: "5mm through-hole LED",
  pads: "1,-50,0,70,70,round,40;2,50,0,70,70,round,40",
  footprints: [...]
}
```

Example — axial resistor 0207:

```
fusion_create_footprint {
  libraryPath: "<.lbr>",
  name: "R-AXIAL-0207",
  description: "Axial resistor, 10.16mm pitch",
  pads: "1,-200,0,70,70,round,32;2,200,0,70,70,round,32"
}
```

### The symbol, so the part is placeable on a schematic

`fusion_create_symbol` pins are `des,name,left|right|top|bottom,offset_mil[,part[,hiddenNet]]`:

```
fusion_create_symbol {
  libraryPath: "<the same .lbr>",
  name: "LED-5MM",
  designator: "D",
  pins: "1,A,right,100;2,K,left,100",
  bodyWidthMil: 300, bodyHeightMil: 300,
  description: "5mm LED"
}
```

Pin numbers MUST match the footprint's pad numbers, or the part will place but never route.

### Verify before using it

```
fusion_lint_library {libraryPath: "<the .lbr>", names: ["LED-5MM-THT"]}
```

Lint checks pads (none, zero-size, unnumbered, duplicate numbers), courtyard (tKeepout), silkscreen,
3D package and description. A footprint with no courtyard lints as a warning — fix it before shipping
the library to anybody else.

## Step 5 - 3D (needs Fusion, optional)

- parametric, IPC-7351, ~50 families, laser-etches a marking:
  `fusion_generate_package {type: "header_straight"|"chip"|"melf"|..., params: {...}}`
- from your own STEP: `fusion_make_3d_package` / `fusion_bind_library_3d`
- all-in-one: `fusion_create_library {parts: [{package, lbrPath, modelPath}]}`

Skipping 3D is fine for a 2D board and for gerbers. It only matters for the 3D PCB view and for a
mechanical enclosure built from the board.

## Never do this

- Never invent a `symbol` name and retry when `place_part` says it is missing — search first.
- Never silently swap a THT part for an SMD one.
- **Never choose the library folder yourself.** Ask, every time, unless the user already named one
  in this conversation. A `.lbr` is a file on their disk.
- Never claim a part exists because a similar name exists (`LED-0603` is not `LED 5mm`).
