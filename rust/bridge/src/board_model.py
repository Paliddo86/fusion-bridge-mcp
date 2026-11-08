"""The EDA Bridge Standard board model from Fusion's own EAGLE board XML.

Embedded by routing.rs beside routing_script.py and run on Fusion's main thread; stdlib only, so
tests/test_board_model.py runs the same code offline on .brd files. Schema:
wiki.adom.inc/adom/eda-bridge-standard (schema/board.schema.json, schema/companions.schema.json).

Millimetres, Fusion's board frame (y up), absolute. Copper layers carry KiCad's names (F.Cu,
In<n>.Cu by stack order, B.Cu, *.Cu); other layers keep EAGLE's names.
"""
import hashlib
import json
import math
import re

MODEL = 'eda-bridge-standard/board 0.1'

# EAGLE's fixed layer numbers for everything that is not copper (copper is 1..16)
EAGLE_LAYERS = {17: 'Pads', 18: 'Vias', 19: 'Unrouted', 20: 'Dimension', 21: 'tPlace', 22: 'bPlace',
                23: 'tOrigins', 24: 'bOrigins', 25: 'tNames', 26: 'bNames', 27: 'tValues', 28: 'bValues',
                29: 'tStop', 30: 'bStop', 31: 'tCream', 32: 'bCream', 33: 'tFinish', 34: 'bFinish',
                35: 'tGlue', 36: 'bGlue', 37: 'tTest', 38: 'bTest', 39: 'tKeepout', 40: 'bKeepout',
                41: 'tRestrict', 42: 'bRestrict', 43: 'vRestrict', 44: 'Drills', 45: 'Holes', 46: 'Milling',
                47: 'Measures', 48: 'Document', 49: 'Reference', 51: 'tDocu', 52: 'bDocu'}
COPPER = set(range(1, 17))
EPS = 0.001          # copper closer than this touches
ARC_STEP = 2.0       # degrees: sample arcs this finely before measuring (the standard's rule)


def _f(node, key, default=0.0):
    try:
        return float(node.get(key, default))
    except (TypeError, ValueError):
        return default


def _deg(rot):
    m = re.search(r'R(-?[\d.]+)', rot or 'R0')
    return float(m.group(1)) if m else 0.0


def _norm(a):
    a = a % 360.0
    return 0.0 if abs(a - 360.0) < 1e-9 else round(a, 6)


# ---------------------------------------------------------------- layers

def copper_stack(board):
    """Copper layer numbers top to bottom. Fusion's layer stack (layerstackup) when the board has
    one, else designrules layerSetup ("(1*16)", "(1+2*15+16)", "[2:1+2*15+16:15]")."""
    ls = board.find('.//layerstackup')
    if ls is not None:
        order = [int(d.get('layer')) for d in ls.findall('layerdef') if d.get('type') in ('Signal', 'Plane')]
        if order:
            return order
    node = board.find("designrules/param[@name='layerSetup']")
    setup = node.get('value', '(1*16)') if node is not None else '(1*16)'
    setup = re.sub(r'\[\d+:', '[', re.sub(r':\d+\]', ']', setup))     # blind/buried bounds are not layers
    order = []
    for n in re.findall(r'\d+', setup):
        if int(n) not in order:
            order.append(int(n))
    return order or [1, 16]


class Layers:
    def __init__(self, board):
        self.order = copper_stack(board)
        inner = [n for n in self.order if n not in (1, 16)]
        self.names = {1: 'F.Cu', 16: 'B.Cu'}
        for i, n in enumerate(inner):
            self.names[n] = 'In%d.Cu' % (i + 1)

    def name(self, n):
        n = int(n)
        if n in self.names:
            return self.names[n]
        if n in COPPER:          # a copper layer the stack does not list (unused): by its number
            return 'In%d.Cu' % (n - 1)
        return EAGLE_LAYERS.get(n, 'L%d' % n)

    def span(self, extent):
        """The copper layers a via of this extent ("1-16", "1-2") passes through, as numbers."""
        try:
            a, b = (int(x) for x in (extent or '1-16').split('-'))
        except ValueError:
            a, b = 1, 16
        order = self.order if a in self.order and b in self.order else sorted(COPPER)
        i, j = sorted((order.index(a), order.index(b)))
        return order[i:j + 1]


# ---------------------------------------------------------------- transforms and rules

def placement(el):
    """The element's transform: local (x, y) -> board (x, y), its angle and whether it is mirrored.
    Rotate, then mirror x: the same order the route planner's pad model has always used."""
    rot = el.get('rot', 'R0')
    mirrored = 'M' in rot
    a = math.radians(_deg(rot))
    ex, ey = _f(el, 'x'), _f(el, 'y')
    c, s = math.cos(a), math.sin(a)

    def xf(x, y):
        x, y = x * c - y * s, x * s + y * c
        if mirrored:
            x = -x
        return (x + ex, y + ey)
    return xf, _deg(rot), mirrored


def abs_rotation(el_angle, mirrored, local):
    return _norm(-(el_angle + local) if mirrored else el_angle + local)


def _rule_mm(board, name, default):
    node = board.find("designrules/param[@name='%s']" % name)
    m = re.fullmatch(r'([0-9.]+)(mm|mil|in|mic)', (node.get('value', '') if node is not None else '').strip())
    if not m:
        return default
    return float(m[1]) * {'mm': 1, 'mil': 0.0254, 'in': 25.4, 'mic': 0.001}[m[2]]


def _rule_num(board, name, default):
    node = board.find("designrules/param[@name='%s']" % name)
    try:
        return float(node.get('value')) if node is not None else default
    except (TypeError, ValueError):
        return default


def pad_diameter(board, drill, lib_diameter):
    """A through pad's copper: its library diameter or the drill plus the board's restring,
    whichever is larger (Fusion's rule; the route planner uses the same)."""
    ring = min(max(drill * _rule_num(board, 'rvPadTop', 0.25), _rule_mm(board, 'rlMinPadTop', 0.254)),
               _rule_mm(board, 'rlMaxPadTop', 0.508))
    return max(lib_diameter, drill + 2 * ring)


def via_diameter(board, drill, diameter):
    ring = min(max(drill * _rule_num(board, 'rvViaOuter', 0.25), _rule_mm(board, 'rlMinViaOuter', 0.2032)),
               _rule_mm(board, 'rlMaxViaOuter', 0.508))
    return max(diameter, drill + 2 * ring)


# ---------------------------------------------------------------- geometry

def arc_of(p1, p2, curve):
    """An EAGLE wire p1 -> p2 sweeping `curve` degrees (positive counterclockwise) as center,
    radius and the counterclockwise angle range (angle1 -> angle2, angle2 - angle1 = |curve|)."""
    c = math.radians(curve)
    dx, dy = p2[0] - p1[0], p2[1] - p1[1]
    L = math.hypot(dx, dy)
    r = L / (2 * abs(math.sin(c / 2)))
    d = (L / 2) / math.tan(c / 2)
    cx, cy = (p1[0] + p2[0]) / 2 - dy / L * d, (p1[1] + p2[1]) / 2 + dx / L * d
    a1 = math.degrees(math.atan2(p1[1] - cy, p1[0] - cx))
    start = a1 if curve > 0 else a1 + curve
    start = _norm(start)
    return (cx, cy), r, start, round(start + abs(curve), 6)


def arc_samples(center, r, a_from, sweep, step=ARC_STEP):
    """Points along an arc from angle a_from sweeping `sweep` degrees (signed), at most `step` apart."""
    n = max(1, int(math.ceil(abs(sweep) / step)))
    return [[center[0] + r * math.cos(math.radians(a_from + sweep * k / n)),
             center[1] + r * math.sin(math.radians(a_from + sweep * k / n))] for k in range(n + 1)]


def wire_points(p1, p2, curve):
    if not curve:
        return [list(p1), list(p2)]
    center, r, _, _ = arc_of(p1, p2, curve)
    a = math.degrees(math.atan2(p1[1] - center[1], p1[0] - center[0]))
    pts = arc_samples(center, r, a, curve)
    pts[0], pts[-1] = list(p1), list(p2)
    return pts


def vertex_ring(vertices):
    """A closed EAGLE vertex list (a vertex's curve bends the edge to the next) as points."""
    vs = [((_f(v, 'x'), _f(v, 'y')), _f(v, 'curve')) for v in vertices]
    out = []
    for i, (p, curve) in enumerate(vs):
        q = vs[(i + 1) % len(vs)][0]
        out += wire_points(p, q, curve)[:-1] if curve and p != q else [list(p)]
    return out


