"""Generate EAGLE .lbr (XML) library files for Fusion 360 Electronics.

Creates minimal but valid .lbr files that Fusion 360 can open via
Document.Open. The XML format matches EAGLE 9.7.0 as used by Fusion's
integrated Electronics workspace.

Usage:
    from eagle_lbr import build_lbr_xml, write_lbr

    # Build XML for a simple 2-pin resistor symbol
    xml = build_lbr_xml(
        library_name="MyLib",
        symbols=[{
            "name": "R",
            "description": "Resistor",
            "pins": [
                {"name": "1", "x": -5.08, "y": 0, "direction": "pas"},
                {"name": "2", "x": 5.08, "y": 0, "direction": "pas", "rot": "R180"},
            ],
            "wires": [
                {"x1": -2.54, "y1": -0.889, "x2": 2.54, "y2": -0.889, "width": 0.254, "layer": 94},
                {"x1": 2.54, "y1": 0.889, "x2": -2.54, "y2": 0.889, "width": 0.254, "layer": 94},
                {"x1": 2.54, "y1": -0.889, "x2": 2.54, "y2": 0.889, "width": 0.254, "layer": 94},
                {"x1": -2.54, "y1": -0.889, "x2": -2.54, "y2": 0.889, "width": 0.254, "layer": 94},
            ],
            "texts": [
                {"value": ">NAME", "x": 0, "y": 2.54, "size": 1.778, "layer": 95, "align": "center"},
                {"value": ">VALUE", "x": 0, "y": -2.54, "size": 1.778, "layer": 96, "align": "center"},
            ],
        }],
    )
    write_lbr(xml, "C:/tmp/MyLib.lbr")
"""

import xml.etree.ElementTree as ET
from xml.dom import minidom


# Standard EAGLE layer definitions used by Fusion 360 Electronics.
# These are the layers from a real Fusion-generated library.
STANDARD_LAYERS = [
    (1, "Top"), (2, "Route2"), (3, "Route3"), (4, "Route4"),
    (5, "Route5"), (6, "Route6"), (7, "Route7"), (8, "Route8"),
    (9, "Route9"), (10, "Route10"), (11, "Route11"), (12, "Route12"),
    (13, "Route13"), (14, "Route14"), (15, "Route15"), (16, "Bottom"),
    (17, "Pads"), (18, "Vias"), (19, "Unrouted"), (20, "BoardOutline"),
    (21, "SilkscreenTop"), (22, "SilkscreenBottom"),
    (23, "OriginsTop"), (24, "OriginsBottom"),
    (25, "NamesTop"), (26, "NamesBottom"),
    (27, "ValuesTop"), (28, "ValuesBottom"),
    (29, "SolderMaskTop"), (30, "SolderMaskBottom"),
    (31, "StencilTop"), (32, "StencilBottom"),
    (33, "FinishTop"), (34, "FinishBottom"),
    (35, "GlueTop"), (36, "GlueBottom"),
    (37, "TestTop"), (38, "TestBottom"),
    (39, "ComponentExcludeTop"), (40, "ComponentExcludeBottom"),
    (41, "RestrictTop"), (42, "RestrictBottom"), (43, "RestrictVias"),
    (44, "Drills"), (45, "Holes"), (46, "Milling"),
    (47, "Measures"), (48, "Document"), (49, "Reference"),
    (51, "DocumentTop"), (52, "DocumentBottom"),
    (88, "SimResults"), (89, "SimProbes"),
    (90, "Modules"), (91, "Nets"), (92, "Busses"), (93, "Pins"),
    (94, "Symbols"), (95, "Names"), (96, "Values"),
    (97, "Info"), (98, "Guide"),
]


def _add_layers(parent: ET.Element):
    """Add standard EAGLE layer definitions."""
    layers_el = ET.SubElement(parent, "layers")
    for num, name in STANDARD_LAYERS:
        attrs = {
            "number": str(num),
            "name": name,
            "blackcolor": "0xccc8c8c8",
            "whitecolor": "0xbb808080",
            "coloredcolor": "0xbb808080",
            "blackhighlight": "0xccc8c8c8",
            "whitehighlight": "0xbb808080",
            "coloredhighlight": "0xbb808080",
            "fill": "1",
            "visible": "yes",
            "active": "yes",
        }
        # Use Fusion's actual colors for key schematic layers
        if num == 94:  # Symbols
            attrs.update({
                "blackcolor": "0xccc90d0c", "whitecolor": "0xccc90d0c",
                "coloredcolor": "0xccc90d0c",
                "blackhighlight": "0xccc90000", "whitehighlight": "0xccc90000",
                "coloredhighlight": "0xccc90000",
            })
        elif num == 93:  # Pins
            attrs.update({
                "blackcolor": "0xcc3ddc97", "whitecolor": "0xcc0dab76",
                "coloredcolor": "0xcc0dab76",
                "blackhighlight": "0xcc00dc79", "whitehighlight": "0xcc00ab6f",
                "coloredhighlight": "0xcc00ab6f",
            })
        ET.SubElement(layers_el, "layer", **attrs)


