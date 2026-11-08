"""KiCad -> EAGLE .lbr conversion (page issue #43).

At Adom, KiCad format is the FUNDAMENTAL representation for symbols and footprints - every
part page carries .kicad_sym / .kicad_mod as first-class artifacts, and the .lbr is DERIVED.
This bridge already ingests one foreign format (STEP -> f3d via bind_library_3d), so refusing
the ecosystem's native symbol/footprint format was an arbitrary line. ab argued it, John
confirmed it, and the concrete blocker was real: adom/cr0603-fx-5601elf shipped a .lbr with an
EMPTY <packages> element while a perfectly good .kicad_mod sat beside it.

Two entry points:

    convert(kicad_sym_path, kicad_mod_path) -> {"symbol": xml, "package": xml, ...}
    patch_lbr(lbr_path, kicad_mod_path=..., kicad_sym_path=...) -> patched .lbr text

`patch_lbr` is the important one: given a half-generated library (empty <packages>, empty
<connects/>), it fills in the missing pieces from the KiCad sources instead of refusing.

Geometry notes that matter (learned from the KiCad/EAGLE docs, verified on the CR0603):
  - KiCad footprint Y axis points DOWN; EAGLE Y points UP. Every Y is negated.
  - KiCad (at x y [rot]) rotation is degrees CCW in symbol space; EAGLE spells it R<deg>.
  - Layers: F.Cu->1  B.Cu->16  F.SilkS->21  B.SilkS->22  F.Fab->51  B.Fab->52
            F.CrtYd->39  B.CrtYd->40  F.Mask->29  B.Mask->30  F.Paste->31  B.Paste->32
            Edge.Cuts->20  (unknown layers land on 51 rather than being dropped)
  - roundrect_rratio 0.25 -> EAGLE smd roundness="25" (percent).
Pure stdlib, no Fusion required: the conversion is text -> text, so it is unit-testable
offline and the browser client can rely on the bridge doing it.
"""

import re

# ── s-expression parser (tiny, sufficient for kicad_sym / kicad_mod) ────────────────────


def _tokenize(text):
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif c in "()":
            yield c
            i += 1
        elif c == '"':
            j = i + 1
            buf = []
            while j < n:
                if text[j] == "\\" and j + 1 < n:
                    buf.append(text[j + 1]); j += 2; continue
                if text[j] == '"':
                    break
                buf.append(text[j]); j += 1
            yield ("STR", "".join(buf))
            i = j + 1
        else:
            j = i
            while j < n and not text[j].isspace() and text[j] not in "()":
                j += 1
            yield ("ATOM", text[i:j])
            i = j


def parse_sexpr(text):
    """Parse into nested lists; atoms are str, quoted strings are str too."""
    stack = [[]]
    for tok in _tokenize(text):
        if tok == "(":
            stack.append([])
        elif tok == ")":
            done = stack.pop()
            stack[-1].append(done)
        else:
            stack[-1].append(tok[1])
    root = stack[0]
    return root[0] if len(root) == 1 else root


def _find_all(node, head):
    """Direct children of `node` whose first element equals `head`."""
    return [c for c in node if isinstance(c, list) and c and c[0] == head]


def _find(node, head):
    hits = _find_all(node, head)
    return hits[0] if hits else None


def _num(v, default=0.0):
    try:
        return float(v)
    except Exception:
        return default


def _fmt(x):
    """EAGLE-friendly number: no trailing zeros, no negative zero."""
    v = round(float(x) + 0.0, 4)
    if v == 0:
        v = 0.0
    s = ("%f" % v).rstrip("0").rstrip(".")
    return s if s else "0"


_LAYER = {
    "F.Cu": 1, "B.Cu": 16, "F.SilkS": 21, "B.SilkS": 22, "F.Fab": 51, "B.Fab": 52,
    "F.CrtYd": 39, "B.CrtYd": 40, "F.Mask": 29, "B.Mask": 30, "F.Paste": 31, "B.Paste": 32,
    "Edge.Cuts": 20, "Dwgs.User": 47, "Cmts.User": 48,
}


def _layer(name):
    return _LAYER.get(str(name), 51)


def _esc(s):
    return (str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
            .replace('"', "&quot;"))


# ── footprint (.kicad_mod) -> <package> ─────────────────────────────────────────────────


