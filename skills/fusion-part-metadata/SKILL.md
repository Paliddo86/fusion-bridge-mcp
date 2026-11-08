---
name: fusion-part-metadata
description: >
  Part numbers and VARIANTS in a Fusion/EAGLE library - the metadata an EE actually orders
  from. MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN, LCSC_PN; how EAGLE expresses variants
  (deviceset + device + technology) so two parts can share one symbol, one footprint and
  one 3D body and still be different products; per-variant descriptions; and the lint that
  catches an unorderable library. Read before building, importing, reviewing or linting any
  .lbr, and before telling anyone a library is complete.
  Trigger words - mpn, manufacturer part number, mouser part number, digikey, lcsc, part
  number, variant, variants, automotive grade, AEC-Q100, high temp, technology, deviceset,
  device, orderable, BOM blank, library metadata, fusion_library_parts.
discovery_triggers:
  - "mpn"
  - "part number"
  - "library variant"
  - "automotive grade part"
  - "fusion_library_parts"
  - "bom is blank"
---

# Part numbers and variants: what makes a library orderable

**The rule:** geometry is not the part. A library that carries a symbol, a footprint and a
3D body but no part numbers *looks* finished and is useless to an EE, because nothing in it
says what to buy.

> "one chip could be the same symbol, footprint, and 3d chip, but there might be an
> automotive high temp version vs a cheaper baseline variant" — John, 2026-08-28

That difference is invisible in the geometry. It lives **entirely** in the metadata. Ship a
library that drops it and you have not shipped an incomplete library, you have shipped a
misleading one.

## How EAGLE expresses a variant (three names, not one)

The orderable identity is **deviceset + device + technology**:

| Element | Carries | Example |
|---|---|---|
| `<deviceset>` | the logical part + its description | `TJA1050` |
| `<device>` | the **package** variant | `T` (SO08) |
| `<technology>` | the **grade / temperature** variant | `-AUTO` |

Attributes hang off `<technology>`, which is exactly *why* two variants can share one
symbol, one footprint and one 3D body and remain different products.

```xml
<deviceset name="TJA1050" prefix="U">
  <description>High-speed CAN transceiver, 1 Mbit/s</description>
  <devices><device name="T" package="SO08">
    <technologies>
      <technology name="">
        <attribute name="MPN"        value="TJA1050T/CM,118"/>
        <attribute name="MANUFACTURER" value="NXP"/>
        <attribute name="DIGIKEY_PN" value="568-4885-1-ND"/>
        <attribute name="DESCRIPTION" value="Baseline, -40 to +125C"/>
      </technology>
      <technology name="-AUTO">
        <attribute name="MPN"        value="TJA1050T/VM,118"/>
        <attribute name="MANUFACTURER" value="NXP"/>
        <attribute name="MOUSER_PN"  value="771-TJA1050TVM118"/>
        <attribute name="DESCRIPTION" value="AEC-Q100 automotive, high temp"/>
      </technology>
    </technologies>
  </device></devices>
</deviceset>
```

Full names: `TJA1050T` and `TJA1050T-AUTO`.

## The attribute names are NOT arbitrary

Use exactly these. They are what this bridge's own BOM exporter reads
(`addin/AdomBridge/commands/manufacturing.py`); any other spelling produces a **blank BOM**:

`MPN` · `MANUFACTURER` · `MOUSER_PN` · `DIGIKEY_PN` · `LCSC_PN`

Add `DESCRIPTION` per technology when a deviceset has more than one variant. The deviceset
description says what the part *is*; the technology description says **which variant this
is**, and it is the only thing distinguishing them in Fusion's Content Manager.

## Show it on screen: `fusion_show_attributes`

`fusion_show_attributes {libraryPath, deviceset}` returns the component's attributes with their
full values (read from the library file) and opens the DEVICE editor for it with a capture:
symbol, the Packages table with the Mapped check, footprint and 3D previews in one frame.
Fusion's own Attribute Sets table sits behind a "..." button in an opaque Qt panel; the reply
carries the positioned-click recipe and the note that a main-window recording does not include
that owned dialog.

## Check before you claim a library is done

```
fusion_library_parts {libraryPath}
```

