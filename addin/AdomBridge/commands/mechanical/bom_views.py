"""The BOM's views, from the walk's plain rows: the indented structure with item numbers, the flat
purchasing list, the cut list, the fastener rollup, totals and data warnings. Pure functions, so
every rule here is tested offline; bill_of_materials.generate feeds them."""
import re


def words(pattern):
    """A case-blind pattern bounded by anything but a letter, so BHCS_M5x8 and L_Bracket still read."""
    return re.compile(r"(?<![A-Za-z])(?:%s)(?![A-Za-z])" % pattern, re.I)


FASTENER = re.compile(words(r"(screw|bolt|nut|washer|bhcs|shcs|fhcs|socket head|button head|flat head|t-?nut|standoff|rivet|"
                            r"insert|set screw|dowel|pin|fastener)s?").pattern + r"|(?<![A-Za-z])m\d+(?:[._]\d+)?\s*x\s*\d+", re.I)
# a circuit board's own parts, as KiCad and Fusion name footprints and reference designators
FOOTPRINT = re.compile(r"^(R|C|L|D|CP|FB|LED)_\d{4}|_\d{4}_\d{4}metric|^(sot|sod|soic|ssop|tssop|msop|qfn|dfn|qfp|lqfp|tqfp|bga|son)[-_]?\d"
                       r"|^d_(sod|sma|smb|smc|do)|^cp_elec", re.I)
REFDES = re.compile(r"^(R|C|L|D|U|Q|J|P|Y|F|FB|LED|SW|TP|K|T|X)\d{1,4}$")
BOARD = re.compile(r"(?<![A-Za-z])pcba?(?![A-Za-z])|^board$", re.I)
BRACKET = words(r"(bracket|bkt|gusset|brace|corner connector|joining plate)s?")
# a McMaster-Carr catalog number (92095A208, 4259N11, 7950K164); no X, so 2160X1440 stays a size
MCMASTER = re.compile(r"(?<![A-Za-z0-9])(\d{4,5}[A-WYZ]\d{1,3})(?![A-Za-z0-9])")
IMPORTED = re.compile(r"\.(f3d|step|stp|iges|igs|sat|x_t)\s*$|\bstep translator\b", re.I)
# what a name says about where a part comes from: a note like "(vendor STEP)" or "(modelled)", a maker, or made here
VENDOR_NOTE = re.compile(r"\([^()]*\b(vendor|oem|cots|as installed|as purchased|genuine|step|stl|modell?ed|no cad)\b[^()]*\)?", re.I)
# geometry borrowed to stand in for part of something bought elsewhere ("(stand-in from 71535K205)"): never ordered by itself
BORROWED = re.compile(r"(?<![A-Za-z])(stand-?in|placeholder)(?![A-Za-z])|(?<!cut )(?<![A-Za-z])from\s+\d{4,5}[A-WYZ]\d{1,3}(?![A-Za-z0-9])", re.I)
# comes in the box with something else that is bought ("power cord, supplied with the machine"); matched even
# when a long name was cut short mid-word ("(supplie")
SUPPLIED = re.compile(r"(?<![A-Za-z])(supplied|included|comes) with(?![A-Za-z])|\(suppli", re.I)
# reasons for a line nobody orders or makes by itself
NOT_ORDERED = ("stand-in", "supplied")
MAKER = re.compile(words(r"abb|adafruit|allen[- ]bradley|amphenol|automationdirect|banner|bosch rexroth|bussmann|eaton|festo|"
                         r"hammond|hiwin|idec|igus|keyence|littelfuse|mean ?well|misumi|molex|omron|orange pi|panduit|phidgets|"
                         r"phoenix contact|raspberry pi|schneider|shelly|siemens|smc|sparkfun|stride|te connectivity|thk|wago|"
                         r"waveshare|weidm[uü]ller").pattern + r"|\b80/20\b", re.I)
