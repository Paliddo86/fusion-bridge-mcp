"""Schematic verbs of the EDA Bridge Standard (sch_*), run on Fusion's main thread through the
script transport (sch.rs embeds this file, as routing.rs does routing_script.py).

A Fusion schematic is one document with numbered sheets; `sheet` is that number (default 1), not
a file. Reads come from Fusion's own export of the schematic (the EAGLE XML inside the .fsch), or
from a .sch/.fsch on the Fusion machine (filePath). Coordinates are Fusion's: mm, y up, which is
the standard frame. EAGLE items carry no ids, so an item's id is built from its kind and geometry
('wire:x1,y1,x2,y2'); it stays valid until that item moves."""
import hashlib
import math
import re
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path


class SchError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def _tab_number(app, i):
    """The tab of the document at index i in app.documents. A document with no visible tab (opened by a script
    with visible=False) has no place in the tab bar, so every index after it was off by one (#141)."""
    return sum(1 for k in range(min(int(i), app.documents.count)) if app.documents.item(k).isVisible)


def sfail(code, message):
    raise SchError(code, message)


def _sf(node, key, default=0.0):
    try:
        return float(node.get(key, default))
    except (TypeError, ValueError):
        return default


def _r4(v):
    return round(float(v), 4)


def _pt(x, y):
    return [_r4(x), _r4(y)]


def _key(p):
    return '%g,%g' % (round(p[0], 4), round(p[1], 4))


def wire_id(x1, y1, x2, y2, kind='wire'):
    a, b = (_r4(x1), _r4(y1)), (_r4(x2), _r4(y2))
    a, b = min(a, b), max(a, b)
    return '%s:%g,%g,%g,%g' % (kind, a[0], a[1], b[0], b[1])


# ── reading the XML ─────────────────────────────────────────────────────────────────────────────

def sch_root_from_file(path):
    p = Path(path)
    if not p.exists():
        sfail('file_not_found', 'No file at %s on the Fusion machine' % path)
    raw = p.read_bytes()
    if zipfile.is_zipfile(p):
        with zipfile.ZipFile(p) as z:
            names = [n for n in z.namelist() if n.lower().endswith('.sch')]
            if len(names) != 1:
                sfail('invalid_file', 'Expected one schematic inside %s, found %d' % (path, len(names)))
            raw = z.read(names[0])
    try:
        root = ET.fromstring(raw)
    except ET.ParseError as e:
        sfail('invalid_file', 'Not a schematic file: %s' % e)
    if root.find('drawing/schematic') is None:
        sfail('invalid_file', '%s holds no schematic' % path)
    return root


def sch_revision(root):
    s = root.find('drawing/schematic')
    h = hashlib.sha256()
    for tag in ('parts', 'sheets', 'modules'):
        n = s.find(tag)
        if n is not None:
            h.update(ET.tostring(n))
    return h.hexdigest()[:16]


class Lib:
    """Symbols, gates and pin-to-pad maps of the libraries embedded in the schematic."""

    def __init__(self, sch):
        self.symbols, self.devicesets = {}, {}
        for lib in sch.findall('libraries/library'):
            ln = lib.get('name')
            for sym in lib.findall('symbols/symbol'):
                self.symbols[(ln, sym.get('name'))] = sym
            for ds in lib.findall('devicesets/deviceset'):
                self.devicesets[(ln, ds.get('name'))] = ds
        self.parts = {p.get('name'): p for p in sch.findall('parts/part')}

    def gate_symbol(self, part, gate):
        p = self.parts.get(part)
        if p is None:
            return None, None
        ds = self.devicesets.get((p.get('library'), p.get('deviceset')))
        if ds is None:
            return None, None
        g = ds.find("gates/gate[@name='%s']" % gate) if "'" not in gate else None
        if g is None:
            g = next((x for x in ds.findall('gates/gate') if x.get('name') == gate), None)
        if g is None:
            return None, None
        return self.symbols.get((p.get('library'), g.get('symbol'))), ds

    def pads(self, part, gate, pin):
        p = self.parts.get(part)
        ds = self.devicesets.get((p.get('library'), p.get('deviceset'))) if p is not None else None
        if ds is None:
            return []
        dev = next((d for d in ds.findall('devices/device') if d.get('name', '') == p.get('device', '')), None)
        if dev is None:
            return []
        out = []
        for c in dev.findall('connects/connect'):
            if c.get('gate') == gate and c.get('pin') == pin:
                out += (c.get('pad') or '').split()
        return out


def _rot(spec):
    spec = spec or 'R0'
    m = re.search(r'R(-?[\d.]+)', spec)
    return ('M' in spec), (float(m.group(1)) if m else 0.0)


def transform(px, py, ix, iy, spec):
    """A symbol-frame point into the sheet: rotate, then mirror x (EAGLE's order, the one the board
    verbs use and Fusion's ULP agrees with), then translate."""
    mirrored, deg = _rot(spec)
    a = math.radians(deg)
    x, y = px * math.cos(a) - py * math.sin(a), px * math.sin(a) + py * math.cos(a)
    if mirrored:
        x = -x
    return ix + x, iy + y


SIDE_OF = {0: 'left', 90: 'bottom', 180: 'right', 270: 'top'}


def pin_side(pin_rot, inst_spec):
    """The side of the body a pin leaves from. A pin at R0 runs from its connection point toward +x,
    so it leaves the body on the left; the instance's turn and mirror then apply to that direction."""
    _, pdeg = _rot(pin_rot)
    a = math.radians(pdeg + 180)              # outward direction, symbol frame
    dx, dy = math.cos(a), math.sin(a)
    x, y = transform(dx, dy, 0, 0, inst_spec)
    if abs(x) >= abs(y):
        return 'left' if x < 0 else 'right'
    return 'bottom' if y < 0 else 'top'


ELECTRICAL = {'in': 'input', 'out': 'output', 'io': 'io', 'oc': 'open_collector', 'pas': 'passive',
              'hiz': 'hiz', 'pwr': 'power', 'sup': 'supply', 'nc': 'not_connected'}


def sheets_of(root):
    return root.findall('drawing/schematic/sheets/sheet')


def pick_sheet(root, args):
    sheets = sheets_of(root)
    if not sheets:
        sfail('no_sheet', 'The schematic has no sheets')
    v = args.get('sheet', 1)
    if isinstance(v, str) and v.strip().isdigit():
        v = int(v)
    if isinstance(v, int) and not isinstance(v, bool):
        if not 1 <= v <= len(sheets):
            sfail('no_sheet', 'Sheet %d does not exist: the schematic has %d sheet(s)' % (v, len(sheets)))
        return v, sheets[v - 1]
    if isinstance(v, str):
        for i, s in enumerate(sheets):
            if (s.findtext('description') or '').strip().lower() == v.strip().lower():
                return i + 1, s
    sfail('no_sheet', 'sheet is the sheet number (1 to %d) or its description' % len(sheets))


def sheet_pins(lib, sheet, refs=None):
    """Every pin placed on the sheet: {ref, gate, pin (its name), number (pads), at, side, electrical}."""
    out = []
    for inst in sheet.findall('instances/instance'):
        ref, gate = inst.get('part'), inst.get('gate')
        if refs and ref not in refs:
            continue
        sym, _ = lib.gate_symbol(ref, gate)
        if sym is None:
            continue
        ix, iy, spec = _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0')
        for p in sym.findall('pin'):
            x, y = transform(_sf(p, 'x'), _sf(p, 'y'), ix, iy, spec)
            pads = lib.pads(ref, gate, p.get('name'))
            out.append({'ref': ref, 'gate': gate, 'pin': p.get('name'), 'number': ' '.join(pads) or None,
                        'at': _pt(x, y), 'side': pin_side(p.get('rot', 'R0'), spec),
                        'electrical': ELECTRICAL.get(p.get('direction', 'io'), p.get('direction', 'io')),
                        'lengthClass': p.get('length', 'long')})
    return out


def segments(sheet):
    """(net, segment, is_bus) for every net and bus segment on the sheet."""
    for n in sheet.findall('nets/net'):
        for s in n.findall('segment'):
            yield n.get('name'), s, False
    for b in sheet.findall('busses/bus'):
        for s in b.findall('segment'):
            yield b.get('name'), s, True


def is_supply(lib, ref):
    p = lib.parts.get(ref)
    if p is None:
        return False
    ds = lib.devicesets.get((p.get('library'), p.get('deviceset')))
    if ds is None:
        return False
    for g in ds.findall('gates/gate'):
        sym = lib.symbols.get((p.get('library'), g.get('symbol')))
        if sym is not None and sym.findall('pin') and all(x.get('direction') == 'sup' for x in sym.findall('pin')):
            return True
    return False


def items(root, sheet, kinds=None):
    lib = Lib(root.find('drawing/schematic'))
    want = set(kinds or ['wires', 'buses', 'busEntries', 'labels', 'powerPorts', 'junctions', 'noConnects', 'texts', 'sheets'])
    out = {k: [] for k in want}
    for name, seg, bus in segments(sheet):
        for w in seg.findall('wire'):
            x1, y1, x2, y2 = (_sf(w, k) for k in ('x1', 'y1', 'x2', 'y2'))
            if bus and 'buses' in want:
                out['buses'].append({'id': wire_id(x1, y1, x2, y2, 'bus'), 'bus': name, 'from': _pt(x1, y1), 'to': _pt(x2, y2)})
            elif not bus and 'wires' in want:
                out['wires'].append({'id': wire_id(x1, y1, x2, y2), 'net': name, 'from': _pt(x1, y1), 'to': _pt(x2, y2)})
        for lb in seg.findall('label'):
            if 'labels' in want:
                kind = 'global' if lb.get('xref') == 'yes' else 'local'
                out['labels'].append({'id': 'label:%s' % _key((_sf(lb, 'x'), _sf(lb, 'y'))), 'text': name, 'kind': kind,
                                      'at': _pt(_sf(lb, 'x'), _sf(lb, 'y')), 'rotation': _rot(lb.get('rot'))[1], 'onBus': bus})
        for j in seg.findall('junction'):
            if 'junctions' in want:
                out['junctions'].append({'id': 'junction:%s' % _key((_sf(j, 'x'), _sf(j, 'y'))), 'net': name, 'at': _pt(_sf(j, 'x'), _sf(j, 'y'))})
        for pr in seg.findall('portref'):
            if 'labels' in want:
                out['labels'].append({'id': 'port:%s:%s' % (pr.get('moduleinst'), pr.get('port')), 'text': name, 'kind': 'hierarchical',
                                      'moduleinst': pr.get('moduleinst'), 'port': pr.get('port'), 'at': None, 'rotation': None, 'onBus': bus})
    if 'busEntries' in want:
        # EAGLE draws a bus entry as a short diagonal net wire ending on a bus: report each such wire
        bus_pts = [((_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))) for _, s, b in segments(sheet) if b for w in s.findall('wire')]
        for name, seg, bus in segments(sheet):
            if bus:
                continue
            for w in seg.findall('wire'):
                a, b = (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))
                if abs(abs(a[0] - b[0]) - abs(a[1] - b[1])) < 1e-6 and a != b:
                    for end, other in ((a, b), (b, a)):
                        if any(_seg_dist(end, p, q) < 1e-4 for p, q in bus_pts):
                            out['busEntries'].append({'id': wire_id(a[0], a[1], b[0], b[1], 'entry'), 'net': name, 'at': _pt(*end), 'end': _pt(*other)})
                            break
    if 'powerPorts' in want:
        for inst in sheet.findall('instances/instance'):
            ref = inst.get('part')
            if is_supply(lib, ref):
                p = lib.parts[ref]
                pins = sheet_pins(lib, sheet, {ref})
                out['powerPorts'].append({'id': 'power:%s' % ref, 'ref': ref, 'net': p.get('value') or p.get('deviceset'),
                                          'at': pins[0]['at'] if pins else _pt(_sf(inst, 'x'), _sf(inst, 'y'))})
    if 'texts' in want:
        for t in sheet.findall('plain/text'):
            out['texts'].append({'id': 'text:%s' % _key((_sf(t, 'x'), _sf(t, 'y'))), 'text': t.text or '', 'at': _pt(_sf(t, 'x'), _sf(t, 'y')),
                                 'sizeMm': _sf(t, 'size'), 'rotation': _rot(t.get('rot'))[1], 'layer': int(t.get('layer', '97'))})
    if 'sheets' in want:
        for mi in sheet.findall('moduleinsts/moduleinst'):
            mod = root.find("drawing/schematic/modules/module[@name='%s']" % mi.get('module')) if "'" not in (mi.get('module') or '') else None
            dx, dy = (_sf(mod, 'dx'), _sf(mod, 'dy')) if mod is not None else (0, 0)
            out['sheets'].append({'id': 'sheet:%s' % mi.get('name'), 'name': mi.get('name'), 'module': mi.get('module'),
                                  'at': _pt(_sf(mi, 'x'), _sf(mi, 'y')), 'sizeMm': [_r4(dx), _r4(dy)]})
    if 'noConnects' in want:
        out['noConnects'] = []                 # EAGLE has no no-connect marker: an unused pin is direction nc
    return {**{k: out[k] for k in out}, 'counts': {k: len(v) for k, v in out.items()}}


def _seg_dist(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    L2 = dx * dx + dy * dy
    t = 0.0 if not L2 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L2))
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def pins_with_connections(root, sheet, refs=None, connections=True):
    lib = Lib(root.find('drawing/schematic'))
    pins = sheet_pins(lib, sheet, set(refs) if refs else None)
    if not pins:
        return pins
    by = {}
    for p in pins:
        by.setdefault((p['ref'], p['gate'], p['pin']), p)
        p['net'] = None
        if connections:
            p.update(wires=[], labels=[], connectedPins=[])
    for name, seg, bus in segments(sheet):
        if bus:
            continue
        refs_here = [(pr.get('part'), pr.get('gate'), pr.get('pin')) for pr in seg.findall('pinref')]
        for k in refs_here:
            p = by.get(k)
            if p is None:
                continue
            p['net'] = name
            if connections:
                for w in seg.findall('wire'):
                    a, b = (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))
                    if _seg_dist(p['at'], a, b) < 1e-3:
                        p['wires'].append(wire_id(a[0], a[1], b[0], b[1]))
                p['labels'] = sorted(set(p['labels']) | {name for _ in seg.findall('label')})
                p['connectedPins'] += ['%s.%s' % (r, pn) for r, g, pn in refs_here if (r, g, pn) != k]
    return pins


def trace_point(root, sheet, at, tol):
    """Everything electrically joined at one point, and the whole nets it belongs to."""
    lib = Lib(root.find('drawing/schematic'))
    p = (float(at[0]), float(at[1]))
    found, nets, bodies = [], set(), []
    for name, seg, bus in segments(sheet):
        for w in seg.findall('wire'):
            a, b = (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))
            d = _seg_dist(p, a, b)
            bodies.append((d, 'bus' if bus else 'wire', wire_id(a[0], a[1], b[0], b[1], 'bus' if bus else 'wire'), name))
            if d <= tol:
                end = min(math.hypot(p[0] - a[0], p[1] - a[1]), math.hypot(p[0] - b[0], p[1] - b[1])) <= tol
                found.append({'kind': 'bus' if bus else 'wire', 'id': wire_id(a[0], a[1], b[0], b[1], 'bus' if bus else 'wire'),
                              'net': name, 'atEnd': end})
                nets.add((name, bus))
        for j in seg.findall('junction'):
            q = (_sf(j, 'x'), _sf(j, 'y'))
            if math.hypot(p[0] - q[0], p[1] - q[1]) <= tol:
                found.append({'kind': 'junction', 'id': 'junction:%s' % _key(q), 'net': name})
                nets.add((name, bus))
        for lb in seg.findall('label'):
            q = (_sf(lb, 'x'), _sf(lb, 'y'))
            if math.hypot(p[0] - q[0], p[1] - q[1]) <= tol:
                found.append({'kind': 'label', 'id': 'label:%s' % _key(q), 'net': name})
                nets.add((name, bus))
    pins = pins_with_connections(root, sheet, connections=False)
    for pn in pins:
        d = math.hypot(p[0] - pn['at'][0], p[1] - pn['at'][1])
        bodies.append((d, 'pin', '%s.%s' % (pn['ref'], pn['pin']), pn['net']))
        if d <= tol:
            found.append({'kind': 'pin', 'id': '%s.%s' % (pn['ref'], pn['pin']), 'number': pn['number'], 'net': pn['net']})
            if pn['net']:
                nets.add((pn['net'], False))
    # wires of two different nets meeting here touch on screen but do not connect (no junction joins them)
    wire_nets = {x['net'] for x in found if x['kind'] == 'wire'}
    crossing = len(wire_nets) > 1
    whole = []
    for name, bus in sorted(nets, key=lambda t: (t[0] or '', t[1])):
        pins_on = ['%s.%s' % (pn['ref'], pn['pin']) for pn in pins if pn['net'] == name] if not bus else []
        on_sheets = []
        for i, sh in enumerate(sheets_of(root)):
            if sh.find("nets/net[@name='%s']" % name) is not None if "'" not in (name or '') else False:
                on_sheets.append(i + 1)
        whole.append({'name': name, 'bus': bus, 'pins': pins_on, 'sheets': on_sheets})
    nearest = None
    if not found and bodies:
        d, kind, iid, net = min(bodies)
        nearest = {'kind': kind, 'id': iid, 'net': net, 'distanceMm': _r4(d)}
    return {'items': found, 'nets': whole, 'crossingWithoutJunction': crossing, 'nearest': nearest}


FRAME_LAYERS = ('94',)