Offline, no Fusion. Returns every variant with its attributes, plus `metadataOk` and
`warnings`. `fusion_preflight_lbr` reports the same under `metadataOk` /
`metadataWarnings` / `partTotals`.

### The four warnings, and why each one matters

| code | Means | Why it is dangerous |
|---|---|---|
| `variant_missing_part_numbers` | No MPN / MANUFACTURER / distributor PN | The BOM comes out blank for this part |
| `variants_indistinguishable` | Several variants, not all with an MPN | An automotive part and a baseline one look identical |
| `duplicate_variant_mpn` | Two variants share one MPN | They are not different parts, or one MPN is wrong |
| `deviceset_no_description` | No `<description>` | Whoever places it is choosing on a symbol outline alone |

Same-net pads are not shorts: `pad_overlap` skips pads that share a base name (`B4` and `B4@2`,
heatsink vias on the exposed pad's own net) and reports them as an info line, `heatsink_vias`.

## These warnings do NOT block opening, deliberately

`fusion_preflight_lbr`'s `ok` keeps meaning **"this file will open in Fusion"**, because a
geometry-only library opens perfectly well. Whether it is fit to **order from** is a
different question, so it has its own key. Do not conflate them, and do not report a
library as complete on `ok: true` alone — check `metadataOk` too.

## For web control and the wiki

The bridge is the **end** of the chain: it can only carry through what it is handed. If a
wiki component's metadata lacks part numbers, this bridge cannot invent them, and the
right response is to report the gap upstream rather than emit a library that looks whole.
Surface `warnings` to the user with the variant names, so the fix happens where the data
lives.

## Verified behaviour

- Attributes **survive the 3D bind** — `<package3dinstances>` is inserted *before*
  `<technologies>`, so per-variant metadata is preserved through `fusion_build_library_3d`.
- Libraries this bridge *generates* from bare geometry emit `<technology name=""/>` with no
  attributes. That is correct for a geometry-only wrapper and **wrong** to ship as a real
  part: add the attributes before it reaches a user.

## Missing part numbers NEVER block the import — they bubble up

John, 2026-08-28: *"i think its ok to continue to import a library if it doesn't have this
data, but you should report it upstream so it bubbles up all the way to the top."*

So the import succeeds and the gap travels with it. `fusion_open_lbr` returns
`partMetadata`, and `fusion_create_library` puts the same block in its run state, so
`fusion_prove_result` carries it on the poll web control already makes — no second call:

```json
"partMetadata": {
  "ok": false,
  "componentsTotal": 5,
  "summary": "2 of 5 components have no distributor part number at all.
              The library imported fine - this is about ordering, not geometry.",
  "distributorNote": "2 of 5 components have no Mouser part number; ...",
  "distributorCoverage": { "MOUSER_PN": 3, "DIGIKEY_PN": 0, "LCSC_PN": 0 },
  "components": [ { "component": "U1", "missing": ["a distributor part number"],
                    "absentDistributors": ["MOUSER_PN","DIGIKEY_PN","LCSC_PN"] } ],
  "userAction": "Add the missing part numbers where the component metadata lives
                 (upstream of this bridge), then re-export."
}
```

### Two severities, and do not conflate them

| Field | Severity | Meaning |
|---|---|---|
| `missing` / `summary` / `ok` | **Actionable** | No MPN, no manufacturer, or **no distributor at all** — this component cannot be ordered |
| `absentDistributors` / `distributorNote` / `distributorCoverage` | **Informational** | *Which* catalogues lack a number |

A part needs an MPN, a manufacturer, and **at least one** distributor — not all three. A
library with DigiKey on one variant and Mouser on another is perfectly orderable; calling
that a deficiency is noise that trains people to ignore the whole report. (I shipped exactly
that bug for one commit and my own test caught it: a complete library reported `ok: true`
while the summary nagged about LCSC.)

Render `summary` as the headline and `distributorNote` as the detail line. `userAction` is
`null` when there is nothing to fix, so a clean library stays quiet.

### The bridge cannot fix this, only report it

We are the end of the chain. If a component's metadata lacks part numbers, they are missing
upstream where the component is defined — surface the names and let the fix happen where the
data lives. Never invent a part number.