ACCESSORY = words(r"(mount|bracket|adapter|plate|holder|clip|cover|shelf|spacer|stand|tray|guard|cradle|shim)s?")
FABRICATED = words(r"fabricated|machined|3dp|3d[-_ ]?printed|printed|laser[-_ ]?cut|water[-_ ]?jet|cut from|weldment")
# a hidden-away palette of parts to copy from ("Vendor library"), not part of the build
LIBRARY = re.compile(r"\b(library|libraries)\b", re.I)
GAUGE = re.compile(r"(?<![0-9])(\d{1,2}(?:-\d{1,2})?)\s*AWG(?![A-Za-z])", re.I)
WIRING_HARDWARE = words(r"(ferrule|terminal|lug|crimp|connector|grommet|clamp|tie|splice|heat ?shrink|sleeve|gland|plug)s?")
COLOR = words(r"green[-/]yellow|black|white|red|green|blue|yellow|orange|brown|gr[ae]y|violet|purple|pink|clear")
WIRE_ID = re.compile(r"^(W\d+)(?![0-9A-Za-z])")
SHORTEST_CUT_MM = 25.0
MANY_BODIES = 20
EXTRUSION = re.compile(r"(?<![0-9A-Za-z])(1010|1515|2020|2040|2060|2080|3030|3060|4040|4080|8020)(?![0-9A-Za-z])|extrusion|\bprofile\b", re.I)
PROFILE = re.compile(r"(?<![0-9A-Za-z])(1010|1515|2020|2040|2060|2080|3030|3060|4040|4080)(?![0-9A-Za-z])")
# other stock that is bought by the stick and cut to length, so it goes on the cut list too
DIN_RAIL = words(r"din[- ]?rails?")
DUCT = words(r"ducts?")
SECTION = re.compile(r"(\d+(?:\.\d+)?)\s*x\s*(\d+(?:\.\d+)?)", re.I)
LINEAR = ("extrusion", "din rail", "duct")
# bought cable assemblies (a cord, a patch cable); a wire cut from a spool names its gauge and is "wire"
CABLE = words(r"(wire|cable|harness|ferrule|cord)s?")
DEFAULT_NAME = re.compile(r"^(component|body|extruded|extrude|untitled)[_ ]?\d*$", re.I)
# Fusion fills a new component's part number with its creation time (2026-10-28-13-58-44-431)
AUTO_PART_NUMBER = re.compile(r"^\d{4}-\d{2}-\d{2}-\d{2}-\d{2}-\d{2}-\d+$")
# reasons that come from someone's metadata rather than from reading the name
STATED = ("metadata", "marked purchased", "supplier set")


def part_number(meta):
    """The part number a buyer can use: empty when Fusion's automatic timestamp or the name stands in."""
    pn = (meta.get("partNumber") or "").strip()
    return "" if not pn or AUTO_PART_NUMBER.match(pn) or pn == (meta.get("name") or "") else pn


def category(meta):
    """The metadata category, else one inferred from the name. A cut wire names its gauge; hardware wins
    over a profile size in the name (a "T Nut Drop In 2020 M5" is a fastener, a "2020 ... Bkt" hardware)."""
    if meta.get("category"):
        return meta["category"]
    name = meta.get("name") or ""
    if GAUGE.search(name) and (WIRE_ID.match(name) or not WIRING_HARDWARE.search(name)):
        return "wire"
    if FASTENER.search(name):
        return "fastener"
    if FOOTPRINT.search(name) or BOARD.search(name):
        return "electronics"
    if BRACKET.search(name):
        return "hardware"
    if DIN_RAIL.search(name):
        return "din rail"
    if DUCT.search(name):
        return "duct"
    if EXTRUSION.search(name):
        return "extrusion"
    if CABLE.search(name):
        return "cable"
    return None


def inferred_supplier(meta):
    """(supplier, supplier part number): the metadata's, else a McMaster-Carr number in the name or part number."""
    if meta.get("supplierPartNumber"):
        return meta.get("supplier"), meta["supplierPartNumber"]
    supplier = meta.get("supplier")
    if supplier and "mcmaster" not in supplier.lower():
        return supplier, None
    match = MCMASTER.search(meta.get("name") or "") or MCMASTER.search(part_number(meta))
    return ("McMaster-Carr", match.group(1)) if match else (supplier, None)