def page(root, sheet):
    """The sheet's frame (EAGLE draws the page as a frame symbol placed like a part) and the title
    block fields Fusion fills from the design's attributes."""
    lib = Lib(root.find('drawing/schematic'))
    frame = None
    for inst in sheet.findall('instances/instance'):
        sym, _ = lib.gate_symbol(inst.get('part'), inst.get('gate'))
        if sym is None or sym.find('frame') is None:
            continue
        f = sym.find('frame')
        x1, y1 = transform(_sf(f, 'x1'), _sf(f, 'y1'), _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0'))
        x2, y2 = transform(_sf(f, 'x2'), _sf(f, 'y2'), _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0'))
        frame = {'ref': inst.get('part'), 'symbol': sym.get('name'), 'deviceset': lib.parts[inst.get('part')].get('deviceset'),
                 'rect': [_r4(min(x1, x2)), _r4(min(y1, y2)), _r4(max(x1, x2)), _r4(max(y1, y2))]}
        break
    attrs = {a.get('name'): a.get('value') for a in root.findall('drawing/schematic/attributes/attribute')}
    w = h = None
    if frame:
        w, h = frame['rect'][2] - frame['rect'][0], frame['rect'][3] - frame['rect'][1]
    paper = None
    if w:
        for name, (pw, ph) in PAPER.items():
            if (abs(w - pw) < 8 and abs(h - ph) < 8) or (abs(w - ph) < 8 and abs(h - pw) < 8):
                paper = name
                break
    return {'paper': paper, 'widthMm': _r4(w) if w else None, 'heightMm': _r4(h) if h else None,
            'orientation': (('landscape' if w >= h else 'portrait') if w else None), 'frame': frame,
            'titleBlock': {'title': attrs.get('DRAWING_NAME') or attrs.get('TITLE'), 'rev': attrs.get('REVISION') or attrs.get('REV'),
                           'date': attrs.get('DATE'), 'company': attrs.get('COMPANY') or attrs.get('AUTHOR'),
                           'attributes': attrs}}


PAPER = {'A4': (297, 210), 'A3': (420, 297), 'A2': (594, 420), 'A1': (841, 594), 'A0': (1189, 841),
         'A': (279.4, 215.9), 'B': (431.8, 279.4), 'C': (558.8, 431.8), 'D': (863.6, 558.8), 'E': (1117.6, 863.6)}


def hierarchy(root):
    """Fusion's hierarchy is modules: a module is a sub-design with its own sheets, placed on a
    parent sheet as a module instance (the sheet symbol) whose ports are its pins."""
    sch = root.find('drawing/schematic')
    mods = {m.get('name'): m for m in sch.findall('modules/module')}
    problems = []

    def node(name, sheets, path, depth):
        out = []
        for i, sh in enumerate(sheets):
            kids = []
            for mi in sh.findall('moduleinsts/moduleinst'):
                m = mods.get(mi.get('module'))
                if m is None:
                    problems.append('module instance %s names a module %s that is not in the design' % (mi.get('name'), mi.get('module')))
                    continue
                if depth > 16:
                    problems.append('module %s nests deeper than 16 levels (a loop?)' % mi.get('module'))
                    continue
                ports = [{'name': p.get('name'), 'kind': p.get('direction', 'io'), 'side': p.get('side'), 'coord': _sf(p, 'coord')}
                         for p in m.findall('ports/port')]
                kids.append({'name': mi.get('name'), 'module': mi.get('module'), 'path': path + [mi.get('name')],
                             'at': _pt(_sf(mi, 'x'), _sf(mi, 'y')), 'sizeMm': [_r4(_sf(m, 'dx')), _r4(_sf(m, 'dy'))],
                             'pins': ports, 'sheets': node(m.get('name'), m.findall('sheets/sheet'), path + [mi.get('name')], depth + 1)})
            out.append({'page': i + 1, 'description': (sh.findtext('description') or None), 'children': kids})
        return out

    return {'tree': {'name': 'root', 'sheets': node('root', sch.findall('sheets/sheet'), [], 0)},
            'modules': sorted(mods), 'problems': problems, 'duplicatePages': [], 'renumbered': False}


# ── the live schematic ──────────────────────────────────────────────────────────────────────────

def _pt_of(d):
    try:
        return d.products.item(0).productType if d.products.count else ''
    except Exception:
        return ''


def sch_doc(app, args):
    """The schematic of the design in front (or named by expectDocument): the active document when
    it is a schematic, else its design's schematic tab. A saved design's documents share their cloud
    file name and folder; an unsaved one's are the same-named tabs opened with its design document."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    anchor = app.activeDocument
    want = args.get('expectDocument')
    if want:
        named = [d for d in docs if d.name == want]
        if not named:
            sfail('wrong_document', 'No open document is named %s' % want)
        if anchor not in named:
            anchor = next((d for d in named if _pt_of(d) == 'SchematicProductType'), named[0])
    if anchor is None:
        sfail('no_document', 'No document is open: open the electronics design first')
    if _pt_of(anchor) == 'SchematicProductType':
        return docs.index(anchor), anchor
    if anchor.dataFile is not None:
        ref = anchor.dataFile
        for i, d in enumerate(docs):
            if _pt_of(d) == 'SchematicProductType' and d.dataFile is not None and d.dataFile.name == ref.name and d.dataFile.parentFolder.id == ref.parentFolder.id:
                return i, d
    k = docs.index(anchor)
    start = k
    while start > 0 and _pt_of(docs[start]) != 'EcadDesignProductType':
        start -= 1
    for i in range(start, len(docs)):
        d = docs[i]
        if i > start and _pt_of(d) == 'EcadDesignProductType':
            break
        if _pt_of(d) == 'SchematicProductType' and d.name == anchor.name and (d.dataFile is None) == (anchor.dataFile is None):
            return i, d
    sfail('no_schematic', 'The document in front (%s) has no schematic open beside it: open the electronics design (its project document)' % anchor.name)


def sch_snapshot(app, args):
    """Export the live schematic and parse it; the user's tab is put back afterwards."""
    if args.get('filePath'):
        root = sch_root_from_file(args['filePath'])
        return root, {'source': 'file', 'filePath': args['filePath'], 'revision': sch_revision(root)}, None
    i, doc = sch_doc(app, args)
    front = app.activeDocument
    docs = [app.documents.item(k) for k in range(app.documents.count)]
    j = docs.index(front) if front in docs else None
    with tempfile.TemporaryDirectory(prefix='adom-sch-') as d:
        p = Path(d) / 'snapshot.fsch'
        try:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
            app.executeTextCommand('Document.CopyToDesktop ' + p.as_posix())
        finally:
            if j is not None and j != i:
                app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
        root = sch_root_from_file(str(p))
    return root, {'source': 'live', 'documentName': doc.name, 'revision': sch_revision(root)}, (i, j)


# ── editing ─────────────────────────────────────────────────────────────────────────────────────
# Edits are EAGLE commands typed into the schematic editor. Learned live (2026-10-27):
# - a question SET CONFIRM should answer is only answered when SET CONFIRM is on the SAME command
#   line ("SET CONFIRM YES; NAME ...; SET CONFIRM OFF;"); sent as its own command it is ignored and
#   Fusion asks ("Connect FL_TEST and N$17?"), blocking the main thread;
# - a group MOVE snaps to the sheet grid (0.1 in): the edit runs at GRID MM 0.001, then GRID LAST;
# - TEXT goes on whatever layer is current: LAYER 97 (Info) first;
# - a JUNCTION placed on a wire splits the wire there; SPLIT with one point twice does nothing.

def mm(p):
    return '(%.4fmm %.4fmm)' % (float(p[0]), float(p[1]))


def quote(t):
    return "'" + str(t).replace("'", "''") + "'"


def find_pin(root, sheet, spec):
    """'U1.3' -> the pin on this sheet: by pad number first, then by pin name."""
    if not isinstance(spec, str) or '.' not in spec:
        sfail('bad_args', 'A pin is Ref.Pin, for example U1.3 (got %r)' % (spec,))
    ref, pn = spec.rsplit('.', 1)
    pins = pins_with_connections(root, sheet, [ref], connections=False)
    if not pins:
        sfail('designator_not_found', 'No part %s on this sheet' % ref)
    hit = [p for p in pins if p['number'] and pn in p['number'].split()] or [p for p in pins if p['pin'] == pn] \
        or [p for p in pins if p['pin'].lower() == pn.lower()]
    if not hit:
        sfail('pin_not_found', '%s has no pin %s (pins: %s)' % (ref, pn, ', '.join(sorted({p['number'] or p['pin'] for p in pins}))[:300]))
    return hit[0]


def wire_cover(sheet, kind='nets'):
    """Each net's wires as merged intervals on their lines: a wire split in two by a junction covers
    the same copper as before, so edits are compared by cover, not by wire list."""
    import collections
    groups = collections.defaultdict(list)
    for name, seg, bus in segments(sheet):
        if bus != (kind == 'buses'):
            continue
        for w in seg.findall('wire'):
            a, b = (round(_sf(w, 'x1'), 3), round(_sf(w, 'y1'), 3)), (round(_sf(w, 'x2'), 3), round(_sf(w, 'y2'), 3))
            if a > b:
                a, b = b, a
            dx, dy = b[0] - a[0], b[1] - a[1]
            L = math.hypot(dx, dy)
            if not L:
                continue
            ux, uy = dx / L, dy / L
            key = (name, round(ux, 5), round(uy, 5), round(-uy * a[0] + ux * a[1], 3))
            groups[key].append((round(ux * a[0] + uy * a[1], 3), round(ux * b[0] + uy * b[1], 3)))
    out = {}
    for key, spans in groups.items():
        merged = []
        for a, b in sorted(spans):
            if merged and a <= merged[-1][1] + 1e-3:
                merged[-1][1] = max(merged[-1][1], b)
            else:
                merged.append([a, b])
        out[key] = [tuple(m) for m in merged]
    return out


def _cover_by_line(cover):
    """The same cover keyed by line only, so a rename (net changes, copper does not) compares equal."""
    out = {}
    for (net, ux, uy, c), spans in cover.items():
        out.setdefault((ux, uy, c), []).extend(spans)
    return {k: sorted(v) for k, v in out.items()}


def junction_points(sheet):
    return sorted((round(_sf(j, 'x'), 3), round(_sf(j, 'y'), 3)) for _, seg, _ in segments(sheet) for j in seg.findall('junction'))


def text_rows(sheet):
    return sorted((round(_sf(t, 'x'), 3), round(_sf(t, 'y'), 3), t.text or '') for t in sheet.findall('plain/text'))


def label_rows(sheet):
    return sorted((name, round(_sf(l, 'x'), 3), round(_sf(l, 'y'), 3), l.get('rot', 'R0'), l.get('xref', 'no'))
                  for name, seg, _ in segments(sheet) for l in seg.findall('label'))


def part_rows(sheet):
    return sorted((i.get('part'), i.get('gate'), round(_sf(i, 'x'), 3), round(_sf(i, 'y'), 3), i.get('rot', 'R0')) for i in sheet.findall('instances/instance'))


def ends_at(sheet, p, tol=1e-3, pins=()):
    """How many wire ends, wire middles (two each) and pins meet at p; a junction is needed at three
    or more. pins are the sheet's pin positions (a junction joining a pin and two wires is needed)."""
    n = sum(1 for q in pins if math.hypot(p[0] - q[0], p[1] - q[1]) <= tol)
    for _, seg, bus in segments(sheet):
        if bus:
            continue
        for w in seg.findall('wire'):
            a, b = (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))
            if math.hypot(p[0] - a[0], p[1] - a[1]) <= tol or math.hypot(p[0] - b[0], p[1] - b[1]) <= tol:
                n += 1
            elif _seg_dist(p, a, b) <= tol:
                n += 2
    return n


def current_sheet(app):
    """The sheet the schematic editor shows (the ULP's sheet context), or None."""
    try:
        with tempfile.TemporaryDirectory(prefix='adom-sch-cur-') as d:
            out, ulp = Path(d) / 'c.txt', Path(d) / 'c.ulp'
            ulp.write_text('if (sheet) sheet(SH) { output("%s") { printf("%%d", SH.number); } }' % out.as_posix())
            app.executeTextCommand("Electron.run RUN '%s';" % ulp.as_posix())
            return int(out.read_text().strip()) if out.exists() else None
    except Exception:
        return None


STEPS = [0]   # undo steps the edits of this script run have sent (sch_batch undoes them on a failure)


class Edit:
    """One edit: the commands, then a read back that must match, or every command is undone."""

    def __init__(self, app, args):
        self.app, self.args = app, args
        if args.get('filePath'):
            sfail('unsupported', 'Edits act on the open design; filePath is for reads')
        self.root, self.meta, tabs = sch_snapshot(app, args)
        self.tab, self.front = tabs
        self.n, self.sheet = pick_sheet(self.root, args)
        if args.get('expectedRevision') not in (None, self.meta['revision']):
            sfail('stale_schematic', 'expectedRevision is %s but the schematic is now %s: read it again' % (args.get('expectedRevision'), self.meta['revision']))
        self.cmds = []
        self.extra_steps = 0      # undo steps made by earlier passes of the same edit

    def run(self):
        app, sent, shown = self.app, 0, None
        try:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, self.tab))
            if len(sheets_of(self.root)) > 1:
                shown = current_sheet(app)
                app.executeTextCommand('Electron.run SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % self.n)
            app.executeTextCommand('Electron.run GRID MM 0.001;')
            for c in self.cmds:
                app.executeTextCommand('Electron.run ' + c)
                sent += 1
                if not c.startswith('GROUP'):
                    STEPS[0] += 1
        finally:
            try:
                app.executeTextCommand('Electron.run GRID LAST;')
                # put the editor back on the sheet the user was looking at
                if shown and shown != self.n:
                    app.executeTextCommand('Electron.run SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % shown)
            finally:
                if self.front is not None:
                    app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, self.front))
        self.after, self.meta2, _ = sch_snapshot(app, self.args)
        _, self.sheet2 = pick_sheet(self.after, {'sheet': self.n})
        return sent

    def again(self, cmds):
        """A further pass of the same edit: the steps of the pass before count toward its undo."""
        self.extra_steps += sum(1 for c in self.cmds if not c.startswith('GROUP'))
        self.cmds = cmds
        start, self.sheet = self.sheet, self.sheet2
        self.run()
        self.sheet = start

    def undo(self, why):
        """Put the schematic back to the revision read before the edit. Nothing is undone when it
        already reads so (a command Fusion declined changed nothing: undoing then would take the
        user's own earlier edit). Steps are undone one short of the edit's count, then one at a time,
        each checked; if the start never comes back, every step taken is redone and the edit is
        reported as left in place, rather than undoing further into the user's work."""
        app = self.app
        if self.meta2['revision'] == self.meta['revision']:
            sfail('readback_mismatch', why + '; Fusion made no change, so nothing was undone')
        steps = sum(1 for c in self.cmds if not c.startswith('GROUP')) + self.extra_steps
        taken = [0]
        def step(cmd, k):
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, self.tab))
            for _ in range(k):
                app.executeTextCommand('Electron.run %s;' % cmd)
            if self.front is not None:
                app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, self.front))
            return sch_snapshot(app, self.args)[1]['revision']
        if steps > 1:
            taken[0] += steps - 1
            if step('UNDO', steps - 1) == self.meta['revision']:
                sfail('readback_mismatch', why + '; every command was undone')
        for _ in range(3):
            taken[0] += 1
            if step('UNDO', 1) == self.meta['revision']:
                sfail('readback_mismatch', why + '; every command was undone')
        back = step('REDO', taken[0])
        sfail('readback_mismatch', why + '; UNDO did not return the schematic to its revision before the edit, so the %d steps undone were redone: the edit is %s. Check it in Fusion'
              % (taken[0], 'left in place' if back == self.meta2['revision'] else 'in an unknown state (revision %s)' % back))

    def same_except(self, wires=True, junctions=True, texts=True, labels=True, parts=True, buses=True):
        """True when everything the edit was not meant to touch reads back unchanged."""
        a, b = self.sheet, self.sheet2
        return ((not wires or wire_cover(a) == wire_cover(b)) and (not junctions or junction_points(a) == junction_points(b))
                and (not texts or text_rows(a) == text_rows(b)) and (not labels or label_rows(a) == label_rows(b))
                and (not parts or part_rows(a) == part_rows(b)) and (not buses or wire_cover(a, 'buses') == wire_cover(b, 'buses')))

    def what_changed(self, **skip):
        a, b = self.sheet, self.sheet2
        checks = {'wires': (wire_cover(a), wire_cover(b)), 'junctions': (junction_points(a), junction_points(b)),
                  'texts': (text_rows(a), text_rows(b)), 'labels': (label_rows(a), label_rows(b)),
                  'parts': (part_rows(a), part_rows(b)), 'buses': (wire_cover(a, 'buses'), wire_cover(b, 'buses'))}
        return ', '.join(k for k, (x, y) in checks.items() if skip.get(k, True) and x != y) or 'nothing else'

    def reply(self, **kw):
        out = {'success': True, 'mutated': True, **self.meta2, 'sheet': self.n, 'saved': False}
        out.update(kw)
        return out


def dry(args):
    return args.get('dryRun') is True


def plan_reply(e, **kw):
    return {'success': True, 'mutated': False, 'dryRun': True, **e.meta, 'sheet': e.n, 'commands': e.cmds, **kw}


def sch_text(app, args):
    e = Edit(app, args)
    texts = args.get('texts') or ([args] if args.get('text') else [])
    if not texts:
        sfail('bad_args', 'texts is [{text, at:[x,y], sizeMm?, angle?, align?}]')
    for t in texts:
        if not str(t.get('text', '')).strip() or not t.get('at'):
            sfail('bad_args', 'Each text needs text and at')
        ang = float(t.get('angle', 0)) % 360
        align = t.get('align')
        e.cmds.append('LAYER 97; CHANGE SIZE %.4fmm; %sTEXT %s R%g %s;' % (float(t.get('sizeMm', 1.778)), ('CHANGE ALIGN %s; ' % align) if align else '', quote(t['text']), ang, mm(t['at'])))
    if dry(args):
        return plan_reply(e)
    e.run()
    want = sorted((round(float(t['at'][0]), 3), round(float(t['at'][1]), 3), str(t['text'])) for t in texts)
    added = sorted(set(text_rows(e.sheet2)) - set(text_rows(e.sheet)))
    if added != want or not e.same_except(texts=False):
        e.undo('The texts read back were not exactly the ones asked')
    return e.reply(added=len(added), texts=[{'id': 'text:%s' % _key((x, y)), 'text': t, 'at': [x, y]} for x, y, t in added])


def junction_pick(sheet, q, r=0.35):
    """A click on a junction dot that no wire is under: a click on the dot's centre takes a wire
    end there instead (live 2026-10-27). The dot is about 0.8 mm across; r stays inside it, on
    the side farthest from every wire."""
    ws = [((_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))) for _, seg, _ in segments(sheet) for w in seg.findall('wire')]
    best = None
    for k in range(16):
        a = math.radians(22.5 * k + 11.25)
        p = (q[0] + r * math.cos(a), q[1] + r * math.sin(a))
        d = min([_seg_dist(p, x, y) for x, y in ws] or [9.0])
        if best is None or d > best[0]:
            best = (d, p)
    return best[1]


def nets_at(sheet, p, tol=1e-3):
    return {name for name, seg, bus in segments(sheet) if not bus for w in seg.findall('wire')
            if _seg_dist(p, (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))) <= tol}


def pin_points(root, sheet):
    return [tuple(x['at']) for x in sheet_pins(Lib(root.find('drawing/schematic')), sheet)]


def sch_junction(app, args):
    e = Edit(app, args)
    pp = pin_points(e.root, e.sheet)
    add_pts = [tuple(map(float, p)) for p in (args.get('points') or [])]
    rem = args.get('remove') or []
    have = junction_points(e.sheet)
    added, skipped, not_needed, removing = [], [], [], []
    for p in add_pts:
        q = (round(p[0], 3), round(p[1], 3))
        if q in have:
            skipped.append({'at': list(q), 'why': 'a junction is already there'})
        elif ends_at(e.sheet, p) == 0:
            skipped.append({'at': list(q), 'why': 'no wire passes there'})
        elif ends_at(e.sheet, p, pins=pp) < 3 and not args.get('force'):
            not_needed.append(list(q))
        elif len(nets_at(e.sheet, p)) > 1:
            skipped.append({'at': list(q), 'why': 'wires of %s cross there: a junction would join them (connect them with a wire instead if you mean to)' % ' and '.join(sorted(nets_at(e.sheet, p)))})
        else:
            added.append(q)
            e.cmds.append('SET CONFIRM NO; JUNCTION %s; SET CONFIRM OFF;' % mm(q))
    for r_ in rem:
        q = tuple(map(float, r_.split(':', 1)[1].split(','))) if isinstance(r_, str) and r_.startswith('junction:') else tuple(map(float, r_))
        q = (round(q[0], 3), round(q[1], 3))
        if q not in have:
            skipped.append({'at': list(q), 'why': 'no junction there'})
            continue
        removing.append(q)
        e.cmds.append('DELETE %s;' % mm(junction_pick(e.sheet, q)))
    if dry(args):
        return plan_reply(e, added=[list(q) for q in added], removed=[list(q) for q in removing], skipped=skipped, notNeeded=not_needed)
    if not e.cmds:
        return {'success': True, 'mutated': False, **e.meta, 'sheet': e.n, 'added': [], 'removed': [], 'skipped': skipped, 'notNeeded': not_needed, 'saved': False}
    e.run()
    import collections
    # counted per point: a design can hold two junctions drawn on one point
    after = collections.Counter(junction_points(e.sheet2))
    expect = collections.Counter(have) + collections.Counter(added) - collections.Counter(removing)
    if after != expect or not e.same_except(junctions=False):
        e.undo('The junctions read back were not the asked ones (missing %s, extra %s), or other items changed (%s)'
               % (sorted(set(expect) - set(after)), sorted(set(after) - set(expect)), e.what_changed(junctions=False)))
    return e.reply(added=[list(q) for q in added], removed=[list(q) for q in removing], skipped=skipped, notNeeded=not_needed)