def footprint_to_package(text, name=None):
    """Convert a .kicad_mod to an EAGLE <package> XML string + pad name list."""
    root = parse_sexpr(text)
    if not (isinstance(root, list) and root and root[0] == "footprint"):
        raise ValueError("not a kicad footprint (expected (footprint ...))")
    fp_name = name or (root[1] if len(root) > 1 else "FOOTPRINT")
    out = []
    pads = []
    descr = _find(root, "descr")
    if descr and len(descr) > 1:
        out.append("<description>%s</description>" % _esc(descr[1]))

    for el in root:
        if not isinstance(el, list) or not el:
            continue
        head = el[0]
        if head == "fp_line":
            s, e = _find(el, "start"), _find(el, "end")
            st = _find(el, "stroke")
            w = _find(st, "width") if st else _find(el, "width")
            ly = _find(el, "layer")
            if s and e:
                out.append('<wire x1="%s" y1="%s" x2="%s" y2="%s" width="%s" layer="%d"/>' % (
                    _fmt(_num(s[1])), _fmt(-_num(s[2])), _fmt(_num(e[1])), _fmt(-_num(e[2])),
                    _fmt(_num(w[1], 0.1) if w else 0.1), _layer(ly[1] if ly else "F.SilkS")))
        elif head == "fp_rect":
            s, e = _find(el, "start"), _find(el, "end")
            st = _find(el, "stroke")
            w = _find(st, "width") if st else None
            ly = _find(el, "layer")
            if s and e:
                x1, y1 = _num(s[1]), -_num(s[2])
                x2, y2 = _num(e[1]), -_num(e[2])
                wd = _fmt(_num(w[1], 0.1) if w else 0.1)
                l = _layer(ly[1] if ly else "F.SilkS")
                # stroked rectangle -> 4 wires (EAGLE <rectangle> is filled copper)
                for (ax, ay, bx, by) in ((x1, y1, x2, y1), (x2, y1, x2, y2),
                                         (x2, y2, x1, y2), (x1, y2, x1, y1)):
                    out.append('<wire x1="%s" y1="%s" x2="%s" y2="%s" width="%s" layer="%d"/>'
                               % (_fmt(ax), _fmt(ay), _fmt(bx), _fmt(by), wd, l))
        elif head == "fp_circle":
            c, e = _find(el, "center"), _find(el, "end")
            st = _find(el, "stroke")
            w = _find(st, "width") if st else None
            ly = _find(el, "layer")
            if c and e:
                r = ((_num(e[1]) - _num(c[1])) ** 2 + (_num(e[2]) - _num(c[2])) ** 2) ** 0.5
                out.append('<circle x="%s" y="%s" radius="%s" width="%s" layer="%d"/>' % (
                    _fmt(_num(c[1])), _fmt(-_num(c[2])), _fmt(r),
                    _fmt(_num(w[1], 0.1) if w else 0.1), _layer(ly[1] if ly else "F.SilkS")))
        elif head == "fp_text":
            kind = el[1] if len(el) > 1 else ""
            at = _find(el, "at")
            ly = _find(el, "layer")
            x, y = (_num(at[1]), -_num(at[2])) if at else (0, 0)
            if kind == "reference":
                out.append('<text x="%s" y="%s" size="1" layer="25">&gt;NAME</text>' % (_fmt(x), _fmt(y)))
            elif kind == "value":
                out.append('<text x="%s" y="%s" size="1" layer="27">&gt;VALUE</text>' % (_fmt(x), _fmt(y)))
            else:
                txt = el[2] if len(el) > 2 and isinstance(el[2], str) else ""
                if txt:
                    out.append('<text x="%s" y="%s" size="1" layer="%d">%s</text>' % (
                        _fmt(x), _fmt(y), _layer(ly[1] if ly else "F.SilkS"), _esc(txt)))
        elif head == "pad":
            pname = el[1] if len(el) > 1 else ""
            ptype = el[2] if len(el) > 2 else ""
            shape = el[3] if len(el) > 3 else ""
            at = _find(el, "at")
            size = _find(el, "size")
            drill = _find(el, "drill")
            x, y = (_num(at[1]), -_num(at[2])) if at else (0, 0)
            rot = _num(at[3]) if (at and len(at) > 3 and not isinstance(at[3], list)) else 0
            rot_attr = (' rot="R%d"' % int(rot % 360)) if rot else ""
            if ptype == "smd":
                dx = _num(size[1], 1) if size else 1
                dy = _num(size[2], 1) if size else 1
                extra = ""
                rr = _find(el, "roundrect_rratio")
                if shape == "roundrect" and rr:
                    extra = ' roundness="%d"' % int(round(_num(rr[1], 0.25) * 100))
                out.append('<smd name="%s" x="%s" y="%s" dx="%s" dy="%s" layer="1"%s%s/>' % (
                    _esc(pname), _fmt(x), _fmt(y), _fmt(dx), _fmt(dy), extra, rot_attr))
                pads.append(str(pname))
            elif ptype in ("thru_hole", "np_thru_hole"):
                d = _num(drill[1], 0.8) if drill else 0.8
                if ptype == "np_thru_hole":
                    out.append('<hole x="%s" y="%s" drill="%s"/>' % (_fmt(x), _fmt(y), _fmt(d)))
                else:
                    dia = _num(size[1], d * 1.6) if size else d * 1.6
                    sh = {"circle": "round", "rect": "square", "oval": "long",
                          "roundrect": "round"}.get(shape, "round")
                    out.append('<pad name="%s" x="%s" y="%s" drill="%s" diameter="%s" shape="%s"%s/>' % (
                        _esc(pname), _fmt(x), _fmt(y), _fmt(d), _fmt(dia), sh, rot_attr))
                    pads.append(str(pname))
    xml = '<package name="%s">\n%s\n</package>' % (_esc(fp_name), "\n".join(out))
    return xml, pads, fp_name