def vendor_signal(name):
    """Why a name reads as a bought product, or None: a McMaster number, a vendor note, a maker's name
    (not on an accessory such as "Orange Pi Mount"). A name that says it is made here never reads as bought."""
    name = name or ""
    if FABRICATED.search(name):
        return None
    if BORROWED.search(name):
        return "stand-in geometry"
    if SUPPLIED.search(name):
        return "supplied with another item"
    match = MCMASTER.search(name)
    if match:
        return "McMaster-Carr " + match.group(1) + " in the name"
    note = VENDOR_NOTE.search(name)
    if note:
        return "vendor note " + note.group(0)
    maker = MAKER.search(name)
    if maker and not ACCESSORY.search(name):
        return "maker named: " + maker.group(0)
    return None


def inferred_kit(name, child_names):
    """Why an assembly should count as one bought line though nobody marked it purchased: an imported
    model (a name ending .stp or Open CASCADE's "STEP translator"), a circuit board (children named like
    footprints and reference designators), or a name that reads as a bought product. None otherwise."""
    name = name or ""
    if FABRICATED.search(name):
        return None
    if IMPORTED.search(name):
        return "imported model"
    board = sum(1 for child in child_names if FOOTPRINT.search(child or "") or REFDES.match(child or "") or BOARD.search(child or ""))
    if board >= 5 or (board >= 3 and any(BOARD.search(child or "") for child in child_names)):
        return "circuit board"
    return vendor_signal(name)


def is_library(name, body_count):
    """A body-less group named as a library: a palette of parts to copy from, not part of the build."""
    return not body_count and bool(LIBRARY.search(name or ""))


def make_buy_reason(meta, has_bodies):
    """(make, buy or None; why). Metadata wins, then a name that says it is made here, then cut stock
    (wire, extrusion), then what the name says about a supplier or maker, then modeled bodies."""
    if meta.get("makeBuy") in ("make", "buy"):
        return meta["makeBuy"], "metadata"
    if meta.get("asPurchased"):
        return "buy", "marked purchased"
    if meta.get("kitReason"):
        return (None if meta["kitReason"].startswith(NOT_ORDERED) else "buy"), meta["kitReason"]
    if meta.get("supplierPartNumber") or meta.get("supplier"):
        return "buy", "supplier set"
    name = meta.get("name") or ""
    if FABRICATED.search(name):
        return "make", "named as made here"
    if BORROWED.search(name):
        return None, "stand-in geometry"
    if SUPPLIED.search(name):
        return None, "supplied with another item"
    kind = category(meta)
    if kind == "wire" or kind in LINEAR:
        return "make", "cut from %s stock" % kind
    if inferred_supplier(meta)[1]:
        return "buy", "McMaster-Carr number in the name"
    if kind in ("fastener", "electronics", "hardware", "cable"):
        return "buy", kind
    signal = vendor_signal(name)
    if signal:
        return "buy", signal
    return ("make", "modeled bodies") if has_bodies else (None, None)


def make_buy(meta, has_bodies):
    return make_buy_reason(meta, has_bodies)[0]


def profile(meta):
    """The stock profile of a linear part: metadata stock, else read from the name (2020, 4040, DIN rail
    35x7.5, duct 25x40 cover)."""
    if meta.get("stock"):
        return meta["stock"]
    name = meta.get("name") or ""
    kind = category(meta)
    if kind in ("din rail", "duct"):
        section = SECTION.search(name)
        label = kind if kind == "duct" else "DIN rail"
        if section:
            label += " %sx%s" % (section.group(1), section.group(2))
        return label + (" cover" if kind == "duct" and ACCESSORY.search(name) else "")
    match = PROFILE.search(name)
    return match.group(1) if match else None


def wire_spec(name):
    """(wire id, gauge, color) read from a wire's name ("W001 1L1 12AWG black XD1:L-Q1:1")."""
    name = name or ""
    gauge = GAUGE.search(name)
    color = COLOR.search(name, gauge.end()) if gauge else None
    wire_id = WIRE_ID.match(name)
    return (wire_id.group(1) if wire_id else None, gauge.group(1) if gauge else None,
            color.group(0).lower().replace("grey", "gray") if color else None)