def shape_vertices(node):
    """A polygon's own vertices: <polygon> holds them directly, Fusion's <polygonshape> and
    <polygonpour> inside <polygonoutlineobjects>."""
    inner = node.find('polygonoutlineobjects')
    return (inner if inner is not None else node).findall('vertex')


def area(pts):
    return abs(sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(pts, pts[1:] + pts[:1]))) / 2


def _r(v):
    return round(v, 6)


def pt_seg(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    t = max(0, min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy))) if dx or dy else 0
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def seg_seg(a, b, c, d):
    def cross(p, q, r):
        return (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    if cross(a, b, c) * cross(a, b, d) < 0 and cross(c, d, a) * cross(c, d, b) < 0:
        return 0.0
    return min(pt_seg(a, c, d), pt_seg(b, c, d), pt_seg(c, a, b), pt_seg(d, a, b))


class Region:
    """A polygon with holes, indexed for touch tests: its edges bucketed in a grid (distance) and
    in horizontal bands (point in polygon), so a 3000-vertex pour costs a few edges per query."""

    def __init__(self, outline, holes=()):
        self.outline = [tuple(v) for v in outline]
        self.rings = [self.outline] + [[tuple(v) for v in h] for h in holes]
        self.edges = [(a, b) for ring in self.rings for a, b in zip(ring, ring[1:] + ring[:1])]
        xs = [v[0] for v in self.outline]
        ys = [v[1] for v in self.outline]
        self.bbox = (min(xs), min(ys), max(xs), max(ys))
        span = max(self.bbox[2] - self.bbox[0], self.bbox[3] - self.bbox[1], 1e-6)
        self.cell = max(span / 64.0, 0.25) if len(self.edges) > 32 else span * 2
        self.grid, self.bands = {}, {}
        for k, (a, b) in enumerate(self.edges):
            i0, i1 = sorted((self._c(a[0]), self._c(b[0])))
            j0, j1 = sorted((self._c(a[1]), self._c(b[1])))
            for j in range(j0, j1 + 1):
                self.bands.setdefault(j, []).append(k)
                for i in range(i0, i1 + 1):
                    self.grid.setdefault((i, j), []).append(k)

    def _c(self, v):
        return int(math.floor(v / self.cell))

    def contains(self, p):
        """Inside the outline and outside every hole: an odd number of crossings over all rings."""
        if not (self.bbox[0] <= p[0] <= self.bbox[2] and self.bbox[1] <= p[1] <= self.bbox[3]):
            return False
        inside = False
        for k in self.bands.get(self._c(p[1]), ()):
            a, b = self.edges[k]
            if (a[1] > p[1]) != (b[1] > p[1]) and p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]):
                inside = not inside
        return inside

    def near(self, box):
        """The edges whose grid cells meet a bounding box."""
        out = set()
        for i in range(self._c(box[0]), self._c(box[2]) + 1):
            for j in range(self._c(box[1]), self._c(box[3]) + 1):
                out.update(self.grid.get((i, j), ()))
        return [self.edges[k] for k in out]

    def vertices_in(self, box):
        return [a for a, _ in self.near(box) if box[0] <= a[0] <= box[2] and box[1] <= a[1] <= box[3]]


def poly(outline, holes=()):
    return ('poly', Region(outline, holes))


FAR = 1e9


def prim_distance(p, q):
    """Edge-to-edge distance of two primitives, a fat segment (seg, a, b, radius) or a polygon
    (poly, Region): 0 or less when they overlap. Exact up to EPS; past that it may answer FAR
    (a region's index only looks at nearby edges), which is all a touch test needs."""
    if p[0] == 'poly' and q[0] == 'seg':
        p, q = q, p
    if p[0] == 'seg' and q[0] == 'seg':
        return seg_seg(p[1], p[2], q[1], q[2]) - p[3] - q[3]
    if p[0] == 'seg':
        _, a, b, r = p
        reg = q[1]
        if reg.contains(a) or reg.contains(b):
            return -r
        box = (min(a[0], b[0]) - r - EPS, min(a[1], b[1]) - r - EPS, max(a[0], b[0]) + r + EPS, max(a[1], b[1]) + r + EPS)
        near = reg.near(box)
        return (min(seg_seg(a, b, c, d) for c, d in near) - r) if near else FAR
    r1, r2 = p[1], q[1]
    box = (max(r1.bbox[0], r2.bbox[0]) - EPS, max(r1.bbox[1], r2.bbox[1]) - EPS,
           min(r1.bbox[2], r2.bbox[2]) + EPS, min(r1.bbox[3], r2.bbox[3]) + EPS)
    if box[0] > box[2] or box[1] > box[3]:
        return FAR
    if any(r2.contains(v) for v in r1.vertices_in(box)) or any(r1.contains(v) for v in r2.vertices_in(box)):
        return 0.0
    e1, e2 = r1.near(box), r2.near(box)
    return min((seg_seg(a, b, c, d) for a, b in e1 for c, d in e2), default=FAR)


def prim_bbox(p):
    if p[0] == 'seg':
        _, a, b, r = p
        return (min(a[0], b[0]) - r, min(a[1], b[1]) - r, max(a[0], b[0]) + r, max(a[1], b[1]) + r)
    return p[1].bbox


def rect_corners(cx, cy, sx, sy, rot):
    a = math.radians(rot)
    c, s = math.cos(a), math.sin(a)
    return [[cx + x * c - y * s, cy + x * s + y * c] for x, y in
            ((-sx / 2, -sy / 2), (sx / 2, -sy / 2), (sx / 2, sy / 2), (-sx / 2, sy / 2))]


def pad_prims(pad):
    """A pad as touch primitives: a circle or stadium is a fat segment, everything else its polygon."""
    x, y, sx, sy, rot = pad['x'], pad['y'], pad['sizeX'], pad['sizeY'], pad['rotation']
    if pad['shape'] == 'round':
        long_ = max(sx, sy) - min(sx, sy)
        a = math.radians(rot + (0 if sx >= sy else 90))
        dx, dy = math.cos(a) * long_ / 2, math.sin(a) * long_ / 2
        return [('seg', (x - dx, y - dy), (x + dx, y + dy), min(sx, sy) / 2)]
    if pad['shape'] == 'octagonal':
        k = min(sx, sy) * (1 - 1 / (1 + math.sqrt(2)))  # an octagon's corner cut
        a = math.radians(rot)
        c, s = math.cos(a), math.sin(a)
        pts = [(-sx / 2 + k / 2, -sy / 2), (sx / 2 - k / 2, -sy / 2), (sx / 2, -sy / 2 + k / 2), (sx / 2, sy / 2 - k / 2),
               (sx / 2 - k / 2, sy / 2), (-sx / 2 + k / 2, sy / 2), (-sx / 2, sy / 2 - k / 2), (-sx / 2, -sy / 2 + k / 2)]
        return [poly([[x + px * c - py * s, y + px * s + py * c] for px, py in pts])]
    return [poly(rect_corners(x, y, sx, sy, rot))]


# ---------------------------------------------------------------- the model