# ── symbol (.kicad_sym) -> <symbol> + pin list ──────────────────────────────────────────

_PIN_ROT = {0: "R0", 90: "R90", 180: "R180", 270: "R270"}


def symbol_to_eagle(text, name=None):
    """Convert the FIRST symbol in a .kicad_sym to an EAGLE <symbol> XML + pin list.

    KiCad nests units as (symbol "NAME_0_1" ...) inside (symbol "NAME" ...); geometry and
    pins live in the units, so all units are walked.
    """
    root = parse_sexpr(text)
    lib = root if (isinstance(root, list) and root and root[0] == "kicad_symbol_lib") else None
    syms = _find_all(lib, "symbol") if lib else ([root] if root and root[0] == "symbol" else [])
    if not syms:
        raise ValueError("no (symbol ...) found in kicad_sym")
    top = syms[0]
    sym_name = name or top[1]
    units = [top] + _find_all(top, "symbol")
    out, pins = [], []
    for u in units:
        for el in u:
            if not isinstance(el, list) or not el:
                continue
            head = el[0]
            if head == "rectangle":
                s, e = _find(el, "start"), _find(el, "end")
                if s and e:
                    x1, y1, x2, y2 = _num(s[1]), _num(s[2]), _num(e[1]), _num(e[2])
                    for (ax, ay, bx, by) in ((x1, y1, x2, y1), (x2, y1, x2, y2),
                                             (x2, y2, x1, y2), (x1, y2, x1, y1)):
                        out.append('<wire x1="%s" y1="%s" x2="%s" y2="%s" width="0.254" layer="94"/>'
                                   % (_fmt(ax), _fmt(ay), _fmt(bx), _fmt(by)))
            elif head == "polyline":
                pts = _find(el, "pts")
                xy = _find_all(pts, "xy") if pts else []
                for a, b in zip(xy, xy[1:]):
                    out.append('<wire x1="%s" y1="%s" x2="%s" y2="%s" width="0.254" layer="94"/>'
                               % (_fmt(_num(a[1])), _fmt(_num(a[2])), _fmt(_num(b[1])), _fmt(_num(b[2]))))
            elif head == "circle":
                c = _find(el, "center")
                r = _find(el, "radius")
                if c and r:
                    out.append('<circle x="%s" y="%s" radius="%s" width="0.254" layer="94"/>'
                               % (_fmt(_num(c[1])), _fmt(_num(c[2])), _fmt(_num(r[1]))))
            elif head == "pin":
                at = _find(el, "at")
                ln = _find(el, "length")
                pn = _find(el, "name")
                pnum = _find(el, "number")
                x, y = (_num(at[1]), _num(at[2])) if at else (0, 0)
                rot = int(_num(at[3])) % 360 if (at and len(at) > 3) else 0
                nm = pn[1] if pn and len(pn) > 1 else (pnum[1] if pnum else "P")
                num = pnum[1] if pnum and len(pnum) > 1 else nm
                length = _num(ln[1], 2.54) if ln else 2.54
                lattr = "short" if length <= 2.6 else "middle"
                # KiCad pins point INTO the body from (at); EAGLE pins point out from their
                # origin - same anchor, same rotation vocabulary.
                out.append('<pin name="%s" x="%s" y="%s" length="%s" rot="%s"/>' % (
                    _esc(nm if nm != "~" else num), _fmt(x), _fmt(y), lattr,
                    _PIN_ROT.get(rot, "R0")))
                pins.append(str(num))
    out.append('<text x="0" y="1" size="1.778" layer="95">&gt;NAME</text>')
    out.append('<text x="0" y="-2" size="1.778" layer="96">&gt;VALUE</text>')
    xml = '<symbol name="%s">\n%s\n</symbol>' % (_esc(sym_name), "\n".join(out))
    return xml, pins, sym_name