def _build_symbol_element(sym: dict) -> ET.Element:
    """Build a <symbol> element from a symbol dict.

    Symbol dict keys:
        name: str - symbol name (required)
        description: str - optional description
        pins: list of pin dicts
        wires: list of wire dicts
        texts: list of text dicts
        circles: list of circle dicts
        rectangles: list of rect dicts
    """
    el = ET.Element("symbol", name=sym["name"])

    if sym.get("description"):
        desc = ET.SubElement(el, "description")
        desc.text = sym["description"]

    for wire in sym.get("wires", []):
        attrs = {
            "x1": str(wire["x1"]), "y1": str(wire["y1"]),
            "x2": str(wire["x2"]), "y2": str(wire["y2"]),
            "width": str(wire.get("width", 0.254)),
            "layer": str(wire.get("layer", 94)),
        }
        ET.SubElement(el, "wire", **attrs)

    for circle in sym.get("circles", []):
        attrs = {
            "x": str(circle["x"]), "y": str(circle["y"]),
            "radius": str(circle["radius"]),
            "width": str(circle.get("width", 0.254)),
            "layer": str(circle.get("layer", 94)),
        }
        ET.SubElement(el, "circle", **attrs)

    for rect in sym.get("rectangles", []):
        attrs = {
            "x1": str(rect["x1"]), "y1": str(rect["y1"]),
            "x2": str(rect["x2"]), "y2": str(rect["y2"]),
            "layer": str(rect.get("layer", 94)),
        }
        ET.SubElement(el, "rectangle", **attrs)

    for pin in sym.get("pins", []):
        attrs = {"name": pin["name"]}
        attrs["x"] = str(pin.get("x", 0))
        attrs["y"] = str(pin.get("y", 0))
        if pin.get("visible"):
            attrs["visible"] = pin["visible"]
        else:
            attrs["visible"] = "off"
        attrs["length"] = pin.get("length", "short")
        if pin.get("direction"):
            attrs["direction"] = pin["direction"]
        if pin.get("swaplevel"):
            attrs["swaplevel"] = str(pin["swaplevel"])
        if pin.get("rot"):
            attrs["rot"] = pin["rot"]
        ET.SubElement(el, "pin", **attrs)

    for text in sym.get("texts", []):
        attrs = {
            "x": str(text.get("x", 0)),
            "y": str(text.get("y", 0)),
            "size": str(text.get("size", 1.778)),
            "layer": str(text.get("layer", 95)),
        }
        if text.get("align"):
            attrs["align"] = text["align"]
        t = ET.SubElement(el, "text", **attrs)
        t.text = text.get("value", "")

    return el


def build_lbr_xml(
    library_name: str = "AdomLib",
    description: str = "",
    symbols: list[dict] | None = None,
) -> str:
    """Build a complete EAGLE .lbr XML string.

    Args:
        library_name: Library name (appears in EAGLE's library manager).
        description: Optional library description.
        symbols: List of symbol dicts. Each symbol dict has:
            - name: str (required)
            - description: str
            - pins: list of {name, x, y, direction?, rot?, visible?, length?}
            - wires: list of {x1, y1, x2, y2, width?, layer?}
            - texts: list of {value, x, y, size?, layer?, align?}
            - circles: list of {x, y, radius, width?, layer?}
            - rectangles: list of {x1, y1, x2, y2, layer?}

    Returns:
        Complete XML string ready to write to a .lbr file.
    """
    symbols = symbols or []

    root = ET.Element("eagle", version="9.7.0")
    drawing = ET.SubElement(root, "drawing")

    # Settings
    settings = ET.SubElement(drawing, "settings")
    ET.SubElement(settings, "setting", alwaysvectorfont="no")
    ET.SubElement(settings, "setting", verticaltext="up")

    # Grid
    ET.SubElement(drawing, "grid",
                  distance="0.1", unitdist="inch", unit="inch",
                  style="lines", multiple="1", display="no",
                  altdistance="0.01", altunitdist="inch", altunit="inch")

    # Layers
    _add_layers(drawing)

    # Library
    library = ET.SubElement(drawing, "library")

    if description:
        desc_el = ET.SubElement(library, "description")
        desc_el.text = description

    # Packages (empty — symbols only for now)
    ET.SubElement(library, "packages")

    # Symbols
    symbols_el = ET.SubElement(library, "symbols")
    for sym in symbols:
        symbols_el.append(_build_symbol_element(sym))

    # Devicesets (one per symbol, minimal — just gates mapping)
    devicesets_el = ET.SubElement(library, "devicesets")
    for sym in symbols:
        ds = ET.SubElement(devicesets_el, "deviceset",
                           name=sym["name"], prefix=sym.get("prefix", "U"))
        if sym.get("description"):
            ds_desc = ET.SubElement(ds, "description")
            ds_desc.text = sym["description"]
        gates = ET.SubElement(ds, "gates")
        ET.SubElement(gates, "gate", name="G$1", symbol=sym["name"],
                      x="0", y="0")
        ET.SubElement(ds, "devices")

    # Compatibility notes
    compat = ET.SubElement(root, "compatibility")
    note = ET.SubElement(compat, "note", version="8.3", severity="warning")
    note.text = ("Since Version 8.3, EAGLE supports URNs for individual library "
                 "assets (packages, symbols, and devices). The URNs of those assets "
                 "will not be understood (or retained) with this version.")

    # Pretty-print
    xml_str = ET.tostring(root, encoding="unicode", xml_declaration=False)
    dom = minidom.parseString(xml_str)
    pretty = dom.toprettyxml(indent="  ", encoding=None)
    # Remove the extra xml declaration minidom adds
    lines = pretty.split("\n")
    if lines[0].startswith("<?xml"):
        lines[0] = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    return "\n".join(lines)


def write_lbr(xml: str, path: str) -> str:
    """Write an LBR XML string to a file. Returns the path."""
    with open(path, "w", encoding="utf-8") as f:
        f.write(xml)
    return path