def pads_of(board, layers, contacts):
    """Every pad: the standard row (ref, name, x, y, sizeX, sizeY, rotation, layer, drill, net,
    shape) keyed by 'REF.PAD', plus the footprint copper that is not a pad (custom pad shapes are
    folded into their pad as its bounding rectangle, the rest is net-less copper)."""
    packages = {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library')
                for p in l.findall('packages/package')}
    pads, extra_tracks, extra_polys, holes = {}, [], [], []
    for el in board.findall('elements/element'):
        pkg = packages.get((el.get('library'), el.get('package')))
        if pkg is None:
            continue
        ref = el.get('name')
        xf, angle, mirrored = placement(el)
        own = []
        for pad in pkg.findall('pad'):
            drill = _f(pad, 'drill')
            d = pad_diameter(board, drill, _f(pad, 'diameter'))
            shape = pad.get('shape', 'round')
            prot = _deg(pad.get('rot'))
            lx, ly = _f(pad, 'x'), _f(pad, 'y')
            sx = sy = d
            kind = {'square': 'rect', 'octagon': 'octagonal'}.get(shape, 'round')
            if shape in ('long', 'offset'):
                sx = d * (1 + _f(pad, 'elongation', 100) / 100.0)
                if shape == 'offset':        # the drill sits at one end: the copper's center moves
                    lx += math.cos(math.radians(prot)) * (sx - d) / 2
                    ly += math.sin(math.radians(prot)) * (sx - d) / 2
            cx, cy = xf(lx, ly)
            hx, hy = xf(_f(pad, 'x'), _f(pad, 'y'))
            row = {'ref': ref, 'name': pad.get('name'), 'x': _r(cx), 'y': _r(cy), 'sizeX': _r(sx), 'sizeY': _r(sy),
                   'rotation': abs_rotation(angle, mirrored, prot), 'layer': '*.Cu', 'drill': _r(drill),
                   'net': contacts.get((ref, pad.get('name')), ''), 'shape': kind}
            if (cx, cy) != (hx, hy):
                row['holeAt'] = [_r(hx), _r(hy)]
            pads[ref + '.' + pad.get('name')] = row
            own.append(row)
        for smd in pkg.findall('smd'):
            cx, cy = xf(_f(smd, 'x'), _f(smd, 'y'))
            dx, dy, round_ = _f(smd, 'dx'), _f(smd, 'dy'), _f(smd, 'roundness')
            layer = int(smd.get('layer', '1'))
            layer = 17 - layer if mirrored and layer in (1, 16) else layer
            kind = 'rect' if round_ <= 0 else ('round' if round_ >= 100 else 'roundrect')
            row = {'ref': ref, 'name': smd.get('name'), 'x': _r(cx), 'y': _r(cy), 'sizeX': _r(dx), 'sizeY': _r(dy),
                   'rotation': abs_rotation(angle, mirrored, _deg(smd.get('rot'))), 'layer': layers.name(layer),
                   'layerId': layer, 'drill': 0, 'net': contacts.get((ref, smd.get('name')), ''), 'shape': kind}
            if kind == 'roundrect':
                row['roundness'] = round_
            pads[ref + '.' + smd.get('name')] = row
            own.append(row)
        # footprint copper that is not a pad: a polygon or rectangle on a pad is that pad's custom
        # shape (reported as the bounding rectangle of both); anything else is free copper
        for node in pkg:
            if node.tag not in ('polygon', 'polygonshape', 'rectangle', 'wire', 'circle') or int(node.get('layer', '0')) not in (1, 16):
                continue
            layer = int(node.get('layer'))
            layer = 17 - layer if mirrored else layer
            if node.tag == 'wire':
                pts = wire_points((_f(node, 'x1'), _f(node, 'y1')), (_f(node, 'x2'), _f(node, 'y2')), _f(node, 'curve'))
                pts = [list(xf(*p)) for p in pts]
                net = next((p['net'] for p in own if p['layer'] in (layers.name(layer), '*.Cu') and
                            any(prim_distance(('seg', a, b, _f(node, 'width') / 2), q) <= EPS
                                for a, b in zip(pts, pts[1:]) for q in pad_prims(p))), '')
                for a, b in zip(pts, pts[1:]):
                    extra_tracks.append({'start': {'x': _r(a[0]), 'y': _r(a[1])}, 'end': {'x': _r(b[0]), 'y': _r(b[1])},
                                         'width': _f(node, 'width'), 'layer': layers.name(layer), 'layerId': layer,
                                         'net': net, 'ref': ref})
                continue
            if node.tag in ('polygon', 'polygonshape'):
                ring = [list(xf(*p)) for p in vertex_ring(shape_vertices(node))]
            elif node.tag == 'rectangle':
                x1, y1, x2, y2 = _f(node, 'x1'), _f(node, 'y1'), _f(node, 'x2'), _f(node, 'y2')
                cx0, cy0 = (x1 + x2) / 2, (y1 + y2) / 2
                ring = [list(xf(*p)) for p in rect_corners(cx0, cy0, abs(x2 - x1), abs(y2 - y1), _deg(node.get('rot')))]
            else:
                ring = [list(xf(*p)) for p in arc_samples((_f(node, 'x'), _f(node, 'y')), _f(node, 'radius'), 0, 360)[:-1]]
            hit = [p for p in own if p['layer'] == layers.name(layer) and
                   any(prim_distance(poly(ring), q) <= EPS for q in pad_prims(p))]
            # a shape shared by several pads (a power FET's drain: pads 5..9 joined by one
            # polygon) becomes part of each of them
            for p in hit:
                corners = rect_corners(p['x'], p['y'], p['sizeX'], p['sizeY'], p['rotation'])
                xs = [v[0] for v in ring] + [c[0] for c in corners]
                ys = [v[1] for v in ring] + [c[1] for c in corners]
                p.update({'x': _r((min(xs) + max(xs)) / 2), 'y': _r((min(ys) + max(ys)) / 2), 'sizeX': _r(max(xs) - min(xs)),
                          'sizeY': _r(max(ys) - min(ys)), 'rotation': 0, 'shape': 'rect', 'customShape': True})
                ring_r = [[_r(a), _r(b)] for a, b in ring]
                # the standard's copperPoints is one ring: the largest drawn polygon on the pad; a
                # pad that gathers several also lists them all under copperOutlines
                if 'copperPoints' not in p or area(ring_r) > area(p['copperPoints']):
                    if 'copperPoints' in p:
                        p.setdefault('copperOutlines', [p['copperPoints']])
                    p['copperPoints'] = ring_r
                if 'copperOutlines' in p:
                    p['copperOutlines'].append(ring_r)
            if not hit:
                extra_polys.append({'net': '', 'layer': layers.name(layer), 'layerId': layer,
                                    'points': [[_r(a), _r(b)] for a, b in ring], 'ref': ref, 'kind': 'graphic', 'source': 'footprint', 'pour': 'drawn'})
        for h in pkg.findall('hole'):
            x, y = xf(_f(h, 'x'), _f(h, 'y'))
            holes.append({'ref': ref, 'x': _r(x), 'y': _r(y), 'drill': _f(h, 'drill')})
    return pads, extra_tracks, extra_polys, holes


def outline_edges(board):
    """The board outline (layer 20 Dimension: the board's own lines, arcs and circles, and those
    inside footprints) as closed loops of edges, largest first. Returns (loops, open_ends)."""
    raw = []

    def add(p1, p2, curve):
        if curve:
            center, r, a1, a2 = arc_of(p1, p2, curve)
            raw.append({'kind': 'arc', 'start': [_r(p1[0]), _r(p1[1])], 'end': [_r(p2[0]), _r(p2[1])],
                        'center': [_r(center[0]), _r(center[1])], 'radius': _r(r), 'angle1': a1, 'angle2': a2,
                        '_ccw': curve > 0})
        else:
            raw.append({'kind': 'line', 'start': [_r(p1[0]), _r(p1[1])], 'end': [_r(p2[0]), _r(p2[1])]})

    def circle(c, r):
        raw.append({'kind': 'arc', 'start': [_r(c[0] + r), _r(c[1])], 'end': [_r(c[0] + r), _r(c[1])],
                    'center': [_r(c[0]), _r(c[1])], 'radius': _r(r), 'angle1': 0, 'angle2': 360, '_ccw': True})

    for w in board.findall("plain/wire[@layer='20']"):
        add((_f(w, 'x1'), _f(w, 'y1')), (_f(w, 'x2'), _f(w, 'y2')), _f(w, 'curve'))
    for c in board.findall("plain/circle[@layer='20']"):
        circle((_f(c, 'x'), _f(c, 'y')), _f(c, 'radius'))
    packages = {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library')
                for p in l.findall('packages/package')}
    for el in board.findall('elements/element'):
        pkg = packages.get((el.get('library'), el.get('package')))
        if pkg is None:
            continue
        xf, angle, mirrored = placement(el)
        for w in pkg.findall("wire[@layer='20']"):
            curve = _f(w, 'curve')
            add(xf(_f(w, 'x1'), _f(w, 'y1')), xf(_f(w, 'x2'), _f(w, 'y2')), -curve if mirrored else curve)
        for c in pkg.findall("circle[@layer='20']"):
            circle(xf(_f(c, 'x'), _f(c, 'y')), _f(c, 'radius'))

    near = lambda p, q: abs(p[0] - q[0]) < 1e-3 and abs(p[1] - q[1]) < 1e-3

    def flip(e):
        e = dict(e)
        e['start'], e['end'] = e['end'], e['start']
        if e['kind'] == 'arc':
            e['_ccw'] = not e['_ccw']
        return e

    loops, open_ends = [], []
    while raw:
        loop = [raw.pop(0)]
        while not near(loop[0]['start'], loop[-1]['end']):
            for i, e in enumerate(raw):
                if near(e['start'], loop[-1]['end']):
                    loop.append(raw.pop(i))
                    break
                if near(e['end'], loop[-1]['end']):
                    loop.append(flip(raw.pop(i)))
                    break
            else:
                open_ends.append(loop[-1]['end'])
                break
        loops.append(loop)
    loops.sort(key=lambda l: area(loop_points(l)), reverse=True)
    return loops, open_ends