def indented(instances, parts):
    """Tree order, one line per distinct child component under each parent instance, with item
    numbers (1, 1.2, 1.2.1), quantity per parent and the total rolled up through every level.
    Lines repeat per parent instance only when the parents differ; identical siblings merge."""
    children = {}
    for row in instances:
        children.setdefault(row["parentPath"], []).append(row)
    lines = []

    def emit(parent_path, prefix, level, multiplier):
        groups, order = {}, []
        for row in children.get(parent_path, []):
            key = row["componentId"]
            if key not in groups:
                groups[key] = []
                order.append(key)
            groups[key].append(row)
        for index, key in enumerate(order, 1):
            rows = groups[key]
            first, number = rows[0], (prefix + "." if prefix else "") + str(index)
            meta = first["metadata"]
            has_bodies = first["bodyCount"] > 0
            # a body-less grouping is a phantom (counted through, never stocked) unless someone gave it
            # a real part number, a make/buy or the assembly category
            kind = ("kit" if first["asPurchased"] else "part" if has_bodies and not first["childCount"]
                    else "assembly" if has_bodies or part_number(meta) or meta.get("makeBuy") or meta.get("category") == "assembly"
                    else "phantom")
            per_parent = len(rows)
            decision, reason = make_buy_reason(meta, has_bodies)
            lines.append({"itemNumber": number, "level": level, "componentId": key, "name": meta.get("name"),
                          "partNumber": part_number(meta), "description": meta.get("description") or "",
                          "kind": kind, "makeBuy": decision, "makeBuyReason": reason, "category": category(meta),
                          "qtyPerParent": per_parent, "totalQty": per_parent * multiplier,
                          "paths": [r["path"] for r in rows][:10], "pathCount": len(rows)})
            if not first["asPurchased"]:
                # every instance of this child has the same subtree; recurse through the first, scaled
                emit(first["path"], number, level + 1, per_parent * multiplier)

    emit("<root>", "", 1, 1)
    return lines


def flat(parts, lengths=None, currency="USD"):
    """The purchasing list: one line per component definition with total quantity, make/buy,
    category, cost and mass (extended by quantity)."""
    lengths = lengths or {}
    lines = []
    for part in parts:
        qty = part["quantity"]
        cost = part.get("unitCost")
        decision, reason = make_buy_reason(part, part.get("scope") == "direct_component_bodies")
        supplier, supplier_pn = inferred_supplier(part)
        stock = profile(part)
        if decision is None and not part.get("supplierPartNumber"):
            supplier, supplier_pn = None, None
        if decision == "make" and supplier_pn and not part.get("supplierPartNumber"):
            # "Bench top HDPE (cut from 8619K87)": the catalog number is the stock it is made from
            stock, supplier, supplier_pn = stock or supplier_pn, None, None
        lines.append({"componentId": part["componentId"], "name": part.get("name"), "partNumber": part_number(part),
                      "description": part.get("description") or "", "quantity": qty,
                      "makeBuy": decision, "makeBuyReason": reason,
                      "category": category(part), "process": part.get("process"), "finish": part.get("finish"),
                      "material": (part.get("material") or {}).get("name"), "stock": stock,
                      "lengthMm": lengths.get(part["componentId"]), "bodyCount": part.get("bodyCount"),
                      "supplier": supplier, "supplierPartNumber": supplier_pn,
                      "supplierInferred": bool(supplier_pn) and not part.get("supplierPartNumber"),
                      "kitReason": part.get("kitReason"),
                      "sourceUrl": part.get("sourceUrl"), "leadTimeDays": part.get("leadTimeDays"),
                      "unitCost": cost, "currency": part.get("currency") or (currency if cost is not None else None),
                      "extendedCost": round(cost * qty, 2) if isinstance(cost, (int, float)) else None,
                      "unitMassKg": part.get("unitMass"), "totalMassKg": (part.get("unitMass") or 0) * qty,
                      "asPurchased": bool(part.get("asPurchased") or (part.get("kitReason") and decision == "buy")),
                      "notes": part.get("notes")})
    return lines