def _wire_targets(sheet, args):
    """The wires named by ids, by their two ends, or every wire through a point."""
    ws = [(name, w) for name, seg, bus in segments(sheet) for w in seg.findall('wire') if not bus]
    def wid(w):
        return wire_id(_sf(w, 'x1'), _sf(w, 'y1'), _sf(w, 'x2'), _sf(w, 'y2'))
    out, missing = [], []
    for i in args.get('ids') or []:
        hit = [x for x in ws if wid(x[1]) == i]
        (out.extend(hit) if hit else missing.append(i))
    if args.get('from') is not None and args.get('to') is not None:
        i = wire_id(*map(float, args['from']), *map(float, args['to']))
        hit = [x for x in ws if wid(x[1]) == i]
        (out.extend(hit) if hit else missing.append(i))
    if args.get('at') is not None:
        p = tuple(map(float, args['at']))
        hit = [x for x in ws if _seg_dist(p, (_sf(x[1], 'x1'), _sf(x[1], 'y1')), (_sf(x[1], 'x2'), _sf(x[1], 'y2'))) <= 1e-3]
        (out.extend(hit) if hit else missing.append('at %s' % _key(p)))
    seen, uniq = set(), []
    for name, w in out:
        if wid(w) not in seen:
            seen.add(wid(w))
            uniq.append((name, w))
    return uniq, missing, wid


def sch_delete_wire(app, args):
    e = Edit(app, args)
    targets, missing, wid = _wire_targets(e.sheet, args)
    if missing and not targets:
        sfail('not_found', 'No wire matches %s' % ', '.join(missing))
    for name, w in targets:
        mid = ((_sf(w, 'x1') + _sf(w, 'x2')) / 2, (_sf(w, 'y1') + _sf(w, 'y2')) / 2)
        e.cmds.append('DELETE %s;' % mm(mid))
    if dry(args):
        return plan_reply(e, deleted=[wid(w) for _, w in targets], notFound=missing)
    e.run()
    # every other wire must still be there; junctions left joining fewer than three items go too
    before = {wid(w) for _, s_, b in segments(e.sheet) if not b for w in s_.findall('wire')}
    after = {wid(w) for _, s_, b in segments(e.sheet2) if not b for w in s_.findall('wire')}
    gone = {wid(w) for _, w in targets}
    # a label goes with its net segment when the segment's last wire goes (a stub and its label):
    # only labels of the deleted wires' nets may go, and none may appear or move
    nets_hit = {name for name, _ in targets}
    lb, la = label_rows(e.sheet), label_rows(e.sheet2)
    labels_ok = set(la) <= set(lb) and all(r[0] in nets_hit for r in set(lb) - set(la))
    if after != before - gone or not labels_ok or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('The wires read back were not the old ones minus the asked ones')
    # only the junctions at the deleted wires' own ends can have lost their reason to be there
    pp = pin_points(e.after, e.sheet2)
    ends = {(round(_sf(w, k1), 3), round(_sf(w, k2), 3)) for _, w in targets for k1, k2 in (('x1', 'y1'), ('x2', 'y2'))}
    orphans = [q for q in junction_points(e.sheet2) if q in ends and ends_at(e.sheet2, q, pins=pp) < 3]
    if orphans:
        e.extra_steps += len(e.cmds)
        e.cmds = ['DELETE %s;' % mm(q) for q in orphans]
        e.sheet = e.sheet2
        e.run()
        import collections
        if collections.Counter(junction_points(e.sheet2)) != collections.Counter(junction_points(e.sheet)) - collections.Counter(orphans) or not e.same_except(junctions=False, labels=False):
            e.undo('Removing the junctions the delete left took more than those (%s)' % e.what_changed(junctions=False))
    return e.reply(deleted=sorted(gone), junctionsRemoved=[list(q) for q in orphans], notFound=missing,
                   labelsRemoved=[list(r) for r in sorted(set(lb) - set(la))])


def route_points(a, b, first, via):
    pts = [tuple(a)] + [tuple(map(float, v)) for v in (via or [])]
    last = pts[-1]
    if abs(last[0] - b[0]) > 1e-6 and abs(last[1] - b[1]) > 1e-6:
        pts.append((b[0], last[1]) if first == 'h' else (last[0], b[1]))
    pts.append(tuple(b))
    out = [pts[0]]
    for p in pts[1:]:
        if math.hypot(p[0] - out[-1][0], p[1] - out[-1][1]) > 1e-6:
            out.append(p)
    return out


def sch_connect_pins(app, args):
    e = Edit(app, args)
    pairs = args.get('pairs') or []
    if not pairs:
        sfail('bad_args', 'pairs is [{from: "U1.1", to: "R5.2", first?: h|v, via?: [[x, y]]}]')
    routes, failed = [], []
    for pr in pairs:
        try:
            a, b = find_pin(e.root, e.sheet, pr.get('from')), find_pin(e.root, e.sheet, pr.get('to'))
        except SchError as x:
            failed.append({'from': pr.get('from'), 'to': pr.get('to'), 'error': str(x)})
            continue
        first = pr.get('first') or ('h' if a['side'] in ('left', 'right') else 'v')
        pts = route_points(a['at'], b['at'], first, pr.get('via'))
        routes.append({'from': pr['from'], 'to': pr['to'], 'points': [list(map(_r4, p)) for p in pts], 'nets': [a.get('net'), b.get('net')]})
        # one command line, so SET CONFIRM answers "Connect A and B?" when the pins are on two nets
        e.cmds.append('SET CONFIRM YES; NET %s; SET CONFIRM OFF;' % ' '.join(mm(p) for p in pts))
    if dry(args):
        return plan_reply(e, routes=routes, failed=failed)
    if not e.cmds:
        sfail('nothing_to_do', 'No pair could be routed: %s' % failed)
    e.run()
    pins = {('%s.%s' % (p['ref'], p['pin'])): p for p in pins_with_connections(e.after, e.sheet2, connections=False)}
    def net_of(spec):
        p = find_pin(e.after, e.sheet2, spec)
        return pins['%s.%s' % (p['ref'], p['pin'])]['net']
    bad = [r for r in routes if not net_of(r['from']) or net_of(r['from']) != net_of(r['to'])]
    # every other pin keeps its net, unless that net was one of the joined ones (a route that
    # touched a third net on its way would join it too)
    joined = {n for r in routes for n in r['nets'] if n}
    before = {('%s.%s' % (p['ref'], p['pin'])): p['net'] for p in pins_with_connections(e.root, e.sheet, connections=False)}
    ends = {r['from'] for r in routes} | {r['to'] for r in routes}
    strays = sorted(k for k, n in before.items() if n and n not in joined and pins.get(k, {}).get('net') != n and k not in ends)
    if strays:
        e.undo('A route also joined pins of other nets (%s)' % ', '.join(strays[:8]))
    if bad or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('After the routes, %s are not on one net' % ', '.join('%s-%s' % (r['from'], r['to']) for r in bad) if bad else 'Parts, texts or buses changed while routing')
    # a T where a route's corner or end lands on another wire's middle gets a dot
    dots = []
    for r in routes:
        for p in r['points']:
            q = (round(p[0], 3), round(p[1], 3))
            if q not in junction_points(e.sheet2) and ends_at(e.sheet2, q, pins=pin_points(e.after, e.sheet2)) >= 3:
                dots.append(q)
    if dots:
        e.extra_steps += sum(1 for c in e.cmds if not c.startswith('GROUP'))
        e.cmds = ['JUNCTION %s;' % mm(q) for q in sorted(set(dots))]
        e.sheet = e.sheet2
        e.run()
    for r in routes:
        r['net'] = net_of(r['from'])
        r['merged'] = sorted({n for n in r.pop('nets') if n and n != r['net']})
    return e.reply(wired=len(routes), routes=routes, junctionsAdded=[list(q) for q in sorted(set(dots))], failed=failed)


def _library_for(e, lib):
    """A library to ADD from: the name of one the design already holds (its schematic's copy is the
    truth), or a .lbr path on this machine (loaded with use_library, which rewrites a compact file
    Fusion 2706 cannot ADD from; #135). If ADD still finds nothing, that is refused honestly."""
    from pathlib import Path as _P
    held = {l_.get('name'): l_ for l_ in e.root.iter('library')}
    if lib in held:
        return held[lib], lib, None
    if lib and _P(lib).is_file():
        return ET.parse(lib).getroot(), _P(lib).stem, lib
    sfail('library_missing', 'library is a library this design already holds (%s) or a .lbr on the Fusion machine: %r'
          % (', '.join(sorted(held))[:300], lib))


def _not_loaded(path):
    return ('Fusion did not take the part from %s: USE loaded it without an error but ADD found nothing. '
            'Add the library to the design first (Fusion\'s Library Manager, or fusion_install_library_bundle for a hub library), then pass its name as library' % path)


def _addable(full, ds, devices):
    """Fusion's ADD and REPLACE take a device by deviceset + variant name from a text command, but
    silently do nothing when the variant name holds parentheses (Fusion's own passives:
    CHIP-0402(1005-METRIC); live 2026-10-29). Refuse those up front instead of failing quietly."""
    if '(' in full or ')' in full:
        sfail('not_supported', "Fusion's ADD/REPLACE text command cannot take the variant %r of %s (parentheses in the name); "
              "variants it can take: %s. Place that one from Fusion's library panel, or use a library whose variants have plain names"
              % (full[len(ds.get('name')):], ds.get('name'), ', '.join(repr(d.get('name')) for d in devices if '(' not in (d.get('name') or '') and ')' not in (d.get('name') or '')) or 'none'))


OUTWARD = {'left': (-1, 0), 'right': (1, 0), 'top': (0, 1), 'bottom': (0, -1)}


def sch_connect(app, args):
    """The standard's sch_connect: per connection a stub from the pin's outer end (stubMil, default
    200), named for the net, and a label at its end (kind label; port: an XREF label; power: a label
    too, as Fusion's power symbols are library parts and nets join by name). kind wire is the named
    stub alone. A pin already on another net is refused, never merged; noerc is refused (EAGLE has
    no No-ERC marker: approve the ERC message instead). Read back; a mismatch undoes every command."""
    e = Edit(app, args)
    conns = args.get('connections') or []
    if not conns:
        sfail('bad_args', 'connections is [{designator, pin, net, kind?: label|power|port|wire, stubMil?}]')
    plans, failed = [], []
    for c in conns:
        spec = '%s.%s' % (c.get('designator'), c.get('pin'))
        kind = str(c.get('kind') or 'label').lower()
        net = str(c.get('net') or '')
        try:
            if kind == 'noerc':
                sfail('not_supported', 'Fusion has no No-ERC marker: leave the pin open and approve its ERC message (fusion_run_erc lists it)')
            if kind not in ('label', 'power', 'port', 'wire'):
                sfail('bad_args', 'kind is label, power, port or wire')
            if not net:
                sfail('bad_args', 'net is required')
            pin = find_pin(e.root, e.sheet, spec)
            have = next((q['net'] for q in pins_with_connections(e.root, e.sheet, [pin['ref']], connections=False)
                         if q['pin'] == pin['pin'] and q['gate'] == pin['gate']), None)
            if have and have != net:
                sfail('pin_on_other_net', '%s is already on %s: disconnect it first (fusion_sch_delete_wire) or connect it to %s' % (spec, have, have))
        except SchError as x:
            failed.append({'designator': c.get('designator'), 'pin': c.get('pin'), 'net': net, 'kind': kind, 'error': str(x), 'code': x.code})
            continue
        L = float(c.get('stubMil') or 200) * 0.0254
        dx, dy = OUTWARD.get(pin['side'], (1, 0))
        a = pin['at']; b = (_r4(a[0] + dx * L), _r4(a[1] + dy * L)); mid = (_r4(a[0] + dx * L / 2), _r4(a[1] + dy * L / 2))
        e.cmds.append('SET CONFIRM YES; NET %s %s %s; SET CONFIRM OFF;' % (quote(net), mm(a), mm(b)))
        if kind != 'wire':
            e.cmds.append('LABEL %s%s %s;' % ('XREF ' if kind == 'port' else '', mm(mid), mm(b)))
        plans.append({'designator': c.get('designator'), 'pin': c.get('pin'), 'ref': pin['ref'], 'pinName': pin['pin'], 'gate': pin['gate'], 'net': net, 'kind': kind,
                      'endMm': list(b), 'endMil': [round(b[0] / 0.0254, 1), round(b[1] / 0.0254, 1)],
                      'note': 'a label: Fusion power symbols are library parts, and the net joins every other copy of its name' if kind == 'power' else None})
    if dry(args):
        return plan_reply(e, results=plans, failed=failed)
    if not e.cmds:
        sfail('nothing_to_do', 'No connection could be made: %s' % failed)
    e.run()
    now = {(q['ref'], q['gate'], q['pin']): q['net'] for q in pins_with_connections(e.after, e.sheet2, connections=False)}
    bad = [p_ for p_ in plans if now.get((p_['ref'], p_['gate'], p_['pinName'])) != p_['net']]
    was = {(q['ref'], q['gate'], q['pin']): q['net'] for q in pins_with_connections(e.root, e.sheet, connections=False)}
    mine = {(p_['ref'], p_['gate'], p_['pinName']) for p_ in plans}
    nets = {p_['net'] for p_ in plans}
    strays = sorted('%s.%s' % (k[0], k[2]) for k, n in was.items() if k not in mine and n and n not in nets and now.get(k) != n)
    if bad or strays or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('After the stubs, %s' % ('; '.join('%s.%s is on %s, not %s' % (p_['designator'], p_['pin'], now.get((p_['ref'], p_['gate'], p_['pinName'])), p_['net']) for p_ in bad)
                                         or ('pins of other nets moved: %s' % ', '.join(strays[:8]) if strays else e.what_changed(wires=False, junctions=False, labels=False))))
    for p_ in plans:
        p_['result'] = 'ok'
        for k in ('ref', 'pinName', 'gate'): p_.pop(k, None)
    return e.reply(connected=len(plans), results=plans, failed=failed,
                   _hint='Nets with the same name connect across the sheets; check with fusion_run_erc. A pin that already had a wire got a second stub only when it was on the same net.')


def sch_place_part(app, args):
    """The standard's sch_place_part: a deviceset from a .lbr on this machine onto the sheet (x, y
    in mil, rotation), with its designator and value. The device is proven in the .lbr first and
    the library USEd, ADD and VALUE pre-answer their questions, so no picker or dialog opens; the
    part is read back on the sheet or the edit is undone. footprint picks the device variant whose
    package has that name. Fusion keeps the board in step, so the part also lands on the board."""
    from pathlib import Path as _P
    e = Edit(app, args)
    lroot, libname, lpath = _library_for(e, args.get('library') or args.get('libraryPath'))
    sym = str(args.get('symbol') or '')
    ds = next((d for d in lroot.iter('deviceset') if d.get('name', '').upper() == sym.upper()), None)
    if ds is None:
        sfail('unknown_device', 'No deviceset %s in %s (devicesets: %s)' % (sym, lib, ', '.join(sorted(d.get('name') for d in lroot.iter('deviceset')))[:300]))
    devices = ds.findall('devices/device')
    fp = args.get('footprint')
    dev = next((d for d in devices if fp and d.get('package', '').upper() == str(fp).upper()), None) if fp else (devices[0] if devices else None)
    if fp and dev is None:
        sfail('unknown_footprint', '%s has no variant with package %s (packages: %s)' % (sym, fp, ', '.join(d.get('package') or '-' for d in devices)))
    full = ds.get('name') + (dev.get('name') if dev is not None else '')
    _addable(full, ds, devices)
    parts = {p_.get('name') for p_ in e.root.iter('part')}
    ref = args.get('designator')
    if not ref or str(ref).endswith('?'):
        prefix = (str(ref)[:-1] if ref else '') or ds.get('prefix') or 'U'
        n = 1
        while '%s%d' % (prefix, n) in parts: n += 1
        ref = '%s%d' % (prefix, n)
    if ref in parts:
        sfail('designator_taken', '%s is already on the schematic' % ref)
    if not re.fullmatch(r'[A-Za-z0-9_$+-]+', ref):
        sfail('bad_args', 'designator %r has characters EAGLE does not take' % ref)
    rot = int(args.get('rotation') or 0)
    if rot not in (0, 90, 180, 270):
        sfail('bad_args', 'rotation is 0, 90, 180 or 270')
    x, y = float(args.get('x', 1000)) * 0.0254, float(args.get('y', 1000)) * 0.0254
    # partId: which gate of a multi-gate device (1 the first). Fusion adds one gate of a part per ADD, named
    gates_ = ds.findall('gates/gate')
    part_id = int(args.get('partId') or 1)
    if not 1 <= part_id <= max(len(gates_), 1):
        sfail('bad_args', 'partId %d: %s has %d gate(s)' % (part_id, sym, len(gates_)))
    gate_arg = " '%s'" % gates_[part_id - 1].get('name') if part_id > 1 else ''
    if max(abs(x), abs(y)) > 1950:
        sfail('off_the_sheet', 'Fusion cannot place a part this far out (past about 2 m it drops it silently and the name stays unusable): keep x and y within 76770 mil')
    e.cmds.append("SET CONFIRM YES; ADD '%s@%s' '%s'%s R%d %s; SET CONFIRM OFF;" % (full, libname, ref, gate_arg, rot, mm((x, y))))
    if args.get('comment'):
        e.cmds.append('SET CONFIRM YES; VALUE %s %s; SET CONFIRM OFF;' % (quote(ref), quote(str(args['comment'])[:40])))
    pins = sum(len(lroot.findall(".//symbols/symbol[@name='%s']/pin" % g.get('symbol'))) for g in ds.findall('gates/gate'))
    out = {'designator': ref, 'libref': '%s@%s' % (full, libname), 'atMil': [round(x / 0.0254, 1), round(y / 0.0254, 1)], 'pins': pins,
           'footprints': [d.get('package') for d in devices if d.get('package')]}
    if dry(args):
        return plan_reply(e, **out)
    if lpath:
        try: use_library(app, lpath)
        except RuntimeError as ex: sfail(str(ex).split(':', 1)[0] if str(ex).startswith(('library_not_writable', 'library_name_in_use')) else 'library_unreadable', str(ex).split(': ', 1)[-1])
    e.run()
    placed = {p_.get('name') for p_ in e.after.iter('part')}
    if lpath and placed == parts:
        sfail('library_not_loaded', _not_loaded(lpath))
    if ref not in placed or not (placed - parts) == {ref} or not e.same_except(parts=False):
        e.undo('%s did not read back as the one part added' % ref)
    return e.reply(**out, _hint='Then wire it: fusion_sch_connect {connections:[{designator, pin, net}]}. Fusion adds it to the board as well (beside the outline): place it with fusion_move_parts.')