def loop_points(loop):
    """A loop's vertices in order, arcs sampled every 2 degrees."""
    pts = []
    for e in loop:
        if e['kind'] == 'line':
            pts.append(list(e['start']))
            continue
        sweep = e['angle2'] - e['angle1']
        a_from = math.degrees(math.atan2(e['start'][1] - e['center'][1], e['start'][0] - e['center'][0]))
        pts += [[_r(x), _r(y)] for x, y in arc_samples(e['center'], e['radius'], a_from, sweep if e['_ccw'] else -sweep)[:-1]]
    return pts


def public_edge(e):
    return {k: v for k, v in e.items() if not k.startswith('_')}


def board_model(board, meta=None):
    """The board model as the standard's schema/board.schema.json defines it."""
    layers = Layers(board)
    contacts = {(c.get('element'), c.get('pad')): s.get('name')
                for s in board.findall('signals/signal') for c in s.findall('contactref')}
    pads, fp_tracks, fp_polys, holes = pads_of(board, layers, contacts)
    tracks, arcs, vias, polygons = [], [], [], []

    def wire(w, net):
        layer = int(w.get('layer'))
        p1, p2, curve = (_f(w, 'x1'), _f(w, 'y1')), (_f(w, 'x2'), _f(w, 'y2')), _f(w, 'curve')
        if curve and p1 != p2:
            center, r, a1, a2 = arc_of(p1, p2, curve)
            arcs.append({'center': {'x': _r(center[0]), 'y': _r(center[1])}, 'radius': _r(r), 'startAngle': a1,
                         'endAngle': a2, 'width': _f(w, 'width'), 'layer': layers.name(layer), 'layerId': layer, 'net': net})
        else:
            # the handle edit_tracks takes: Fusion stores no ids, so it is the track's own net, layer and ends
            tracks.append({'id': 'track:%s:%d:%g,%g,%g,%g' % (net, layer, _r(p1[0]), _r(p1[1]), _r(p2[0]), _r(p2[1])),
                           'start': {'x': _r(p1[0]), 'y': _r(p1[1])}, 'end': {'x': _r(p2[0]), 'y': _r(p2[1])},
                           'width': _f(w, 'width'), 'layer': layers.name(layer), 'layerId': layer, 'net': net})

    def pour(p, net):
        layer = int(p.get('layer', '1'))
        ring = vertex_ring(shape_vertices(p))
        polygons.append({'net': net, 'layer': layers.name(layer), 'layerId': layer, 'points': [[_r(a), _r(b)] for a, b in ring],
                         'kind': 'zone', 'pour': p.get('pour', 'solid'), 'isolate': _f(p, 'isolate'), 'rank': int(p.get('rank', '1')),
                         'width': _f(p, 'width')})

    for s in board.findall('signals/signal'):
        net = s.get('name')
        for w in s.findall('wire'):
            if int(w.get('layer')) in COPPER:
                wire(w, net)
        for v in s.findall('via'):
            drill = _f(v, 'drill')
            span = layers.span(v.get('extent'))
            vias.append({'position': {'x': _f(v, 'x'), 'y': _f(v, 'y')}, 'size': _r(via_diameter(board, drill, _f(v, 'diameter'))),
                         'drill': drill, 'layers': [layers.name(span[0]), layers.name(span[-1])], 'net': net})
        for p in list(s.findall('polygonpour')) + list(s.findall('polygon')):
            if int(p.get('layer', '1')) in COPPER:
                pour(p, net)
    # free copper drawn on the board itself has no net: kept, with an empty net
    for w in board.findall('plain/wire'):
        if int(w.get('layer', '0')) in COPPER:
            wire(w, '')
    for p in board.findall('plain/polygon') + board.findall('plain/polygonshape'):
        if int(p.get('layer', '0')) in COPPER:
            pour(p, '')
            polygons[-1]['pour'] = 'drawn'
            polygons[-1]['kind'] = 'graphic'
    for rct in board.findall('plain/rectangle'):
        layer = int(rct.get('layer', '0'))
        if layer in COPPER:
            x1, y1, x2, y2 = _f(rct, 'x1'), _f(rct, 'y1'), _f(rct, 'x2'), _f(rct, 'y2')
            ring = rect_corners((x1 + x2) / 2, (y1 + y2) / 2, abs(x2 - x1), abs(y2 - y1), _deg(rct.get('rot')))
            polygons.append({'net': '', 'layer': layers.name(layer), 'layerId': layer, 'points': [[_r(a), _r(b)] for a, b in ring], 'kind': 'graphic', 'pour': 'drawn'})
    for h in board.findall('plain/hole'):
        holes.append({'ref': '', 'x': _f(h, 'x'), 'y': _f(h, 'y'), 'drill': _f(h, 'drill')})
    tracks += fp_tracks
    polygons += fp_polys

    components = []
    for el in board.findall('elements/element'):
        rot = el.get('rot', 'R0')
        components.append({'ref': el.get('name'), 'x': _f(el, 'x'), 'y': _f(el, 'y'), 'rotation': _norm(_deg(rot)),
                           'side': 'bottom' if 'M' in rot else 'top', 'pattern': el.get('package'),
                           'has3d': bool(el.get('package3d_urn')), 'value': el.get('value'), 'library': el.get('library'),
                           'locked': el.get('locked') == 'yes'})

    loops, open_ends = outline_edges(board)
    outer = loops[0] if loops else []
    model = {
        'model': MODEL, 'units': 'mm', 'file': (meta or {}).get('file'), 'boardId': (meta or {}).get('boardId'),
        'revision': (meta or {}).get('revision') or board_revision(board),
        'copperLayers': [layers.name(n) for n in layers.order],
        'tracks': tracks, 'arcs': arcs, 'vias': vias, 'pads': list(pads.values()), 'components': components,
        'polygons': polygons, 'holes': holes,
        'outline': loop_points(outer), 'outlineEdges': [public_edge(e) for e in outer],
        'outlineCutouts': [{'outline': loop_points(l), 'edges': [public_edge(e) for e in l]} for l in loops[1:]],
        'outlineClosed': not open_ends,
    }
    if open_ends:
        model['outlineProblem'] = 'the layer-20 outline is not closed: an edge ends at %s with nothing joined to it' % open_ends
    model['nets'] = sorted({s.get('name') for s in board.findall('signals/signal') if s.get('name')})
    unconnected, split, floating = connectivity(board, model, layers)
    model['unconnected'] = unconnected
    model['splitNets'] = split
    model['isolatedCopper'] = floating[:200]   # the standard's key (0.1.1): poured copper reaching nothing else
    if len(floating) > 200:
        model['isolatedCopperCount'] = len(floating)
    if floating:      # the older name, kept for callers that read it
        model['floatingPourIslandCount'] = len(floating)
        model['floatingPourIslands'] = [{'net': f['net'], 'layer': f['layer'], 'at': [f['at']['x'], f['at']['y']]} for f in floating[:50]]
    model['counts'] = {k: len(model[k]) for k in ('tracks', 'arcs', 'vias', 'pads', 'components', 'polygons', 'nets')}
    return model, pads


def pour_islands(board, layers):
    """Fusion's own fill, island by island: (net, layer number, outline, holes)."""
    out = []
    for s in board.findall('signals/signal'):
        for p in list(s.findall('polygonpour')) + list(s.findall('polygon')):
            fd = p.find('polygonfilldetails')
            if fd is None:
                continue
            for sh in fd.findall('polygonshape'):
                seg = sh.find('polygonoutlinesegments')
                if seg is None:
                    continue
                holes = [vertex_ring(h.findall('vertex')) for h in sh.iter('polygonholesegments')]
                out.append((s.get('name'), int(sh.get('layer', p.get('layer', '1'))), vertex_ring(seg.findall('vertex')), holes))
    return out