# ── the patch path: fill a half-generated .lbr from KiCad sources ───────────────────────


def patch_lbr(lbr_text, kicad_mod_text=None, kicad_sym_text=None, name=None):
    """Fill missing pieces of an .lbr from KiCad sources. Returns (new_text, report).

    THE case from issue #43: <packages></packages> is empty while the .kicad_mod is fine.
    Also generates <connects> (pin->pad by matching names, falling back to order) when a
    deviceset exists with an empty <connects/>, because the ganged Library Manager view -
    John's evidence image #4 - shows pin connections, and an empty connects renders none.
    Existing content is NEVER overwritten: this adds what is absent, only.
    """
    report = {"addedPackage": False, "addedSymbol": False, "addedConnects": False,
              "packageName": None, "pads": [], "pins": []}
    text = lbr_text
    pads, pkg_name = [], None

    if kicad_mod_text and re.search(r"<packages>\s*</packages>", text):
        pkg_xml, pads, pkg_name = footprint_to_package(kicad_mod_text, name)
        text = re.sub(r"<packages>\s*</packages>",
                      "<packages>\n%s\n</packages>" % pkg_xml, text, count=1)
        report.update(addedPackage=True, packageName=pkg_name, pads=pads)

    if kicad_sym_text and re.search(r"<symbols>\s*</symbols>", text):
        sym_xml, pins, _ = symbol_to_eagle(kicad_sym_text, name)
        text = re.sub(r"<symbols>\s*</symbols>",
                      "<symbols>\n%s\n</symbols>" % sym_xml, text, count=1)
        report.update(addedSymbol=True, pins=pins)

    # connects: needs pins (from the lbr symbol or the kicad_sym) and pads (just added)
    if pads and ("<connects/>" in text or re.search(r"<connects>\s*</connects>", text)):
        pins = report["pins"] or re.findall(r'<pin name="([^"]+)"', text)
        gate = (re.findall(r'<gate name="([^"]+)"', text) or ["G$1"])[0]
        by_name = {p.lower(): p for p in pins}
        conns = []
        for i, pad in enumerate(pads):
            pin = by_name.get(str(pad).lower()) or (pins[i] if i < len(pins) else None)
            if pin:
                conns.append('<connect gate="%s" pin="%s" pad="%s"/>' % (
                    _esc(gate), _esc(pin), _esc(pad)))
        if conns:
            joined = "<connects>\n%s\n</connects>" % "\n".join(conns)
            text = text.replace("<connects/>", joined, 1) if "<connects/>" in text else \
                re.sub(r"<connects>\s*</connects>", joined, text, count=1)
            report["addedConnects"] = True
            report["pins"] = pins
    # a deviceset whose <devices> has a device with no package attr: point it at the new pkg
    if pkg_name:
        text = re.sub(r'(<device name="[^"]*")(?![^>]*package=)([^>]*>)',
                      r'\1 package="%s"\2' % pkg_name, text, count=1)
    return text, report