def sch_swap_part(app, args):
    """The standard's sch_swap_part with EAGLE's REPLACE: the part keeps its name, place and turn and
    takes another device from a .lbr on this machine (footprint picks the variant). Fusion keeps the
    connections of pins that land where the old ones were; the reply compares every pin's net by
    pad number before and after, and lists the pins whose net was lost (wire them with
    fusion_sch_connect). keepComment puts the old value back. Read back; a mismatch undoes it."""
    from pathlib import Path as _P
    e = Edit(app, args)
    lroot, libname, lpath = _library_for(e, args.get('library') or args.get('libraryPath'))
    sym = str(args.get('symbol') or '')
    ds = next((d for d in lroot.iter('deviceset') if d.get('name', '').upper() == sym.upper()), None)
    if ds is None:
        sfail('unknown_device', 'No deviceset %s in %s' % (sym, lib))
    devices = ds.findall('devices/device'); fp = args.get('footprint')
    dev = next((d for d in devices if fp and d.get('package', '').upper() == str(fp).upper()), None) if fp else (devices[0] if devices else None)
    if fp and dev is None:
        sfail('unknown_footprint', '%s has no variant with package %s' % (sym, fp))
    if len(ds.findall('gates/gate')) > 1:
        sfail('not_supported', 'a multi-gate device: replace it gate by gate in Fusion, or place the new part and delete the old')
    full = ds.get('name') + (dev.get('name') if dev is not None else '')
    _addable(full, ds, devices)
    ref = str(args.get('designator') or '')
    part = next((p_ for p_ in e.root.iter('part') if p_.get('name') == ref), None)
    if part is None:
        sfail('designator_not_found', 'No part %s on the schematic' % ref)
    old_value = part.get('value')
    def by_pad(root, sheet):
        out = {}
        for q in pins_with_connections(root, sheet, [ref], connections=False):
            for pad in (q['number'] or q['pin']).split():
                out[pad] = q['net']
        return out
    def ends(root, sheet):
        out = {}
        for q in pins_with_connections(root, sheet, [ref], connections=False):
            for pad in (q['number'] or q['pin']).split():
                out[pad] = tuple(q['at'])
        return out
    before = by_pad(e.root, e.sheet)
    ends_before = ends(e.root, e.sheet)
    # REPLACE takes the part first, then the device (the other order answers "Device not found"). Fusion names
    # the part afresh when the new device's prefix differs; the name is put back after the run.
    e.cmds.append("SET CONFIRM YES; REPLACE %s '%s@%s'; SET CONFIRM OFF;" % (quote(ref), full, libname))
    if dry(args):
        return plan_reply(e, was={'deviceset': part.get('deviceset'), 'device': part.get('device'), 'library': part.get('library')})
    if lpath:
        try: use_library(app, lpath)
        except RuntimeError as ex: sfail(str(ex).split(':', 1)[0] if str(ex).startswith(('library_not_writable', 'library_name_in_use')) else 'library_unreadable', str(ex).split(': ', 1)[-1])
    e.run()
    names_before = {p_.get('name') for p_ in e.root.iter('part')}
    if not any(p_.get('name') == ref for p_ in e.after.iter('part')):
        fresh = [p_.get('name') for p_ in e.after.iter('part') if p_.get('name') not in names_before]
        if len(fresh) == 1:
            # right after REPLACE Fusion ignores NAME without a word until its events have been pumped (live, 2706)
            import adsk, time as _t
            for _ in range(3):
                adsk.doEvents(); _t.sleep(0.5); adsk.doEvents()
                e.again(["SET CONFIRM YES; NAME %s %s; SET CONFIRM OFF;" % (quote(fresh[0]), quote(ref))])
                if any(p_.get('name') == ref for p_ in e.after.iter('part')):
                    break
    if args.get('keepComment') and old_value:   # after the name is back
        e.again(['SET CONFIRM YES; VALUE %s %s; SET CONFIRM OFF;' % (quote(ref), quote(old_value[:40]))])
    now_part = next((p_ for p_ in e.after.iter('part') if p_.get('name') == ref), None)
    if lpath and now_part is not None and now_part.get('library') == part.get('library') and now_part.get('deviceset') == part.get('deviceset') and now_part.get('device') == part.get('device'):
        sfail('library_not_loaded', _not_loaded(lpath))
    after = by_pad(e.after, e.sheet2)
    if now_part is None or now_part.get('deviceset', '').upper() != ds.get('name').upper() or not e.same_except(parts=False, wires=False, junctions=False, labels=False):
        e.undo('%s did not read back as %s' % (ref, full))
    lost = sorted(p_ for p_, n in before.items() if n and after.get(p_) != n)
    # where a new pin ends elsewhere, a wire joins it to where the old pin ended (matched by pad number)
    ends_after = ends(e.after, e.sheet2)
    join = [p_ for p_ in lost if p_ in ends_after and ends_after[p_] != ends_before.get(p_)]
    if join:
        e.again(['SET CONFIRM YES; SET WIRE_BEND 2; NET %s %s %s; SET WIRE_BEND 0; SET CONFIRM OFF;' % (quote(before[p_]), mm(ends_before[p_]), mm(ends_after[p_])) for p_ in join])
        after = by_pad(e.after, e.sheet2)
        lost = sorted(p_ for p_, n in before.items() if n and after.get(p_) != n)
    # every pin that ended elsewhere and kept its net: joined by Fusion's REPLACE, or by the wire above
    joined = [{'pin': p_, 'from': list(ends_before[p_]), 'to': list(ends_after[p_]), 'net': before[p_]}
              for p_ in sorted(before) if before[p_] and after.get(p_) == before[p_] and p_ in ends_after and ends_after[p_] != ends_before.get(p_)]
    return e.reply(now={'libReference': '%s@%s' % (full, libname), 'footprints': [dev.get('package')] if dev is not None and dev.get('package') else []},
                   oldPinsNotOnNew=sorted(set(before) - set(after)), newPinsNotOnOld=sorted(set(after) - set(before)),
                   lostConnections=[{'pin': p_, 'net': before[p_]} for p_ in lost], pinsJoined=joined,
                   _hint='Fusion updates the board footprint with the part. A pin left in lostConnections has no pin of that pad number on the new part, or could not be joined: wire it with fusion_sch_connect.')


def sch_label(app, args):
    """The standard's sch_label: a net label on an existing wire at a point (LABEL; kind global is an XREF
    label), or delete one by its sch_items id, or by text with at. A label shows its wire's net, so text must
    name that net: renaming a net is sch_edit_label's. Fusion has no hierarchical label. Read back; a mismatch
    undoes it."""
    e = Edit(app, args)
    if args.get('remove'):
        at = args.get('at')
        lid = args.get('id')
        if lid:
            if not str(lid).startswith('label:'):
                sfail('bad_args', 'id is a label id from sch_items (label:x,y)')
            at = [float(v) for v in str(lid).split(':', 1)[1].split(',')]
        if not at or len(at) != 2:
            sfail('bad_args', 'remove needs id, or text with at')
        q = (round(float(at[0]), 3), round(float(at[1]), 3))
        rows = [r for r in label_rows(e.sheet) if (r[1], r[2]) == q and (lid or not args.get('text') or r[0] == args.get('text'))]
        if not rows:
            sfail('not_found', 'No label at %g,%g' % q)
        e.cmds.append('DELETE %s;' % mm(q))
        if dry(args):
            return plan_reply(e, removed=['label:%s' % _key(q)])
        e.run()
        if set(label_rows(e.sheet2)) != set(label_rows(e.sheet)) - set(rows) or not e.same_except(labels=False):
            e.undo('The label did not read back as removed')
        return e.reply(removed=['label:%s' % _key(q)])
    text, at = args.get('text'), args.get('at')
    if not text:
        sfail('bad_args', 'text is the net name (required to add)')
    if not at or len(at) != 2:
        sfail('bad_args', 'at is [x, y] mm, a point on the wire')
    p_ = (float(at[0]), float(at[1]))
    kind = str(args.get('kind') or 'local').lower()
    if kind == 'hierarchical':
        sfail('not_supported', 'Fusion has no hierarchical label: a module port carries a net across a hierarchy')
    if kind not in ('local', 'global'):
        sfail('bad_args', 'kind is local, global or hierarchical')
    nets = nets_at(e.sheet, p_)
    if not nets:
        sfail('no_wire', 'No wire at %g,%g: draw one first (fusion_sch_wire), or use fusion_sch_connect to label a pin' % p_)
    if text not in nets:
        sfail('net_mismatch', 'The wire at %g,%g is on %s, and a label shows its wire\'s net: pass that name, or rename the net with fusion_sch_edit_label' % (p_[0], p_[1], ' and '.join(sorted(nets))))
    rot = int(args.get('rotation') or 0)
    if rot not in (0, 90, 180, 270):
        sfail('bad_args', 'rotation is 0, 90, 180 or 270')
    e.cmds.append('LABEL %s%s R%d %s;' % ('XREF ' if kind == 'global' else '', mm(p_), rot, mm(p_)))
    if dry(args):
        return plan_reply(e)
    e.run()
    fresh = sorted(set(label_rows(e.sheet2)) - set(label_rows(e.sheet)))
    if len(fresh) != 1 or fresh[0][0] != text or set(label_rows(e.sheet)) - set(label_rows(e.sheet2)) or not e.same_except(labels=False):
        e.undo('The label did not read back as the one added')
    r = fresh[0]
    return e.reply(label={'id': 'label:%s' % _key((r[1], r[2])), 'text': r[0], 'kind': kind, 'at': [r[1], r[2]], 'rotation': rot})


def sch_wire(app, args):
    """The standard's sch_wire: a wire through given points, mm. Fusion names a wire's net: it takes the net of a
    wire or pin it starts on, net names one, else Fusion gives it a free name. junctions (default true) adds a
    junction where the wire ends on another wire. Read back; a mismatch undoes it."""
    e = Edit(app, args)
    pts = args.get('points') or []
    if len(pts) < 2 or any(len(p_) != 2 for p_ in pts):
        sfail('bad_args', 'points is [[x, y], ...] mm, two or more')
    pts = [(float(p_[0]), float(p_[1])) for p_ in pts]
    if max(abs(v) for p_ in pts for v in p_) > 1950:
        sfail('off_the_sheet', 'Keep points within 1950 mm: Fusion drops drawing past about 2 m')
    name = args.get('net')
    # WIRE_BEND 2 = straight: the default bend style makes an angled segment into two
    e.cmds.append('SET CONFIRM YES; SET WIRE_BEND 2; NET %s%s; SET WIRE_BEND 0; SET CONFIRM OFF;' % (quote(str(name)) + ' ' if name else '', ' '.join(mm(p_) for p_ in pts)))
    if dry(args):
        return plan_reply(e)
    e.run()
    want = set(zip(pts, pts[1:]))
    have = wire_cover(e.sheet2)
    if have == wire_cover(e.sheet) or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('The wire did not read back as drawn')
    added = []
    if args.get('junctions', True):
        pp = pin_points(e.after, e.sheet2)
        for q in (pts[0], pts[-1]):
            r = (round(q[0], 3), round(q[1], 3))
            if ends_at(e.sheet2, q, pins=pp) >= 3 and r not in junction_points(e.sheet2) and len(nets_at(e.sheet2, q)) == 1:
                added.append(r)
        if added:
            e.again(['SET CONFIRM NO; JUNCTION %s; SET CONFIRM OFF;' % mm(q) for q in added])
    return e.reply(wires=[{'id': wire_id(a[0], a[1], b[0], b[1]), 'from': list(a), 'to': list(b)} for a, b in zip(pts, pts[1:])],
                   junctionsAdded=[{'id': 'junction:%s' % _key(q), 'at': list(q)} for q in added], net=name or sorted(nets_at(e.sheet2, pts[0]))[0] if nets_at(e.sheet2, pts[0]) else None)



def sch_split_wire(app, args):
    if args.get('junction') is False:
        sfail('unsupported', 'Fusion splits a wire by placing a junction on it (SPLIT at one point does nothing); call with junction true, or delete the junction afterwards with sch_junction {remove}')
    e = Edit(app, args)
    targets, missing, wid = _wire_targets(e.sheet, {'ids': [args['id']]} if args.get('id') else {'at': args.get('at')})
    if not targets:
        sfail('not_found', 'No wire there')
    name, w = targets[0]
    a, b = (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))
    p = tuple(map(float, args['at'])) if args.get('at') is not None else ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
    if _seg_dist(p, a, b) > 1e-3 or min(math.hypot(p[0] - a[0], p[1] - a[1]), math.hypot(p[0] - b[0], p[1] - b[1])) < 1e-3:
        sfail('bad_args', 'The cut must be inside the wire, not at an end')
    if len(nets_at(e.sheet, p)) > 1:
        sfail('would_join_nets', 'Wires of %s cross there: a junction would join them' % ' and '.join(sorted(nets_at(e.sheet, p))))
    e.cmds.append('SET CONFIRM NO; JUNCTION %s; SET CONFIRM OFF;' % mm(p))
    if dry(args):
        return plan_reply(e)
    e.run()
    ids = [wire_id(a[0], a[1], p[0], p[1]), wire_id(p[0], p[1], b[0], b[1])]
    after = {wid(x) for _, s_, bb in segments(e.sheet2) if not bb for x in s_.findall('wire')}
    if not all(i in after for i in ids) or not e.same_except(wires=False, junctions=False):
        e.undo('The wire did not read back as two wires meeting at the cut')
    return e.reply(ids=ids, junction='junction:%s' % _key(p))


def sch_bus(app, args):
    e = Edit(app, args)
    pts = [tuple(map(float, p)) for p in (args.get('points') or [])]
    if len(pts) < 2:
        sfail('bad_args', 'points is [[x, y], [x, y], ...] (at least two)')
    label = args.get('label')
    e.cmds.append('BUS %s%s;' % ((quote(label) + ' ') if label else '', ' '.join(mm(p) for p in pts)))
    if label:
        e.cmds.append('LABEL %s %s;' % (mm(pts[0]), mm((pts[0][0], pts[0][1] + 1.27))))
    if dry(args):
        return plan_reply(e)
    e.run()
    cov = wire_cover(e.sheet2, 'buses')
    ok = e.same_except(buses=False, labels=False) and len(cov) >= len(wire_cover(e.sheet, 'buses'))
    if not ok:
        e.undo('The bus did not read back, or other items changed')
    ids = [wire_id(pts[k][0], pts[k][1], pts[k + 1][0], pts[k + 1][1], 'bus') for k in range(len(pts) - 1)]
    return e.reply(ids=ids, label=label)