def cut_list(lines, round_to=0.5):
    """Extrusions grouped by profile and cut length (mm, rounded to round_to), most first."""
    groups = {}
    for line in lines:
        if line["category"] not in LINEAR or not line.get("lengthMm") or line["lengthMm"] < SHORTEST_CUT_MM:
            continue
        length = round(line["lengthMm"] / round_to) * round_to
        key = (line.get("stock") or "unknown profile", length)
        entry = groups.setdefault(key, {"profile": key[0], "category": line["category"], "lengthMm": length, "quantity": 0, "components": []})
        entry["quantity"] += line["quantity"]
        entry["components"].append(line["name"])
    return sorted(groups.values(), key=lambda g: (g["profile"], -g["lengthMm"]))


def stock_plan(cuts, stock_length, kerf=3.0):
    """Bars of stock_length per extrusion profile for the cut list, first-fit decreasing with a kerf per
    cut. A cut longer than the stock is reported, never silently dropped. DIN rail and duct come in
    other stick lengths, so they stay on the cut list without a bar plan."""
    plans = {}
    for profile_name in sorted({c["profile"] for c in cuts if c.get("category", "extrusion") == "extrusion"}):
        pieces = sorted((c["lengthMm"] for c in cuts if c["profile"] == profile_name for _ in range(c["quantity"])), reverse=True)
        bars, too_long = [], [p for p in pieces if p > stock_length]
        for piece in (p for p in pieces if p <= stock_length):
            for bar in bars:
                if bar["used"] + piece + (kerf if bar["cuts"] else 0) <= stock_length:
                    bar["used"] += piece + (kerf if bar["cuts"] else 0)
                    bar["cuts"].append(piece)
                    break
            else:
                bars.append({"used": piece, "cuts": [piece]})
        plans[profile_name] = {"stockLengthMm": stock_length, "kerfMm": kerf, "bars": len(bars),
                               "offcutsMm": [round(stock_length - b["used"], 1) for b in bars],
                               "cutsPerBar": [b["cuts"] for b in bars], "tooLong": too_long}
    return plans


def wire_list(lines):
    """Cut wires grouped by gauge and color, with each wire's length and the total to buy, thickest first."""
    groups = {}
    for line in lines:
        if line["category"] != "wire":
            continue
        wire_id, gauge, color = wire_spec(line["name"])
        entry = groups.setdefault((gauge, color), {"gauge": gauge, "color": color, "count": 0, "totalLengthMm": 0.0, "wires": []})
        entry["count"] += line["quantity"]
        if line.get("lengthMm"):
            entry["totalLengthMm"] = round(entry["totalLengthMm"] + line["lengthMm"] * line["quantity"], 1)
        entry["wires"].append({"wireId": wire_id, "name": line["name"], "lengthMm": line.get("lengthMm"), "quantity": line["quantity"]})
    def thickness(entry):
        try:
            return int((entry["gauge"] or "99").split("-")[0])
        except ValueError:
            return 99
    return sorted(groups.values(), key=lambda g: (thickness(g), g["color"] or ""))


def order_list(lines):
    """What to order: bought lines merged by supplier part number (else part number, else name), so 17
    cable ties modeled as 17 components become one line of 17."""
    groups = {}
    for line in lines:
        if line["makeBuy"] != "buy":
            continue
        if line["supplierPartNumber"]:
            key = (line["supplier"] or "", line["supplierPartNumber"])
        else:
            key = ("", line["partNumber"] or line["name"] or line["componentId"])
        entry = groups.setdefault(key, {"supplier": key[0] or None, "orderNumber": key[1], "quantity": 0, "names": [],
                                        "unitCost": line["unitCost"], "currency": line["currency"], "extendedCost": 0.0,
                                        "inferred": False, "lines": 0})
        entry["quantity"] += line["quantity"]
        entry["lines"] += 1
        entry["inferred"] = entry["inferred"] or line["supplierInferred"] or line["makeBuyReason"] not in STATED
        if line["name"] not in entry["names"] and len(entry["names"]) < 20:
            entry["names"].append(line["name"])
        if entry["extendedCost"] is not None:
            entry["extendedCost"] = round(entry["extendedCost"] + line["extendedCost"], 2) if line["extendedCost"] is not None else None
        if line["unitCost"] != entry["unitCost"]:
            entry["unitCost"] = None
    return sorted(groups.values(), key=lambda g: (g["supplier"] or "~", g["orderNumber"]))