def connectivity(board, model, layers):
    """`unconnected` from the copper itself: per net, the pieces of copper (pads, tracks, arcs, vias,
    Fusion's poured fill; a pour Fusion never filled counts as its drawn outline) joined where they
    touch on a shared layer. Every group beyond the first is a connection still to make, as on
    Altium. A group that is only poured copper (an island, or a pour that reaches no pad) is dead
    copper, not a missing connection: it is listed under isolatedCopper instead (the EDA Bridge
    Standard 0.1.1 rule, KiCad's name); a stray track or via still counts."""
    name_to_num = {v: k for k, v in layers.names.items()}
    all_cu = set(layers.order)

    def lset(name):
        if name == '*.Cu':
            return all_cu
        return {name_to_num.get(name, -1)}
    items = {}

    def add(net, prims, ls, kind):
        if net:
            items.setdefault(net, []).append({'prims': prims, 'layers': ls, 'kind': kind,
                                              'bbox': tuple(f(prim_bbox(p)[i] for p in prims) for i, f in enumerate((min, min, max, max)))})
    for p in model['pads']:
        add(p['net'], pad_prims(p), lset(p['layer']), 'pad')
    for t in model['tracks']:
        add(t['net'], [('seg', (t['start']['x'], t['start']['y']), (t['end']['x'], t['end']['y']), t['width'] / 2)], {t['layerId']}, 'track')
    for a in model['arcs']:
        pts = arc_samples((a['center']['x'], a['center']['y']), a['radius'], a['startAngle'], a['endAngle'] - a['startAngle'])
        add(a['net'], [('seg', p, q, a['width'] / 2) for p, q in zip(pts, pts[1:])], {a['layerId']}, 'arc')
    for v in model['vias']:
        c = (v['position']['x'], v['position']['y'])
        span = set(layers.order[layers.order.index(name_to_num[v['layers'][0]]):layers.order.index(name_to_num[v['layers'][1]]) + 1]) \
            if all(l in name_to_num for l in v['layers']) else all_cu
        add(v['net'], [('seg', c, c, v['size'] / 2)], span, 'via')
    filled = set()
    for net, layer, outline, holes in pour_islands(board, layers):
        if len(outline) >= 3:
            add(net, [poly(outline, holes)], {layer}, 'fill')
            filled.add((net, layer))
    for g in model['polygons']:
        if g['net'] and (g['net'], g['layerId']) not in filled and len(g['points']) >= 3 and g.get('pour') != 'drawn':
            add(g['net'], [poly(g['points'])], {g['layerId']}, 'pour outline')
        elif g['net'] and g.get('pour') == 'drawn':
            add(g['net'], [poly(g['points'])], {g['layerId']}, 'copper')
    total, split, floating = 0, [], []
    for net, its in sorted(items.items()):
        n = len(its)
        parent = list(range(n))

        def find(x):
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x
        for i in range(n):
            a = its[i]
            for j in range(i + 1, n):
                b = its[j]
                if not (a['layers'] & b['layers']):
                    continue
                if a['bbox'][0] > b['bbox'][2] + EPS or b['bbox'][0] > a['bbox'][2] + EPS or \
                   a['bbox'][1] > b['bbox'][3] + EPS or b['bbox'][1] > a['bbox'][3] + EPS:
                    continue
                if find(i) == find(j):
                    continue
                if any(prim_distance(p, q) <= EPS for p in a['prims'] for q in b['prims']):
                    parent[find(i)] = find(j)
        groups = {}
        for i in range(n):
            groups.setdefault(find(i), []).append(its[i])
        live = []
        for g in groups.values():
            # only poured copper (an island, or a pour that reaches no pad): isolated copper, listed
            # and not counted (EDA Bridge Standard 0.1.1); a stray track or via still counts
            if all(x['kind'] in ('fill', 'pour outline') for x in g):
                region = g[0]['prims'][0][1]
                area_ = sum(area(list(x['prims'][0][1].rings[0])) - sum(area(list(h)) for h in x['prims'][0][1].rings[1:]) for x in g)
                floating.append({'net': net, 'layer': layers.name(min(g[0]['layers'])),
                                 'at': {'x': _r(region.outline[0][0]), 'y': _r(region.outline[0][1])}, 'areaMm2': round(area_, 4)})
            else:
                live.append(g)
        if len(live) > 1:
            total += len(live) - 1
            split.append({'net': net, 'groups': len(live)})
    return total, split, floating


def board_revision(board):
    """A hash of the copper, the parts and the outline, and nothing else: the edit verbs' guard.
    Silkscreen text, rules, views and Fusion's own ratsnest (layer 19) leave it alone."""
    def canonical(node):
        if node.tag == 'wire' and node.get('layer') == '19':
            return None
        if node.tag == 'attribute':      # a part's name/value text placement is silkscreen
            return None
        return [node.tag, sorted(node.attrib.items()), (node.text or '').strip(),
                [v for ch in node if (v := canonical(ch)) is not None]]
    used = {(e.get('library'), e.get('package')) for e in board.findall('elements/element')}
    parts = [canonical(s) for s in board.findall('signals/signal')]
    parts += [canonical(e) for e in board.findall('elements/element')]
    parts += [canonical(n) for n in board.findall('plain/*')
              if n.tag == 'hole' or (n.get('layer') or '').isdigit() and (int(n.get('layer')) in COPPER or n.get('layer') in ('20', '46'))]
    parts += [canonical(p) for l in board.findall('libraries/library') for p in l.findall('packages/package')
              if (l.get('name'), p.get('name')) in used]
    return hashlib.sha256(json.dumps(parts, separators=(',', ':')).encode()).hexdigest()[:16]


# ---------------------------------------------------------------- companions

def layer_stack(board):
    """The stack-up as the standard's companions schema (stack) defines it: copper top to bottom,
    each with the dielectric below it. From Fusion's Layer Stack Manager (layerstackup) when the
    board has one, else the design rules' layerSetup, mtCopper and mtIsolate."""
    layers = Layers(board)
    ls = board.find('.//layerstackup')
    rows, notes = [], []

    def um(v):
        return round(_len(v) * 1000, 3)
    if ls is not None:
        cu, dielectric = None, []
        defs = ls.findall('layerdef')
        for d in defs + [None]:
            if d is None or d.get('type') in ('Signal', 'Plane'):
                if cu is not None:
                    if dielectric:
                        t = sum(x[0] for x in dielectric)
                        cu['dielectricUm'] = round(t, 3)
                        dks = [(x[0], x[1]) for x in dielectric if x[1]]
                        if dks:
                            cu['dk1000'] = round(1000 * sum(t_ * k for t_, k in dks) / sum(t_ for t_, _ in dks))
                        cu['dielectric'] = [{'kind': x[2], 'um': x[0], 'dk': x[1], 'material': x[3]} for x in dielectric]
                    rows.append(cu)
                if d is None:
                    break
                m = d.find('material')
                n = int(d.get('layer'))
                cu = {'name': layers.name(n), 'layerId': n, 'kind': 'plane' if d.get('type') == 'Plane' else 'signal',
                      'copperUm': um(m.get('thickness')) if m is not None else None, 'fusionName': d.get('name')}
                if n not in (1, 16):
                    cu['mid'] = int(layers.name(n)[2:-3])
                dielectric = []
            else:
                m = d.find('material')
                dk = m.get('dielectric_constant_1g') if m is not None else None
                dielectric.append((um(m.get('thickness')) if m is not None else 0.0, float(dk) if dk else None,
                                   d.get('type', '').lower(), m.get('name') if m is not None else None))
        source = 'Fusion Layer Stack Manager (layerstackup)'
    else:
        node = lambda k: board.find("designrules/param[@name='%s']" % k)
        cu_t = (node('mtCopper').get('value').split() if node('mtCopper') is not None else [])
        iso = (node('mtIsolate').get('value').split() if node('mtIsolate') is not None else [])
        for i, n in enumerate(layers.order):
            row = {'name': layers.name(n), 'layerId': n, 'kind': 'signal',
                   'copperUm': um(cu_t[n - 1]) if len(cu_t) >= n else 35.0}
            if n not in (1, 16):
                row['mid'] = int(layers.name(n)[2:-3])
            if i < len(layers.order) - 1 and i < len(iso):
                row['dielectricUm'] = um(iso[i])
            rows.append(row)
        notes.append('No Layer Stack Manager data in this board: thicknesses come from the design rules '
                     '(mtCopper, mtIsolate), and the dielectric constant is unknown (dk1000 absent). '
                     'Set the stack in Fusion (Layer Stack Manager) for impedance work.')
        source = 'design rules (layerSetup, mtCopper, mtIsolate)'
    total = sum((r.get('copperUm') or 0) + (r.get('dielectricUm') or 0) for r in rows)
    return {'copperLayers': len(rows), 'totalUm': round(total, 3), 'layers': rows, 'source': source, 'notes': notes}


def _len(v):
    m = re.fullmatch(r'\s*(-?[0-9.]+)\s*(mm|mil|mic|um|in)?\s*', v or '0')
    if not m:
        return 0.0
    return float(m[1]) * {'mm': 1, None: 1, 'mil': 0.0254, 'mic': 0.001, 'um': 0.001, 'in': 25.4}[m[2]]


def pour_id(net, layer, outline):
    """A pour's handle. Fusion stores no id on a polygon, so it's made from what the pour is: its net, layer and
    outline, whichever vertex the outline starts at. The same pour keeps it across saves and when other pours come
    and go; a new outline is a new pour."""
    pts = sorted((round(float(a), 3), round(float(b), 3)) for a, b in outline)
    return 'pour:%s:%d:%s' % (net, int(layer), hashlib.sha1(repr(pts).encode()).hexdigest()[:12])