def sch_edit_label(app, args):
    e = Edit(app, args)
    labels = [(name, l) for name, seg, _ in segments(e.sheet) for l in seg.findall('label')]
    def lid(l):
        return 'label:%s' % _key((_sf(l, 'x'), _sf(l, 'y')))
    edits, failed, plan = args.get('edits') or [], [], []
    for ed in edits:
        if ed.get('net'):
            hits = [(n, l) for n, l in labels if n.lower() == str(ed['net']).lower()]
        else:
            key = ed.get('id') or ('label:%s' % _key(tuple(map(float, ed['at']))) if ed.get('at') else None)
            hits = [(n, l) for n, l in labels if lid(l) == key]
        if not hits:
            failed.append({'edit': ed, 'error': 'no label matches'})
            continue
        for n, l in hits:
            p = (_sf(l, 'x'), _sf(l, 'y'))
            q = p
            if ed.get('moveTo') is not None:
                q = tuple(map(float, ed['moveTo']))
            elif ed.get('by') is not None:
                q = (p[0] + float(ed['by'][0]), p[1] + float(ed['by'][1]))
            if q != p:
                e.cmds.append('SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (mm(p), mm(q)))
            if ed.get('rotation') is not None:
                e.cmds.append('ROTATE =R%g %s;' % (float(ed['rotation']) % 360, mm(q)))
            if ed.get('rename'):
                # the label shows its net's name: renaming it renames the net (all of it on this sheet);
                # onto an existing name Fusion asks "Connect A and B?", answered on the same line
                e.cmds.append('SET CONFIRM YES; NAME %s %s; SET CONFIRM OFF;' % (quote(ed['rename']), mm(q)))
            plan.append({'id': lid(l), 'net': n, 'to': list(q), 'rotation': ed.get('rotation'), 'rename': ed.get('rename')})
    if dry(args):
        return plan_reply(e, edited=plan, failed=failed)
    if not e.cmds:
        sfail('nothing_to_do', 'No label edit applies: %s' % failed)
    e.run()
    after = {(round(_sf(l, 'x'), 3), round(_sf(l, 'y'), 3)): (name, l.get('rot', 'R0')) for name, seg, _ in segments(e.sheet2) for l in seg.findall('label')}
    bad = []
    for pl in plan:
        got = after.get((round(pl['to'][0], 3), round(pl['to'][1], 3)))
        if got is None or (pl['rename'] and got[0] != pl['rename']) or (pl['rotation'] is not None and _rot(got[1])[1] % 360 != float(pl['rotation']) % 360):
            bad.append(pl['id'])
    renamed = any(pl['rename'] for pl in plan)
    copper_ok = (_cover_by_line(wire_cover(e.sheet)) == _cover_by_line(wire_cover(e.sheet2))) if renamed else wire_cover(e.sheet) == wire_cover(e.sheet2)
    if bad or not copper_ok or not e.same_except(wires=False, labels=False, junctions=not renamed):
        e.undo('Labels %s did not read back as asked' % ', '.join(bad) if bad else 'Wires or parts changed while editing labels')
    return e.reply(edited=plan, failed=failed)


def sheet_grid_mm(root):
    g = root.find('drawing/grid')
    if g is None:
        return 2.54
    d = float(g.get('distance', '0.1'))
    unit = g.get('unitdist', 'inch')
    return d * {'inch': 25.4, 'mil': 0.0254, 'mm': 1.0, 'mic': 0.001}.get(unit, 25.4)


def sch_move_region(app, args):
    """Move a box's contents as one GROUP. Fusion snaps a group's outline to the sheet grid (live
    2026-10-27: an edge at 147.16 took a part at 147.32), so the box is snapped here first and the
    parts it takes are checked against the asked box before anything moves. Wires on the pins of a
    moved part follow it (Fusion rubber-bands them), so the check is: parts, texts and labels in the
    box moved exactly, nothing outside moved, and every pin is on the net it was on."""
    e = Edit(app, args)
    box, by = args.get('box'), args.get('by')
    if not box or len(box) != 4 or not by or len(by) != 2:
        sfail('bad_args', 'box is [x1, y1, x2, y2] and by is [dx, dy] (mm)')
    if any(args.get(k) is False for k in ('wires', 'labels', 'junctions')):
        sfail('unsupported', 'Fusion moves a group whole: wires, labels and junctions inside the box always move with it')
    x1, y1, x2, y2 = min(box[0], box[2]), min(box[1], box[3]), max(box[0], box[2]), max(box[1], box[3])
    G = sheet_grid_mm(e.root)
    snap = lambda v: round(round(v / G) * G, 4)
    sx1, sy1, sx2, sy2 = snap(x1), snap(y1), snap(x2), snap(y2)
    if sx2 <= sx1 or sy2 <= sy1:
        sfail('bad_args', 'The box is smaller than the sheet grid (%g mm)' % G)
    in_box = lambda p: x1 - 1e-6 <= p[0] <= x2 + 1e-6 and y1 - 1e-6 <= p[1] <= y2 + 1e-6
    inside = lambda p: sx1 - 1e-6 <= p[0] <= sx2 + 1e-6 and sy1 - 1e-6 <= p[1] <= sy2 + 1e-6
    asked = {r[0] for r in part_rows(e.sheet) if in_box((r[2], r[3]))}
    taken = {r[0] for r in part_rows(e.sheet) if inside((r[2], r[3]))}
    if asked != taken:
        sfail('box_edge_off_grid', 'Fusion snaps a group to the sheet grid (%g mm): this box becomes [%g, %g, %g, %g], which takes %s and leaves %s. '
              'Give the box on the grid so it separates the parts the way you want' %
              (G, sx1, sy1, sx2, sy2, sorted(taken - asked) or 'no extra part', sorted(asked - taken) or 'no part out'))
    anchor = None
    for r in part_rows(e.sheet):
        if inside((r[2], r[3])):
            anchor = (r[2], r[3])
            break
    if anchor is None:
        for _, seg, _ in segments(e.sheet):
            for w in seg.findall('wire'):
                q = (_sf(w, 'x1'), _sf(w, 'y1'))
                if inside(q) and inside((_sf(w, 'x2'), _sf(w, 'y2'))):
                    anchor = q
                    break
            if anchor:
                break
    if anchor is None:
        for x, y, _ in text_rows(e.sheet):
            if inside((x, y)):
                anchor = (x, y)
                break
    if anchor is None:
        sfail('nothing_to_do', 'Nothing in that box to move')
    dx, dy = float(by[0]), float(by[1])
    e.cmds.append('GROUP %s %s %s %s %s;' % (mm((sx1, sy1)), mm((sx2, sy1)), mm((sx2, sy2)), mm((sx1, sy2)), mm((sx1, sy1))))
    # a pin landing on another net's wire makes Fusion ask "Merge net A into B?": No, on the same
    # line (asked separately it is not answered); Fusion then makes no move at all
    e.cmds.append('SET CONFIRM NO; MOVE (C>%.4fmm %.4fmm) %s; SET CONFIRM OFF;' % (anchor[0], anchor[1], mm((anchor[0] + dx, anchor[1] + dy))))
    boxed = [sx1, sy1, sx2, sy2]
    if dry(args):
        return plan_reply(e, moved={'parts': sorted(taken)}, boxUsed=boxed)
    nets_before = {('%s.%s' % (p['ref'], p['pin'])): p['net'] for p in pins_with_connections(e.root, e.sheet, connections=False)}
    e.run()
    # a group MOVE leaves global (XREF) labels where they were (live 2026-10-27): move each label
    # inside the box that the group left behind on its own
    before_l = [(n, round(x, 3), round(y, 3)) for n, x, y, _, _ in label_rows(e.sheet) if inside((x, y))]
    after_l = {(n, round(x, 3), round(y, 3)) for n, x, y, _, _ in label_rows(e.sheet2)}
    left = [(n, x, y) for n, x, y in before_l if (n, x, y) in after_l and (n, round(x + dx, 3), round(y + dy, 3)) not in after_l]
    if left:
        e.again(['SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (mm((x, y)), mm((x + dx, y + dy))) for _, x, y in left])
    import collections
    shift = lambda x, y: (round(x + dx, 3), round(y + dy, 3)) if inside((x, y)) else (round(x, 3), round(y, 3))
    # a scripted group MOVE moves wires, labels and texts but not the parts in it (live 2026-10-27):
    # each part inside that has not moved is moved by name, and the wires on its pins follow it
    after = {(r[0], r[1]): r for r in part_rows(e.sheet2)}
    gates = collections.Counter(r[0] for r in part_rows(e.sheet))
    stay = [r for r in part_rows(e.sheet) if inside((r[2], r[3])) and (round(after[(r[0], r[1])][2], 3), round(after[(r[0], r[1])][3], 3)) != shift(r[2], r[3])]
    if stay:
        e.again(['SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (quote(r[0] + (r[1] if gates[r[0]] > 1 else '')), mm(shift(r[2], r[3]))) for r in stay])
        after = {(r[0], r[1]): r for r in part_rows(e.sheet2)}
    bad = [r[0] for r in part_rows(e.sheet) if (round(after[(r[0], r[1])][2], 3), round(after[(r[0], r[1])][3], 3)) != shift(r[2], r[3])]
    texts = collections.Counter((*shift(x, y), t) for x, y, t in text_rows(e.sheet)) == collections.Counter((round(x, 3), round(y, 3), t) for x, y, t in text_rows(e.sheet2))
    labels = collections.Counter((n, *shift(x, y)) for n, x, y, _, _ in label_rows(e.sheet)) == collections.Counter((n, round(x, 3), round(y, 3)) for n, x, y, _, _ in label_rows(e.sheet2))
    nets_after = {('%s.%s' % (p['ref'], p['pin'])): p['net'] for p in pins_with_connections(e.after, e.sheet2, connections=False)}
    lost = sorted(k for k, n in nets_before.items() if nets_after.get(k) != n)
    if e.meta2['revision'] == e.meta['revision']:
        sfail('would_join_nets', 'Fusion made no move: at the new place a pin or wire end lands on another net, and joining nets is declined. Choose another offset, or clear that spot first (sch_items and sch_trace_point show what is there)')
    if bad or not texts or not labels or lost:
        e.undo('After the move: ' + '; '.join(x for x in (
            ('parts not moved exactly: %s' % ', '.join('%s %s -> %s, asked %s' % (b, [r[2], r[3]], [after[(r[0], r[1])][2], after[(r[0], r[1])][3]], list(shift(r[2], r[3])))
                                                       for b in bad[:4] for r in part_rows(e.sheet) if r[0] == b)) if bad else '', '' if texts else 'texts did not move as asked',
            '' if labels else 'labels did not move as asked', ('pins changed net: %s' % ', '.join(lost[:8])) if lost else '') if x))
    return e.reply(moved={'parts': sorted(taken), 'by': [dx, dy]}, boxUsed=boxed)


def sch_fix_connectivity(app, args):
    e = Edit(app, args)
    tol = float(args.get('toleranceMm', 0.5))
    pins = pins_with_connections(e.root, e.sheet, connections=False)
    targets = [tuple(p['at']) for p in pins]
    ends = []
    for name, seg, bus in segments(e.sheet):
        if bus:
            continue
        for w in seg.findall('wire'):
            for k in ('1', '2'):
                ends.append((name, (_sf(w, 'x' + k), _sf(w, 'y' + k)), w))
    # every point a wire end can join, with the net it belongs to
    pts = [(tuple(p['at']), p['net']) for p in pins] + [(p, name) for name, p, _ in ends] + \
          [((_sf(l, 'x'), _sf(l, 'y')), name) for name, seg, _ in segments(e.sheet) for l in seg.findall('label')]
    auto = lambda n: not n or re.fullmatch(r'N\$\d+', n) is not None
    moves, unresolved = [], []
    for name, p, w in ends:
        if len([q for q, _ in pts if math.hypot(p[0] - q[0], p[1] - q[1]) <= 1e-3]) > 1:
            continue                                     # it already meets something
        near = sorted((math.hypot(p[0] - q[0], p[1] - q[1]), q, n) for q, n in pts if 1e-3 < math.hypot(p[0] - q[0], p[1] - q[1]) <= tol)
        if not near:
            continue
        if len(near) > 1 and abs(near[0][0] - near[1][0]) < 1e-3 and near[0][1] != near[1][1]:
            unresolved.append({'net': name, 'at': _pt(*p), 'why': 'two targets at the same distance'})
            continue
        d, q, other = near[0]
        if other and other != name and not auto(name) and not auto(other):
            unresolved.append({'net': name, 'at': _pt(*p), 'why': 'it almost touches net %s: joining two named nets is left to you' % other})
            continue
        # two loose ends near each other: move only one onto the other, or they swap places
        if any(m['from'] == _pt(*q) and m['to'] == _pt(*p) for m in moves):
            continue
        moves.append({'net': name, 'from': _pt(*p), 'to': _pt(*q), 'distanceMm': _r4(d), 'joins': other if other and other != name else None})
    if args.get('dryRun', True) is not False:
        return {'success': True, 'mutated': False, 'dryRun': True, **e.meta, 'sheet': e.n, 'moves': moves, 'unresolved': unresolved, 'saved': False}
    for m in moves:
        # the join is the point (only an unnamed N$ net is joined to another): answered yes on the line
        e.cmds.append('SET CONFIRM %s; MOVE %s %s; SET CONFIRM OFF;' % ('YES' if m['joins'] else 'NO', mm(m['from']), mm(m['to'])))
    if not e.cmds:
        return {'success': True, 'mutated': False, **e.meta, 'sheet': e.n, 'moves': [], 'unresolved': unresolved, 'saved': False}
    e.run()
    after_pts = [(_sf(w, 'x' + k), _sf(w, 'y' + k)) for _, seg, b in segments(e.sheet2) if not b for w in seg.findall('wire') for k in ('1', '2')]
    bad = [m for m in moves if not any(math.hypot(q[0] - m['to'][0], q[1] - m['to'][1]) <= 1e-3 for q in after_pts)]
    if bad or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('Wire ends did not land where asked')
    return e.reply(moves=moves, unresolved=unresolved)


def bus_members(name):
    """EAGLE bus names: comma-separated members, each a name or a range such as D[0..7]."""
    out = []
    for part in re.split(r'[,\s]+', (name or '').strip('{} ')):
        m = re.fullmatch(r'(.*)\[(\d+)\.\.(\d+)\](.*)', part)
        if m:
            a, b = int(m[2]), int(m[3])
            step = 1 if b >= a else -1
            out += ['%s%d%s' % (m[1], k, m[4]) for k in range(a, b + step, step)]
        elif part:
            out.append(part)
    return out


def bus_segments(sheet):
    return [(name, (_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))) for name, seg, bus in segments(sheet) if bus for w in seg.findall('wire')]


ENTRY = {'down-right': (1, -1), 'up-right': (1, 1), 'down-left': (-1, -1), 'up-left': (-1, 1)}


def sch_bus_entry(app, args):
    """A 45-degree entry from a member wire's end onto a bus. Fusion has no separate entry object:
    it is a short net wire named for its bus member, drawn from the bus side (a net started on a
    bus with its member named raises no member menu) in a straight bend style."""
    e = Edit(app, args)
    buses = bus_segments(e.sheet)
    added, cmds = [], []
    for en in args.get('entries') or []:
        at = tuple(map(float, en['at']))
        sx, sy = ENTRY.get(en.get('direction', 'down-right'), (None, None))
        if sx is None:
            sfail('bad_args', 'direction is up-right, down-right, up-left or down-left')
        size = float(en.get('sizeMm', 2.54))
        end = (at[0] + sx * size, at[1] + sy * size)
        on = [b for b in buses if _seg_dist(end, b[1], b[2]) <= 1e-3]
        if not on:
            sfail('entry_off_bus', 'The entry from %s would end at %s, which is on no bus: extend the bus with sch_bus first' % (_key(at), _key(end)))
        member = en.get('member') or next(iter(nets_at(e.sheet, at)), None)
        if not member:
            sfail('bad_args', 'No wire ends at %s: give member (the bus member the entry carries)' % _key(at))
        if member not in bus_members(on[0][0]):
            sfail('not_a_member', '%s is not a member of bus %s (%s)' % (member, on[0][0], ', '.join(bus_members(on[0][0]))[:200]))
        cmds.append('SET CONFIRM NO; SET WIRE_BEND 2; NET %s %s %s; SET WIRE_BEND 0; SET CONFIRM OFF;' % (quote(member), mm(end), mm(at)))
        added.append({'id': wire_id(at[0], at[1], end[0], end[1], 'entry'), 'at': list(map(_r4, at)), 'end': list(map(_r4, end)), 'onBus': True, 'member': member})
    if not added:
        sfail('bad_args', 'entries is [{at: [x, y], direction?: down-right, member?}]')
    e.cmds = cmds
    if dry(args):
        return plan_reply(e, added=added)
    e.run()
    got = {x['id'] for x in items(e.after, e.sheet2, ['busEntries'])['busEntries']}
    missing = [a['id'] for a in added if a['id'] not in got]
    if missing or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('Entries %s did not read back' % ', '.join(missing) if missing else 'Parts, texts or buses changed')
    return e.reply(added=added)


def sch_connect_to_bus(app, args):
    """Fan pins onto a bus: per pin, a wire from the pin to beside the bus, a 45-degree entry onto
    it and a member label on the wire. Only a pin that is unconnected, or already on that member's
    net, is wired: anything else would join two nets."""
    e = Edit(app, args)
    buses = bus_segments(e.sheet)
    if args.get('bus'):
        buses = [b for b in buses if wire_id(b[1][0], b[1][1], b[2][0], b[2][1], 'bus') == args['bus'] or b[0] == args['bus']]
    names = sorted({b[0] for b in buses})
    if len(names) != 1:
        sfail('bad_args', 'Name the bus (bus: its id or name); the sheet has %s' % (', '.join(names) or 'none'))
    pins_ = args.get('pins') or []
    members = args.get('members') or ['%s%d' % (args.get('prefix', 'D'), int(args.get('start', 0)) + k) for k in range(len(pins_))]
    if len(members) != len(pins_):
        sfail('bad_args', 'members must name one member per pin')
    allowed = bus_members(names[0])
    connected, failed, cmds = [], [], []
    for spec, member in zip(pins_, members):
        try:
            pn = find_pin(e.root, e.sheet, spec)
        except SchError as x:
            failed.append({'pin': spec, 'error': str(x)})
            continue
        if member not in allowed:
            failed.append({'pin': spec, 'error': '%s is not a member of bus %s' % (member, names[0])})
            continue
        if pn['net'] and pn['net'] != member:
            failed.append({'pin': spec, 'error': 'already on net %s: wiring it to %s would join the two' % (pn['net'], member)})
            continue
        P = tuple(pn['at'])
        # the bus segment nearest the pin, and the point on it the entry reaches
        best = min(buses, key=lambda b: _seg_dist(P, b[1], b[2]))
        (ax, ay), (bx, by) = best[1], best[2]
        horizontal = abs(ay - by) < 1e-6
        lean = args.get('entry') or ('right' if horizontal else 'up')
        d = 2.54
        if horizontal:
            side = 1 if P[1] > ay else -1                       # pin above or below the bus
            sx = 1 if lean == 'right' else -1
            near = (P[0], ay + side * d)                        # end of the member wire
            onbus = (P[0] + sx * d, ay)
        else:
            side = 1 if P[0] > ax else -1
            sy = 1 if lean == 'up' else -1
            near = (ax + side * d, P[1])
            onbus = (ax, P[1] + sy * d)
        lo, hi = (min(ax, bx), max(ax, bx)) if horizontal else (min(ay, by), max(ay, by))
        if not lo - 1e-6 <= (onbus[0] if horizontal else onbus[1]) <= hi + 1e-6:
            failed.append({'pin': spec, 'error': 'entry_off_bus: the entry would end at %s, past the bus: extend it with sch_bus first' % _key(onbus)})
            continue
        route = route_points(P, near, 'h' if pn['side'] in ('left', 'right') else 'v', None)
        pts = [onbus] + list(reversed(route))
        cmds.append('SET CONFIRM NO; SET WIRE_BEND 2; NET %s %s; SET WIRE_BEND 0; SET CONFIRM OFF;' % (quote(member), ' '.join(mm(p) for p in pts)))
        lab = ((near[0] + route[-2][0]) / 2, (near[1] + route[-2][1]) / 2) if len(route) > 1 else near
        cmds.append('LABEL %s %s;' % (mm(lab), mm(lab)))
        connected.append({'pin': spec, 'member': member, 'wireId': wire_id(near[0], near[1], route[-2][0], route[-2][1]) if len(route) > 1 else None,
                          'entryId': wire_id(onbus[0], onbus[1], near[0], near[1], 'entry')})
    if args.get('busLabel'):
        ax, ay = buses[0][1]
        cmds.append('LABEL %s %s;' % (mm((ax, ay)), mm((ax, ay + 1.27))))
    e.cmds = cmds
    if dry(args):
        return plan_reply(e, connected=connected, failed=failed, busLabel=args.get('busLabel'))
    if not connected:
        sfail('nothing_to_do', 'No pin could be wired: %s' % failed)
    e.run()
    after = {('%s.%s' % (p['ref'], p['pin'])): p['net'] for p in pins_with_connections(e.after, e.sheet2, connections=False)}
    bad = []
    for c in connected:
        p = find_pin(e.after, e.sheet2, c['pin'])
        if after.get('%s.%s' % (p['ref'], p['pin'])) != c['member']:
            bad.append(c['pin'])
    if bad or not e.same_except(wires=False, junctions=False, labels=False):
        e.undo('Pins %s did not read back on their member nets' % ', '.join(bad) if bad else 'Parts, texts or buses changed')
    return e.reply(connected=connected, failed=failed, busLabel=args.get('busLabel'))


FIELD = {'designator': 'NAME', 'value': 'VALUE', 'name': 'NAME'}


def field_home(lib, inst, field):
    """Where the library symbol places a field of this part: its >FIELD text, through the part's
    turn and mirror. Every field at its default on BQ25792 (155 of them) carries the part's own
    rotation and the symbol text's alignment, which is the rule used here."""
    sym, _ = lib.gate_symbol(inst.get('part'), inst.get('gate'))
    if sym is None:
        return None
    t = next((t for t in sym.findall('text') if (t.text or '').strip() == '>' + field), None)
    if t is None:
        return None
    x, y = transform(_sf(t, 'x'), _sf(t, 'y'), _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0'))
    mi, ai = _rot(inst.get('rot'))
    mt, at_ = _rot(t.get('rot'))
    ang = (ai + (-at_ if mi else at_)) % 360
    return {'at': (x, y), 'rot': ('M' if mi != mt else '') + 'R%g' % ang, 'align': t.get('align', 'bottom-left')}


def sch_reset_fields(app, args):
    e = Edit(app, args)
    lib = Lib(e.root.find('drawing/schematic'))
    refs = set(args.get('references') or [])
    if not refs and not args.get('all'):
        sfail('bad_args', 'Give references, or all:true')
    fields = [FIELD.get(f.lower(), f.upper()) for f in (args.get('fields') or ['designator', 'value'])]
    plan, cmds, untouched = [], [], []
    for inst in e.sheet.findall('instances/instance'):
        ref = inst.get('part')
        if refs and ref not in refs:
            continue
        smashed = {a.get('name'): a for a in inst.findall('attribute') if a.get('x') is not None}
        for f in fields:
            a = smashed.get(f)
            home = field_home(lib, inst, f)
            if a is None or home is None:
                untouched.append({'ref': ref, 'field': f, 'why': 'not smashed: already where the symbol puts it' if a is None else 'the symbol has no >%s text' % f})
                continue
            cur = (_sf(a, 'x'), _sf(a, 'y'))
            steps = []
            if math.hypot(cur[0] - home['at'][0], cur[1] - home['at'][1]) > 1e-3:
                steps.append('SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (mm(cur), mm(home['at'])))
            if (a.get('rot') or 'R0') != home['rot']:
                steps.append('ROTATE =%s %s;' % (home['rot'], mm(home['at'])))
            if (a.get('align') or 'bottom-left') != home['align']:
                steps.append('CHANGE ALIGN %s %s;' % (home['align'], mm(home['at'])))
            if not steps:
                untouched.append({'ref': ref, 'field': f, 'why': 'already where the symbol puts it'})
            if steps:
                cmds += steps
                plan.append({'ref': ref, 'field': f, 'from': list(map(_r4, cur)), 'to': list(map(_r4, home['at'])), 'rot': home['rot'], 'align': home['align']})
    if refs:
        missing = sorted(refs - {i.get('part') for i in e.sheet.findall('instances/instance')})
        if missing:
            sfail('designator_not_found', 'No part %s on sheet %d' % (', '.join(missing), e.n))
    e.cmds = cmds
    if dry(args):
        return plan_reply(e, reset=plan, untouched=untouched)
    if not cmds:
        return {'success': True, 'mutated': False, **e.meta, 'sheet': e.n, 'reset': [], 'untouched': untouched, 'saved': False}
    e.run()
    got = {(i.get('part'), a.get('name')): a for i in e.sheet2.findall('instances/instance') for a in i.findall('attribute')}
    bad = [p_ for p_ in plan if (p_['ref'], p_['field']) not in got
           or abs(_sf(got[(p_['ref'], p_['field'])], 'x') - p_['to'][0]) > 1e-3 or abs(_sf(got[(p_['ref'], p_['field'])], 'y') - p_['to'][1]) > 1e-3
           or (got[(p_['ref'], p_['field'])].get('rot') or 'R0') != p_['rot'] or (got[(p_['ref'], p_['field'])].get('align') or 'bottom-left') != p_['align']]
    if bad or not e.same_except():
        e.undo('Fields %s did not read back where the symbol puts them' % ', '.join('%s.%s' % (b['ref'], b['field']) for b in bad[:8]) if bad else 'Something besides the fields changed (%s)' % e.what_changed())
    return e.reply(reset=[{'ref': p_['ref'], 'field': p_['field']} for p_ in plan], untouched=untouched)


def sch_create(app, args):
    """A new, empty sheet after the last one (Fusion numbers sheets; it has no sheet files)."""
    for k in ('paper', 'orientation', 'widthMm', 'heightMm', 'title', 'date', 'rev', 'company', 'comments'):
        if args.get(k):
            sfail('unsupported', 'Fusion sets a page through a frame part and design attributes that its commands do not reach: create the sheet without %s' % k)
    for k in ('project', 'sheetName', 'at'):
        if args.get(k):
            sfail('unsupported', 'A child sheet under a sheet symbol is a Fusion module, which this verb does not create: leave out %s' % k)
    a = {k: v for k, v in args.items() if k != 'sheet'}
    e = Edit(app, a)
    count = len(sheets_of(e.root))
    want = args.get('sheet')
    if want not in (None, count + 1) and str(want) != str(count + 1):
        sfail('bad_args', 'Fusion adds sheets in order: the next one is %d' % (count + 1))
    e.cmds = ['SET CONFIRM YES; EDIT .s%d; SET CONFIRM OFF;' % (count + 1), 'SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % e.n]
    if dry(args):
        return plan_reply(e, page=count + 1, created=False)
    e.run()
    if len(sheets_of(e.after)) != count + 1 or not e.same_except():
        e.undo('The schematic did not read back with one sheet more')
    return e.reply(sheet=count + 1, page=count + 1, sheetSymbol=None, created=True)



# ── cross probe ─────────────────────────────────────────────────────────────────────────────────

def board_doc_of(app, sdoc):
    """The board tab of the schematic's design, found the way sch_doc finds a schematic."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    if sdoc.dataFile is not None:
        ref = sdoc.dataFile
        for i, d in enumerate(docs):
            if _pt_of(d) == 'BoardProductType' and d.dataFile is not None and d.dataFile.name == ref.name and d.dataFile.parentFolder.id == ref.parentFolder.id:
                return i, d
    for i, d in enumerate(docs):
        if _pt_of(d) == 'BoardProductType' and d.name == sdoc.name and (d.dataFile is None) == (sdoc.dataFile is None):
            return i, d
    return None, None


def copy_tab(app, i, front_index, suffix):
    with tempfile.TemporaryDirectory(prefix='adom-xp-') as d:
        p = Path(d) / ('snapshot.' + suffix)
        try:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
            app.executeTextCommand('Document.CopyToDesktop ' + p.as_posix())
        finally:
            if front_index is not None and front_index != i:
                app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, front_index))
        raw = p.read_bytes()
        if zipfile.is_zipfile(p):
            # a copied Fusion document is a zip with the EAGLE file inside
            with zipfile.ZipFile(p) as z:
                inner = [n for n in z.namelist() if n.lower().endswith('.' + suffix[1:])]
                if len(inner) != 1:
                    sfail('invalid_file', 'Expected one .%s inside the copied %s, found %d' % (suffix[1:], suffix, len(inner)))
                raw = z.read(inner[0])
        return ET.fromstring(raw)


def cross_probe_pairs(sroot, broot, args):
    """Pair schematic parts and board parts by designator, the link Fusion keeps between them
    (forward and back annotation rename both). A schematic part with no package (a supply symbol,
    a frame) has no board part and is not reported."""
    sch = sroot.find('drawing/schematic')
    pk = {}
    for lib in sch.findall('libraries/library'):
        for ds in lib.findall('devicesets/deviceset'):
            for dev in ds.findall('devices/device'):
                pk[(lib.get('name'), ds.get('name'), dev.get('name', ''))] = dev.get('package')
    parts = {p.get('name'): p for p in sch.findall('parts/part')}
    where = {}
    for n, sh in enumerate(sheets_of(sroot), 1):
        for inst in sh.findall('instances/instance'):
            where.setdefault(inst.get('part'), []).append((n, inst))
    els = {e.get('name'): e for e in broot.findall('drawing/board/elements/element')} if broot is not None else {}
    want = set()
    if args.get('ref'):
        want.add(str(args['ref']))
    if args.get('refs'):
        want.update(str(r) for r in args['refs'])
    if args.get('symbolId'):
        want.add(str(args['symbolId']).split(':')[0])
    if args.get('path') not in (None, ''):
        sheet_no = int(str(args['path']).strip('/').split('/')[0] or 0)
        want.update(r for r, ws in where.items() if any(n == sheet_no for n, _ in ws))
    names = sorted(want) if want else sorted(set(parts) | set(els))
    matches, without = [], []
    for r in names:
        p, e = parts.get(r), els.get(r)
        has_pkg = p is not None and bool(pk.get((p.get('library'), p.get('deviceset'), p.get('device', ''))))
        if p is not None and not has_pkg and e is None:
            continue
        m = {'ref': r, 'linked': p is not None and e is not None}
        if e is not None:
            rot = e.get('rot') or 'R0'
            m['board'] = {'ref': r, 'id': r, 'side': 'bottom' if 'M' in rot else 'top', 'x': float(e.get('x')), 'y': float(e.get('y')),
                          'rotation': float(re.sub(r'[^0-9.\-]', '', rot) or 0), 'package': e.get('package'), 'value': e.get('value')}
        if p is not None:
            ws = where.get(r, [])
            m['schematic'] = {'sheet': ws[0][0] if ws else None, 'id': '%s:%s' % (r, ws[0][1].get('gate')) if ws else r, 'ref': r,
                              'symbol': p.get('deviceset'), 'value': p.get('value') or '', 'gates': [{'sheet': n, 'gate': i.get('gate'),
                              'at': [float(i.get('x')), float(i.get('y'))]} for n, i in ws]}
        if p is not None and e is None:
            without.append(r)
        if p is None and e is None:
            m['found'] = False
        matches.append(m)
    return matches, without


def cross_probe(app, args):
    i, sdoc = sch_doc(app, args)
    front = app.activeDocument
    docs = [app.documents.item(k) for k in range(app.documents.count)]
    j = docs.index(front) if front in docs else None
    sroot = copy_tab(app, i, j, 'fsch')
    bi, bdoc = board_doc_of(app, sdoc)
    broot = copy_tab(app, bi, j, 'fbrd') if bdoc is not None else None
    matches, without = cross_probe_pairs(sroot, broot, args)
    selected = []
    if args.get('select'):
        refs = [m['ref'] for m in matches if m.get('linked')][:50]
        if refs:
            show = 'SHOW ' + ' '.join(quote(r) for r in refs)
            try:
                for k in ([i] + ([bi] if bi is not None else [])):
                    app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, k))
                    app.executeTextCommand('Electron.run ' + show)  # no ';' so the highlight stays
            finally:
                if j is not None:
                    app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
            selected = refs
    unknown = [m['ref'] for m in matches if m.get('found') is False]
    hint = '%d match(es); linked:false means one side is missing. Fusion links a schematic part to its board part by designator (renames go both ways).' % len(matches)
    if unknown:
        hint = 'No part named %s in the schematic or on the board. ' % ', '.join(unknown) + hint
    return {'success': not unknown or len(unknown) < len(matches), 'mutated': False, 'schematic': sdoc.name, 'board': bdoc.name if bdoc is not None else None,
            'matches': matches, 'symbolWithoutPart': without, 'selected': selected, 'errorCode': 'designator_not_found' if unknown and len(unknown) == len(matches) else None,
            '_hint': hint + ('' if bdoc is not None else ' No board is open beside this schematic, so every part reads linked:false.')}


# ── schematic data, parameters, cross check (read) ──────────────────────────────────────────────

ELECTRICAL = {'in': 0, 'io': 1, 'out': 2, 'oc': 3, 'pas': 4, 'hiz': 5, 'pwr': 7, 'sup': 7, 'nc': 4}


def _part_attrs(lib, p):
    """A placed part's attributes: its library device's technology attributes, then the part's own."""
    out = {}
    ds = lib.devicesets.get((p.get('library'), p.get('deviceset')))
    if ds is not None:
        dev = next((d for d in ds.findall('devices/device') if d.get('name', '') == p.get('device', '')), None)
        if dev is not None:
            tech = next((t for t in dev.findall('technologies/technology') if t.get('name', '') == p.get('technology', '')), None)
            for a in (tech.findall('attribute') if tech is not None else []):
                if a.get('name') != 'ADOM_PINMAP':
                    out[a.get('name')] = a.get('value', '')
    for a in p.findall('attribute'):
        out[a.get('name')] = a.get('value', '')
    return out


def schematic_data(root, args):
    """The standard's schematic_data: every part (on one sheet, or all) with its pins (number, name,
    electrical type), position and turn of each gate, value, library part and attributes."""
    sch = root.find('drawing/schematic')
    lib = Lib(sch)
    want_sheet = args.get('sheet') if args.get('sheet') not in (None, '', 'all') else None
    where = {}
    for n, sh in enumerate(sheets_of(root), 1):
        if want_sheet is not None and n != int(want_sheet):
            continue
        for inst in sh.findall('instances/instance'):
            where.setdefault(inst.get('part'), []).append((n, inst))
    comps = []
    for name, p in lib.parts.items():
        if args.get('designator') and name != args['designator']:
            continue
        if name not in where:
            continue
        ds = lib.devicesets.get((p.get('library'), p.get('deviceset')))
        gates = []
        pins = []
        for n, inst in where[name]:
            g = inst.get('gate')
            rot = inst.get('rot', 'R0')
            gates.append({'gate': g, 'sheet': n, 'at': [_sf(inst, 'x'), _sf(inst, 'y')], 'rotation': _rot(rot)[1], 'mirrored': 'M' in rot})
            if args.get('pins', True) is not False:
                sym, _ = lib.gate_symbol(name, g)
                for pin in (sym.findall('pin') if sym is not None else []):
                    pins.append({'number': ' '.join(lib.pads(name, g, pin.get('name'))), 'name': pin.get('name').split('@')[0], 'pin': pin.get('name'), 'gate': g,
                                 'electrical': ELECTRICAL.get(pin.get('direction', 'io'), 1)})
        row = {'designator': name, 'libReference': p.get('deviceset'), 'device': p.get('device', ''), 'library': p.get('library'), 'comment': p.get('value') or p.get('deviceset'),
               'description': (ds.findtext('description') or '').strip()[:300] if ds is not None else '', 'gates': gates,
               'xMm': gates[0]['at'][0], 'yMm': gates[0]['at'][1], 'rotation': gates[0]['rotation'], 'pins': pins}
        if args.get('params') or args.get('paramNamesOnly'):
            a = _part_attrs(lib, p)
            row['params'] = sorted(a) if args.get('paramNamesOnly') else a
        comps.append(row)
    comps.sort(key=lambda c: c['designator'])
    limit = int(args.get('limit') or 500)
    out = {'count': len(comps), 'components': comps[:limit], 'truncated': len(comps) > limit}
    if args.get('drawing'):
        out['sheetDrawing'] = sheet_drawing(root)
    return out


PIN_LEN = {'point': 0.0, 'short': 2.54, 'middle': 5.08, 'long': 7.62}


def _nearest_on(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    L = dx * dx + dy * dy
    k = 0.0 if L == 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L))
    return (a[0] + k * dx, a[1] + k * dy)


def sheet_drawing(root):
    """The standard's sheetDrawing companion, one entry per sheet, mm with y DOWN (EAGLE's y up is
    negated). EAGLE joins by segment, not by geometry, so the drawing says what EAGLE means: a
    label (whose text may sit off its wire) is put on the nearest point of its own segment's wires,
    and a wire ending on the middle of another wire of the same segment gets a junction there."""
    lib = Lib(root.find('drawing/schematic'))
    fl = lambda p: [_r4(p[0]), _r4(-p[1] + 0.0)]
    out = []
    for n, sheet in enumerate(sheets_of(root), 1):
        wires, buses, junctions, labels = [], [], [], []
        for name, seg, bus in segments(sheet):
            ws = [((_sf(w, 'x1'), _sf(w, 'y1')), (_sf(w, 'x2'), _sf(w, 'y2'))) for w in seg.findall('wire')]
            (buses if bus else wires).extend({'a': fl(a), 'b': fl(b)} for a, b in ws)
            pts = [tuple(j) for j in ((_sf(j, 'x'), _sf(j, 'y')) for j in seg.findall('junction'))]
            for a, b in ws:
                for end in (a, b):
                    for c, d in ws:
                        if (c, d) != (a, b) and end not in (c, d) and _seg_dist(end, c, d) < 1e-3:
                            pts.append(end)
            junctions.extend(fl(q) for q in dict.fromkeys(pts))
            for lb in seg.findall('label'):
                at = (_sf(lb, 'x'), _sf(lb, 'y'))
                if ws:
                    at = min((_nearest_on(at, a, b) for a, b in ws), key=lambda q: (q[0] - at[0]) ** 2 + (q[1] - at[1]) ** 2)
                labels.append({'text': name, 'kind': 'global' if lb.get('xref') == 'yes' else 'local', 'at': fl(at)})
        pins, symbols = [], []
        for inst in sheet.findall('instances/instance'):
            ref, gate = inst.get('part'), inst.get('gate')
            sym, _ = lib.gate_symbol(ref, gate)
            if sym is None:
                continue
            ix, iy, spec = _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0')
            xs, ys = [], []
            def add(px, py):
                x, y = transform(px, py, ix, iy, spec)
                xs.append(x)
                ys.append(y)
            supply = is_supply(lib, ref)
            for pn in sym.findall('pin'):
                px, py = _sf(pn, 'x'), _sf(pn, 'y')
                x, y = transform(px, py, ix, iy, spec)
                pads = lib.pads(ref, gate, pn.get('name'))
                # a supply symbol's pin joins its net by name: it is a power label, not a part pin
                if supply:
                    p0 = lib.parts[ref]
                    labels.append({'text': p0.get('value') or p0.get('deviceset'), 'kind': 'power', 'at': fl((x, y))})
                else:
                    pins.append({'ref': ref, 'number': ' '.join(pads) or pn.get('name'), 'at': fl((x, y))})
                L = PIN_LEN.get(pn.get('length', 'long'), 7.62)
                a = math.radians(_rot(pn.get('rot', 'R0'))[1])
                add(px, py)
                add(px + L * math.cos(a), py + L * math.sin(a))
            for w in sym.findall('wire'):
                add(_sf(w, 'x1'), _sf(w, 'y1'))
                add(_sf(w, 'x2'), _sf(w, 'y2'))
            for r in sym.findall('rectangle'):
                add(_sf(r, 'x1'), _sf(r, 'y1'))
                add(_sf(r, 'x2'), _sf(r, 'y2'))
            for c in sym.findall('circle'):
                cx, cy, rr = _sf(c, 'x'), _sf(c, 'y'), _sf(c, 'radius')
                add(cx - rr, cy - rr)
                add(cx + rr, cy + rr)
            for pg in sym.findall('polygon'):
                for v in pg.findall('vertex'):
                    add(_sf(v, 'x'), _sf(v, 'y'))
            if xs and not supply:
                symbols.append({'ref': ref if len(lib.devicesets.get((lib.parts[ref].get('library'), lib.parts[ref].get('deviceset'))).findall('gates/gate')) < 2 else '%s.%s' % (ref, gate),
                                'box': [_r4(min(xs)), _r4(-max(ys)), _r4(max(xs)), _r4(-min(ys))]})
        out.append({'sheet': str(n), 'wires': wires, 'buses': buses, 'junctions': junctions, 'noConnects': [],
                    'labels': labels, 'pins': pins, 'symbols': symbols})
    return out


def part_parameters(root, args):
    sch = root.find('drawing/schematic')
    lib = Lib(sch)
    placed = {inst.get('part'): n for n, sh in enumerate(sheets_of(root), 1) for inst in sh.findall('instances/instance')}
    if args.get('name'):
        want = str(args['name']).upper()
        vals = []
        for name, p in sorted(lib.parts.items()):
            a = {k.upper(): v for k, v in _part_attrs(lib, p).items()}
            if want in a and name in placed:
                vals.append({'designator': name, 'value': a[want], 'sheet': placed.get(name)})
        return {'name': args['name'], 'values': vals, 'count': len(vals)}
    counts = {}
    for name, p in lib.parts.items():
        if name not in placed:
            continue
        for k in _part_attrs(lib, p):
            counts[k] = counts.get(k, 0) + 1
    return {'names': [{'name': k, 'parts': v} for k, v in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))]}


def cross_check(app, args):
    """The standard's cross_check: the schematic's designators against the board's (Fusion keeps one
    design's schematic and board linked; a difference is a part placed or deleted on one side only)."""
    i, sdoc = sch_doc(app, args)
    docs = [app.documents.item(k) for k in range(app.documents.count)]
    j = docs.index(app.activeDocument) if app.activeDocument in docs else None
    sroot = copy_tab(app, i, j, 'fsch')
    bi, bdoc = board_doc_of(app, sdoc)
    if bdoc is None:
        sfail('no_board', 'No board is open beside the schematic %s' % sdoc.name)
    broot = copy_tab(app, bi, j, 'fbrd')
    matches, without = cross_probe_pairs(sroot, broot, {})
    on_board_only = [m['ref'] for m in matches if 'board' in m and 'schematic' not in m]
    return {'success': True, 'mutated': False, 'sheet': sdoc.name, 'board': bdoc.name, 'sameProject': True,
            'schematicComponents': sum(1 for m in matches if 'schematic' in m), 'boardComponents': sum(1 for m in matches if 'board' in m),
            'matched': sum(1 for m in matches if m.get('linked')), 'onSheetNotOnBoard': without, 'onBoardNotOnSheet': on_board_only,
            '_hint': 'Fusion forward-annotates: a part only on the board was placed there (logos, mounting holes), one only on the sheet has no package (supply symbols are left out) or the board is out of step.'}


# ── document and part parameters, part edits, annotate (edit) ──────────────────────────────────

def doc_params(app, args):
    """The standard's doc_params: the schematic's global attributes (Fusion's title-block values)
    read, and with set written by ATTRIBUTE * NAME 'value'."""
    to_set = args.get('set') or {}
    if not to_set:
        root, meta, _ = sch_snapshot(app, args)
        attrs = root.findall('drawing/schematic/attributes/attribute')
        return {'success': True, 'mutated': False, **meta, 'params': [{'name': a.get('name'), 'value': a.get('value', '')} for a in attrs], 'set': 0}
    e = Edit(app, args)
    for k, v in to_set.items():
        e.cmds.append('SET CONFIRM YES; ATTRIBUTE * %s %s; SET CONFIRM OFF;' % (quote(str(k).upper()), quote(str(v))))
    if dry(args):
        return plan_reply(e, set=to_set)
    e.run()
    got = {a.get('name').upper(): a.get('value', '') for a in e.after.findall('drawing/schematic/attributes/attribute')}
    bad = [k for k, v in to_set.items() if got.get(str(k).upper()) != str(v)]
    if bad:
        e.undo('global attributes %s did not read back' % bad)
    return {'success': True, 'mutated': True, **e.meta2, 'params': [{'name': k, 'value': v} for k, v in sorted(got.items())], 'set': len(to_set)}


def sheet_params(app, args):
    """The standard's sheet_params: attributes written onto placed parts (ATTRIBUTE 'REF' NAME 'value'),
    the BOM write-back; Fusion keeps the board's copy in step."""
    refs = args.get('designators') or []
    if isinstance(refs, str):
        refs = [r.strip() for r in refs.split(',') if r.strip()]
    params = args.get('params') or {}
    if isinstance(params, str):
        params = dict(kv.split('=', 1) for kv in params.split(';') if '=' in kv)
    if not refs or not params:
        sfail('bad_args', 'designators and params are required')
    e = Edit(app, args)
    lib = Lib(e.root.find('drawing/schematic'))
    found = [r for r in refs if r in lib.parts]
    missing = [r for r in refs if r not in lib.parts]
    for r in found:
        for k, v in params.items():
            e.cmds.append('SET CONFIRM YES; ATTRIBUTE %s %s %s; SET CONFIRM OFF;' % (quote(r), quote(str(k).upper()), quote(str(v))))
    if dry(args):
        return plan_reply(e, updated=found, notFound=missing)
    if not e.cmds:
        sfail('designator_not_found', 'None of %s is in the schematic' % refs)
    e.run()
    lib2 = Lib(e.after.find('drawing/schematic'))
    bad = []
    for r in found:
        a = {k.upper(): v for k, v in _part_attrs(lib2, lib2.parts[r]).items()} if r in lib2.parts else {}
        bad += ['%s.%s' % (r, k) for k, v in params.items() if a.get(str(k).upper()) != str(v)]
    if bad:
        e.undo('attributes %s did not read back' % bad[:5])
    return {'success': True, 'mutated': True, **e.meta2, 'updated': found, 'notFound': missing, 'sheets': [{'sheet': e.n, 'designators': found, 'paramsSet': len(found) * len(params)}]}


MIL = 0.0254


def sch_edit(app, args):
    """The standard's sch_edit on placed parts: move (x, y mil), moveBy (dx, dy mil), rotate (degrees),
    designator (rename), comment (value) and delete, each read back; a mismatch undoes the edit."""
    e = Edit(app, args)
    lib = Lib(e.root.find('drawing/schematic'))
    insts = {inst.get('part'): inst for inst in e.sheet.findall('instances/instance')}
    plan, failed = [], []
    for ed in args.get('edits') or []:
        r, act = ed.get('designator'), ed.get('action')
        if r not in lib.parts or r not in insts:
            failed.append({'edit': ed, 'error': 'no part %s on this sheet' % r}); continue
        inst = insts[r]
        at = (_sf(inst, 'x'), _sf(inst, 'y'))
        if act == 'move':
            q = (float(ed['x']) * MIL, float(ed['y']) * MIL)
            e.cmds.append('SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (quote(r), mm(q))); plan.append((r, 'at', q))
        elif act == 'moveBy':
            q = (at[0] + float(ed['dx']) * MIL, at[1] + float(ed['dy']) * MIL)
            e.cmds.append('SET CONFIRM NO; MOVE %s %s; SET CONFIRM OFF;' % (quote(r), mm(q))); plan.append((r, 'at', q))
        elif act == 'rotate':
            deg = float(ed['degrees']) % 360
            e.cmds.append('ROTATE =R%g %s;' % (deg, quote(r))); plan.append((r, 'rot', deg))
        elif act == 'designator':
            if ed.get('to') in lib.parts:
                failed.append({'edit': ed, 'error': '%s is taken' % ed.get('to')}); continue
            e.cmds.append('SET CONFIRM YES; NAME %s %s; SET CONFIRM OFF;' % (quote(r), quote(ed['to']))); plan.append((ed['to'], 'exists', r))
        elif act == 'comment':
            e.cmds.append('SET CONFIRM YES; VALUE %s %s; SET CONFIRM OFF;' % (quote(r), quote(ed.get('text', '')))); plan.append((r, 'value', ed.get('text', '')))
        elif act == 'delete':
            e.cmds.append('SET CONFIRM YES; DELETE %s; SET CONFIRM OFF;' % quote(r)); plan.append((r, 'gone', None))
        else:
            failed.append({'edit': ed, 'error': 'action is move, moveBy, rotate, designator, comment or delete'})
    if dry(args):
        return plan_reply(e, planned=[{'designator': a, 'check': b} for a, b, _ in plan], failed=failed)
    if not e.cmds:
        sfail('nothing_to_do', 'No edit applies: %s' % failed)
    e.run()
    lib2 = Lib(e.after.find('drawing/schematic'))
    insts2 = {inst.get('part'): inst for inst in e.sheet2.findall('instances/instance')}
    bad, results = [], []
    for r, kind, v in plan:
        ok = True
        if kind == 'at': i2 = insts2.get(r); ok = i2 is not None and abs(_sf(i2, 'x') - v[0]) < 0.01 and abs(_sf(i2, 'y') - v[1]) < 0.01
        elif kind == 'rot': i2 = insts2.get(r); ok = i2 is not None and abs(_rot(i2.get('rot', 'R0'))[1] % 360 - v) < 0.01
        elif kind == 'exists': ok = r in lib2.parts and v not in lib2.parts
        elif kind == 'value': ok = r in lib2.parts and (lib2.parts[r].get('value') or '') == v
        elif kind == 'gone': ok = r not in lib2.parts
        results.append({'designator': r, 'check': kind, 'result': 'ok' if ok else 'mismatch'})
        if not ok: bad.append(r)
    if bad:
        e.undo('parts %s did not read back as asked' % bad)
    return {'success': True, 'mutated': True, **e.meta2, 'applied': len(plan), 'failed': failed, 'results': results}


def annotate(app, args):
    """The standard's annotate: every 'R?'-style designator gets the lowest free number of its prefix,
    sheet by sheet, top-left first (Fusion names parts as they are added, so this is rare)."""
    e = Edit(app, args)
    lib = Lib(e.root.find('drawing/schematic'))
    used = {}
    for n in lib.parts:
        m = re.fullmatch(r'([A-Za-z_$]+)(\d+)', n)
        if m:
            used.setdefault(m[1], set()).add(int(m[2]))
    todo = []
    for k, sh in enumerate(sheets_of(e.root), 1):
        for inst in sorted(sh.findall('instances/instance'), key=lambda i: (-_sf(i, 'y'), _sf(i, 'x'))):
            name = inst.get('part')
            if name.endswith('?') and name not in [t[1] for t in todo]:
                todo.append((k, name))
    planned = []
    for k, name in todo:
        pre = name[:-1] or 'U'
        num = 1
        while num in used.setdefault(pre, set()):
            num += 1
        used[pre].add(num)
        planned.append({'sheet': k, 'from': name, 'to': '%s%d' % (pre, num)})
        e.cmds.append('SET CONFIRM YES; NAME %s %s; SET CONFIRM OFF;' % (quote(name), quote('%s%d' % (pre, num))))
    if dry(args) or not planned:
        return {'success': True, 'mutated': False, **e.meta, 'planned': planned, 'renamed': [], 'skippedMultiPart': []}
    e.run()
    lib2 = Lib(e.after.find('drawing/schematic'))
    bad = [p for p in planned if p['to'] not in lib2.parts]
    if bad:
        e.undo('renames %s did not read back' % bad[:3])
    return {'success': True, 'mutated': True, **e.meta2, 'renamed': [dict(p, result='ok') for p in planned], 'skippedMultiPart': []}

def _variant_rows(root):
    """The schematic's assembly variants: [{name, current, notFitted, alternates}]."""
    sch = root.find('drawing/schematic')
    out = []
    for vd in (sch.findall('variantdefs/variantdef') if sch is not None else []):
        name = vd.get('name')
        nf, alt = [], []
        for p in sch.findall('parts/part'):
            for v in p.findall('variant'):
                if v.get('name') != name:
                    continue
                if v.get('populate') == 'no':
                    nf.append(p.get('name'))
                elif v.get('value') or v.get('technology'):
                    alt.append({'designator': p.get('name'), 'value': v.get('value'), 'technology': v.get('technology')})
        out.append({'name': name, 'current': vd.get('current') == 'yes', 'notFitted': sorted(nf), 'alternates': alt})
    return out


def variants(app, args):
    """The standard's variants: assembly variants of the design (which parts are fitted on which
    build). With no name, the list. With a name: made when new (EAGLE's VARIANT, which also makes it
    the current variant), then notFitted parts marked NOPOPULATE and fitted ones put back, each by
    name so no dialog opens; read back, or every command undone. remove deletes the variant."""
    e = Edit(app, args)
    before = _variant_rows(e.root)
    name = args.get('name')
    if not name:
        return {'success': True, 'mutated': False, **e.meta, 'variants': before}
    # Fusion 2706 takes EAGLE's VARIANT from a text command (name; name part NOPOPULATE; either
    # order; schematic or board) and changes nothing, with no message (checked live 2026-10-29), so
    # an edit is refused rather than reported done
    sfail('not_supported', "Fusion's VARIANT text command creates and changes no assembly variant (it answers nothing and the design keeps its variants), "
          "so %s cannot be made, changed or removed from the bridge. Variants: %s. Use Fusion's assembly variants in the schematic editor; this verb lists them."
          % (name, ', '.join(v['name'] for v in before) or 'none'))
    if re.search(r"[';]", str(name)):
        sfail('invalid_args', "a variant name cannot hold ' or ;")
    lib = Lib(e.root.find('drawing/schematic'))
    nf = [str(x) for x in (args.get('notFitted') or args.get('dnp') or [])]
    fit = [str(x) for x in (args.get('fitted') or [])]
    missing = [x for x in nf + fit if x not in lib.parts]
    if missing:
        sfail('unknown_part', 'No part %s on the schematic' % ', '.join(missing))
    have = next((v for v in before if v['name'] == name), None)
    if args.get('remove'):
        if have is None:
            sfail('unknown_variant', 'No variant %s (variants: %s)' % (name, ', '.join(v['name'] for v in before) or 'none'))
        e.cmds.append("SET CONFIRM YES; VARIANT '%s' DELETE; SET CONFIRM OFF;" % name)
    else:
        if have is None:
            e.cmds.append("SET CONFIRM YES; VARIANT '%s'; SET CONFIRM OFF;" % name)
        for x in nf:
            e.cmds.append("SET CONFIRM YES; VARIANT '%s' %s NOPOPULATE; SET CONFIRM OFF;" % (name, quote(x)))
        for x in fit:
            e.cmds.append("SET CONFIRM YES; VARIANT '%s' %s POPULATE; SET CONFIRM OFF;" % (name, quote(x)))
    if dry(args):
        return plan_reply(e, variants=before)
    if not e.cmds:
        return {'success': True, 'mutated': False, **e.meta, 'variants': before}
    e.run()
    after = _variant_rows(e.after)
    now = next((v for v in after if v['name'] == name), None)
    if args.get('remove'):
        if now is not None:
            e.undo('variant %s is still there' % name)
    else:
        want_nf = set((have or {}).get('notFitted', [])) | set(nf)
        want_nf -= set(fit)
        if now is None or set(now['notFitted']) != want_nf:
            e.undo('variant %s reads back as %s, not the parts asked' % (name, now and now['notFitted']))
    return e.reply(variants=after, variant=now,
                   _hint='Variants live in the schematic; Fusion keeps the board in step. The variant asked is now the current one (EAGLE makes a new variant current). fusion_board_bom reads the fitted parts of the current variant.')


def _pins_of(sym):
    return {pn.get('name'): tuple(pn.get(k, '') for k in ('x', 'y', 'length', 'direction', 'rot')) for pn in sym.findall('pin')}


def _art_of(sym):
    return sorted(ET.tostring(c).decode() for c in sym if c.tag != 'pin')


def sch_update_from_library(root, args):
    """The standard's sch_update_from_library, the plan: each placed part's component as the schematic
    holds it (its embedded library) against the same-named deviceset in the .lbr files named: gates,
    each symbol's pins (added, removed, moved) and drawing, and the device's attributes. A pin the
    library drops that is wired on the sheet is named in droppedWiredPins (Altium's force case)."""
    paths = args.get('libraries') or ([args['library']] if args.get('library') else []) or ([args['libraryPath']] if args.get('libraryPath') else [])
    if not paths:
        sfail('bad_args', 'library (or libraries: [.lbr]) is required: a Fusion design keeps a copy of each library it uses, so name the .lbr files to compare against')
    libs = []
    for path in paths:
        try:
            lr = ET.parse(path).getroot()
        except Exception as ex:
            sfail('library_unreadable', '%s: %s' % (path, ex))
        lb = lr.find('drawing/library')
        if lb is None:
            sfail('library_unreadable', '%s holds no library' % path)
        libs.append((path, {s.get('name'): s for s in lb.findall('symbols/symbol')}, {d.get('name').lower(): d for d in lb.findall('devicesets/deviceset')}))
    sch = root.find('drawing/schematic')
    lib = Lib(sch)
    wired = set()
    for pr in sch.iter('pinref'):
        wired.add((pr.get('part'), pr.get('gate'), pr.get('pin')))
    want = {str(s).lower() for s in (args.get('symbols') or [])}
    rows, differs = [], 0
    for name, part in sorted(lib.parts.items()):
        dsn, ln = part.get('deviceset') or '', part.get('library') or ''
        if want and dsn.lower() not in want:
            continue
        held = lib.devicesets.get((ln, dsn))
        hit = next(((path, syms, dss[dsn.lower()]) for path, syms, dss in libs if dsn.lower() in dss), None)
        if held is None or hit is None:
            rows.append({'designator': name, 'symbol': dsn, 'library': ln if hit is None else hit[0], 'status': 'library_not_found',
                         'why': 'no deviceset of that name in the libraries given' if hit is None else 'the schematic holds no copy of it'})
            continue
        path, syms, ds = hit
        diff = {}
        g_held = {g.get('name'): g.get('symbol') for g in held.findall('gates/gate')}
        g_lib = {g.get('name'): g.get('symbol') for g in ds.findall('gates/gate')}
        if set(g_held) != set(g_lib):
            diff['gates'] = {'added': sorted(set(g_lib) - set(g_held)), 'removed': sorted(set(g_held) - set(g_lib))}
        dropped = []
        for g in sorted(set(g_held) & set(g_lib)):
            a, b = lib.symbols.get((ln, g_held[g])), syms.get(g_lib[g])
            if a is None or b is None:
                continue
            pa, pb = _pins_of(a), _pins_of(b)
            d = {}
            if set(pb) - set(pa):
                d['pinsAdded'] = sorted(set(pb) - set(pa))
            if set(pa) - set(pb):
                d['pinsRemoved'] = sorted(set(pa) - set(pb))
                dropped += ['%s.%s' % (g, x) if len(g_held) > 1 else x for x in d['pinsRemoved'] if (name, g, x) in wired]
            moved = sorted(x for x in set(pa) & set(pb) if pa[x] != pb[x])
            if moved:
                d['pinsChanged'] = moved
            if _art_of(a) != _art_of(b):
                d['drawingChanged'] = True
            if d:
                diff.setdefault('symbols', {})[g] = d
        dev = part.get('device') or ''
        tech = part.get('technology') or ''
        def attrs(dset):
            dv = next((x for x in dset.findall('devices/device') if (x.get('name') or '') == dev), None)
            if dv is None:
                return None
            tn = next((x for x in dv.findall('technologies/technology') if (x.get('name') or '') == tech), None)
            return {a.get('name'): a.get('value') for a in (tn.findall('attribute') if tn is not None else [])}
        ah, al = attrs(held), attrs(ds)
        if al is None:
            diff['device'] = 'the library has no device %r (technology %r) of %s' % (dev, tech, dsn)
        elif ah is not None and ah != al:
            diff['attributes'] = {k: {'held': ah.get(k), 'library': al.get(k)} for k in sorted(set(ah) | set(al)) if ah.get(k) != al.get(k)}
        row = {'designator': name, 'symbol': dsn, 'library': path, 'status': 'differs' if diff else 'up_to_date'}
        if diff:
            differs += 1
            row['diff'] = diff
        if dropped:
            row['droppedWiredPins'] = dropped
        rows.append(row)
    return {'plan': True, 'differs': differs, 'parts': rows}


def sch_batch(app, args):
    """The standard's sch_batch: a list of schematic edits, all or nothing. Each op is a standard sch_*
    verb with its own arguments, run as that verb runs (commands, read back, undone on a mismatch), so
    it sees the sheet as the earlier ops left it. When an op fails, the ops before it are undone and the
    schematic is checked back at its revision before the batch; if UNDO cannot bring it back, the steps
    are redone and the batch is reported as partial with the ops that ran."""
    ops = args.get('ops')
    if not isinstance(ops, list) or not ops:
        sfail('bad_args', 'ops is [{op: a schematic verb (sch_place_part, sch_connect_pins, ...), ...its arguments}]')
    names = []
    for i, o in enumerate(ops):
        n = str((o or {}).get('op') or '').replace('fusion_', '', 1) if isinstance(o, dict) else ''
        if n not in EDITS:
            sfail('bad_args', 'op %d (%s) is not a schematic edit this batch can run: one of %s' % (i, n or 'missing', ', '.join(sorted(EDITS))))
        names.append(n)
    shared = {k: args[k] for k in ('expectDocument', 'sheet') if k in args}
    root0, meta0, tabs = sch_snapshot(app, args)
    if args.get('expectedRevision') not in (None, meta0['revision']):
        sfail('stale_schematic', 'expectedRevision is %s but the schematic is now %s: read it again' % (args.get('expectedRevision'), meta0['revision']))
    results = []
    if dry(args):
        for n, o in zip(names, ops):
            a = {**shared, **{k: v for k, v in o.items() if k != 'op'}, 'dryRun': True}
            try:
                results.append({'op': n, 'output': EDITS[n](app, a)})
            except SchError as e:
                results.append({'op': n, 'output': {'success': False, 'errorCode': e.code, 'error': str(e)}})
        return {'success': True, 'mutated': False, 'dryRun': True, **meta0, 'results': results,
                '_hint': 'Each op planned against the sheet as it is now (a dry run writes nothing, so an op that needs an earlier op\'s part or wire plans against the sheet without it).'}
    STEPS[0] = 0
    for i, (n, o) in enumerate(zip(names, ops)):
        before = STEPS[0]
        a = {**shared, **{k: v for k, v in o.items() if k != 'op'}}
        a.pop('expectedRevision', None)
        try:
            r = EDITS[n](app, a)
            err = None if r.get('success') is not False else (r.get('errorCode'), r.get('error'))
        except SchError as e:
            r, err = None, (e.code, str(e))
        if err is None:
            results.append({'op': n, 'output': r})
            continue
        # the failing op undid its own commands unless it says it left them in place
        if 'left in place' not in str(err[1]) and 'unknown state' not in str(err[1]):
            STEPS[0] = before
        steps = STEPS[0]
        undone, state = 0, None
        if steps:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, tabs[0]))
            for _ in range(steps):
                app.executeTextCommand('Electron.run UNDO;')
                undone += 1
            if tabs[1] is not None:
                app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, tabs[1]))
            state = sch_snapshot(app, args)[1]['revision']
            if state != meta0['revision']:
                app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, tabs[0]))
                for _ in range(undone):
                    app.executeTextCommand('Electron.run REDO;')
                if tabs[1] is not None:
                    app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, tabs[1]))
                return {'success': False, 'errorCode': 'batch_partial', 'failedOp': i, 'mutated': True, 'partial': True, 'ranOps': list(range(i)), 'results': results,
                        'error': 'op %d (%s) failed: %s. UNDO did not bring the schematic back to its revision before the batch, so ops 0-%d are left in place' % (i, n, err[1], i - 1),
                        'opError': {'errorCode': err[0], 'error': err[1]}, **sch_snapshot(app, args)[1]}
        return {'success': False, 'errorCode': 'batch_failed', 'failedOp': i, 'mutated': False, 'results': results, 'undoneSteps': undone,
                'error': 'op %d (%s) failed: %s. Nothing is changed: the ops before it were undone' % (i, n, err[1]),
                'opError': {'errorCode': err[0], 'error': err[1]}, **meta0}
    meta1 = sch_snapshot(app, args)[1]
    return {'success': True, 'mutated': True, **meta1, 'results': results, 'saved': False,
            '_hint': 'Every op ran and read back. save:true saves the design as a new version.'}