def fastener_rollup(lines):
    """Fasteners grouped by part number (else name), with total quantity."""
    groups = {}
    for line in lines:
        if line["category"] != "fastener":
            continue
        key = line["partNumber"] or line["supplierPartNumber"] or line["name"]
        entry = groups.setdefault(key, {"spec": key, "quantity": 0, "names": [], "supplierPartNumber": line["supplierPartNumber"]})
        entry["quantity"] += line["quantity"]
        if line["name"] not in entry["names"]:
            entry["names"].append(line["name"])
    return sorted(groups.values(), key=lambda g: -g["quantity"])


def warnings(lines):
    """What would stop someone ordering or building from this BOM, one row per problem."""
    out = []
    for line in lines:
        name = line["name"] or ""
        if DEFAULT_NAME.match(name):
            out.append({"code": "default_name", "component": name, "message": "Rename it: a default name tells a buyer nothing."})
        if line["makeBuy"] == "buy" and not line["partNumber"] and not line["supplierPartNumber"]:
            out.append({"code": "no_part_number", "component": name, "message": "Set a real part number: Fusion's automatic one is a timestamp."})
        if line["makeBuy"] == "buy" and not line["supplierPartNumber"]:
            out.append({"code": "buy_without_supplier_part_number", "component": name, "message": "Bought, but no supplier part number to order by."})
        if line["makeBuy"] == "buy" and line["unitCost"] is None:
            out.append({"code": "buy_without_cost", "component": name, "message": "Bought, but no unit cost: the total cost is incomplete."})
        if line["makeBuy"] == "make" and not line["material"] and line["category"] != "wire":
            out.append({"code": "make_without_material", "component": name, "message": "Made, but no material: the mass and the process are guesses."})
        if line["makeBuy"] == "make" and (line.get("bodyCount") or 0) > MANY_BODIES:
            out.append({"code": "make_with_many_bodies", "component": name, "message": "Made, but %d bodies in one component reads like an imported model: mark it asPurchased or excludeFromBom." % line["bodyCount"]})
        if line["category"] in LINEAR and not line.get("lengthMm"):
            out.append({"code": "cut_without_length", "component": name, "message": "Cut from %s stock, but no measurable length to put on the cut list." % line["category"]})
        elif line["category"] in LINEAR and line["lengthMm"] < SHORTEST_CUT_MM:
            out.append({"code": "short_cut", "component": name, "message": "Named like %s stock but %.1f mm long: kept off the cut list (set its category if it is hardware)." % (line["category"], line["lengthMm"])})
        if line["makeBuy"] is None and (line.get("makeBuyReason") or "").startswith("stand-in"):
            out.append({"code": "stand_in", "component": name, "message": "Borrowed geometry, ordered by nobody: check that what it stands in for is on the BOM."})
    return out


def totals(lines):
    priced = [l for l in lines if l["makeBuy"] == "buy"]
    costs = {}
    for line in lines:
        if line["extendedCost"] is not None:
            costs[line["currency"]] = round(costs.get(line["currency"], 0) + line["extendedCost"], 2)
    return {"lines": len(lines), "buyLines": sum(1 for l in lines if l["makeBuy"] == "buy"),
            "makeLines": sum(1 for l in lines if l["makeBuy"] == "make"),
            # how much of make/buy was read from names rather than stated in metadata: the review list
            "inferredLines": sum(1 for l in lines if l["makeBuy"] and l["makeBuyReason"] not in STATED),
            "kitLines": sum(1 for l in lines if l.get("kitReason")),
            "totalQuantity": sum(l["quantity"] for l in lines),
            "costCoverage": round(sum(1 for l in priced if l["unitCost"] is not None) / len(priced), 3) if priced else None,
            "extendedCost": costs, "totalMassKg": round(sum(l["totalMassKg"] for l in lines), 4)}