_LBR_SHELL = """<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE eagle SYSTEM "eagle.dtd">
<eagle version="9.6.2">
<drawing>
<settings><setting alwaysvectorfont="no"/><setting verticaltext="up"/></settings>
<grid distance="0.1" unitdist="inch" unit="inch" style="lines" multiple="1" display="no" altdistance="0.01" altunitdist="inch" altunit="inch"/>
<layers>
<layer number="1" name="Top" color="4" fill="1" visible="yes" active="yes"/>
<layer number="16" name="Bottom" color="1" fill="1" visible="yes" active="yes"/>
<layer number="20" name="Dimension" color="15" fill="1" visible="yes" active="yes"/>
<layer number="21" name="tPlace" color="7" fill="1" visible="yes" active="yes"/>
<layer number="22" name="bPlace" color="7" fill="1" visible="yes" active="yes"/>
<layer number="25" name="tNames" color="7" fill="1" visible="yes" active="yes"/>
<layer number="27" name="tValues" color="7" fill="1" visible="yes" active="yes"/>
<layer number="29" name="tStop" color="7" fill="3" visible="no" active="yes"/>
<layer number="30" name="bStop" color="7" fill="6" visible="no" active="yes"/>
<layer number="31" name="tCream" color="7" fill="4" visible="no" active="yes"/>
<layer number="32" name="bCream" color="7" fill="5" visible="no" active="yes"/>
<layer number="39" name="tKeepout" color="4" fill="11" visible="no" active="yes"/>
<layer number="40" name="bKeepout" color="1" fill="11" visible="no" active="yes"/>
<layer number="47" name="Measures" color="7" fill="1" visible="no" active="yes"/>
<layer number="48" name="Document" color="7" fill="1" visible="yes" active="yes"/>
<layer number="51" name="tDocu" color="7" fill="1" visible="yes" active="yes"/>
<layer number="52" name="bDocu" color="7" fill="1" visible="yes" active="yes"/>
<layer number="94" name="Symbols" color="4" fill="1" visible="yes" active="yes"/>
<layer number="95" name="Names" color="7" fill="1" visible="yes" active="yes"/>
<layer number="96" name="Values" color="7" fill="1" visible="yes" active="yes"/>
</layers>
<library>
<packages>
%(package)s
</packages>
<symbols>
%(symbol)s
</symbols>
<devicesets>
<deviceset name="%(name)s" prefix="%(prefix)s">
<description>%(descr)s</description>
<gates>
<gate name="G$1" symbol="%(symname)s" x="0" y="0"/>
</gates>
<devices>
<device name="" package="%(pkgname)s">
%(connects)s
<technologies><technology name=""/></technologies>
</device>
</devices>
</deviceset>
</devicesets>
</library>
</drawing>
</eagle>
"""


def build_lbr(kicad_sym_text=None, kicad_mod_text=None, name=None, prefix="U", descr=""):
    """Create a COMPLETE .lbr from KiCad sources: package + symbol + deviceset + connects.

    The create half of issue #43 (patch_lbr is the patch half). Either source may be absent:
    a missing symbol gets a minimal 2-pin box (so the footprint still has a home), a missing
    footprint yields a symbol-only library the show verbs can already handle.
    """
    pkg_xml, pads, pkg_name = ("", [], "")
    if kicad_mod_text:
        pkg_xml, pads, pkg_name = footprint_to_package(kicad_mod_text, name)
    sym_xml, pins, sym_name = ("", [], "")
    if kicad_sym_text:
        sym_xml, pins, sym_name = symbol_to_eagle(kicad_sym_text, name)
    the_name = name or pkg_name or sym_name or "PART"
    if not sym_xml:
        sym_name = the_name
        pins = [str(p) for p in pads] or ["1", "2"]
        body = ['<wire x1="-5.08" y1="2.54" x2="5.08" y2="2.54" width="0.254" layer="94"/>',
                '<wire x1="5.08" y1="2.54" x2="5.08" y2="-2.54" width="0.254" layer="94"/>',
                '<wire x1="5.08" y1="-2.54" x2="-5.08" y2="-2.54" width="0.254" layer="94"/>',
                '<wire x1="-5.08" y1="-2.54" x2="-5.08" y2="2.54" width="0.254" layer="94"/>']
        for i, p in enumerate(pins[:2]):
            side = -1 if i == 0 else 1
            body.append('<pin name="%s" x="%s" y="0" length="short" rot="%s"/>'
                        % (_esc(p), _fmt(side * 7.62), "R0" if side < 0 else "R180"))
        body.append('<text x="0" y="3.3" size="1.778" layer="95">&gt;NAME</text>')
        body.append('<text x="0" y="-4.3" size="1.778" layer="96">&gt;VALUE</text>')
        sym_xml = '<symbol name="%s">\n%s\n</symbol>' % (_esc(sym_name), "\n".join(body))
    by_name = {p.lower(): p for p in pins}
    conns = []
    for i, pad in enumerate(pads):
        pin = by_name.get(str(pad).lower()) or (pins[i] if i < len(pins) else None)
        if pin:
            conns.append('<connect gate="G$1" pin="%s" pad="%s"/>' % (_esc(pin), _esc(pad)))
    connects = ("<connects>\n%s\n</connects>" % "\n".join(conns)) if conns else "<connects/>"
    text = _LBR_SHELL % {
        "package": pkg_xml, "symbol": sym_xml, "name": _esc(the_name),
        "symname": _esc(sym_name), "pkgname": _esc(pkg_name or the_name),
        "prefix": _esc(prefix), "descr": _esc(descr), "connects": connects,
    }
    return text, {"name": the_name, "pads": pads, "pins": pins,
                  "packageName": pkg_name, "symbolName": sym_name}