EDITS = {'sch_label': sch_label, 'sch_wire': sch_wire, 'doc_params': doc_params, 'sheet_params': sheet_params, 'sch_edit': sch_edit, 'annotate': annotate, 'sch_text': sch_text, 'sch_junction': sch_junction, 'sch_delete_wire': sch_delete_wire, 'sch_connect_pins': sch_connect_pins,
         'sch_split_wire': sch_split_wire, 'sch_bus': sch_bus, 'sch_edit_label': sch_edit_label, 'sch_move_region': sch_move_region,
         'sch_fix_connectivity': sch_fix_connectivity, 'sch_bus_entry': sch_bus_entry, 'sch_connect_to_bus': sch_connect_to_bus, 'sch_reset_fields': sch_reset_fields, 'sch_create': sch_create, 'sch_connect': sch_connect, 'sch_place_part': sch_place_part, 'sch_swap_part': sch_swap_part, 'variants': variants}


def drawing_extent(root, sheet):
    """The sheet's drawn extent in mm (wires, pins with their length, symbol bodies, labels,
    texts): EXPORT IMAGE crops to it, so it maps the image back to sheet coordinates (to within
    a text's overhang)."""
    lib = Lib(root.find('drawing/schematic'))
    xs, ys = [], []
    def add(x, y):
        xs.append(x)
        ys.append(y)
    for _, seg, _ in segments(sheet):
        for w in seg.findall('wire'):
            add(_sf(w, 'x1'), _sf(w, 'y1'))
            add(_sf(w, 'x2'), _sf(w, 'y2'))
        for l in seg.findall('label'):
            add(_sf(l, 'x'), _sf(l, 'y'))
    def text_box(x, y, txt, size, spec, frame=None):
        """A text's box, by the vector font's proportions (a character is about 0.8 x its size
        wide): the export includes text, and tall rotated values reach well past the parts."""
        L = max(1, len(txt or '')) * size * 0.8
        corners = [(0, 0), (L, 0), (L, size), (0, size)]
        mirrored, deg = _rot(spec)
        for cx, cy in corners:
            px_, py_ = transform(cx, cy, x, y, spec)
            if frame:
                px_, py_ = transform(px_, py_, *frame)
            add(px_, py_)
    for t in sheet.findall('plain/text'):
        text_box(_sf(t, 'x'), _sf(t, 'y'), t.text, _sf(t, 'size', 1.778), t.get('rot', 'R0'))
    for inst in sheet.findall('instances/instance'):
        sym, _ = lib.gate_symbol(inst.get('part'), inst.get('gate'))
        if sym is None:
            continue
        ix, iy, spec = _sf(inst, 'x'), _sf(inst, 'y'), inst.get('rot', 'R0')
        part = lib.parts.get(inst.get('part'))
        names = {'>NAME': inst.get('part'), '>VALUE': (part.get('value') if part is not None else '') or (part.get('deviceset') if part is not None else '')}
        smashed = {a.get('name'): a for a in inst.findall('attribute')}
        for a in smashed.values():
            if a.get('x') is not None and a.get('display', 'value') != 'off':
                text_box(_sf(a, 'x'), _sf(a, 'y'), names.get('>' + a.get('name', ''), a.get('value', '')), _sf(a, 'size', 1.778), a.get('rot', 'R0'))
        for t in sym.findall('text'):
            key = (t.text or '').strip()
            if key.startswith('>') and key[1:] in smashed:
                continue                        # drawn where the smashed attribute says
            text_box(_sf(t, 'x'), _sf(t, 'y'), names.get(key, key), _sf(t, 'size', 1.778), t.get('rot', 'R0'), (ix, iy, spec))
        for w in sym.findall('wire'):
            for k in ('1', '2'):
                add(*transform(_sf(w, 'x' + k), _sf(w, 'y' + k), ix, iy, spec))
        for r_ in sym.findall('rectangle'):
            for k in ('1', '2'):
                add(*transform(_sf(r_, 'x' + k), _sf(r_, 'y' + k), ix, iy, spec))
        for pn in sym.findall('pin'):
            add(*transform(_sf(pn, 'x'), _sf(pn, 'y'), ix, iy, spec))
    if not xs:
        sfail('empty_sheet', 'The sheet has nothing drawn on it')
    return min(xs), min(ys), max(xs), max(ys)