def zones(board):
    """The pours model (companions schema `pours`): each pour as drawn and the copper Fusion
    actually poured, island by island with its holes."""
    layers = Layers(board)
    out = []
    for s in board.findall('signals/signal'):
        for p in list(s.findall('polygonpour')) + list(s.findall('polygon')):
            layer = int(p.get('layer', '1'))
            fd = p.find('polygonfilldetails')
            fills = []
            if fd is not None:
                for sh in fd.findall('polygonshape'):
                    seg = sh.find('polygonoutlinesegments')
                    if seg is None:
                        continue
                    fills.append({'outline': [[_r(a), _r(b)] for a, b in vertex_ring(seg.findall('vertex'))],
                                  'holes': [[[_r(a), _r(b)] for a, b in vertex_ring(h.findall('vertex'))] for h in sh.iter('polygonholesegments')]})
            filled = sum(area(f['outline']) - sum(area(h) for h in f['holes']) for f in fills)
            out.append({'id': pour_id(s.get('name'), layer, vertex_ring(shape_vertices(p))), 'net': s.get('name'), 'layer': layers.name(layer), 'layerId': layer,
                        'outline': [[_r(a), _r(b)] for a, b in vertex_ring(shape_vertices(p))],
                        'filledMm2': round(filled, 3), 'filled': fd is not None, 'fills': fills,
                        'pour': p.get('pour', 'solid'), 'isolate': _f(p, 'isolate'), 'rank': int(p.get('rank', '1'))})
    return out


def netlist(board):
    """{net: ["U1.3", ...]}: every signal with its pads as Designator.Pad, from the board (Fusion
    keeps board and schematic in sync, so this is the compiled netlist by pad)."""
    nets = {}
    for s in board.findall('signals/signal'):
        refs = sorted('%s.%s' % (c.get('element'), c.get('pad')) for c in s.findall('contactref'))
        if refs:
            nets[s.get('name')] = refs
    return nets


# ---------------------------------------------------------------- part identifiers

# The attribute names libraries put part numbers under: Adom's own (adom-lbr: MPN, MANUFACTURER,
# MOUSER_PN, DIGIKEY_PN, LCSC_PN), LCSC/JLCPCB libraries (LCSC), SnapEDA/SamacSys
# (MANUFACTURER_PART_NUMBER, MANUFACTURER_NAME) and common hand-made variants. First match wins.
PART_ID_ATTRS = {
    'mpn': ('MPN', 'MANUFACTURER_PART_NUMBER', 'MFR_PN', 'MPN#', 'PART_NUMBER', 'PARTNUMBER', 'MP'),
    'manufacturer': ('MANUFACTURER', 'MANUFACTURER_NAME', 'MFR', 'MF', 'MANUF'),
    'lcsc': ('LCSC_PN', 'LCSC', 'LCSC_PART', 'LCSC#', 'JLCPCB', 'JLC_PN', 'JLCPCB_PN'),
    'mouser': ('MOUSER_PN', 'MOUSER', 'MOUSER_PART_NUMBER', 'MOUSER#'),
    'digikey': ('DIGIKEY_PN', 'DIGIKEY', 'DIGI-KEY_PN', 'DIGI-KEY_PART_NUMBER', 'DIGIKEY_PART_NUMBER', 'DIGIKEY#'),
}


def part_ids(element):
    """A placed part's identifiers from its attributes, by the names above (case-insensitive)."""
    attrs = {(a.get('name') or '').upper(): (a.get('value') or '').strip() for a in element.findall('attribute')}
    return {k: next((attrs[n] for n in names if attrs.get(n)), '') for k, names in PART_ID_ATTRS.items()}


# ---------------------------------------------------------------- reads served from the same export
# Each takes (root, board, args) and answers the EDA Bridge Standard's verb of the same name, in mm.

def _natural(s):
    return [int(t) if t.isdigit() else t.lower() for t in re.split(r'(\d+)', s or '')]