def sheet_area(app, n):
    with tempfile.TemporaryDirectory(prefix='adom-sch-area-') as d:
        out, ulp = Path(d) / 'a.txt', Path(d) / 'a.ulp'
        ulp.write_text('schematic(S) { output("%s") { S.sheets(SH) { printf("%%d %%f %%f %%f %%f\\n", SH.number, u2mm(SH.area.x1), u2mm(SH.area.y1), u2mm(SH.area.x2), u2mm(SH.area.y2)); } } }' % out.as_posix())
        app.executeTextCommand("Electron.run RUN '%s';" % ulp.as_posix())
        for line in out.read_text().splitlines():
            k, a, b, c, e = line.split()
            if int(k) == n:
                return float(a), float(b), float(c), float(e)
    raise SchError('render_failed', 'Fusion reported no area for sheet %d' % n)


def sch_render(app, args):
    """The sheet as a PNG through Fusion's own EXPORT IMAGE, without opening a window: dpi is
    chosen so the image is about width pixels wide."""
    root, meta, tabs = sch_snapshot(app, args)
    n, sheet = pick_sheet(root, args)
    if args.get('drawingSheet') is False:
        sfail('unsupported', 'Fusion draws the frame as a part on the sheet: drawingSheet false is not offered')
    out = args.get('png')
    if not out:
        sfail('bad_args', 'png: the file to write is required (the bridge fills it in)')
    i, j = tabs
    try:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
        shown = current_sheet(app) if len(sheets_of(root)) > 1 else None
        if shown and shown != n:
            app.executeTextCommand('Electron.run SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % n)
        # the sheet's drawn area as Fusion counts it (UL_SHEET.area), which is what EXPORT IMAGE
        # crops to; an estimate from the XML missed tall rotated texts by 24 mm
        try:
            x0, y0, x1, y1 = sheet_area(app, n)
        except Exception:
            x0, y0, x1, y1 = drawing_extent(root, sheet)
        width = int(args.get('width', 2000))
        dpi = max(20, min(1200, int(round(width / max((x1 - x0) / 25.4, 0.1)))))
        # SET CONFIRM on the same line answers "overwrite?" when the file exists
        app.executeTextCommand("Electron.run SET CONFIRM YES; EXPORT IMAGE %s %d; SET CONFIRM OFF;" % (quote(out), dpi))
        if shown and shown != n:
            app.executeTextCommand('Electron.run SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % shown)
    finally:
        if j is not None and j != i:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
    if not Path(out).exists():
        sfail('render_failed', 'Fusion did not write %s' % out)
    head = Path(out).read_bytes()[:24]
    import struct
    w, h = struct.unpack('>II', head[16:24])
    return {'success': True, 'mutated': False, **meta, 'sheet': n, 'png': out, 'width': w, 'height': h, 'dpi': dpi,
            'extentMm': [_r4(x0), _r4(y0), _r4(x1), _r4(y1)], 'fusionVersion': app.version}


def sch_zoom(app, args):
    """The standard's sch_zoom: the parts found on their sheet, highlighted (SHOW), the schematic
    view framed on them (WINDOW round their pins, margin mm) and a picture of that view written as
    the proof frame (EXPORT IMAGE ... WINDOW, no foreground). The user's tab is put back."""
    import os
    refs = args.get('refs') or ([args['ref']] if args.get('ref') else [])
    if not refs:
        sfail('bad_args', 'refs is [designator] (or ref)')
    root, meta, tabs = sch_snapshot(app, args)
    i, front = tabs
    found, missing, box, at_sheet = [], [], None, None
    for n_, sh in enumerate(sheets_of(root), start=1):
        pins = pins_with_connections(root, sh, refs, connections=False)
        here = sorted({p_['ref'] for p_ in pins})
        if not here: continue
        if at_sheet is None: at_sheet = n_
        if n_ != at_sheet: continue
        found += here
        for p_ in pins:
            x, y = p_['at']
            box = [x, y, x, y] if box is None else [min(box[0], x), min(box[1], y), max(box[2], x), max(box[3], y)]
        for inst in sh.findall('instances/instance'):
            if inst.get('part') in refs:
                x, y = _sf(inst, 'x'), _sf(inst, 'y')
                box = [min(box[0], x), min(box[1], y), max(box[2], x), max(box[3], y)]
    missing = [r for r in refs if r not in found]
    if not found:
        sfail('designator_not_found', 'None of %s is on the schematic' % ', '.join(refs))
    m = float(args.get('marginMm', 5))
    box = [box[0] - m, box[1] - m, box[2] + m, box[3] + m]
    out = args.get('outputPath') or os.path.join(tempfile.gettempdir(), 'adom-sch-zoom', 'sch_zoom.png')
    out = out.replace('\\', '/'); os.makedirs(os.path.dirname(out), exist_ok=True)
    if os.path.exists(out):
        try: os.remove(out)
        except OSError: pass
    try:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
        if len(sheets_of(root)) > 1:
            app.executeTextCommand('Electron.run SET CONFIRM NO; EDIT .s%d; SET CONFIRM OFF;' % at_sheet)
        app.executeTextCommand('Electron.run SHOW %s;' % ' '.join(quote(r) for r in found))
        app.executeTextCommand('Electron.run WINDOW %s %s;' % (mm((box[0], box[1])), mm((box[2], box[3]))))
        app.executeTextCommand('Electron.ImmediateRepaint /full')
        app.executeTextCommand("Electron.run SET CONFIRM YES; EXPORT IMAGE '%s' WINDOW 150; SET CONFIRM OFF;" % out)
    finally:
        if args.get('stay') is not True and front is not None and front != i:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, front))
    return {'success': True, 'mutated': False, **meta, 'sheet': at_sheet, 'found': found, 'missing': missing,
            'rectMm': [round(v, 3) for v in box], 'image': out if os.path.isfile(out) else None,
            '_hint': 'The parts are highlighted and the schematic view framed on them; image is that view. stay:true leaves the schematic in front. fusion_cross_probe finds the same parts on the board.'}


def export_spice(app, args):
    """The standard's export_spice: EAGLE's own SPICE netlist (EXPORT SPICENETLIST, no dialog) of
    the design's schematic, written on this machine and parsed: device lines, the parts with no
    SPICE mapping (EAGLE writes them with a leading _), analyses, includes and models."""
    import os
    i, doc = sch_doc(app, args)
    out = args.get('outputPath') or os.path.join(tempfile.gettempdir(), 'adom-spice', re.sub(r'[^A-Za-z0-9_.-]', '_', doc.name) + '.net')
    out = out.replace('\\', '/')
    os.makedirs(os.path.dirname(out), exist_ok=True)
    front = app.activeDocument
    docs = [app.documents.item(k) for k in range(app.documents.count)]
    j = docs.index(front) if front in docs else None
    # EAGLE asks "already exists. Overwrite?" for a file that is there: remove it first, and answer
    # yes on the same command line in case it could not be removed (live 2026-10-29: the question was drawn)
    if os.path.exists(out):
        try: os.remove(out)
        except OSError: pass
    try:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
        app.executeTextCommand("Electron.run SET CONFIRM YES; EXPORT SPICENETLIST '%s'; SET CONFIRM OFF;" % out)
    finally:
        if j is not None and j != i:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
    if not os.path.isfile(out):
        sfail('export_failed', 'Fusion did not write the netlist %s' % out)
    text = open(out, 'rb').read().decode('utf-8', 'replace')
    lines = [l.strip() for l in text.splitlines()]
    devices = [l for l in lines if l and l[0] not in '*.+' and not l.startswith('_')]
    unmapped = sorted({l.split()[0][1:] for l in lines if l.startswith('_')})
    analyses = [l for l in lines if re.match(r'\.(op|dc|ac|tran|noise|tf|pz|sens)\b', l, re.I)]
    includes = [l for l in lines if re.match(r'\.(include|lib|model|subckt)\b', l, re.I)]
    return {'success': True, 'mutated': False, 'documentName': doc.name, 'path': out, 'dialect': 'ngspice', 'elements': len(devices),
            'unmapped': unmapped, 'analyses': analyses, 'includes': includes, 'sizeBytes': len(text.encode()),
            '_hint': 'EAGLE writes parts with no SPICE mapping as comments starting with _ (unmapped): give them a model (Fusion\'s ngspice-simulation library, or MAPTOMODEL) before fusion_simulate.'
            if unmapped else 'Every part has a SPICE mapping: fusion_simulate runs it with Fusion\'s bundled ngspice.'}


def handle(app, args, action):
    try:
        if action in EDITS:
            return EDITS[action](app, args)
        if action in ('sch_batch', 'sch_batch_run'):
            return sch_batch(app, args)
        if action == 'export_spice':
            return export_spice(app, args)
        if action == 'sch_zoom':
            return sch_zoom(app, args)
        if action == 'sch_render':
            return sch_render(app, args)
        if action == 'cross_probe':
            return cross_probe(app, args)
        if action == 'cross_check':
            return cross_check(app, args)
        root, meta, _ = sch_snapshot(app, args)
        n, sheet = pick_sheet(root, args) if action not in ('sch_hierarchy', 'schematic_data', 'part_parameters', 'sch_update_from_library') else (None, None)
        base = {'success': True, 'mutated': False, **meta, 'sheet': n, 'sheetCount': len(sheets_of(root))}
        if action == 'schematic_data':
            return {**base, **schematic_data(root, args), '_hint': 'Pins carry the pad numbers their device joins them to (empty for a pin with no pad, such as a supply symbol). Positions are each gate on its sheet, in mm.'}
        if action == 'part_parameters':
            return {**base, **part_parameters(root, args)}
        if action == 'sch_update_from_library':
            plan = sch_update_from_library(root, args)
            if args.get('apply'):
                return {**base, **plan, 'success': False, 'errorCode': 'not_supported',
                        'error': "apply is not supported in Fusion: its UPDATE asks about each part in a dialog, which the bridge never raises. The plan is below.",
                        '_hint': 'Update the parts in Fusion (Library Manager > Update, or right-click a part > Update); then fusion_run_erc.'}
            return {**base, **plan, '_hint': 'A plan: each placed part against the same-named deviceset in the libraries given (gates, pins, symbol drawing, attributes). Fusion updates parts from their library itself (it asks about each one), so apply is refused. droppedWiredPins names pins a new symbol would drop that are wired on the sheet.'}
        if action == 'sch_items':
            return {**base, **items(root, sheet, args.get('kinds')),
                    '_hint': 'Ids are built from each item\'s kind and position (Fusion stores none): pass them to the sch_* edit verbs; an id stops matching once its item moves. Parts are in schematic_data or sch_pins.'}
        if action == 'sch_pins':
            refs = args.get('references') or args.get('refs')
            if isinstance(refs, str):
                refs = [refs]
            pins = pins_with_connections(root, sheet, refs, args.get('connections', True) is not False)
            if refs:
                missing = sorted(set(refs) - {p['ref'] for p in pins})
                if missing and len(missing) == len(set(refs)):
                    sfail('designator_not_found', 'No part %s on sheet %d' % (', '.join(missing), n))
            return {**base, 'pins': pins, 'count': len(pins),
                    '_hint': 'pin is the symbol pin name and number the pad(s) it maps to; at is the connection point (mm, y up); side is where it leaves the body. connectedPins are the pins on the same wire segment.'}
        if action == 'sch_trace_point':
            at = args.get('at')
            if isinstance(at, dict):
                at = [at.get('x'), at.get('y')]
            if not at or len(at) != 2:
                sfail('bad_args', 'at is [x, y] in mm')
            return {**base, **trace_point(root, sheet, at, float(args.get('toleranceMm', 0.01)))}
        if action == 'sch_page':
            if any(k in args for k in ('paper', 'orientation', 'widthMm', 'heightMm', 'title', 'date', 'rev', 'company', 'comments')):
                sfail('unsupported', 'Setting the page is not built yet in the Fusion bridge: this answers the page as it is')
            return {**base, **page(root, sheet), 'changed': False}
        if action == 'sch_sheet_pins':
            if (args.get('action') or 'list') != 'list' or args.get('import'):
                sfail('unsupported', 'Fusion sheet symbols are module instances whose pins are the module ports: add, rename or delete the port in the module (Fusion\'s PORT command); this verb lists and validates')
            out, problems = [], []
            for mi in sheet.findall('moduleinsts/moduleinst'):
                if args.get('sheetSymbol') and args['sheetSymbol'] not in (mi.get('name'), 'sheet:%s' % mi.get('name')):
                    continue
                mod = next((m for m in root.findall('drawing/schematic/modules/module') if m.get('name') == mi.get('module')), None)
                if mod is None:
                    problems.append({'sheetSymbol': mi.get('name'), 'pin': None, 'problem': 'no_module'})
                    continue
                used = {pr.get('port') for _, seg, _ in segments(sheet) for pr in seg.findall('portref') if pr.get('moduleinst') == mi.get('name')}
                for p_ in mod.findall('ports/port'):
                    out.append({'id': 'port:%s:%s' % (mi.get('name'), p_.get('name')), 'sheetSymbol': mi.get('name'), 'name': p_.get('name'),
                                'kind': p_.get('direction', 'io'), 'side': p_.get('side'), 'coord': _sf(p_, 'coord'), 'connected': p_.get('name') in used})
                    if args.get('validate') and p_.get('name') not in used:
                        problems.append({'sheetSymbol': mi.get('name'), 'pin': p_.get('name'), 'problem': 'not_wired'})
            return {**base, 'pins': out, 'added': [], 'problems': problems, 'saved': False}
        if action == 'sch_update_from_library':
            libs = sorted({(l.get('name'), l.get('urn') or l.get('library_urn')) for l in root.findall('drawing/schematic/libraries/library')})
            sfail('unsupported', 'Fusion updates a design\'s parts from their libraries in its Library Manager, which asks about each part: there is no command this verb can answer up front. The design embeds %d libraries (%s)'
                  % (len(libs), ', '.join(n for n, _ in libs)[:300]))
        if action == 'sch_hierarchy':
            if args.get('renumber'):
                sfail('unsupported', 'Fusion numbers sheets by their order; renumbering is reordering, which this verb does not do')
            return {**base, **hierarchy(root)}
        sfail('unknown_action', 'No schematic verb %s' % action)
    except SchError as e:
        return {'success': False, 'errorCode': e.code, 'error': str(e)}