def _packages(board):
    return {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library') for p in l.findall('packages/package')}


def part_box(el, pkg):
    """A part's extent on the board: every footprint primitive but text, transformed."""
    xf, _, _ = placement(el)
    pts = []
    for n in pkg:
        if n.tag in ('smd',):
            pts += [xf(*c) for c in rect_corners(_f(n, 'x'), _f(n, 'y'), _f(n, 'dx'), _f(n, 'dy'), _deg(n.get('rot')))]
        elif n.tag == 'pad':
            d = max(_f(n, 'diameter'), _f(n, 'drill') * 1.5)
            pts += [xf(_f(n, 'x') + dx, _f(n, 'y') + dy) for dx in (-d / 2, d / 2) for dy in (-d / 2, d / 2)]
        elif n.tag == 'wire':
            w = _f(n, 'width') / 2
            pts += [xf(_f(n, 'x1') + s * w, _f(n, 'y1') + t * w) for s in (-1, 1) for t in (-1, 1)]
            pts += [xf(_f(n, 'x2') + s * w, _f(n, 'y2') + t * w) for s in (-1, 1) for t in (-1, 1)]
        elif n.tag == 'rectangle':
            pts += [xf(_f(n, 'x1'), _f(n, 'y1')), xf(_f(n, 'x2'), _f(n, 'y2')), xf(_f(n, 'x1'), _f(n, 'y2')), xf(_f(n, 'x2'), _f(n, 'y1'))]
        elif n.tag == 'circle':
            r = _f(n, 'radius') + _f(n, 'width') / 2
            pts += [xf(_f(n, 'x') + s * r, _f(n, 'y') + t * r) for s in (-1, 1) for t in (-1, 1)]
        elif n.tag in ('polygon', 'polygonshape'):
            pts += [xf(_f(v, 'x'), _f(v, 'y')) for v in shape_vertices(n)]
    if not pts:
        return None
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    return [_r(min(xs)), _r(min(ys)), _r(max(xs)), _r(max(ys))]


def rd_component_data(root, board, args):
    model, _ = board_model(board)
    pkgs = _packages(board)
    els = {e.get('name'): e for e in board.findall('elements/element')}
    want = args.get('designator')
    if want is not None and want not in els:
        fail('designator_not_found', 'No part %s on the board' % want)
    rows = []
    for c in sorted(model['components'], key=lambda c: _natural(c['ref'])):
        if want is not None and c['ref'] != want:
            continue
        el = els[c['ref']]
        mine = [p for p in model['pads'] if p['ref'] == c['ref']]
        box = part_box(el, pkgs.get((el.get('library'), el.get('package')))) if pkgs.get((el.get('library'), el.get('package'))) is not None else None
        ids = part_ids(el)
        rows.append({'designator': c['ref'], 'pattern': c['pattern'], 'comment': c['value'], 'library': c['library'], 'side': c['side'],
                     'layer': 'B.Cu' if c['side'] == 'bottom' else 'F.Cu', 'xMm': c['x'], 'yMm': c['y'], 'rotation': c['rotation'],
                     'box': box, 'wMm': _r(box[2] - box[0]) if box else None, 'hMm': _r(box[3] - box[1]) if box else None,
                     'pads': len(mine), 'nets': sorted({p['net'] for p in mine if p['net']}), 'has3d': c['has3d'], 'locked': c['locked'],
                     'partNumber': ids['mpn'] or None, 'manufacturer': ids['manufacturer'] or None, 'lcsc': ids['lcsc'] or None,
                     'mouser': ids['mouser'] or None, 'digikey': ids['digikey'] or None})
    limit = int(args.get('limit', 500))
    if want is not None:
        return {'component': rows[0]}
    return {'count': len(rows), 'components': rows[:limit], 'truncated': len(rows) > limit}


def rd_component_pins(root, board, args):
    want = args.get('designator')
    if not want:
        fail('bad_args', 'designator is required (U1)')
    model, _ = board_model(board)
    pins = [p for p in model['pads'] if p['ref'] == want]
    if not pins and want not in {c['ref'] for c in model['components']}:
        fail('designator_not_found', 'No part %s on the board' % want)
    pins.sort(key=lambda p: _natural(p['name']))
    return {'designator': want, 'count': len(pins), 'pins': pins}


def rd_list_nets(root, board, args):
    model, _ = board_model(board)
    classes = {c.get('number'): c.get('name') for c in board.findall('classes/class')}
    by = {n: {'name': n, 'pads': 0, 'tracks': 0, 'arcs': 0, 'vias': 0, 'pours': 0} for n in model['nets']}
    for kind, key in (('pads', 'pads'), ('tracks', 'tracks'), ('arcs', 'arcs'), ('vias', 'vias'), ('polygons', 'pours')):
        for x in model[kind]:
            if x['net'] in by:
                by[x['net']][key] += 1
    for s in board.findall('signals/signal'):
        if s.get('name') in by:
            by[s.get('name')]['class'] = classes.get(s.get('class', '0'), s.get('class', '0'))
    nets = sorted(by.values(), key=lambda n: _natural(n['name']))
    return {'count': len(nets), 'nets': nets}


def airline(points):
    """The minimum spanning tree's length over points (Prim): the shortest copper that could join them."""
    if len(points) < 2:
        return 0.0
    inside = [False] * len(points)
    best = [float('inf')] * len(points)
    best[0] = 0.0
    total = 0.0
    for _ in points:
        i = min((k for k in range(len(points)) if not inside[k]), key=lambda k: best[k])
        inside[i] = True
        total += best[i]
        for k in range(len(points)):
            if not inside[k]:
                d = math.hypot(points[i][0] - points[k][0], points[i][1] - points[k][1])
                if d < best[k]:
                    best[k] = d
    return total


def rd_net_connections(root, board, args):
    model, _ = board_model(board)
    one = args.get('net')
    names = {n.lower(): n for n in model['nets']}
    if one is not None and one.lower() not in names:
        fail('no_such_net', 'No net named %s' % one)
    pads = [{'net': p['net'], 'pad': '%s.%s' % (p['ref'], p['name']), 'x': p['x'], 'y': p['y'], 'layer': p['layer']}
            for p in model['pads'] if p['net'] and (one is None or p['net'] == names[one.lower()])]
    nets = {}
    for p in pads:
        nets.setdefault(p['net'], []).append((p['x'], p['y']))
    rows = [{'net': n, 'pads': len(v), 'span': [_r(min(x for x, _ in v)), _r(min(y for _, y in v)), _r(max(x for x, _ in v)), _r(max(y for _, y in v))],
             'airlineMm': round(airline(v), 3)} for n, v in sorted(nets.items(), key=lambda kv: _natural(kv[0]))]
    limit = int(args.get('limit', 4000))
    return {'count': len(pads), 'pads': pads[:limit], 'nets': rows, 'truncated': len(pads) > limit}


def rd_list_designators(root, board, args):
    refs = sorted((e.get('name') for e in board.findall('elements/element')), key=_natural)
    return {'count': len(refs), 'designators': refs}


def _layer_kind(n, name):
    if n in COPPER:
        return 'copper'
    return {20: 'outline', 46: 'outline', 21: 'silkscreen', 22: 'silkscreen', 25: 'silkscreen', 26: 'silkscreen', 27: 'silkscreen', 28: 'silkscreen',
            29: 'mask', 30: 'mask', 31: 'paste', 32: 'paste', 41: 'keepout', 42: 'keepout', 43: 'keepout', 39: 'keepout', 40: 'keepout',
            44: 'drill', 45: 'drill', 17: 'copper (pads)', 18: 'copper (vias)', 19: 'ratsnest', 51: 'documentation', 52: 'documentation',
            47: 'documentation', 48: 'documentation', 49: 'documentation'}.get(n, 'mechanical' if n >= 100 else 'other')


def _usage(board):
    """Primitives per EAGLE layer: the board's own, every placed footprint's (mirrored parts on the
    other side), and the signals' copper. Vias count on every copper layer they pass through."""
    layers = Layers(board)
    used = {}

    def add(n, k=1):
        used[n] = used.get(n, 0) + k
    flip = {1: 16, 16: 1, 21: 22, 22: 21, 25: 26, 26: 25, 27: 28, 28: 27, 29: 30, 30: 29, 31: 32, 32: 31, 35: 36, 36: 35,
            39: 40, 40: 39, 41: 42, 42: 41, 51: 52, 52: 51}
    for n in board.findall('plain/*'):
        if (n.get('layer') or '').isdigit():
            add(int(n.get('layer')))
    pkgs = _packages(board)
    for el in board.findall('elements/element'):
        pkg = pkgs.get((el.get('library'), el.get('package')))
        if pkg is None:
            continue
        mirrored = 'M' in el.get('rot', 'R0')
        for n in pkg:
            if n.tag == 'pad':
                for c in layers.order:
                    add(c)
            elif (n.get('layer') or '').isdigit():
                k = int(n.get('layer'))
                add(flip.get(k, k) if mirrored else k)
    for s in board.findall('signals/signal'):
        for w in s.findall('wire'):
            add(int(w.get('layer')))
        for v in s.findall('via'):
            for c in layers.span(v.get('extent')):
                add(c)
        for p in list(s.findall('polygonpour')) + list(s.findall('polygon')):
            add(int(p.get('layer', '1')))
    return layers, used


def rd_layers(root, board, args):
    layers, used = _usage(board)
    out = []
    for l in (root.find('drawing/layers') if root is not None and root.find('drawing/layers') is not None else []):
        n = int(l.get('number'))
        if n in COPPER and n not in layers.order:
            continue                       # a copper layer this board's stack does not have
        out.append({'id': n, 'name': layers.name(n) if n in COPPER else l.get('name'), 'fusionName': l.get('name'), 'kind': _layer_kind(n, l.get('name')),
                    'displayed': l.get('visible') == 'yes', 'primitives': used.get(n, 0)})
    return {'count': len(out), 'layers': out}


def rd_copper_layers(root, board, args):
    layers, used = _usage(board)
    shown = {int(l.get('number')): l.get('visible') == 'yes' for l in (root.find('drawing/layers') if root is not None and root.find('drawing/layers') is not None else [])}
    ls = board.find('.//layerstackup')
    kinds = {int(d.get('layer')): ('plane' if d.get('type') == 'Plane' else 'signal') for d in (ls.findall('layerdef') if ls is not None else []) if d.get('type') in ('Signal', 'Plane')}
    rows = [{'name': layers.name(n), 'id': n, 'kind': kinds.get(n, 'signal'), 'displayed': shown.get(n, True), 'primitives': used.get(n, 0)} for n in layers.order]
    return {'count': len(rows), 'layers': rows}


def rd_layers_in_use(root, board, args):
    layers, used = _usage(board)
    names = {int(l.get('number')): l.get('name') for l in (root.find('drawing/layers') if root is not None and root.find('drawing/layers') is not None else [])}
    rows = [{'layerId': n, 'name': layers.name(n) if n in COPPER else names.get(n, EAGLE_LAYERS.get(n, 'L%d' % n)), 'primitives': k}
            for n, k in sorted(used.items(), key=lambda kv: -kv[1]) if k]
    return {'copperLayersInUse': [r for r in rows if r['layerId'] in COPPER], 'layersWithPrimitives': rows}


def _rules(board):
    dr = board.find('designrules')
    rows = []
    # <rules> holds one <rulecategory> per kind (whether live and manual DRC check it) and, beside
    # them, the <rule> elements themselves
    cats = {c.get('type'): c for c in (dr.findall('rules/rulecategory') if dr is not None else [])}
    for r in (dr.findall('rules/rule') + dr.findall('rules/rulecategory/rule') if dr is not None else []):
        a = dict(r.attrib)
        cat = cats.get(a.get('type'))
        rows.append({'name': a.pop('name', ''), 'kind': a.pop('type', ''), 'enabled': a.pop('enabled', 'yes') == 'yes',
                     'priority': int(a.pop('priority', '0') or 0), 'scope1': a.pop('onescope', ''), 'scope2': a.pop('twoscope', ''),
                     'value': a.pop('value', None), 'preferredValue': a.pop('preferredvalue', None), 'attrs': a,
                     'liveDrc': cat is not None and cat.get('livedrc') == 'yes', 'manualDrc': cat is not None and cat.get('manualdrc') == 'yes'})
    params = {p.get('name'): p.get('value') for p in (dr.findall('param') if dr is not None else [])}
    return (dr.get('name') if dr is not None else None), rows, params


def rd_read_rules(root, board, args):
    name, rows, params = _rules(board)
    return {'ruleSet': name, 'count': len(rows), 'rules': rows, 'params': params,
            'note': 'rules are Fusion\'s rule manager (type, scope, value); params are the classic EAGLE design-rule values the same checks also read'}


def rd_rules_by_kind(root, board, args):
    name, rows, _ = _rules(board)
    by = {}
    for r in sorted((r for r in rows if r['enabled']), key=lambda r: r['priority']):
        by.setdefault(r['kind'], []).append(r)
    return {'ruleSet': name, 'rulesByKind': by}


def _text(node, xf=None, mirrored=False, layer_flip=False):
    x, y = (_f(node, 'x'), _f(node, 'y'))
    if xf:
        x, y = xf(x, y)
    layer = int(node.get('layer', '0') or 0)
    return {'x': _r(x), 'y': _r(y), 'size': _f(node, 'size'), 'rotation': _deg(node.get('rot')), 'layer': layer,
            'side': 'bottom' if layer in (22, 26, 28, 52) or 'M' in (node.get('rot') or '') else 'top'}


def rd_silk_state(root, board, args):
    """Each part's body box, designator and value text as placed (a smashed part's own text, else
    the footprint's >NAME / >VALUE placeholder, transformed), and the board's free silkscreen."""
    pkgs = _packages(board)
    comps = []
    for el in sorted(board.findall('elements/element'), key=lambda e: _natural(e.get('name'))):
        pkg = pkgs.get((el.get('library'), el.get('package')))
        xf, angle, mirrored = placement(el)
        texts = {}
        for a in el.findall('attribute'):
            if a.get('name') in ('NAME', 'VALUE') and a.get('x') is not None:
                t = _text(a)
                t['shown'] = a.get('display', 'value') != 'off'
                texts[a.get('name')] = t
        if pkg is not None:
            for t in pkg.findall('text'):
                key = {'>NAME': 'NAME', '>VALUE': 'VALUE'}.get((t.text or '').strip().upper())
                if key and key not in texts:
                    row = _text(t, xf)
                    row['rotation'] = abs_rotation(angle, mirrored, _deg(t.get('rot')))
                    if mirrored:
                        row['side'] = 'bottom'
                    row['shown'] = True
                    texts[key] = row
        name = dict(texts.get('NAME', {}), text=el.get('name')) if 'NAME' in texts else {'text': el.get('name'), 'shown': False}
        value = dict(texts.get('VALUE', {}), text=el.get('value')) if 'VALUE' in texts else {'text': el.get('value'), 'shown': False}
        comps.append({'ref': el.get('name'), 'pattern': el.get('package'), 'x': _f(el, 'x'), 'y': _f(el, 'y'), 'rotation': _norm(_deg(el.get('rot'))),
                      'side': 'bottom' if 'M' in el.get('rot', '') else 'top', 'body': part_box(el, pkg) if pkg is not None else None,
                      'name': name, 'comment': value, 'smashed': el.get('smashed') == 'yes'})
    # raw: the handle apply_silkscreen's remove takes back (what the item is and where it sits)
    lines = [{'side': 'bottom' if w.get('layer') == '22' else 'top', 'x1': _f(w, 'x1'), 'y1': _f(w, 'y1'), 'x2': _f(w, 'x2'), 'y2': _f(w, 'y2'), 'width': _f(w, 'width'),
              'raw': 'wire|%s|%s|%s|%s|%s' % (w.get('layer'), w.get('x1'), w.get('y1'), w.get('x2'), w.get('y2'))}
             for w in board.findall('plain/wire') if w.get('layer') in ('21', '22')]
    texts = [dict(_text(t), text=t.text or '', raw='text|%s|%s|%s|%s' % (t.get('layer'), t.get('x'), t.get('y'), t.text or ''))
             for t in board.findall('plain/text') if t.get('layer') in ('21', '22', '25', '26', '27', '28')]
    return {'components': comps, 'lines': lines, 'texts': texts}


def rd_net_classes(root, board, args):
    nets = {}
    for s in board.findall('signals/signal'):
        nets.setdefault(s.get('class', '0'), []).append(s.get('name'))
    rows = []
    for c in board.findall('classes/class'):
        rows.append({'name': c.get('name'), 'number': int(c.get('number')), 'widthMm': _f(c, 'width'), 'drillMm': _f(c, 'drill'),
                     'clearances': [{'toClass': int(k.get('class')), 'mm': _f(k, 'value')} for k in c.findall('clearance')],
                     'nets': sorted(nets.get(c.get('number'), []), key=_natural)})
    return {'count': len(rows), 'classes': rows}


def rd_diff_pairs(root, board, args):
    """Differential pairs as Fusion keeps them: by the nets' names. A pair is two nets that differ
    only in a final _P/_N, P/N, +/- or DP/DN (Fusion's differential-pair routing pairs NAME_P with
    NAME_N). There is no pair object to define or remove: rename the nets in the schematic."""
    if args.get('pos') or args.get('neg') or args.get('remove'):
        fail('not_supported', "Fusion pairs nets by their names (NAME_P / NAME_N), not by a pair object: to make or break a pair, rename the nets in the schematic (fusion_sch_edit_label or fusion_sch_edit)")
    names = sorted({s_.get('name') for s_ in board.findall('signals/signal')}, key=_natural)
    have = set(names)
    pairs, seen = [], set()
    for n in names:
        for p_, q in (('_P', '_N'), ('P', 'N'), ('+', '-'), ('DP', 'DN'), ('_DP', '_DN')):
            if n.upper().endswith(p_) and n not in seen:
                stem = n[:len(n) - len(p_)]
                twin = next((m for m in have if m.upper() == (stem + q).upper()), None)
                if twin and twin not in seen and stem.strip('_'):
                    pairs.append({'name': stem.rstrip('_') or stem, 'pos': n, 'neg': twin, 'by': '%s/%s' % (p_, q)})
                    seen |= {n, twin}
                    break
    if args.get('name'):
        pairs = [x for x in pairs if x['name'].lower() == str(args['name']).lower()]
    return {'count': len(pairs), 'pairs': pairs,
            '_hint': 'Pairs come from the nets\' names. Route a pair with Fusion\'s differential-pair router; set its width and gap with fusion_add_rule or the net classes.'}


READS = {'component_data': rd_component_data, 'component_pins': rd_component_pins, 'list_nets': rd_list_nets,
         'net_connections': rd_net_connections, 'list_designators': rd_list_designators, 'layers': rd_layers,
         'copper_layers': rd_copper_layers, 'layers_in_use': rd_layers_in_use, 'read_rules': rd_read_rules,
         'rules_by_kind': rd_rules_by_kind, 'silk_state': rd_silk_state, 'net_classes': rd_net_classes, 'diff_pairs': rd_diff_pairs}


def export_bom_csv(board, args):
    """fusion_export_bom: the BOM CSV from the live board, with each part's MPN, manufacturer, LCSC,
    Mouser and DigiKey numbers from its attributes (blank when a part has none). Grouped (default):
    one line per value, footprint, library and MPN; else one line per part. The columns the export
    always had come first, in their old order, so readers of the old file keep working."""
    import csv, io
    els = sorted(board.findall('elements/element'), key=lambda e: _natural(e.get('name')))
    rows = []
    for e in els:
        ids = part_ids(e)
        rows.append({'name': e.get('name'), 'value': e.get('value') or '', 'package': e.get('package') or '',
                     'library': e.get('library') or '', 'side': 'Bottom' if 'M' in e.get('rot', 'R0') else 'Top', **ids})
    grouped = args.get('grouped', True) is not False
    ID_COLS = ['MPN', 'Manufacturer', 'LCSC', 'Mouser', 'DigiKey']
    ids_of = lambda r: [r['mpn'], r['manufacturer'], r['lcsc'], r['mouser'], r['digikey']]
    out = io.StringIO()
    w = csv.writer(out, lineterminator='\n')
    if grouped:
        groups = {}
        for r in rows:
            g = groups.setdefault((r['value'], r['package'], r['library'], r['mpn']), dict(r, designators=[]))
            g['designators'].append(r['name'])
        lines = sorted(groups.values(), key=lambda g: _natural(g['designators'][0]))
        w.writerow(['Comment', 'Designator', 'Footprint', 'Quantity', 'Library'] + ID_COLS)
        for g in lines:
            w.writerow([g['value'], ' '.join(g['designators']), g['package'], len(g['designators']), g['library']] + ids_of(g))
        unique = len(lines)
    else:
        w.writerow(['Designator', 'Comment', 'Footprint', 'Library', 'Side'] + ID_COLS)
        for r in rows:
            w.writerow([r['name'], r['value'], r['package'], r['library'], r['side']] + ids_of(r))
        unique = len({(r['value'], r['package'], r['library'], r['mpn']) for r in rows})
    missing = {k: [r['name'] for r in rows if not r[k]] for k in ('mpn', 'lcsc')}
    return out.getvalue(), {'componentCount': len(rows), 'uniquePartCount': unique, 'grouped': grouped,
                            'withMpn': len(rows) - len(missing['mpn']), 'withLcsc': len(rows) - len(missing['lcsc']),
                            'missingMpn': missing['mpn'], 'missingLcsc': missing['lcsc']}
