"""Explicit live Fusion copper edits; embedded by routing.rs, run on Fusion's main thread.

No open-file rewrites, UI clicks, add-in replacement or application restart.
The geometry preflight is deliberately conservative and is NOT Autodesk DRC.
"""
import collections
import datetime
import hashlib
import json
import math
from pathlib import Path
import re
import tempfile
import xml.etree.ElementTree as ET
import zipfile


class RoutingError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def fail(code, message):
    raise RoutingError(code, message)


def number(v, positive=False):
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v):
        fail('invalid_geometry', 'Coordinates and dimensions must be finite numbers')
    if abs(v) > 2000 or (positive and v < 0.0001):
        fail('invalid_geometry', 'Geometry is outside supported millimetre bounds')
    return round(v, 6)


def eagle_layer(v, board=None):
    """A copper layer argument: Fusion's number or the standard's name (F.Cu, In1.Cu, B.Cu), so a caller
    can pass back what fusion_routing_state reported. With a board, it must be one of that board's layers;
    an inner layer's name means a number only on a board, as In2.Cu is 15 on 4 layers and 3 on 6."""
    names = {'F.CU': 1, 'B.CU': 16}
    if board is not None:
        L = Layers(board)
        names = {nm.upper(): n for n, nm in L.names.items()}
    if isinstance(v, str):
        v = names.get(v.strip().upper(), v.strip())
        if isinstance(v, str) and v.isdigit():
            v = int(v)
    if isinstance(v, bool) or not isinstance(v, int):
        fail('invalid_layer', 'A layer is its number or its name (F.Cu, In1.Cu, B.Cu): got %r' % (v,))
    if board is not None and v not in L.order:
        fail('invalid_layer', 'Layer %s is not a copper layer of this board (%s)' % (v, ', '.join(L.name(n) for n in L.order)))
    return v


def identifier(v):
    # The command language also interprets wildcards. Fail closed on all metacharacters.
    # '!' is allowed: EAGLE uses it for an overbar (!CE, !INT are active-low nets), and inside
    # quotes it is plain text.
    if not isinstance(v, str) or not re.fullmatch(r'[A-Za-z0-9_+./:$!-]{1,128}', v):
        fail('invalid_net', 'Net names must use letters, digits, _ + . / : $ ! or -')
    return v


def _tab_number(app, i):
    """The tab of the document at index i in app.documents. A document with no visible tab (opened by a script
    with visible=False) has no place in the tab bar, so every index after it was off by one (#141)."""
    return sum(1 for k in range(min(int(i), app.documents.count)) if app.documents.item(k).isVisible)


def show_tab_of(app, doc):
    """Put Fusion's UI on the tab of `doc`. doc.activate() moves the API's active document but not
    always the UI, and the board export and every Electron command act on the UI's board: with the
    two apart, a read named one board and returned another, and an edit landed on the wrong one
    (live 2026-10-26)."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    i = next((k for k, d in enumerate(docs) if d == doc), None)
    if i is not None:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))


def snapshot(app, args):
    doc = app.activeDocument
    if (not doc or app.activeProduct.productType != 'BoardProductType') and args.get('expectDocument'):
        # the named design's board, when its schematic, 3D PCB or another document is in front:
        # bring the board's tab forward rather than refuse (an error the bridge can fix itself)
        docs = [app.documents.item(i) for i in range(app.documents.count)]
        def pt(d):
            try: return d.products.item(0).productType if d.products.count else ''
            except Exception: return ''
        boards = [i for i, d in enumerate(docs) if d.name == args['expectDocument'] and pt(d) == 'BoardProductType']
        if len(boards) == 1:
            docs[boards[0]].activate()
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, boards[0]))
            doc = app.activeDocument
    if not doc or app.activeProduct.productType != 'BoardProductType':
        fail('wrong_document', 'Activate a PCB board before reading or routing it')
    show_tab_of(app, doc)
    if args.get('expectDocument') != doc.name:
        fail('wrong_document', 'expectDocument must match the active board name exactly')
    identity = str(doc.creationId)
    if args.get('boardId') is not None and args['boardId'] != identity:
        fail('board_mismatch', 'boardId does not identify the active Fusion document')
    with tempfile.TemporaryDirectory(prefix='adom-routing-') as d:
        p = Path(d) / 'snapshot.fbrd'
        app.executeTextCommand('Document.CopyToDesktop ' + p.as_posix())
        with zipfile.ZipFile(p) as z:
            entries = [n for n in z.namelist() if n.endswith('.brd')]
            if len(entries) != 1:
                fail('ambiguous_export', 'Expected exactly one board XML in the fresh Fusion export')
            raw = z.read(entries[0])
    root = ET.fromstring(raw)
    board = root.find('drawing/board')
    if board is None:
        fail('invalid_export', 'Fusion did not export an EAGLE board')
    # The standard's revision: copper, parts and outline only (board_model.py), so a silkscreen or
    # rule edit does not invalidate a caller's plan.
    return root, board, {'boardId': identity, 'documentName': doc.name, 'revision': board_revision(board)}


def _rule_len(board, name, default):
    node = board.find("designrules/param[@name='%s']" % name)
    m = re.fullmatch(r'([0-9.]+)(mm|mil)', node.get('value', '')) if node is not None else None
    return float(m[1]) * (0.0254 if m[2] == 'mil' else 1) if m else default


def objects(board):
    rvn = board.find("designrules/param[@name='rvPadTop']")
    try: rv_pad = float(rvn.get('value')) if rvn is not None else 0.25
    except ValueError: rv_pad = 0.25
    rl_min_pad, rl_max_pad = _rule_len(board, 'rlMinPadTop', 0.254), _rule_len(board, 'rlMaxPadTop', 0.508)
    nets = {s.get('name'): s for s in board.findall('signals/signal')}
    contacts = {(c.get('element'), c.get('pad')): name for name, s in nets.items() for c in s.findall('contactref')}
    pads, wires, vias = {}, [], []
    packages = {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library') for p in l.findall('packages/package')}
    for el in board.findall('elements/element'):
        pkg = packages.get((el.get('library'), el.get('package')))
        if pkg is None:
            fail('unsupported_board', 'Missing embedded footprint definition')
        rotation = el.get('rot', 'R0')
        mirrored = 'M' in rotation
        angle = math.radians(float(re.search(r'R(-?[\d.]+)', rotation).group(1)))
        for pad in list(pkg.findall('pad')) + list(pkg.findall('smd')):
            x, y = float(pad.get('x')), float(pad.get('y'))
            x, y = x*math.cos(angle)-y*math.sin(angle), x*math.sin(angle)+y*math.cos(angle)
            if mirrored: x = -x
            x, y = x+float(el.get('x')), y+float(el.get('y'))
            key = el.get('name')+'.'+pad.get('name')
            poly = None
            if pad.tag == 'pad':
                # the copper is the library diameter or the drill plus the board's pad restring,
                # whichever is larger; round pads are their radius, square ones their bounding
                # circle, long/offset ones their full length (the old code treated a round 1.3 mm
                # contact as a 1.84 mm square and refused legal copper beside it)
                drill_ = float(pad.get('drill'))
                ring = min(max(drill_ * rv_pad, rl_min_pad), rl_max_pad)
                diameter = max(float(pad.get('diameter', '0')), drill_ + 2 * ring)
                shape = pad.get('shape', 'round')
                radius = diameter * (1 if shape in ('long','offset') else (math.sqrt(2)/2 if shape in ('square','octagon') else 0.5))
                layers = [1,16]
            else:
                radius = math.hypot(float(pad.get('dx')), float(pad.get('dy'))) / 2
                # The exact rectangle (roundness ignored, so still conservative): a circle of
                # the half diagonal refused legal traces running past a pad's flat edge.
                hx, hy = float(pad.get('dx'))/2, float(pad.get('dy'))/2
                pr = math.radians(float((re.search(r'R(-?[\d.]+)', pad.get('rot', 'R0')) or [0, 0])[1]))
                poly = []
                for cx, cy in ((-hx,-hy),(hx,-hy),(hx,hy),(-hx,hy)):
                    cx, cy = cx*math.cos(pr)-cy*math.sin(pr), cx*math.sin(pr)+cy*math.cos(pr)
                    cx, cy = cx+float(pad.get('x')), cy+float(pad.get('y'))
                    cx, cy = cx*math.cos(angle)-cy*math.sin(angle), cx*math.sin(angle)+cy*math.cos(angle)
                    if mirrored: cx = -cx
                    poly.append([cx+float(el.get('x')), cy+float(el.get('y'))])
                layer = int(pad.get('layer'))
                layers = [17-layer if mirrored else layer]
            pads[key] = {'name':key, 'net':contacts.get((el.get('name'),pad.get('name'))),
                         'at':[round(x,6),round(y,6)], 'radius':radius, 'layers':layers, 'poly':poly}
    for name,s in nets.items():
        for w in s.findall('wire'):
            if int(w.get('layer')) == 19: continue
            wires.append({'net':name, 'start':[float(w.get('x1')),float(w.get('y1'))],
                          'end':[float(w.get('x2')),float(w.get('y2'))], 'width':float(w.get('width')),
                          'layer':int(w.get('layer')), 'curve':float(w.get('curve','0'))})
        for v in s.findall('via'):
            vias.append({'net':name,'at':[float(v.get('x')),float(v.get('y'))],
                         'diameter':float(v.get('diameter','0')), 'drill':float(v.get('drill')),
                         'extent':v.get('extent')})
    return nets,pads,wires,vias


def state(board, meta):
    """The standard board model (board_model.py) with the keys the AI flow has always read
    beside it: segments, elements, airwireCount, remainingNets, and each pad's 'key' (REF.PAD),
    'at', 'radius' and 'layers', each via's 'at' and 'diameter'."""
    nets,pads,wires,vias=objects(board)
    model,std_pads=board_model(board,meta)
    rows=[]
    for name,s in nets.items():
        airwires=[{'start':[float(w.get('x1')),float(w.get('y1'))], 'end':[float(w.get('x2')),float(w.get('y2'))]}
                  for w in s.findall('wire') if w.get('layer')=='19']
        rows.append({'name':name,'pads':[p['name'] for p in pads.values() if p['net']==name],
                     'airwires':airwires,'airwireCount':len(airwires),
                     'segments':sum(w['net']==name for w in wires),'vias':sum(v['net']==name for v in vias)})
    elements=[{'name':el.get('name'),'package':el.get('package'),'library':el.get('library'),'value':el.get('value'),
               'x':float(el.get('x')),'y':float(el.get('y')),'rot':el.get('rot','R0'),
               'has3d':bool(el.get('package3d_urn')),'package3dUrn':el.get('package3d_urn'),
               'mpn':next((a.get('value') for a in el.findall('attribute') if (a.get('name') or '').upper()=='MPN'),None)}
              for el in board.findall('elements/element')]
    # one row per pad: the standard's fields (name is the pad's own name, ref the part) over the
    # planner's (key 'U1.3', at, radius, layers, poly)
    pad_rows=[]
    for key,p in pads.items():
        row={**p,'key':key}; row.update(std_pads.get(key,{})); pad_rows.append(row)
    # objects() and board_model() walk the signals in the same order, so vias pair by index
    via_rows=[{**v,**w} for v,w in zip(vias,model['vias'])] if len(vias)==len(model['vias']) else model['vias']
    airwires=sum(n['airwireCount'] for n in rows)
    invalid_vias=_invalid_via_nets(board)
    return {**meta,**model,**({'invalidViaNets':invalid_vias,'_hintInvalidVias':'These nets have vias Fusion marks invalid, drawn when the board\'s stack had no through via. Call fusion_set_layer_count at the same count: it repairs the stack and lands them again.'} if invalid_vias else {}),'pads':pad_rows,'vias':via_rows,'netRows':rows,'segments':wires,'elements':elements,
            'segmentCount':len(wires),'viaCount':len(vias),'airwireCount':airwires,'fusionAirwires':airwires,
            'remainingNets':[n['name'] for n in rows if n['airwireCount']],
            'connectivitySource':'unconnected and splitNets are computed from the copper (pads, tracks, arcs, vias and Fusion\'s poured fill joined where they touch); fusionAirwires is Fusion\'s own ratsnest from the export',
            'drc':{'checked':False,'note':'Use fusion_routing_drc for a fresh Autodesk DRC'}}


def shoelace(pts):
    a=0.0
    for i in range(len(pts)):
        x1,y1=pts[i]; x2,y2=pts[(i+1)%len(pts)]; a+=x1*y2-x2*y1
    return abs(a)/2


def pours(board):
    """Every Fusion pour on the board: net, layer, outline, isolate, rank, and its filled area
    from Fusion's own fill (polygonfilldetails: each island's outline minus its holes)."""
    out=[]
    for s in board.findall('signals/signal'):
        for p in list(s.findall('polygonpour'))+list(s.findall('polygon')):
            if p.tag=='polygonpour':
                ol=p.find('polygonoutlineobjects'); outline=[[float(v.get('x')),float(v.get('y'))] for v in (ol.findall('vertex') if ol is not None else [])]
            else:
                outline=[[float(v.get('x')),float(v.get('y'))] for v in p.findall('vertex')]
            filled=0.0; islands=0
            fd=p.find('polygonfilldetails')
            if fd is not None:
                for sh in fd.findall('polygonshape'):
                    seg=sh.find('polygonoutlinesegments')
                    if seg is None: continue
                    a=shoelace([(float(v.get('x')),float(v.get('y'))) for v in seg.findall('vertex')])
                    for h in sh.iter('polygonholesegments'):
                        a-=shoelace([(float(v.get('x')),float(v.get('y'))) for v in h.findall('vertex')])
                    filled+=a; islands+=1
            out.append({'id':pour_id(s.get('name'),int(p.get('layer','1')),outline),'net':s.get('name'),'layer':int(p.get('layer','1')),'outline':outline,
                        'isolate':float(p.get('isolate','0')),'rank':int(p.get('rank','1')),'width':float(p.get('width','0')),
                        'thermals':p.get('thermals','yes')!='no','orphans':p.get('orphans','no')=='yes','pour':p.get('pour','solid'),
                        'filledMm2':round(filled,3),'islands':islands,'filled':fd is not None})
    return out


def pour_state(board):
    rows=pours(board); by_layer={}; by_net={}
    for r in rows:
        by_layer[str(r['layer'])]=round(by_layer.get(str(r['layer']),0)+r['filledMm2'],3)
        by_net[r['net']]=round(by_net.get(r['net'],0)+r['filledMm2'],3)
    return {'pours':rows,'zones':zones(board),'filledAreaByLayerMm2':by_layer,'filledAreaByNetMm2':by_net,
            'unfilled':[r['net'] for r in rows if not r['filled']]}


def same_outline(a,b,tol=0.01):
    if len(a)!=len(b): return False
    return all(abs(p[0]-q[0])<=tol and abs(p[1]-q[1])<=tol for p,q in zip(a,b))


def element_rows(board):
    return {el.get('name'):{'x':float(el.get('x')),'y':float(el.get('y')),'rot':el.get('rot','R0')} for el in board.findall('elements/element')}


def rot_of(v,current=None):
    """An element's rotation. A number is an angle and says nothing about the side, so it keeps the side the part is
    on (current, its rotation now): ROTATE =R90 on a mirrored part takes the mirror off, which moved bottom parts to
    the top (adom-aiflow#142). Saying R or MR in a string names the side outright, and that's the way to change it."""
    if isinstance(v,(int,float)) and not isinstance(v,bool):
        d=float(v)%360.0
        return ('M' if current and 'M' in current else '')+'R'+('%g'%d)
    if isinstance(v,str) and re.fullmatch(r'S?M?R-?[\d.]+',v): return v.lstrip('S')
    fail('invalid_geometry','rotation must be degrees or an EAGLE rotation like R90 / MR180')


def point_distance(p,a,b):
    dx,dy=b[0]-a[0],b[1]-a[1]
    t=max(0,min(1,((p[0]-a[0])*dx+(p[1]-a[1])*dy)/(dx*dx+dy*dy))) if dx or dy else 0
    return math.hypot(p[0]-a[0]-t*dx,p[1]-a[1]-t*dy)


def distance(a,b,c,d):
    def cross(p,q,r): return (q[0]-p[0])*(r[1]-p[1])-(q[1]-p[1])*(r[0]-p[0])
    if cross(a,b,c)*cross(a,b,d)<0 and cross(c,d,a)*cross(c,d,b)<0: return 0
    return min(point_distance(a,c,d),point_distance(b,c,d),point_distance(c,a,b),point_distance(d,a,b))


def poly_distance(a,b,poly):
    # Segment to polygon (convex or not, e.g. a comb-shaped footprint copper): 0 when the segment
    # touches or enters it, else the nearest edge.
    if point_in_loop(a,poly) or point_in_loop(b,poly): return 0
    return min(distance(a,b,p0,q) for p0,q in zip(poly,poly[1:]+poly[:1]))


def artwork_obstacles(board,pads,stack):
    """Free copper and keepouts as route obstacles, from the board and from each placed footprint:
    wires (arcs as chords), circles, rectangles and polygons on copper; tRestrict (41) and
    bRestrict (42) keep every net's copper off their side. Footprint copper takes the net of the
    part's pad it touches, so the pad's own net may run through it."""
    out=[]
    def add(n,xf,mirrored,ref):
        layer=n.get('layer')
        num={'41':1,'42':16}.get(layer) or int(layer)
        if mirrored and num in (1,16): num=17-num
        keepout=layer in ('41','42')
        w=float(n.get('width','0') or 0)/2
        f=lambda x,y: list(xf(float(x),float(y)))
        def owner(ring):
            if keepout or ref is None: return None
            for k,p_ in pads.items():
                if k.split('.',1)[0]==ref and num in p_['layers'] and (point_in_loop(p_['at'],ring) or any(point_in_loop(v,ring) for v in (p_.get('poly') or []))):
                    return p_['net']
            return None
        if n.tag=='wire':
            pts,sag=arc_points(f(n.get('x1'),n.get('y1')),f(n.get('x2'),n.get('y2')),float(n.get('curve','0') or 0)*(-1 if mirrored else 1))
            for a_,b_ in zip(pts,pts[1:]): out.append((None if keepout else owner([a_,b_,a_]),a_,b_,w+sag,[num],None))
        elif n.tag=='circle':
            c=f(n.get('x'),n.get('y')); r=float(n.get('radius'))
            # a filled disc (width 0) or a ring; a ring's hole is not trusted as free space
            out.append((owner([[c[0]+r*math.cos(k*math.pi/8),c[1]+r*math.sin(k*math.pi/8)] for k in range(16)]),c,c,r+w,[num],None))
        else:
            if n.tag=='rectangle':
                x1,y1,x2,y2=(float(n.get(k)) for k in ('x1','y1','x2','y2'))
                cx,cy=(x1+x2)/2,(y1+y2)/2; rot=math.radians(float(re.sub(r'[^0-9.\-]','',n.get('rot','R0')) or 0))
                loc=[(cx+(x-cx)*math.cos(rot)-(y-cy)*math.sin(rot),cy+(x-cx)*math.sin(rot)+(y-cy)*math.cos(rot)) for x,y in ((x1,y1),(x2,y1),(x2,y2),(x1,y2))]
                ring=[f(x,y) for x,y in loc]
            else:
                ring=[f(v.get('x'),v.get('y')) for v in shape_vertices(n)]
            if len(ring)<3: return
            c=ring[0]; out.append((owner(ring),c,c,w,[num],ring))
    for n in board.findall('plain/*'):
        if n.tag in ('wire','circle','rectangle','polygon','polygonshape') and n.get('layer') in set(str(x) for x in stack)|{'41','42'}:
            add(n,lambda x,y:(x,y),False,None)
    packages={(l.get('name'),p_.get('name')):p_ for l in board.findall('libraries/library') for p_ in l.findall('packages/package')}
    for el in board.findall('elements/element'):
        pkg=packages.get((el.get('library'),el.get('package')))
        if pkg is None: continue
        xf,_,mirrored=placement(el)
        for n in pkg:
            if n.tag in ('wire','circle','rectangle','polygon','polygonshape') and n.get('layer') in ('1','16','41','42'):
                add(n,xf,mirrored,el.get('name'))
    return out


def arc_points(p1,p2,curve):
    """An EAGLE wire from p1 to p2 sweeping `curve` degrees (positive = counterclockwise), as
    points p1..p2 at most 5 degrees apart, plus the largest gap between chord and arc."""
    if not curve: return [p1,p2],0.0
    c=math.radians(curve); dx,dy=p2[0]-p1[0],p2[1]-p1[1]; L=math.hypot(dx,dy)
    if L<1e-9: return [p1,p2],0.0
    r=L/(2*abs(math.sin(c/2))); d=(L/2)/math.tan(c/2)
    cx,cy=(p1[0]+p2[0])/2-dy/L*d,(p1[1]+p2[1])/2+dx/L*d
    a0=math.atan2(p1[1]-cy,p1[0]-cx); n=max(2,int(math.ceil(abs(curve)/5.0)))
    pts=[[cx+r*math.cos(a0+c*k/n),cy+r*math.sin(a0+c*k/n)] for k in range(n+1)]
    pts[0],pts[-1]=list(p1),list(p2)
    return pts,r*(1-math.cos(abs(c)/n/2))


def outline_loops(board):
    """Layer-20 wires chained into closed loops, largest (the board edge) first, and the largest
    arc sagitta. Fails unless every wire belongs to a closed loop."""
    segs=[]; sag=0.0
    for w in board.findall("plain/wire[@layer='20']"):
        p1=[float(w.get('x1')),float(w.get('y1'))]; p2=[float(w.get('x2')),float(w.get('y2'))]
        pts,sg=arc_points(p1,p2,float(w.get('curve','0') or 0)); sag=max(sag,sg); segs.append(pts)
    for c in board.findall("plain/circle[@layer='20']"):
        x,y,r=float(c.get('x')),float(c.get('y')),float(c.get('radius'))
        n=72; segs.append([[x+r*math.cos(2*math.pi*k/n),y+r*math.sin(2*math.pi*k/n)] for k in range(n+1)]); sag=max(sag,r*(1-math.cos(math.pi/n)))
    if not segs: fail('unsupported_board','The board has no outline on layer 20 (Dimension)')
    near=lambda p,q: abs(p[0]-q[0])<1e-3 and abs(p[1]-q[1])<1e-3
    loops=[]
    while segs:
        loop=list(segs.pop(0))
        while not near(loop[0],loop[-1]):
            for i,sg in enumerate(segs):
                if near(sg[0],loop[-1]): loop+=sg[1:]; segs.pop(i); break
                if near(sg[-1],loop[-1]): loop+=list(reversed(sg))[1:]; segs.pop(i); break
            else: fail('unsupported_board','The board outline is not closed: a layer-20 line or arc ends near (%.3f, %.3f) with nothing joined to it'%tuple(loop[-1]))
        loops.append(loop[:-1])
    area=lambda l: abs(sum(a[0]*b[1]-b[0]*a[1] for a,b in zip(l,l[1:]+l[:1])))/2
    loops.sort(key=area,reverse=True)
    return loops,sag


def outline_problem(ctx,a,b,radius):
    """Why copper from a to b (half-width radius) breaks the board outline, or None. Inside the
    edge loop, outside every cutout, and the edge clearance away from every outline segment."""
    outer,cutouts=ctx['outline'][0],ctx['outline'][1:]
    if not (point_in_loop(a,outer) and point_in_loop(b,outer)) or any(point_in_loop(p,c) for p in (a,b) for c in cutouts):
        return 'Candidate copper lies outside the board outline or inside a cutout'
    margin=ctx['edgeClearance']+radius
    for loop in ctx['outline']:
        for p0,q in zip(loop,loop[1:]+loop[:1]):
            if distance(a,b,p0,q)<margin-1e-6: return 'Candidate copper violates the board-edge margin'
    return None


def point_in_loop(p,loop):
    inside=False
    for a,b in zip(loop,loop[1:]+loop[:1]):
        if (a[1]>p[1])!=(b[1]>p[1]) and p[0]<a[0]+(p[1]-a[1])*(b[0]-a[0])/(b[1]-a[1]): inside=not inside
    return inside


def plan(board,args):
    nets,pads,wires,vias=objects(board)
    net=identifier(args.get('net'))
    if net not in nets: fail('unknown_net','No exact net named '+net)
    # any copper layer of the board's stack, by number or by the standard's name (F.Cu, In1.Cu, B.Cu)
    L=Layers(board); by_name={v.upper():k for k,v in L.names.items()}
    def layer(v):
        if isinstance(v,str) and v.strip().upper() in by_name: v=by_name[v.strip().upper()]
        v=eagle_layer(v)
        if v not in L.order: fail('invalid_layer','Layer %s is not a copper layer of this board (%s)'%(v,', '.join(L.name(n) for n in L.order)))
        return v
    width=number(args.get('width',0.25),True)
    # viaSize is the standard's plan_route name (its routeNet passes through unchanged)
    dia=number(args.get('viaDiameter',args.get('viaSize',0.6)),True); drill=number(args.get('viaDrill',0.3),True)
    if drill>=dia: fail('invalid_geometry','Via drill must be smaller than its diameter')
    paths=args.get('paths',[args.get('points')])
    if not isinstance(paths,list) or not 1<=len(paths)<=32: fail('invalid_path','Supply 1..32 paths')
    planned=[]
    for path in paths:
        if not isinstance(path,list) or not 2<=len(path)<=256: fail('invalid_path','Each path needs 2..256 waypoints')
        current=layer(args.get('layer',1)); previous=None
        for spec in path:
            pad=None; outgoing=current
            if isinstance(spec,dict):
                if 'pad' in spec:
                    pad=pads.get(spec['pad'])
                    if pad is None: fail('unknown_pad','Unknown pad '+str(spec['pad']))
                    if pad['net']!=net: fail('net_mismatch','Pad '+pad['name']+' does not belong to '+net)
                    xy=pad['at']
                else: xy=[spec.get('x'),spec.get('y')]
                outgoing=layer(spec.get('layer',current))
            elif isinstance(spec,list) and len(spec)==2: xy=spec
            else: fail('invalid_path','Waypoints are [x,y], {x,y,layer?}, or {pad,layer?}')
            xy=[grid(number(v)) for v in xy]   # on Fusion's grid, so the readback matches exactly
            if pad and current not in (L.order if pad['layers']==[1,16] else pad['layers']): fail('pad_layer_mismatch','Pad %s has no copper on %s'%(spec['pad'],L.name(current)))
            if previous is not None and previous!=xy:
                planned.append({'kind':'segment','net':net,'start':previous,'end':xy,'layer':current,'width':width})
            if outgoing!=current:
                planned.append({'kind':'via','net':net,'at':xy,'diameter':dia,'drill':drill,'extent':'1-16'})
            current=outgoing; previous=xy
    # Multiple branches can name the same layer transition. Emit one physical via.
    unique=[]; seen_vias=set()
    for item in planned:
        if item['kind']=='via':
            key=(tuple(item['at']),item['diameter'],item['drill'],item['extent'])
            if key in seen_vias: continue
            seen_vias.add(key)
        unique.append(item)
    planned=unique
    if not planned or len(planned)>512: fail('invalid_path','Need 1..512 nonzero copper items')
    ctx=routing_context(board,args,net,width,drill if any(i['kind']=='via' for i in planned) else 0)
    clearance=ctx['clearance']; obstacles=ctx['obstacles']
    for item in planned:
        via=item['kind']=='via'; a=item['at'] if via else item['start']; b=a if via else item['end']
        radius=ctx['via_dia'](item['diameter'],item['drill'])/2 if via else item['width']/2; layers=Layers(board).order if via else [item['layer']]
        why=outline_problem(ctx,a,b,radius)
        if why: fail('clearance_violation',why)
        for other,c,d,r,ls,poly in obstacles:
            if other==net or not set(layers).intersection(ls): continue
            gap=(min(prim_distance(('seg',tuple(a),tuple(b),0.0),q) for q in poly[1]) if poly and poly[0]=='prims' else poly_distance(a,b,poly) if poly else distance(a,b,c,d)-r)-radius
            if gap<clearance-1e-6:
                what=('net %s'%other) if other else 'a hole, keepout or free copper'
                fail('clearance_violation','Candidate copper (%s at (%.3f, %.3f)%s on %s) comes %.3f mm from %s at (%.3f, %.3f); the clearance is %.3f mm'%(
                    'via' if via else 'track',a[0],a[1],'' if via else '-(%.3f, %.3f)'%(b[0],b[1]),'all layers' if via else Layers(board).name(item['layer']),max(gap,0),what,c[0],c[1],clearance))
    return {'net':net,'items':planned,'clearance':clearance,'preflight':'Conservative geometry only; run native Autodesk DRC after edits'}


def routing_context(board,args,net,width,drill):
    nets,pads,wires,vias=objects(board)
    # any stack-up: a through pad, via or hole is on every copper layer of it. Pours are not
    # obstacles: Fusion repours them round the new copper (RATSNEST after the route).
    stack=Layers(board).order
    # artwork the preflight models (artwork_obstacles); a via keepout, milling or copper text it does not
    copper=set(str(n) for n in stack)
    for n in board.findall('plain/*') + board.findall('libraries/library/packages/package/*'):
        if n.tag in ('pad','smd','hole'): continue
        modelled=n.tag in ('wire','circle','rectangle','polygon','polygonshape') and n.get('layer') in copper|{'41','42'}
        if n.get('layer') in copper|{'41','42','43','46'} and not modelled:
            fail('unsupported_board','%s on layer %s (a via keepout, milling or copper text) is not modelled by this preflight'%(n.tag,n.get('layer')))
    def rule(name,default):
        node=board.find("designrules/param[@name='%s']"%name)
        if node is None: return default
        value=node.get('value',''); m=re.fullmatch(r'([0-9.]+)(mm|mil)',value)
        if not m: fail('unsupported_rules','Cannot interpret '+name+'='+value)
        return float(m[1])*(0.0254 if m[2]=='mil' else 1)
    # the board's own rules decide; a caller's clearance can only raise them
    clearance=max(number(args.get('clearance',0.0001),True),*(rule(k,0.2) for k in
                  ('mdWireWire','mdWirePad','mdWireVia','mdPadPad','mdPadVia','mdViaVia')))
    # Honor the requested net class and global minimums; cross-class clearance below.
    classes=board.findall('classes/class')
    selected=next((c for c in classes if c.get('number')==nets[net].get('class','0')),None)
    if selected is not None:
        clearance=max([clearance]+[float(c.get('value')) for c in selected.findall('clearance')])
        if width<float(selected.get('width','0')) or (drill and drill<float(selected.get('drill','0'))):
            fail('design_rule_violation','Width/drill is below this net class minimum')
    # the via drill only matters when the path changes layer (a pure trace has no hole)
    if width<rule('msWidth',0.2) or (drill and drill<rule('msDrill',0.3)):
        fail('design_rule_violation','Width/drill is below the board minimum')
    # ANY closed outline (page issue #123): straight lines and arcs (rounded corners, curves,
    # notches) on layer 20, chained into loops. The largest loop is the board edge, the rest are
    # cutouts. Arcs become polylines; their sagitta is added to the edge margin so the check
    # stays conservative on a concave curve.
    loops,sagitta=outline_loops(board)
    obstacles=[]
    # pads by their real outline (board_model's primitives: a round or long pad a fat segment, the
    # rest its polygon), as the engine's plan_route measures them; the bounding circle refused a via
    # 1.3 mm from a long pad's centre on BQ25792
    contacts={(c.get('element'),c.get('pad')):sg.get('name') for sg in board.findall('signals/signal') for c in sg.findall('contactref')}
    exact={row['ref']+'.'+row['name']:pad_prims(row) for row in pads_of(board,Layers(board),contacts)[0].values()}
    for k,p in pads.items():
        prims=exact.get(k)
        obstacles.append((p['net'],p['at'],p['at'],p['radius'],stack if p['layers']==[1,16] else p['layers'],('prims',prims) if prims else p.get('poly')))
    for w in wires:
        # a curved track is its arc as short chords, each grown by the chord-to-arc gap
        pts,sag=arc_points(w['start'],w['end'],w['curve'])
        for a_,b_ in zip(pts,pts[1:]): obstacles.append((w['net'],a_,b_,w['width']/2+sag,[w['layer']],None))
    # a via's copper is its diameter, or the drill plus the board's via restring when the
    # diameter is left to the rules (0); the old drill*2+0.6 doubled a 0.6 mm via
    def ratio(name,default):
        node=board.find("designrules/param[@name='%s']"%name)
        try: return float(node.get('value')) if node is not None else default
        except ValueError: return default
    def via_dia(d,drill):
        ring=min(max(drill*ratio('rvViaOuter',0.25),rule('rlMinViaOuter',0.2032)),rule('rlMaxViaOuter',0.508))
        return max(d,drill+2*ring)
    for v in vias: obstacles.append((v['net'],v['at'],v['at'],via_dia(v['diameter'],v['drill'])/2,stack,None))
    for h in board.findall('plain/hole'):
        xy=[float(h.get('x')),float(h.get('y'))]; obstacles.append((None,xy,xy,float(h.get('drill'))/2,stack,None))
    obstacles+=artwork_obstacles(board,pads,stack)
    # a footprint's mounting holes, placed with their part
    packages={(l.get('name'),p_.get('name')):p_ for l in board.findall('libraries/library') for p_ in l.findall('packages/package')}
    for el in board.findall('elements/element'):
        pkg=packages.get((el.get('library'),el.get('package')))
        if pkg is None: continue
        xf,_,_=placement(el)
        for h in pkg.findall('hole'):
            xy=list(xf(float(h.get('x')),float(h.get('y')))); obstacles.append((None,xy,xy,float(h.get('drill'))/2,stack,None))
    return {'via_dia':via_dia,'clearance':clearance,'edgeClearance':rule('mdCopperDimension',0.5)+sagitta,'outline':loops,'bounds':(min(p[0] for p in loops[0]),max(p[0] for p in loops[0]),min(p[1] for p in loops[0]),max(p[1] for p in loops[0])),'obstacles':obstacles,'pads':pads}


def native_drc(app):
    with tempfile.TemporaryDirectory(prefix='adom-routing-drc-') as d:
        out=Path(d)/'drc.tsv'; ulp=Path(d)/'drc.ulp'
        # UL_ERROR exposes code/type/state/layer/x/y/description (x,y in 1/320000 mm); it has
        # no signal or object members, so the caller locates the objects from the coordinates.
        ulp.write_text('board(B) { output("'+out.as_posix()+'") { printf("CHECKED\\t%d\\n", B.checked); B.errors(E) { printf("ERROR\\t%d\\t%d\\t%d\\t%d\\t%d\\t%d\\t%s\\n", E.code, E.type, E.state, E.layer, E.x, E.y, E.description); } } }')
        app.executeTextCommand('Electron.run DRC;')
        app.executeTextCommand("Electron.run RUN '"+ulp.as_posix()+"';")
        lines=out.read_text().splitlines()
    checked=bool(lines and lines[0]=='CHECKED\t1'); errors=[]
    U=320000.0
    for line in lines[1:]:
        _,code,kind,status,layer,x,y,description=line.split('\t',7)
        errors.append({'code':int(code),'type':int(kind),'state':int(status),'layer':int(layer),
                       'x':round(int(x)/U,4),'y':round(int(y)/U,4),'unit':'mm','description':description})
    # UL_ERROR.type is ERROR_TYPE_NONE 0, _WARNING 1, _ERROR 2, _CONSISTENCY 3, and .state is
    # ERROR_STATE_ACTIVE 0, _PROCESSED 1, _APPROVED 2 (read from Fusion's ULP constants, 2026-10-26;
    # before 1.12.0 the bridge read type 2 as a warning and so swapped the two counts)
    for e in errors:
        e['severity']='warning' if e['type']==1 else 'error'
        e['approved']=e['state']==2
    summary={}
    for e in errors:
        k=f"{e['description']} (code {e['code']}, {e['severity']})"
        summary[k]=summary.get(k,0)+1
    notes=[]
    if sum(e['code']==19 for e in errors)>20:
        notes.append('Many Overlap errors (code 19) at trace joints, pad entries and vias usually come from a Copper Clearance rule with samesignal="yes" and an all/all scope (for example a profile rule named "Same-net track spacing"): it flags every place a trace meets its own pad, via or next segment. Check the loaded rules with fusion_read_rules (or fusion_set_design_rules {action:"export"}) before treating them as shorts.')
    return {'checked':checked,'source':'Autodesk DRC + UL_BOARD.errors', 'errors':errors,'summary':summary,'notes':notes,
            'errorCount':sum(e['code']!=21 and e['severity']=='error' for e in errors),
            'warningCount':sum(e['code']!=21 and e['severity']=='warning' for e in errors),
            'airwireCount':sum(e['code']==21 for e in errors),
            'clean':checked and not errors}


def grid(v):
    """Fusion stores board coordinates in 1/320000 mm (page issue #123: a via planned at
    7.280047896 reads back at 7.280046875). Plans and readbacks both snap to that grid."""
    return round(float(v)*320000.0)/320000.0


def item_key(item):
    if 'start' in item:
        return ('segment',item['net'],item['layer'],round(item['width'],6),tuple(sorted((tuple(item['start']),tuple(item['end'])))))
    return ('via',item['net'],tuple(grid(c) for c in item['at']),round(item['drill'],6),round(item['diameter'],6),item['extent'])


def _pour_pick(e,others):
    """Points on this pour's outline and on no other pour's on its layer, best first: farthest from them. A pick is
    taken by whichever outline runs through it, so one on a shared edge can take the other pour (two pours sharing
    three edges lost the wrong one and answered verified, 2026-11-05). A copy with the same id is the same pour."""
    rings=[list(zip(o['outline'],o['outline'][1:]+o['outline'][:1])) for o in others if o['layer']==e['layer'] and o.get('id')!=e.get('id')]
    ol=e['outline']; out=[]
    for a_,b_ in zip(ol,ol[1:]+ol[:1]):
        for t_ in (0.5,0.25,0.75,0.1,0.9):
            q=(a_[0]+(b_[0]-a_[0])*t_,a_[1]+(b_[1]-a_[1])*t_)
            near=min((point_distance(q,c,d) for ring in rings for c,d in ring),default=99)
            if near>0.2: out.append((near,q))
    return [q for _,q in sorted(out,key=lambda x:-x[0])]


def _pour_delete_cmd(e,pad_pts,others=()):
    """DELETE of one whole pour, picked where only its own outline runs, and far from every pad, so it can never take
    a part with it. Shift-click (S x y) deletes the WHOLE polygon; a plain click deletes one vertex of it."""
    picks=_pour_pick(e,others)
    if not picks: fail('unsafe_pick','Every edge of the %s pour on layer %d lies along another pour, so a pick could take the wrong one: remove the other pour first, or remove it in Fusion by hand'%(e['net'],e['layer']))
    best=None
    for q in picks:
        dmin=min((math.hypot(q[0]-pp[0],q[1]-pp[1]) for pp in pad_pts),default=99)
        if best is None or dmin>best[0]: best=(dmin,q)
    if best[0]<1.0: fail('unsafe_pick','No outline point of the %s pour is 1 mm clear of a pad and of other pours; remove it in Fusion by hand'%e['net'])
    return 'LAYER %d; DELETE (S %.6fmm %.6fmm);'%(e['layer'],*best[1])


def _pour_draw_cmd(net,layer,outline,o,pour='solid'):
    path=' '.join('(%.6fmm %.6fmm)'%(x,y) for x,y in list(outline)+list(outline[:1]))
    return ("CHANGE ISOLATE %.6fmm; CHANGE RANK %d; CHANGE POUR %s; CHANGE ORPHANS %s; CHANGE THERMALS %s; LAYER %d; POLYGON '%s' %.6fmm %s;"
            %(number(o['isolate']),int(o['rank']),pour.upper(),'ON' if o['orphans'] else 'OFF','ON' if o['thermals'] else 'OFF',layer,identifier(net),number(o['width'],True),path))


def _cutout_rects(board,cutouts):
    """apply_pours' cutouts as rectangles on Fusion's restrict layers (tRestrict, bRestrict), which pours
    stay out of. Row by row, as POLYGON would ask for a signal name in a modal. Fusion has no restrict layer
    for an inner layer, so a cutout there is refused before anything is drawn."""
    if not cutouts: return []
    if not isinstance(cutouts,list): fail('invalid_args','cutouts must be a list of {layer, points}')
    out=[]
    for c_ in cutouts:
        layer=eagle_layer(c_.get('layer',1),board)
        if layer not in (1,16):
            fail('not_supported','A cutout on %s: Fusion has restrict layers for the top and bottom only. Shape that pour\'s outline around the area instead'%Layers(board).name(layer))
        pts=[(number(a),number(b)) for a,b in c_.get('points') or []]
        if len(pts)<3: fail('invalid_geometry','a cutout needs at least three points')
        rects=[r_ for r_ in svg_rects([pts],0.05) if r_[2]-r_[0]>=0.005 and r_[3]-r_[1]>=0.005]
        if len(rects)>1000: fail('too_detailed','A cutout needs %d rectangles: give it fewer sloped edges'%len(rects))
        out.append({'layer':Layers(board).name(layer),'restrictLayer':41 if layer==1 else 42,'rects':rects})
    return out


def undo_route(app,args,commands,before_items):
    """Undo a route one step at a time until the copper is what it was. A command that landed nothing
    leaves no undo step, so it stops on the copper rather than on a count: that way it cannot take back
    an earlier edit. Returns (restored, board, meta)."""
    restored=False
    for _ in commands:
        app.executeTextCommand('Electron.run UNDO;')
        _,back,back_meta=snapshot(app,args); o=objects(back)
        restored=copper_signature(o[2]+o[3])==before_items
        if restored: break
    app.executeTextCommand('Electron.run RATSNEST;')
    _,back,back_meta=snapshot(app,args)
    return restored,back,back_meta


def copper_signature(items):
    """Compare occupied centerline intervals, tolerating Fusion splitting/merging wires."""
    groups=collections.defaultdict(list); vs=[]
    for item in items:
        if 'start' not in item:
            vs.append(item_key(item));continue
        a,b=(tuple(grid(c) for c in item['start']),tuple(grid(c) for c in item['end']))
        if a>b:a,b=b,a
        dx,dy=b[0]-a[0],b[1]-a[1];length=math.hypot(dx,dy)
        if not length:continue
        ux,uy=dx/length,dy/length
        key=(item['net'],item['layer'],round(item['width'],6),round(ux,7),round(uy,7),round(-uy*a[0]+ux*a[1],6))
        groups[key].append((round(ux*a[0]+uy*a[1],6),round(ux*b[0]+uy*b[1],6)))
    result=[]
    for key,spans in sorted(groups.items()):
        merged=[]
        for a,b in sorted(spans):
            if merged and a<=merged[-1][1]+1e-6:merged[-1][1]=max(merged[-1][1],b)
            else:merged.append([a,b])
        result.append((key,merged))
    return result,sorted(vs)


# ---- schematic: complete, non-interactive EAGLE commands only, each its own step ----
_PT = r"\(\s*-?[0-9.]+mm\s+-?[0-9.]+mm\s*\)"
SCH_FORMS = [
    ('ADD', re.compile(r"^ADD '([^'@]+)@([A-Za-z0-9_.-]+)' '([A-Za-z0-9_$+-]+)' (?:M?R(?:0|90|180|270)) " + _PT + r";$")),
    ('VALUE', re.compile(r"^VALUE '[A-Za-z0-9_$+-]+' '[^';]{1,40}';$")),
    ('NET', re.compile(r"^NET '[A-Za-z0-9_+./:$!-]+'(?: " + _PT + r"){2,32};$")),
    ('LABEL', re.compile(r"^LABEL " + _PT + " " + _PT + r";$")),
    ('WIRE', re.compile(r"^WIRE [0-9.]+mm(?: " + _PT + r"){2,32};$")),
    ('TEXT', re.compile(r"^TEXT '[^';]{1,80}' (?:R0|R90) " + _PT + r";$")),
    ('LAYER', re.compile(r"^LAYER [0-9]{1,3};$")),
    ('CHANGE', re.compile(r"^CHANGE (?:STYLE (?:Continuous|LongDash|ShortDash|DashDot)|SIZE [0-9.]+mm|WIDTH [0-9.]+mm|RATIO [0-9]{1,2});$")),
    ('WINDOW', re.compile(r"^WINDOW FIT;$")),
]


def sch_snapshot(app):
    doc = app.activeDocument
    if not doc or app.activeProduct.productType != 'SchematicProductType':
        fail('wrong_document', 'Activate the schematic (fusion_schematic {action:"show"}) before building it')
    with tempfile.TemporaryDirectory(prefix='adom-sch-') as d:
        p = Path(d) / 'snapshot.fsch'
        app.executeTextCommand('Document.CopyToDesktop ' + p.as_posix())
        with zipfile.ZipFile(p) as z:
            entries = [n for n in z.namelist() if n.endswith('.sch')]
            if len(entries) != 1: fail('ambiguous_export', 'Expected exactly one schematic XML in the fresh Fusion export')
            root = ET.fromstring(z.read(entries[0]))
    sch = root.find('drawing/schematic')
    parts = sorted(p.get('name') for p in sch.iter('part'))
    nets = {}
    for n in sch.iter('net'):
        nets.setdefault(n.get('name'), set()).update('%s.%s' % (r.get('part'), r.get('pin')) for r in n.iter('pinref'))
    return doc.name, parts, {k: sorted(v) for k, v in nets.items()}


SHEET_LIMIT_MM = 1950.0


def sch_run(app, args):
    """Run a list of schematic commands. ADD is proven against the .lbr on disk first (the
    device must exist there), so the component picker can never open."""
    import time
    name, before, _ = sch_snapshot(app)
    if args.get('expectDocument') not in (None, name): fail('wrong_document', 'expectDocument must match the active schematic name exactly')
    cmds = args.get('commands')
    # the bridge sends a longer list in batches (routing.rs), having checked the whole of it here first
    limit = 20000 if args.get('validateOnly') else 400
    if not isinstance(cmds, list) or not 1 <= len(cmds) <= limit: fail('invalid_args', 'commands must be a list of 1..%d schematic commands' % limit)
    lbr = args.get('libraryPath')
    devices = set()
    if lbr:
        if not Path(lbr).is_file(): fail('library_missing', 'libraryPath is not a file on this machine: ' + str(lbr))
        lroot = ET.parse(lbr).getroot()
        devices = {d.get('name').upper() for d in lroot.iter('deviceset')}
        libname = Path(lbr).stem
    for c in cmds:
        form = next((f for f, rx in SCH_FORMS if isinstance(c, str) and rx.match(c.strip())), None)
        if not form: fail('command_not_allowed', 'Not a complete, non-interactive schematic command: %r' % (c,))
        if form == 'ADD':
            m = SCH_FORMS[0][1].match(c.strip())
            if not lbr: fail('library_required', 'ADD needs libraryPath so the device is proven to exist before ADD runs')
            if m.group(2) != libname: fail('library_mismatch', 'ADD names library %s but libraryPath is %s' % (m.group(2), libname))
            if m.group(1).upper() not in devices: fail('unknown_device', 'No deviceset %s in %s' % (m.group(1), lbr))
    # Fusion silently drops a part placed about 2 m out (a small part landed at 1988 mm, a larger one failed
    # at 1990 mm and 2100 mm: live on 2706), answers nothing, and leaves that part name unusable afterwards
    for c in cmds:
        if c.strip().startswith('ADD '):
            at = re.search(r'\(\s*(-?[0-9.]+)mm\s+(-?[0-9.]+)mm\s*\)', c)
            if at and max(abs(float(at.group(1))), abs(float(at.group(2)))) > SHEET_LIMIT_MM:
                fail('off_the_sheet', 'Fusion cannot place a part this far out (past about 2 m it drops it silently and the name stays unusable): %s. Keep positions within %g mm; nothing was run.' % (c.strip(), SHEET_LIMIT_MM))
    if args.get('validateOnly'):
        return {'success': True, 'mutated': False, 'documentName': name, 'commands': len(cmds)}
    done = []
    # ADD takes the device from the .lbr loaded with USE. (Fusion cannot take a library out of use from a
    # command, so two in-use libraries with the same name at different paths make ADD ask which one.)
    prod = app.activeProduct
    def part_names():
        try: return {prod.parts.item(i).name.upper() for i in range(prod.parts.count)}
        except Exception: return None
    held = set()
    try:
        for i in range(prod.libraries.count): held.add(prod.libraries.item(i).name)
    except Exception: pass
    used = 'use' if lbr else None
    reformatted = False
    orphans_removed = []
    if lbr:
        # a compact .lbr (no line break between elements) is rewritten first: Fusion 2706 asserts
        # (PlacePartWorker assetAdded) on every ADD from one (#135; lbr_use.py)
        try: reformatted = use_library(app, lbr, orphans_removed)
        except RuntimeError as ex:
            code = str(ex).split(':', 1)[0] if str(ex).startswith(('library_not_writable', 'library_name_in_use')) else 'library_unreadable'
            fail(code, str(ex).split(': ', 1)[-1] if code != 'library_unreadable' else str(ex))
    pause = min(max(float(args.get('pauseMs', 0)), 0), 3000) / 1000.0
    for c in cmds:
        c = c.strip()
        # VALUE on a part whose deviceset takes no user value asks "change it anyway?": we do
        # intend to, so the answer goes with the command and the modal is never drawn
        if c.startswith('VALUE '): c = 'SET CONFIRM YES; ' + c + ' SET CONFIRM OFF;'
        # ADD of a device set the design already holds in another version asks "Ok to update?":
        # the library on disk was just proven, so updating to it is the intent
        is_add = c.startswith('ADD ')
        if is_add: c = 'SET CONFIRM YES; ' + c + ' SET CONFIRM OFF;'
        app.executeTextCommand('Electron.run ' + c)
        done.append(c)
        if is_add:
            # the part must be on the sheet now: stop at the first ADD that did not land, naming it,
            # instead of running on (#135 retried for minutes past the caller's timeout)
            want = SCH_FORMS[0][1].match(c.replace('SET CONFIRM YES; ', '').replace(' SET CONFIRM OFF;', '').strip()).group(3)
            names = part_names()
            if names is not None and want.upper() not in names:   # EAGLE keeps part names in upper case
                return {'success': False, 'mutated': len(done) > 1, 'errorCode': 'add_failed', 'failedAt': len(done) - 1, 'failedCommand': c, 'part': want,
                        'libraryFrom': used, 'libraryLoaded': lbr, 'libraryReformatted': reformatted, 'library3dEntriesRemoved': orphans_removed, 'librariesHeld': sorted(held), 'commands': len(done), 'documentName': name,
                        'error': 'ADD of %s did not land: Fusion found no such device in %s (%s). The commands before it ran.' % (want, libname,
                                 'the design holds that library, so the device is missing from it' if used == 'design' else 'Fusion did not take it from the .lbr loaded with USE'),
                        '_hint': 'The bridge loads libraryLoaded with USE itself, so USE is not in the command list. Check the deviceset is in that file and that the schematic in front is the design you meant (documentName); then run the rest of the commands.'}
        if pause:
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            import adsk; adsk.doEvents(); time.sleep(pause)
    app.executeTextCommand('Electron.run WINDOW FIT;')
    app.executeTextCommand('Electron.ImmediateRepaint /full')
    _, after, nets = sch_snapshot(app)
    want = sorted({SCH_FORMS[0][1].match(c.strip()).group(3) for c in cmds if c.strip().startswith('ADD ')})
    missing = [p for p in want if p.upper() not in {a.upper() for a in after}]
    ok = not missing
    return {'success': ok, 'mutated': True, 'verified': ok, 'errorCode': None if ok else 'readback_mismatch',
            'error': None if ok else 'parts not on the sheet after ADD: %s' % missing, 'documentName': name,
            'parts': after, 'added': [p for p in after if p not in before], 'nets': nets, 'commands': len(done), 'libraryFrom': used, 'libraryLoaded': lbr, 'libraryReformatted': reformatted, 'library3dEntriesRemoved': orphans_removed}


def board_bom(board, args):
    """The BOM from the live board's own element attributes: one line per part number, and the
    JLCPCB form (Comment, Designator, Footprint, LCSC). Parts with no MPN (test pads) are listed
    as board-only, never dropped silently."""
    import csv, io
    rows = {}
    excluded = []
    for e in board.iter('element'):
        flagged = {a.get('name') for a in e.findall('attribute') if str(a.get('value', '')).lower() in ('yes', 'true', '1')}
        if e.get('populate') == 'no' or flagged & {'DNP', 'EXCLUDE_FROM_BOM'}:
            excluded.append(e.get('name')); continue
        ids = part_ids(e)        # MPN, LCSC... under any of the names libraries use (board_model.py)
        mpn = ids['mpn']
        # without an MPN, parts are one line only when footprint, value and LCSC agree: grouping by
        # footprint alone put a 47 nF and a 10 uF cap on one line, named 'test point'
        key = mpn or ('board-only:%s|%s|%s' % (e.get('package', ''), e.get('value', ''), ids['lcsc']))
        r = rows.setdefault(key, {'designators': [], 'value': e.get('value', ''), 'package': e.get('package', ''),
                                 'mpn': mpn, 'manufacturer': ids['manufacturer'], 'mouser': ids['mouser'],
                                 'digikey': ids['digikey'], 'lcsc': ids['lcsc']})
        r['designators'].append(e.get('name')); r.setdefault('_values', set()).add(e.get('value', ''))
    for r in rows.values():
        # one value for the line only when every part on it agrees (contacts carry signal names)
        vals = r.pop('_values')
        r['value'] = vals.pop() if len(vals) == 1 and '' not in vals else (r['mpn'] or 'test point')
    def nat(n): return [int(t) if t.isdigit() else t for t in re.split(r'(\d+)', n)]
    lines = sorted(rows.values(), key=lambda r: nat(sorted(r['designators'], key=nat)[0]))
    for r in lines: r['designators'] = sorted(r['designators'], key=nat); r['quantity'] = len(r['designators'])
    out = io.StringIO(); w = csv.writer(out)
    w.writerow(['Designators', 'Quantity', 'Value', 'Package', 'MPN', 'Manufacturer', 'Mouser', 'DigiKey', 'LCSC'])
    for r in lines: w.writerow([' '.join(r['designators']), r['quantity'], r['value'], r['package'], r['mpn'] or '(board only)', r['manufacturer'], r['mouser'], r['digikey'], r['lcsc']])
    jo = io.StringIO(); jw = csv.writer(jo)
    jw.writerow(['Comment', 'Designator', 'Footprint', 'LCSC'])
    for r in lines:
        if r['mpn']: jw.writerow([r['value'] or r['mpn'], ','.join(r['designators']), r['package'], r['lcsc']])
    files = {}
    d = args.get('outputDir')
    if d:
        Path(d).mkdir(parents=True, exist_ok=True)
        for name, text in (('bom.csv', out.getvalue()), ('bom-jlcpcb.csv', jo.getvalue())):
            (Path(d) / name).write_text(text, encoding='utf-8'); files[name] = (Path(d) / name).as_posix()
    missing = {k: [' '.join(r['designators']) for r in lines if r['mpn'] and not r[k]] for k in ('mouser', 'digikey', 'lcsc')}
    return {'lines': lines, 'excluded': sorted(excluded), 'csv': out.getvalue(), 'jlcpcbCsv': jo.getvalue(), 'files': files,
            'boardOnly': [' '.join(r['designators']) for r in lines if not r['mpn']], 'missingPartNumbers': missing}


def schematic_plan(args):
    """A grouped schematic for a netlist, as fusion_schematic_run command groups. Every block of the
    netlist gets a labelled frame; its parts sit in a row; every connected pin gets a short stub
    named for its net and a label at the stub's end (no wire ever ends on a pin: Fusion attaches a
    NET's first and middle points to pins, never its last). Pure computation from the .lbr on disk."""
    lbr = args.get('libraryPath')
    if not lbr or not Path(lbr).is_file(): fail('library_missing', 'libraryPath must be the .lbr on this machine')
    lroot = ET.parse(lbr).getroot(); lib = lroot.find('drawing/library'); libname = Path(lbr).stem
    sym = {x.get('name'): x for x in lib.iter('symbol')}
    dsets = {d.get('name').upper(): d for d in lib.iter('deviceset')}
    parts = args.get('parts')
    if not isinstance(parts, list) or not parts: fail('invalid_args', 'parts must be [{ref, deviceset, value?, block?}]')
    nets = args.get('nets') or {}
    def P(p): return '(%.3fmm %.3fmm)' % p
    info = {}
    for q in parts:
        d = dsets.get(str(q.get('deviceset', '')).upper())
        if d is None: fail('unknown_device', 'No deviceset %s in %s' % (q.get('deviceset'), lbr))
        g = d.find('gates/gate'); s_ = sym[g.get('symbol')]
        dev = d.find('devices/device'); pad2pin = {}
        for c in dev.iter('connect'):
            for pad in c.get('pad', '').split(): pad2pin[pad] = c.get('pin')
        pins = {p.get('name'): (float(p.get('x')), float(p.get('y')), p.get('rot', 'R0')) for p in s_.iter('pin')}
        xs = [v[0] for v in pins.values()] + [0]; ys = [v[1] for v in pins.values()] + [0]
        for w in s_.iter('wire'):
            xs += [float(w.get('x1')), float(w.get('x2'))]; ys += [float(w.get('y1')), float(w.get('y2'))]
        info[q['ref']] = {'ds': d.get('name'), 'pins': pins, 'pad2pin': pad2pin, 'bbox': (min(xs), min(ys), max(xs), max(ys)),
                          'value': q.get('value'), 'block': q.get('block') or 'parts'}
    # net of each (ref, pin): netlist pins may be named by pad ("U1.3") or by pin ("U1.VIN")
    pin_net = {}
    for n, members in nets.items():
        identifier(n)
        for m in members:
            ref, _, p_ = str(m).partition('.')
            if ref not in info: continue
            pn = p_ if p_ in info[ref]['pins'] else info[ref]['pad2pin'].get(p_)
            if pn: pin_net[(ref, pn)] = n
    blocks = []
    for q in parts:
        b = info[q['ref']]['block']
        if b not in blocks: blocks.append(b)
    out_dir = {'R0': (-1, 0), 'R90': (0, -1), 'R180': (1, 0), 'R270': (0, 1)}
    steps = []; cols = int(args.get('columns', 3)); cell_w, x0, y_top = 0.0, 20.32, 165.1
    col_x = [x0]; row_y = y_top; row_h = 0.0; placed_in_row = 0
    for bi, b in enumerate(blocks):
        refs = [q['ref'] for q in parts if info[q['ref']]['block'] == b]
        # parts in a row, each spaced by its own width plus room for labels
        layout = []; cx = 0.0; h = 0.0
        for r in refs:
            x1, y1, x2, y2 = info[r]['bbox']
            px = cx - x1 + 12.7
            layout.append((r, px)); cx = px + x2 + 12.7; h = max(h, y2 - y1)
        fw = max(cx + 7.62, 50.8); fh = h + 30.48
        if placed_in_row >= cols: row_y -= row_h + 10.16; placed_in_row = 0; row_h = 0.0; cell_x = x0
        cell_x = x0 if placed_in_row == 0 else cell_x
        fx1, fy2 = cell_x, row_y; fx2, fy1 = fx1 + fw, fy2 - fh
        cmds = ['LAYER 97;', 'CHANGE STYLE ShortDash;', 'WIRE 0.1524mm %s %s %s %s %s;' % (P((fx1, fy1)), P((fx2, fy1)), P((fx2, fy2)), P((fx1, fy2)), P((fx1, fy1))),
                'CHANGE STYLE Continuous;', 'CHANGE SIZE 2.54mm;', "TEXT '%s' R0 %s;" % (str(b).replace("'", '')[:60].title(), P((fx1 + 2.54, fy2 - 5.08))), 'CHANGE SIZE 1.778mm;', 'LAYER 91;']
        cy = fy1 + 10.16 - min(info[r]['bbox'][1] for r, _ in layout) if layout else fy1
        for r, px in layout:
            at = (round(fx1 + px, 3), round(cy, 3))
            cmds.append("ADD '%s@%s' '%s' R0 %s;" % (info[r]['ds'], libname, r, P(at)))
            if info[r]['value']: cmds.append("VALUE '%s' '%s';" % (r, str(info[r]['value']).replace("'", '')[:40]))
            for pn, (x, y, rot) in info[r]['pins'].items():
                n = pin_net.get((r, pn))
                if not n: continue
                dx, dy = out_dir.get(rot.lstrip('MS'), (-1, 0))
                a = (round(at[0] + x, 3), round(at[1] + y, 3)); e = (round(a[0] + dx * 5.08, 3), round(a[1] + dy * 5.08, 3))
                cmds += ["NET '%s' %s %s;" % (n, P(a), P(e)), 'LABEL %s %s;' % (P(e), P(e))]
        steps.append({'group': b, 'parts': refs, 'commands': cmds})
        cell_x = fx2 + 10.16; placed_in_row += 1; row_h = max(row_h, fh)
    unplaced = sorted({m for ms in nets.values() for m in ms if str(m).partition('.')[0] not in info})
    return {'success': True, 'mutated': False, 'libraryPath': lbr, 'groups': steps,
            'commandCount': sum(len(x['commands']) for x in steps), 'notInParts': unplaced,
            '_hint': 'Run each group with fusion_schematic_run {libraryPath, commands} (one call per group, so a viewer sees the sheet grow block by block), then read the nets it returns against your netlist.'}


def board_seed(args):
    """The design a board starts from, written as EAGLE source for fusion_new_electronics_from_eagle:
    the outline, the fab's design rules (Fusion's own rules block with the given values set), the
    fixed parts at their places with their nets, and the library they come from. Fusion's defaults
    (0.35 mm drill, 1 mm to the edge, 25 % pad restring) are the wrong rules for most fabs."""
    lbr = args.get('libraryPath'); out = args.get('outDir'); name = args.get('name') or 'Board'
    if not lbr or not Path(lbr).is_file(): fail('library_missing', 'libraryPath must be the .lbr on this machine')
    if not out: fail('invalid_args', 'outDir (a folder on this machine) is required')
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,48}', name): fail('invalid_args', 'name: letters, digits, _ and - only')
    w, h = [number(v, True) for v in (args.get('outlineMm') or [0, 0])]
    root = ET.parse(lbr).getroot(); layers = ET.tostring(root.find('drawing/layers'), encoding='unicode')
    lib = root.find('drawing/library'); LIB = Path(lbr).stem; lib.set('name', LIB)
    fixed = args.get('fixed') or []
    keep = {str(f.get('deviceset')).upper() for f in fixed}
    # the fixed parts' devices only (wholeLibrary:true embeds every one); schematic_run ADDs the rest
    # through USE of the .lbr
    if args.get('wholeLibrary') is not True:
        for d in list(lib.find('devicesets')):
            if d.get('name').upper() not in keep: lib.find('devicesets').remove(d)
    used_sym = {g.get('symbol') for d in lib.iter('deviceset') for g in d.iter('gate')}
    used_pkg = {x.get('package') for d in lib.iter('deviceset') for x in d.iter('device')}
    for x in list(lib.find('symbols')):
        if x.get('name') not in used_sym: lib.find('symbols').remove(x)
    for x in list(lib.find('packages')):
        if x.get('name') not in used_pkg: lib.find('packages').remove(x)
    p3 = lib.find('packages3d'); urn = {}
    if p3 is not None:
        for q in list(p3):
            names = [pi.get('name') for pi in q.iter('packageinstance')]
            if not any(n in used_pkg for n in names): p3.remove(q); continue
            for n in names: urn[n] = q.get('wip_urn') or q.get('urn')
    lib_sch = ET.tostring(lib, encoding='unicode'); lib_b = ET.fromstring(lib_sch)
    for tag in ('symbols', 'devicesets'):
        n = lib_b.find(tag)
        if n is not None: lib_b.remove(n)
    lib_brd = ET.tostring(lib_b, encoding='unicode')
    # no parts in the seed: no library either, rather than an empty one under the project library's
    # name (#136). schematic_run loads the .lbr itself for its ADDs.
    if not any(True for _ in lib.iter('deviceset')): lib_sch = lib_brd = ''
    # the rules: Fusion's own block, with the values the caller gives
    rules = FUSION_DEFAULT_RULES; r = args.get('rules') or {}
    def setp(k, v):
        nonlocal rules; rules = re.sub(r'(<param name="%s" value=")[^"]*"' % k, lambda m: m.group(1) + v + '"', rules)
    def setr(t, v):
        nonlocal rules; rules = re.sub(r'(<rule type="%s"[^>]*?) value="[^"]*" preferredvalue="[^"]*"' % t, lambda m: m.group(1) + ' value="%s" preferredvalue="%s"' % (v, v), rules)
    mm = lambda k: '%gmm' % number(r[k], True)
    if 'clearance' in r:
        for k in ('mdWireWire', 'mdWirePad', 'mdWireVia', 'mdPadPad', 'mdPadVia', 'mdViaVia', 'mdSmdPad', 'mdSmdVia', 'mdSmdSmd', 'mdViaViaSameLayer'): setp(k, mm('clearance'))
        setr('Copper Clearance', mm('clearance'))
    if 'edge' in r: setp('mdCopperDimension', mm('edge')); setr('Dimension Clearance', mm('edge'))
    if 'minWidth' in r: setp('msWidth', mm('minWidth')); setr('Minimum Copper Width', mm('minWidth'))
    if 'minDrill' in r: setp('msDrill', mm('minDrill')); setr('Minimum Drill Size', mm('minDrill'))
    if 'holeToHole' in r: setp('mdDrill', mm('holeToHole')); setr('Drill Distance', mm('holeToHole'))
    if 'padRestringRatio' in r:
        for k in ('rvPadTop', 'rvPadInner', 'rvPadBottom'): setp(k, '%g' % number(r['padRestringRatio'], True))
    if 'padRestring' in r:
        for k in ('rlMinPadTop', 'rlMinPadInner', 'rlMinPadBottom'): setp(k, mm('padRestring'))
    if 'viaRestring' in r:
        for k in ('rlMinViaOuter', 'rlMinViaInner'): setp(k, mm('viaRestring'))
    if 'copperUm' in r: setp('mtCopper', ' '.join(['%gmm' % (number(r['copperUm'], True) / 1000.0)] * 16))
    rules = rules.replace('name="Fusion Electronics Design Rules"', 'name="%s rules"' % name)
    parts, inst, nets, elements, signals = [], [], {}, [], {}
    for i, f in enumerate(fixed):
        ref = identifier(f.get('ref')); dsn = str(f.get('deviceset'))
        d = next((x for x in lib.iter('deviceset') if x.get('name').upper() == dsn.upper()), None)
        g = d.find('gates/gate').get('name'); dev = d.find('devices/device'); pkg = dev.get('package')
        val = str(f.get('value', '')).replace('"', '')
        sym_pins = {pp.get('name'): pp for pp in lib.find("symbols/symbol[@name='%s']" % d.find('gates/gate').get('symbol')).iter('pin')}
        parts.append('<part name="%s" library="%s" deviceset="%s" device="%s" value="%s"%s/>' % (ref, LIB, d.get('name'), dev.get('name') or '', val, ' package3d_urn="%s"' % urn[pkg] if pkg in urn else ''))
        sx, sy = 20.32, 152.4 - i * 7.62
        inst.append('<instance part="%s" gate="%s" x="%.2f" y="%.2f"/>' % (ref, g, sx, sy))
        tech = dev.find('technologies/technology')
        atts = ''.join('<attribute name="%s" value="%s" x="%.3f" y="%.3f" size="1.778" layer="27" display="off"/>' % (a.get('name'), a.get('value'), number(f['x']), number(f['y'])) for a in (tech.findall('attribute') if tech is not None else []))
        elements.append('<element name="%s" library="%s" package="%s" value="%s" x="%.3f" y="%.3f"%s%s>%s</element>' % (ref, LIB, pkg, val, number(f['x']), number(f['y']), ' rot="%s"' % rot_of(f['rot']) if f.get('rot') else '', ' package3d_urn="%s"' % urn[pkg] if pkg in urn else '', atts))
        for pin, net in (f.get('nets') or {}).items():
            identifier(net); pp = sym_pins.get(pin)
            if pp is None: fail('unknown_pin', '%s has no pin %s' % (ref, pin))
            px, py = sx + float(pp.get('x')), sy + float(pp.get('y'))
            nets.setdefault(net, []).append('<segment><pinref part="%s" gate="%s" pin="%s"/><wire x1="%.2f" y1="%.2f" x2="%.2f" y2="%.2f" width="0.1524" layer="91"/><label x="%.2f" y="%.2f" size="1.778" layer="95"/></segment>' % (ref, g, pin, px, py, px + 7.62, py, px + 7.62, py))
            for c in dev.iter('connect'):
                if c.get('pin') == pin:
                    for pad in c.get('pad').split(): signals.setdefault(net, []).append('<contactref element="%s" pad="%s"/>' % (ref, pad))
    head = '<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE eagle SYSTEM "eagle.dtd">\n<eagle version="9.6.2"><drawing><settings><setting alwaysvectorfont="no"/><setting verticaltext="up"/></settings><grid distance="0.1" unitdist="inch" unit="inch" style="lines" multiple="1" display="no" altdistance="0.01" altunitdist="inch" altunit="inch"/>'
    title = str(args.get('title', name)).replace('<', '').replace('&', 'and')[:80]
    sch = (head + layers + '<schematic xreflabel="%F%N/%S.%C%R" xrefpart="/%S.%C%R"><libraries>' + lib_sch + '</libraries><attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes><parts>' + ''.join(parts) +
           '</parts><sheets><sheet><plain><text x="20.32" y="175.26" size="3.81" layer="97">' + title + '</text></plain><instances>' + ''.join(inst) + '</instances><busses/><nets>' +
           ''.join('<net name="%s" class="0">%s</net>' % (n, ''.join(v)) for n, v in nets.items()) + '</nets></sheet></sheets></schematic></drawing></eagle>\n')
    outline = ''.join('<wire x1="%g" y1="%g" x2="%g" y2="%g" width="0" layer="20"/>' % s_ for s_ in [(0, 0, w, 0), (w, 0, w, h), (w, h, 0, h), (0, h, 0, 0)])
    brd = (head + layers + '<board><plain>' + outline + '</plain><libraries>' + lib_brd + '</libraries><attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes>' + rules +
           '<autorouter></autorouter><elements>' + ''.join(elements) + '</elements><signals>' + ''.join('<signal name="%s">%s</signal>' % (n, ''.join(v)) for n, v in signals.items()) + '</signals></board></drawing></eagle>\n')
    Path(out).mkdir(parents=True, exist_ok=True)
    sp, bp = Path(out) / (name + '.sch'), Path(out) / (name + '.brd')
    sp.write_text(sch, encoding='utf-8'); bp.write_text(brd, encoding='utf-8')
    return {'success': True, 'mutated': False, 'schPath': sp.as_posix(), 'brdPath': bp.as_posix(), 'fixedParts': len(fixed), 'outlineMm': [w, h],
            '_hint': 'Import it: fusion_new_electronics_from_eagle {schPath, brdPath} with an electronics document in front, poll fusion_eagle_import_status, then fusion_schematic {action:"show"} and fusion_schematic_plan / fusion_schematic_run for the rest of the circuit. Start from a clean Fusion session: a design imported while half-finished imports are open can come up with schematic/board sync off.'}


def _silk_text_ok(t):
    if not isinstance(t, str) or not t or len(t) > 120 or any(c in t for c in "';\n"):
        fail('bad_args', 'Silkscreen text must be 1..120 characters without quotes, semicolons or line breaks: %r' % (t,))
    return t


def apply_silkscreen(app, root, board, meta, args):
    """Land a silkscreen plan (EDA Bridge Standard apply_silkscreen): designators (names) and values
    (comments) moved, sized, turned or hidden; free texts and lines added; earlier free items removed
    by their silk_state raw handle. EAGLE's MOVE, ROTATE, CHANGE and DELETE pick the object nearest
    the point given, so only the layer holding the target is displayed while it is picked (a pad or
    a wire can never be taken instead), and the board's own layer display is put back after. Read
    back afterwards; if copper, parts or the outline moved (the revision), every command is undone."""
    if args.get('expectedRevision') not in (None, meta['revision']):
        fail('stale_board', 'expectedRevision must match the current live board')
    names, comments = args.get('names') or [], args.get('comments') or []
    texts, lines, remove = args.get('texts') or [], args.get('lines') or [], args.get('remove') or {}
    if not (names or comments or texts or lines or remove.get('texts') or remove.get('lines')):
        fail('nothing_to_do', 'Give names, comments, texts, lines or remove')
    visible = [l.get('number') for l in root.find('drawing/layers') if l.get('visible') == 'yes']
    commands, results = [], []
    def run(cmd):
        app.executeTextCommand('Electron.run ' + cmd); commands.append(cmd)
    els = {e.get('name'): e for e in board.findall('elements/element')}
    # parts whose name or value text has not been smashed yet: smash them first so the text is its own object
    want = [('NAME', n) for n in names] + [('VALUE', c) for c in comments]
    for _, item in want:
        ref = identifier(item.get('ref'))
        if ref not in els: fail('designator_not_found', 'No part %s on the board' % ref)
    # by the text itself: a part can say smashed="yes" and still have no NAME attribute of its own (a board from a
    # file, or part of one), and then MOVE and CHANGE pick nothing and the text stays where it is (#144)
    def has_own_text(el, kind): return any(a.get('name') == kind and a.get('x') is not None for a in el.findall('attribute'))
    to_smash = sorted({it['ref'] for kind, it in want if not has_own_text(els[it['ref']], kind)})
    for ref in to_smash: run("SMASH '%s';" % ref)
    if to_smash:
        root, board, _ = snapshot(app, {**args, 'expectedRevision': None})
    silk_before = rd_silk_state(root, board, {})
    silk = {c['ref']: c for c in silk_before['components']}
    try:
        for kind, it in want:
            ref = it['ref']; key = 'name' if kind == 'NAME' else 'comment'
            cur = silk[ref][key]
            if cur.get('x') is None:
                results.append('err:%s %s has no text to move' % (ref, kind)); continue
            layer = cur['layer']
            x, y = number(it['x']), number(it['y'])
            run('DISPLAY NONE %d;' % layer)
            # size first, picked where the text is now: picked at its new place, CHANGE SIZE took no
            # text there (BQ25792's SW1 name, 2026-10-29) and the move stood with the old size
            if it.get('size') is not None: run('CHANGE SIZE %.6fmm (%.6fmm %.6fmm);' % (number(it['size'], True), cur['x'], cur['y']))
            run('MOVE (%.6fmm %.6fmm) (%.6fmm %.6fmm);' % (cur['x'], cur['y'], x, y))
            if it.get('rotation') is not None:
                mirror = 'M' if layer in (22, 26, 28) else ''
                run('ROTATE =%sR%g (%.6fmm %.6fmm);' % (mirror, float(it['rotation']) % 360, x, y))
            if it.get('show') is False: run('CHANGE DISPLAY OFF (%.6fmm %.6fmm);' % (x, y))
            results.append('ok')
        for t in texts:
            side_b = t.get('side') == 'bottom'
            run('LAYER %d; CHANGE SIZE %.6fmm; TEXT \'%s\' %sR%g (%.6fmm %.6fmm);' % (22 if side_b else 21, number(t.get('size', 1.0), True), _silk_text_ok(t.get('text')),
                'M' if side_b else '', float(t.get('rotation', 0)) % 360, number(t['x']), number(t['y'])))
            results.append('ok')
        for l in lines:
            run('LAYER %d; WIRE %.6fmm (%.6fmm %.6fmm) (%.6fmm %.6fmm);' % (22 if l.get('side') == 'bottom' else 21, number(l.get('width', 0.15), True),
                number(l['x1']), number(l['y1']), number(l['x2']), number(l['y2'])))
            results.append('ok')
        for raw in list(remove.get('texts') or []) + list(remove.get('lines') or []):
            parts = str(raw).split('|')
            if parts[0] == 'text' and len(parts) >= 4:
                run('DISPLAY NONE %d;' % int(parts[1])); run('DELETE (%.6fmm %.6fmm);' % (float(parts[2]), float(parts[3]))); results.append('ok')
            elif parts[0] == 'wire' and len(parts) == 6:
                mx, my = (float(parts[2]) + float(parts[4])) / 2, (float(parts[3]) + float(parts[5])) / 2
                run('DISPLAY NONE %d;' % int(parts[1])); run('DELETE (%.6fmm %.6fmm);' % (mx, my)); results.append('ok')
            else:
                results.append('err:not a silk_state raw handle: %r' % (raw,))
    finally:
        # one layer per command: naming a layer Fusion will not toggle (the origin layers 23 and
        # 24 are always shown) makes it drop every layer after it in the same DISPLAY command
        app.executeTextCommand('Electron.run DISPLAY NONE;')
        for n in visible:
            if n not in ('23', '24'):
                try: app.executeTextCommand('Electron.run DISPLAY %s;' % n)
                except Exception: pass
    app.executeTextCommand('Electron.ImmediateRepaint /full')
    root2, after, meta2 = snapshot(app, {**args, 'expectedRevision': None})
    if meta2['revision'] != meta['revision']:
        # something that is not silkscreen moved: take every command back
        for _ in range(len(commands) + 2):
            app.executeTextCommand('Electron.run UNDO;')
            _, _, m3 = snapshot(app, {**args, 'expectedRevision': None})
            if m3['revision'] == meta['revision']: break
        fail('readback_mismatch', 'Copper, parts or the outline changed while the silkscreen was edited; every command was undone')
    st = rd_silk_state(root2, after, {})
    comps = {c['ref']: c for c in st['components']}
    i = 0
    for kind, it in want:
        if results[i] == 'ok':
            got = comps[it['ref']]['name' if kind == 'NAME' else 'comment']
            bad = []
            if abs(got.get('x', 1e9) - number(it['x'])) > 0.01 or abs(got.get('y', 1e9) - number(it['y'])) > 0.01: bad.append('at (%s, %s)' % (got.get('x'), got.get('y')))
            if it.get('size') is not None and abs(got.get('size', 0) - float(it['size'])) > 0.01: bad.append('size %s' % got.get('size'))
            if it.get('rotation') is not None and abs((got.get('rotation', 0) - float(it['rotation'])) % 360) > 0.01: bad.append('rotation %s' % got.get('rotation'))
            if it.get('show') is False and got.get('shown'): bad.append('still shown')
            if bad: results[i] = 'err:%s %s landed %s' % (it['ref'], kind, ', '.join(bad))
        i += 1
    failed = sum(1 for r_ in results if r_ != 'ok')
    if failed and not args.get('keepPartial'):
        # one text not as asked: take every command back, so the board is as it was (checked against
        # the silkscreen read before the edit), and say which texts failed
        def sig(root_, board_, st_):
            # the free texts and lines too: comparing the parts' names alone stopped at the first undo when no
            # name had moved, and left every other new text on the board while saying all was undone (#144)
            return (sorted((c['ref'], tuple(sorted((k, round(v, 3) if isinstance(v, float) else v) for k, v in (c.get('name') or {}).items())),
                            tuple(sorted((k, round(v, 3) if isinstance(v, float) else v) for k, v in (c.get('comment') or {}).items()))) for c in st_['components']),
                    sorted(g['id'] for g in plain_graphics(board_, Layers(board_))))
        start = sig(root, board, silk_before)
        for _ in range(len(commands) + 2):
            app.executeTextCommand('Electron.run UNDO;')
            r_, b_, _m = snapshot(app, {**args, 'expectedRevision': None})
            if sig(r_, b_, rd_silk_state(r_, b_, {})) == start: break
        r_, b_, m_ = snapshot(app, {**args, 'expectedRevision': None})
        back = sig(r_, b_, rd_silk_state(r_, b_, {})) == start
        return {'success': False, 'mutated': not back, 'errorCode': 'readback_mismatch', 'applied': 0, 'failed': failed, 'results': results,
                'error': '%d text(s) did not land as asked; %s' % (failed, 'the silkscreen is back as it was' if back else 'the undo did not restore the silkscreen exactly: read fusion_silk_state to see what is left'),
                'revision': m_['revision'], 'commands': commands}
    return {'success': failed == 0, 'mutated': True, 'applied': len(results) - failed, 'failed': failed, 'results': results,
            'smashed': to_smash, 'commands': len(commands), **meta2, 'after': st,
            '_hint': 'Read back from the board: every result is ok or names what landed where. Each EAGLE command is its own undo step in Fusion (no single-step undo for a plan). Free texts and lines are removed by the raw handle silk_state gives.'}


def _visible_layers(root):
    return [l.get('number') for l in root.find('drawing/layers') if l.get('visible') == 'yes']


def _restore_display(app, visible):
    """One layer per command (see apply_silkscreen: a DISPLAY that names the origin layers drops
    every layer after them)."""
    app.executeTextCommand('Electron.run DISPLAY NONE;')
    for n in visible:
        if n not in ('23', '24'):
            try: app.executeTextCommand('Electron.run DISPLAY %s;' % n)
            except Exception: pass


def _layer_number(board, v):
    """A layer given as Fusion's number, the standard's copper name, or Fusion's own layer name."""
    if isinstance(v, int) and not isinstance(v, bool): return v
    if isinstance(v, str):
        if v.strip().isdigit(): return int(v)
        L = Layers(board)
        for n, nm in L.names.items():
            if nm.lower() == v.strip().lower(): return n
        if v.strip().lower() in ('top',): return 1
        if v.strip().lower() in ('bottom',): return 16
    fail('invalid_layer', 'Unknown layer %r: pass an id from fusion_layers, a copper name (F.Cu, In1.Cu, B.Cu) or top/bottom' % (v,))


def set_layer_visibility(app, root, board, meta, args):
    n = _layer_number(board, args.get('layerId', args.get('layer')))
    if root.find("drawing/layers/layer[@number='%d']" % n) is None: fail('invalid_layer', 'This board has no layer %d' % n)
    on = args.get('displayed', True) is not False
    app.executeTextCommand('Electron.run DISPLAY %s%d;' % ('' if on else '-', n))
    app.executeTextCommand('Electron.ImmediateRepaint /full')
    root2, _, meta2 = snapshot(app, args)
    got = root2.find("drawing/layers/layer[@number='%d']" % n).get('visible') == 'yes'
    name = Layers(board).name(n) if n in COPPER else root2.find("drawing/layers/layer[@number='%d']" % n).get('name')
    return {'success': got == on, 'mutated': False, **meta2, 'layerId': n, 'name': name,
            'displayed': got, 'errorCode': None if got == on else 'readback_mismatch', 'windowEtiquette': 'background'}


def select_net(app, root, board, meta, args):
    """Highlight one whole net (EAGLE SHOW by name: every pad, track, via and pour of it) and frame it."""
    net = args.get('net')
    model, _ = board_model(board)
    if not net:
        app.executeTextCommand('Electron.run WINDOW;')
        return {'success': True, 'mutated': False, **meta, 'net': None, 'selected': 0, 'cleared': True}
    names = {n.lower(): n for n in model['nets']}
    if net.lower() not in names: fail('no_such_net', 'No net named %s' % net)
    net = identifier(names[net.lower()])
    by = {'tracks': sum(t['net'] == net for t in model['tracks']), 'arcs': sum(a['net'] == net for a in model['arcs']),
          'vias': sum(v['net'] == net for v in model['vias']), 'pads': sum(p['net'] == net for p in model['pads']),
          'pours': sum(g['net'] == net for g in model['polygons']), 'other': 0}
    out = {'success': True, 'mutated': False, **meta, 'net': net, 'byKind': by, 'selected': sum(by.values()), 'windowEtiquette': 'background'}
    # frame first: the highlight lasts until the next redraw
    if args.get('zoom'):
        pts = [(p['x'], p['y']) for p in model['pads'] if p['net'] == net] + [(t[k]['x'], t[k]['y']) for t in model['tracks'] if t['net'] == net for k in ('start', 'end')]
        if pts:
            m = number(args.get('margin', 1.0))
            box = [min(p[0] for p in pts) - m, min(p[1] for p in pts) - m, max(p[0] for p in pts) + m, max(p[1] for p in pts) + m]
            app.executeTextCommand('Electron.run WINDOW (%.4fmm %.4fmm) (%.4fmm %.4fmm);' % tuple(box))
            out['zoom'] = [round(v, 4) for v in box]
    # no closing ';': Fusion clears a finished SHOW's highlight at once, so the command is left
    # waiting for a click (a prompt in the command line, not a dialog); the next command ends it
    app.executeTextCommand("Electron.run SHOW '%s'" % net)
    return out


def _undo_until(app, args, revision, tries):
    for _ in range(tries):
        app.executeTextCommand('Electron.run UNDO;')
        _, _, m = snapshot(app, args)
        if m['revision'] == revision: return True
    return False


def _seg_dist(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    L2 = dx * dx + dy * dy
    t = 0.0 if not L2 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L2))
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def _pick_points(x, others, pads):
    """Points along wire x to pick it at, best first: farthest from the other wires on its layer
    and from pad centers (a midpoint pick took a neighbour: a stub into a pad, a wire end)."""
    out = []
    for t in (0.5, 0.35, 0.65, 0.2, 0.8, 0.1, 0.9):
        p = (x['start'][0] + t * (x['end'][0] - x['start'][0]), x['start'][1] + t * (x['end'][1] - x['start'][1]))
        d = min([_seg_dist(p, o['start'], o['end']) for o in others] + [math.hypot(p[0] - q[0], p[1] - q[1]) for q in pads] + [9.0])
        out.append((-round(d, 4), t, p))
    return [p for _, _, p in sorted(out)]


def _wire_key(x):
    a, b = tuple(round(v, 4) for v in x['start']), tuple(round(v, 4) for v in x['end'])
    return (x['net'], x['layer'], min(a, b), max(a, b), round(x['curve'], 3))


def edit_tracks(app, root, board, meta, args):
    """The standard's edit_tracks: given tracks by their routing_state handle: width, layer, ends. Each is picked
    with only its layer displayed, at the point farthest from other copper. An end moves with MOVE along the track's
    line; at an angle the track is drawn again and the old one deleted.
    Read back: only the asked tracks may differ, and only as asked, or every change is undone."""
    _guard(meta, args)
    edits = args.get('tracks') or []
    if not edits:
        fail('bad_args', 'tracks is [{id, width?, layer?, start?, end?}]')
    _, pads, wires, _ = objects(board)
    def handle(x):
        return 'track:%s:%d:%g,%g,%g,%g' % (x['net'], x['layer'], round(x['start'][0], 4), round(x['start'][1], 4), round(x['end'][0], 4), round(x['end'][1], 4))
    by = {handle(x): x for x in wires if not x['curve']}
    results, plan, cmds = [], [], []
    for ed in edits:
        x = by.get(ed.get('id'))
        if x is None:
            results.append('err:no such track %s: read routing_state again' % ed.get('id'))
            continue
        others = [o for o in wires if o['layer'] == x['layer'] and _wire_key(o) != _wire_key(x)]
        pick = _pick_points(x, others, [p['at'] for p in pads.values() if x['layer'] in p['layers']])[0]
        step, want = [], dict(x)
        step.append('DISPLAY NONE %d;' % x['layer'])
        if ed.get('width') is not None:
            want['width'] = number(ed['width'], True)
            step.append('CHANGE WIDTH %.6fmm (%.6fmm %.6fmm);' % (want['width'], pick[0], pick[1]))
        for key in ('start', 'end'):
            if ed.get(key) is not None:
                want[key] = [number(ed[key][0]), number(ed[key][1])]
        if want['start'] != x['start'] or want['end'] != x['end']:
            dx, dy = x['end'][0] - x['start'][0], x['end'][1] - x['start'][1]
            along = all(abs((q[0] - x['start'][0]) * dy - (q[1] - x['start'][1]) * dx) < 1e-6 * max(math.hypot(dx, dy), 1e-9) for q in (want['start'], want['end']))
            if along:
                for key in ('start', 'end'):
                    if want[key] != x[key]:
                        step.append('MOVE (%.6fmm %.6fmm) (%.6fmm %.6fmm);' % (x[key][0], x[key][1], want[key][0], want[key][1]))
            else:
                # MOVE keeps one segment only along the track's own line: at an angle Fusion bends it into two,
                # whatever WIRE_BEND says. So the new track goes down first and the old one comes off after:
                # that order, as DELETE of a net's last wire removes the whole signal, pad connections too.
                step.append("SET WIRE_BEND 2; LAYER %d; LINE '%s' %.6fmm (%.6fmm %.6fmm) (%.6fmm %.6fmm);" % (x['layer'], identifier(x['net']), want['width'], want['start'][0], want['start'][1], want['end'][0], want['end'][1]))
                away = _pick_points(x, others + [dict(x, start=want['start'], end=want['end'])], [p['at'] for p in pads.values() if x['layer'] in p['layers']])[0]
                step.append('DELETE (%.6fmm %.6fmm);' % (away[0], away[1]))
                # the width is on the new track, so nothing is changed on the old one
                step = [c for c in step if not c.startswith('CHANGE WIDTH')]
        if ed.get('layer') is not None:
            want['layer'] = _layer_number(board, ed['layer'])
            if ed.get('start') is None and ed.get('end') is None:
                step.append('CHANGE LAYER %d (%.6fmm %.6fmm);' % (want['layer'], pick[0], pick[1]))
            else:
                results.append('err:change a track\'s layer and its ends in two calls')
                continue
        plan.append({'id': ed['id'], 'before': x, 'want': want})
        cmds += step
        results.append('ok')
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'results': results, 'commands': cmds}
    if not plan:
        return {'success': False, 'mutated': False, **meta, 'errorCode': 'nothing_to_do', 'results': results, 'error': 'No track could be edited'}
    visible = _visible_layers(root)
    try:
        app.executeTextCommand('Electron.run GRID MM 0.001;')
        for c in cmds:
            app.executeTextCommand('Electron.run ' + c)
    finally:
        app.executeTextCommand('Electron.run GRID LAST;')
        _restore_display(app, visible)
    _, after, meta2 = snapshot(app, args)
    now = objects(after)[2]
    keys = {_wire_key(x) for x in now}
    gone = {_wire_key(p_['before']) for p_ in plan}
    fresh = {_wire_key(p_['want']) for p_ in plan}
    ok = fresh <= keys and {_wire_key(x) for x in wires} - gone <= keys
    width = {_wire_key(x): x['width'] for x in now}
    ok = ok and all(abs(width[_wire_key(p_['want'])] - p_['want']['width']) < 1e-6 for p_ in plan if _wire_key(p_['want']) in width)
    if not ok:
        _undo_until(app, args, meta['revision'], sum(1 for c in cmds if not c.startswith('DISPLAY')) + 2)
        fail('readback_mismatch', 'The copper after the edit was not exactly the asked tracks; every change was undone')
    app.executeTextCommand('Electron.run RATSNEST;')
    return {'success': True, 'mutated': True, **meta2, 'results': results, 'verified': True,
            'changes': [{'id': p_['id'], 'before': {k: p_['before'][k] for k in ('width', 'layer', 'start', 'end')}, 'after': {k: p_['want'][k] for k in ('width', 'layer', 'start', 'end')}} for p_ in plan],
            '_hint': 'Run fusion_run_drc to check clearances. A moved end leaves the track\'s old connection open unless it lands on the same net.'}


def change_vias(app, root, board, meta, args):
    """Change existing vias, and nothing but vias: drill and diameter, picked by position or by a filter on what they
    are now. Only the Vias layer is displayed while they're picked, so a track end or pad at the same point can't be
    taken instead, and the board's display goes back after. Read back: only the asked vias may differ, and only as
    asked, or every change is undone."""
    _guard(meta, args)
    _, _, _, vias = objects(board)
    where, setting, flt = args.get('vias'), args.get('set') or {}, args.get('filter')
    if bool(where) == bool(flt):
        fail('bad_args', 'Give vias: [{x, y, drill?, diameter?}], or filter: {drill?, diameter?, net?} with set: {drill?, diameter?}')
    def same(a, b): return a is not None and abs(float(a) - float(b)) < 1e-4
    plan, results = [], []
    if flt:
        if setting.get('drill') is None and setting.get('diameter') is None: fail('bad_args', 'filter needs set: {drill?, diameter?}')
        for v in vias:
            if (flt.get('drill') is None or same(v['drill'], flt['drill'])) and (flt.get('diameter') is None or same(v['diameter'], flt['diameter'])) and (flt.get('net') is None or v['net'] == flt['net']):
                plan.append((v, setting.get('drill'), setting.get('diameter'))); results.append('ok')
    else:
        for w in where:
            x, y = number(w.get('x')), number(w.get('y'))
            hit = [v for v in vias if abs(v['at'][0] - x) < 0.01 and abs(v['at'][1] - y) < 0.01]
            if len(hit) != 1:
                results.append('err:%d vias at (%g, %g)' % (len(hit), x, y)); continue
            d, dia = w.get('drill', setting.get('drill')), w.get('diameter', setting.get('diameter'))
            if d is None and dia is None:
                results.append('err:nothing to change at (%g, %g): give drill or diameter' % (x, y)); continue
            plan.append((hit[0], d, dia)); results.append('ok')
    cmds, want = [], []
    for v, d, dia in plan:
        new_d = number(d, True) if d is not None else v['drill']
        new_dia = number(dia, True) if dia is not None else v['diameter']
        if new_dia and new_d >= new_dia:
            fail('invalid_geometry', 'A via at (%g, %g) would have a %g mm drill in a %g mm diameter' % (v['at'][0], v['at'][1], new_d, new_dia))
        if d is not None: cmds.append('CHANGE DRILL %.6fmm (%.6fmm %.6fmm);' % (new_d, v['at'][0], v['at'][1]))
        if dia is not None: cmds.append('CHANGE DIAMETER %.6fmm (%.6fmm %.6fmm);' % (new_dia, v['at'][0], v['at'][1]))
        want.append(dict(v, drill=new_d, diameter=new_dia))
    changes = [{'net': v['net'], 'x': v['at'][0], 'y': v['at'][1], 'drill': [v['drill'], w_['drill']], 'diameter': [v['diameter'], w_['diameter']]} for (v, _, _), w_ in zip(plan, want)]
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'results': results, 'changes': changes, 'commands': cmds}
    if not cmds:
        return {'success': False, 'mutated': False, **meta, 'errorCode': 'nothing_to_do', 'results': results, 'error': 'No via matched'}
    visible = _visible_layers(root)
    try:
        app.executeTextCommand('Electron.run DISPLAY NONE 18;')
        for b_ in range(0, len(cmds), 40):
            app.executeTextCommand('Electron.run ' + ' '.join(cmds[b_:b_ + 40]))
    finally:
        _restore_display(app, visible)
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    _, _, w1, v1 = objects(after)
    untouched = [v for v in vias if not any(v is p_[0] for p_ in plan)]
    expected = copper_signature(objects(board)[2] + untouched + want)
    if copper_signature(w1 + v1) != expected:
        before = copper_signature(objects(board)[2] + vias)
        undone, _, _ = undo_route(app, args, cmds, before)
        fail('readback_mismatch', 'The vias after the change were not exactly the asked ones; %s' % ('it was undone' if undone else 'the undo did not restore them exactly: read fusion_routing_state'))
    return {'success': True, 'mutated': True, 'verified': True, **meta2, 'results': results, 'changes': changes, 'changed': len(plan),
            '_hint': 'Run fusion_run_drc: a larger via can break a clearance that the smaller one met.'}


def place_parts(app, root, board, meta, args):
    """The standard's place_parts on a board with no schematic (fusion_open_board, fusion_build_board): footprints
    from a .lbr with position, rotation and side. A board linked to a schematic gets its parts from the sheet
    (fusion_sch_place_part, then fusion_move_parts), since Fusion keeps the two in step, so it is refused."""
    _guard(meta, args)
    parts = args.get('parts') or []
    if not parts:
        fail('bad_args', 'parts is [{library, footprint, ref?, value?, x, y, rotation?, side?}]')
    doc = app.activeDocument
    linked = False
    for i in range(app.documents.count):
        d = app.documents.item(i)
        try:
            if d.name == doc.name and d.products.itemByProductType('SchematicProductType') is not None:
                linked = True
        except Exception:
            pass
    if linked:
        fail('linked_board', 'This board is linked to a schematic, and Fusion takes its parts from there: fusion_sch_place_part, then fusion_move_parts')
    names = {e.get('name') for e in board.findall('elements/element')}
    results, placed, cmds, used = [], [], [], set()
    for k, pp in enumerate(parts):
        lbr, fp = pp.get('library'), pp.get('footprint')
        if not lbr or not Path(lbr).is_file() or not fp:
            results.append('err:library (a .lbr on this machine) and footprint are required'); continue
        lroot = ET.parse(lbr).getroot()
        if not any(q.get('name', '').upper() == str(fp).upper() for q in lroot.iter('package')):
            results.append('err:no footprint %s in %s' % (fp, Path(lbr).name)); continue
        ref = identifier(pp.get('ref') or 'P%d' % (k + 1))
        if ref in names:
            results.append('err:designator %s is already taken' % ref); continue
        x, y = number(pp.get('x')), number(pp.get('y'))
        if max(abs(x), abs(y)) > SHEET_LIMIT_MM:
            results.append('err:off the sheet (keep within %g mm)' % SHEET_LIMIT_MM); continue
        side = pp.get('side') or 'top'
        rot = float(pp.get('rotation') or 0) % 360
        used.add(lbr)
        cmds.append("SET CONFIRM YES; ADD '%s@%s' '%s' %sR%g (%.6fmm %.6fmm); SET CONFIRM OFF;" % (fp, Path(lbr).stem, ref, 'M' if side == 'bottom' else '', rot, x, y))
        if pp.get('value'):
            cmds.append("SET CONFIRM YES; VALUE '%s' '%s'; SET CONFIRM OFF;" % (ref, str(pp['value']).replace("'", "''")))
        names.add(ref)
        placed.append({'ref': ref, 'x': x, 'y': y, 'rotation': rot, 'side': side})
        results.append('ok')
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'results': results, 'placed': placed, 'commands': cmds}
    for lbr in sorted(used):
        try: use_library(app, lbr)
        except RuntimeError as ex: fail(str(ex).split(':', 1)[0] if ':' in str(ex)[:24] else 'library_unreadable', str(ex).split(': ', 1)[-1])
    for c in cmds:
        app.executeTextCommand('Electron.run ' + c)
    _, after, meta2 = snapshot(app, args)
    have = {e.get('name'): e for e in after.findall('elements/element')}
    missing = [q['ref'] for q in placed if q['ref'] not in have]
    if missing:
        _undo_until(app, args, meta['revision'], len(cmds) + 2)
        fail('readback_mismatch', 'Not on the board after ADD: %s; every part was undone' % ', '.join(missing))
    return {'success': all(r == 'ok' for r in results), 'mutated': True, **meta2, 'results': results, 'placed': placed}


def read_board(app, root, board, meta, args):
    """The standard's read_board: the board in one round trip, as sections (components, nets,
    layers, copper, rules, occupancy, outline; sections picks some), each list capped at limit
    with its full count beside it."""
    import time as _t
    t0 = _t.time()
    want = {x.strip() for x in str(args.get('sections') or '').split(',') if x.strip()} or \
        {'components', 'nets', 'layers', 'copper', 'rules', 'occupancy', 'outline'}
    limit = int(args.get('limit') or 0)
    model = state(board, meta)
    def cap(v):
        return v[:limit] if limit and isinstance(v, list) else v
    data, counts = {}, {}
    def put(sec, key, v):
        if isinstance(v, list): counts['%s.%s' % (sec, key)] = len(v)
        data.setdefault(sec, {})[key] = cap(v)
    if 'components' in want: put('components', 'components', model.get('components') or [])
    if 'nets' in want:
        put('nets', 'nets', model.get('nets') or []); put('nets', 'classes', rd_net_classes(root, board, {})['classes'])
    if 'layers' in want:
        put('layers', 'copperLayers', model.get('copperLayers') or []); put('layers', 'stack', layer_stack(board))
    if 'copper' in want:
        for k in ('tracks', 'arcs', 'vias', 'pads', 'polygons'): put('copper', k, model.get(k) or [])
    if 'rules' in want: data['rules'] = rd_read_rules(root, board, {})
    if 'occupancy' in want: data['occupancy'] = rd_layers_in_use(root, board, {})
    if 'outline' in want:
        for k in ('outline', 'outlineClosed', 'outlineCutouts'): data.setdefault('outline', {})[k] = model.get(k)
    unknown = sorted(want - {'components', 'nets', 'layers', 'copper', 'rules', 'occupancy', 'outline'})
    return {'success': True, 'mutated': False, **meta, 'data': data, 'counts': counts, 'limit': limit or None,
            'unknownSections': unknown, 'roundtripMs': int((_t.time() - t0) * 1000),
            '_hint': 'Every section from one export of the live board; counts has each list\'s full length when limit cut it.'}


def _classes_sig(board):
    """Every net class (number, name, width, drill, clearances) and every net's class."""
    cls = tuple(sorted((c.get('number'), c.get('name'), round(float(c.get('width', '0') or 0), 4), round(float(c.get('drill', '0') or 0), 4),
                        tuple(sorted((k.get('class'), round(float(k.get('value')), 4)) for k in c.findall('clearance'))))
                       for c in board.findall('classes/class')))
    nets = tuple(sorted((s_.get('name'), s_.get('class', '0')) for s_ in board.findall('signals/signal')))
    return cls, nets


def _remove_rule(app, root, board, meta, args):
    """add_rule {remove}: the Adom_<name> class's nets go back to the default class and the class is
    zeroed (EAGLE keeps a class once made; an empty, zeroed one constrains nothing)."""
    cname = ('Adom_' + re.sub(r'[^A-Za-z0-9_]', '_', str(args.get('name') or '').strip()))[:30]
    cls = next((c for c in board.findall('classes/class') if c.get('name') == cname), None)
    if cls is None: fail('no_such_rule', 'No class named %s on this board' % cname)
    num = cls.get('number')
    nets, pads, wires, _ = objects(board)
    members = sorted(n for n, s_ in nets.items() if s_.get('class', '0') == num)
    before = _classes_sig(board); n_cmd = 0
    zero = ' '.join('%s:0mm' % k.get('class') for k in cls.findall('clearance') if k.get('class') != num)
    app.executeTextCommand("Electron.run CLASS %s '%s' 0mm 0mm 0mm %s;" % (num, cname, zero)); n_cmd += 1
    visible = _visible_layers(root)
    try:
        for n in members:
            x = next((w for w in wires if w['net'] == n and not w['curve']), None)
            if x is None: continue
            others = [o for o in wires if o['layer'] == x['layer'] and _wire_key(o) != _wire_key(x)]
            px, py = _pick_points(x, others, [p['at'] for p in pads.values() if x['layer'] in p['layers']])[0]
            app.executeTextCommand('Electron.run DISPLAY NONE %d;' % x['layer'])
            app.executeTextCommand('Electron.run CHANGE CLASS 0 (%.6fmm %.6fmm);' % (px, py)); n_cmd += 1
    finally:
        _restore_display(app, visible)
    _, after, meta2 = snapshot(app, args)
    left = sorted(n for n, c in _classes_sig(after)[1] if c == num)
    if left:
        for _ in range(n_cmd + 1):
            app.executeTextCommand('Electron.run UNDO;')
            _, back, _m = snapshot(app, args)
            if _classes_sig(back) == before: break
        fail('readback_mismatch', '%s still in %s after the removal; every change was undone' % (', '.join(left), cname))
    return {'success': True, 'mutated': True, **meta2, 'name': cname, 'removed': True, 'released': members,
            '_hint': 'The nets are back in the default class; the zeroed class stays on the board (EAGLE keeps a class once made) and constrains nothing.'}


def add_rule(app, root, board, meta, args):
    """The standard's add_rule as EAGLE net classes, the board's only scoped rules: a class holds a
    track width, a drill and clearances (to each class and inside itself). scope1 InNetClass('X')
    edits that class; InNet('A') (or nets) puts the nets in a class of their own, 'Adom <name>',
    each joined with one CHANGE CLASS pick on its wire (only that wire's layer displayed). Read back
    from the board, every change undone on a mismatch."""
    if args.get('remove'):
        return _remove_rule(app, root, board, meta, args)
    kind = str(args.get('kind') or '').lower()
    if kind not in ('width', 'clearance', 'via'):
        fail('not_supported', "kind %r: an EAGLE net class holds a width, a drill and clearances (kind width|clearance|via); length, matched, diffpair and polyconnect have no class equivalent in Fusion" % kind)
    name = str(args.get('name') or '').strip()
    if not name: fail('bad_args', 'name is required (the class is named Adom <name>)')
    scope = str(args.get('scope1') or '').strip()
    m_net = re.findall(r"InNet\('([^']+)'\)", scope); m_cls = re.match(r"InNetClass\('([^']+)'\)", scope)
    nets_arg = list(args.get('nets') or []) + m_net
    nets, pads, wires, _ = objects(board)
    classes = {c.get('number'): c for c in board.findall('classes/class')}
    if m_cls or args.get('netClass'):
        want = (m_cls.group(1) if m_cls else str(args['netClass'])).lower()
        target = next((c for c in classes.values() if c.get('name', '').lower() == want), None)
        if target is None: fail('no_such_class', 'No net class named %s (fusion_net_classes lists them)' % want)
        num, cname, join = target.get('number'), target.get('name'), []
    elif nets_arg:
        unknown = [n for n in nets_arg if n not in nets]
        if unknown: fail('unknown_net', 'No net named %s' % ', '.join(unknown))
        cname = ('Adom_' + re.sub(r'[^A-Za-z0-9_]', '_', name))[:30]
        target = next((c for c in classes.values() if c.get('name') == cname), None)
        if target is not None: num = target.get('number')
        else:
            free = [str(i) for i in range(1, 8) if str(i) not in classes]
            if not free: fail('no_free_class', 'EAGLE boards have 8 net classes (0..7) and all are used: scope the rule to an existing class with InNetClass')
            num = free[0]
        join = [n for n in nets_arg if nets[n].get('class', '0') != num]
        bare = [n for n in join if not any(w['net'] == n and not w['curve'] for w in wires)]
        if bare: fail('no_track', '%s has no straight track to pick: route it first, or set its class in the schematic' % ', '.join(bare))
    else:
        fail('bad_args', "scope1 is InNet('A'), InNet('A') or InNet('B'), or InNetClass('X') (nets / netClass also work)")
    cur = classes.get(num)
    width = float(cur.get('width', '0') or 0) if cur is not None else 0.0
    drill = float(cur.get('drill', '0') or 0) if cur is not None else 0.0
    clear = {k.get('class'): float(k.get('value')) for k in cur.findall('clearance')} if cur is not None else {}
    if kind == 'width':
        width = number(args.get('min', args.get('preferred', args.get('width'))), True)
    elif kind == 'via':
        drill = number(args.get('drill', args.get('minDrill')), True)
    else:
        gap = number(args.get('gap'), True)
        for k in set(classes) | {num}: clear[k] = gap
    own = clear.pop(num, 0.0)
    tokens = ' '.join('%s:%.6fmm' % (k, v) for k, v in sorted(clear.items(), key=lambda kv: int(kv[0])))
    cmd = "CLASS %s '%s' %.6fmm %.6fmm %.6fmm %s;" % (num, cname, width, own, drill, tokens)
    before = _classes_sig(board); n_cmd = 0
    app.executeTextCommand('Electron.run ' + cmd); n_cmd += 1
    visible = _visible_layers(root)
    try:
        for n in join:
            x = next(w for w in wires if w['net'] == n and not w['curve'])
            others = [o for o in wires if o['layer'] == x['layer'] and _wire_key(o) != _wire_key(x)]
            px, py = _pick_points(x, others, [p['at'] for p in pads.values() if x['layer'] in p['layers']])[0]
            app.executeTextCommand('Electron.run DISPLAY NONE %d;' % x['layer'])
            app.executeTextCommand("Electron.run CHANGE CLASS %s (%.6fmm %.6fmm);" % (num, px, py)); n_cmd += 1
    finally:
        _restore_display(app, visible)
    _, after, meta2 = snapshot(app, args)
    got = {c.get('number'): c for c in after.findall('classes/class')}.get(num)
    sig_cls, sig_nets = _classes_sig(after)
    nets_after = dict(sig_nets); nets_before = dict(before[1])
    moved = sorted(n for n in nets_after if nets_after[n] != nets_before.get(n))
    ok = got is not None and abs(float(got.get('width', '0') or 0) - width) < 1e-4 and abs(float(got.get('drill', '0') or 0) - drill) < 1e-4 \
        and set(moved) == set(join) and all(nets_after[n] == num for n in join) \
        and _classes_sig(after)[0] != () and dict((k.get('class'), round(float(k.get('value')), 4)) for k in got.findall('clearance')).get(num, 0.0) == round(own, 4)
    if not ok:
        for _ in range(n_cmd + 1):
            app.executeTextCommand('Electron.run UNDO;')
            _, back, _m = snapshot(app, args)
            if _classes_sig(back) == before: break
        fail('readback_mismatch', 'The board\'s classes after %s were not as asked (moved nets: %s); every change was undone' % (cmd, ', '.join(moved) or 'none'))
    row = rd_net_classes(None, after, {})
    cls_row = next((c for c in row['classes'] if str(c['number']) == num), None)
    return {'success': True, 'mutated': True, **meta2, 'name': cname, 'class': cls_row, 'joined': join, 'command': cmd,
            '_hint': 'An EAGLE net class: Fusion\'s DRC holds its nets to this width, drill and clearance. fusion_net_classes lists every class; fusion_run_drc checks the board.'}


def set_track_width(app, root, board, meta, args):
    """Change the width of existing tracks on nets (or a net class), optionally one layer. Each
    wire is picked with only its layer displayed, at the point along it farthest from other copper,
    in up to three passes (each read back); then only the asked wires may differ, and only in
    width, or every change is undone. A wire no pick reaches (one lying on another) is reported."""
    w = number(args.get('widthMm', args.get('width')), True)
    nets, pads, wires, _ = objects(board)
    want = set(args.get('nets') or [])
    if args.get('netClass'):
        cls = {c.get('name').lower(): c.get('number') for c in board.findall('classes/class')}
        num = cls.get(str(args['netClass']).lower())
        if num is None: fail('no_such_class', 'No net class named %s' % args['netClass'])
        want |= {n for n, s_ in nets.items() if s_.get('class', '0') == num}
    if not want: fail('bad_args', 'Give nets or netClass')
    missing = sorted(n for n in want if n not in nets)
    layer = _layer_number(board, args['layer']) if args.get('layer') is not None else None
    targets = [x for x in wires if x['net'] in want and (layer is None or x['layer'] == layer) and not x['curve'] and abs(x['width'] - w) > 1e-6]
    tkeys = {_wire_key(x) for x in targets}
    before = {_wire_key(x): x['width'] for x in wires}
    visible = _visible_layers(root); n_cmd = 0; left = list(targets); meta2 = meta; after_wires = wires
    for attempt in range(3):
        if not left: break
        try:
            for x in left:
                others = [o for o in after_wires if o['layer'] == x['layer'] and _wire_key(o) != _wire_key(x)]
                pp = [p['at'] for p in pads.values() if x['layer'] in p['layers']]
                pts = _pick_points(x, others, pp)
                px, py = pts[min(attempt, len(pts) - 1)]
                app.executeTextCommand('Electron.run DISPLAY NONE %d;' % x['layer'])
                app.executeTextCommand('Electron.run CHANGE WIDTH %.6fmm (%.6fmm %.6fmm);' % (w, px, py)); n_cmd += 1
        finally:
            _restore_display(app, visible)
        _, after, meta2 = snapshot(app, args)
        after_wires = objects(after)[2]
        now = {_wire_key(x): x['width'] for x in after_wires}
        bad = set(now) != set(before) or any(abs(now[k] - before[k]) > 1e-6 and (k not in tkeys or abs(now[k] - w) > 1e-6) for k in now)
        if bad:
            _undo_until(app, args, meta['revision'], n_cmd + 2)
            fail('readback_mismatch', 'The copper after the width change was not exactly the asked wires at %.4f mm; every change was undone' % w)
        left = [x for x in left if abs(now[_wire_key(x)] - w) > 1e-6]
    # a stub lying wholly under a wider wire of its own net, or inside its own pad, adds no copper
    # at its width: it is covered, not a failure (Fusion leaves such stubs where a trace was redrawn)
    def under(x):
        def inside(p):
            return any(o['net'] == x['net'] and o['layer'] == x['layer'] and o['width'] >= w - 1e-6 and _seg_dist(p, o['start'], o['end']) <= (o['width'] - x['width']) / 2 + 1e-4
                       for o in after_wires if _wire_key(o) != _wire_key(x)) or \
                   any(q['net'] == x['net'] and x['layer'] in q['layers'] and math.hypot(p[0] - q['at'][0], p[1] - q['at'][1]) <= q['radius'] for q in pads.values())
        mid = ((x['start'][0] + x['end'][0]) / 2, (x['start'][1] + x['end'][1]) / 2)
        return all(inside(p) for p in (x['start'], mid, x['end']))
    covered = [x for x in left if under(x)]
    left = [x for x in left if x not in covered]
    app.executeTextCommand('Electron.run RATSNEST;')
    by = {}
    for x in targets:
        if x not in left and x not in covered: by[x['net']] = by.get(x['net'], 0) + 1
    out = {'success': not left, 'mutated': len(left) + len(covered) < len(targets), **meta2, 'changed': sum(by.values()), 'byNet': by, 'notOnBoard': missing, 'widthMm': w,
           'covered': [{'net': x['net'], 'layer': x['layer'], 'start': x['start'], 'end': x['end'], 'widthMm': x['width']} for x in covered],
           '_hint': 'Read back: exactly these wires changed width, nothing else. Run fusion_run_drc to check clearances at the new width.'}
    if left:
        out['errorCode'] = 'partial'
        out['unchanged'] = [{'net': x['net'], 'layer': x['layer'], 'start': x['start'], 'end': x['end']} for x in left]
        out['_hint'] = ('%d wire(s) could not be picked on their own (a stub lying on another wire, or inside a pad): they keep their width; '
                        'the rest changed and were read back. Delete a redundant stub with fusion_remove_items if it is one.' % len(left))
    return out


def remove_items(app, root, board, meta, args):
    """Delete tracks and vias given exactly as fusion_routing_state returned them; read back that
    exactly those went (each removal is its own undo step; a wrong one undoes all)."""
    if args.get('expectedRevision') not in (None, meta['revision']): fail('stale_board', 'expectedRevision must match the current live board')
    _, _, wires, vias = objects(board)
    L = Layers(board)
    num = {nm: n for n, nm in L.names.items()}
    results, gone_w, gone_v = [], [], []
    def xy(p): return (float(p['x']), float(p['y'])) if isinstance(p, dict) else (float(p[0]), float(p[1]))
    for t in args.get('tracks') or []:
        a, b = xy(t['start']), xy(t['end'])
        ln = num.get(t.get('layer'), t.get('layer'))
        hit = next((x for x in wires if x not in gone_w and (str(x['layer']) == str(ln)) and
                    {tuple(map(lambda v: round(v, 4), x['start'])), tuple(map(lambda v: round(v, 4), x['end']))} == {tuple(map(lambda v: round(v, 4), a)), tuple(map(lambda v: round(v, 4), b))}), None)
        if hit: gone_w.append(hit); results.append('ok')
        else: results.append('err:not found')
    for v in args.get('vias') or []:
        p = xy(v.get('position', v))
        hit = next((x for x in vias if x not in gone_v and abs(x['at'][0] - p[0]) < 1e-4 and abs(x['at'][1] - p[1]) < 1e-4), None)
        if hit: gone_v.append(hit); results.append('ok')
        else: results.append('err:not found')
    if not (gone_w or gone_v):
        return {'success': False, 'errorCode': 'nothing_to_do', 'error': 'None of the given tracks or vias is on the board', 'results': results, **meta}
    visible = _visible_layers(root); n_cmd = 0
    try:
        for x in gone_w:
            app.executeTextCommand('Electron.run DISPLAY NONE %d;' % x['layer'])
            app.executeTextCommand('Electron.run DELETE (%.6fmm %.6fmm);' % ((x['start'][0] + x['end'][0]) / 2, (x['start'][1] + x['end'][1]) / 2)); n_cmd += 1
        for x in gone_v:
            app.executeTextCommand('Electron.run DISPLAY NONE 18;')
            app.executeTextCommand('Electron.run DELETE (%.6fmm %.6fmm);' % tuple(x['at'])); n_cmd += 1
    finally:
        _restore_display(app, visible)
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    _, _, w2, v2 = objects(after)
    expected = [x for x in wires if x not in gone_w] + [x for x in vias if x not in gone_v]
    if copper_signature(w2 + v2) != copper_signature(expected):
        _undo_until(app, args, meta['revision'], n_cmd + 2)
        fail('readback_mismatch', 'The board after the removal was not exactly the board minus the given items; every removal was undone')
    st = state(after, meta2)
    return {'success': all(r_ == 'ok' for r_ in results), 'mutated': True, **meta2, 'results': results, 'removed': {'tracks': len(gone_w), 'vias': len(gone_v)},
            'unconnected': st['unconnected']}


def set_outline(app, root, board, meta, args):
    """Replace the board outline (layer 20 Dimension) with a rectangle (rounded corners optional) or
    a polygon. The old board-level outline lines and arcs are deleted (each picked with only layer
    20 displayed), the new one drawn, and the result read back as outlineEdges."""
    if args.get('expectedRevision') not in (None, meta['revision']): fail('stale_board', 'expectedRevision must match the current live board')
    o = args.get('outline') or {}
    if 'rect' in o:
        x0, y0, w, h = [number(v) for v in o['rect']]
        if w <= 0 or h <= 0: fail('bad_size', 'rect width and height must be > 0')
        r = number(o.get('cornerRadius', 0))
        if r * 2 > min(w, h): fail('bad_size', 'cornerRadius is more than half the shorter side')
        x1, y1 = x0 + w, y0 + h
        if r > 0:
            segs = [((x0 + r, y0), (x1 - r, y0), 0), ((x1 - r, y0), (x1, y0 + r), 90), ((x1, y0 + r), (x1, y1 - r), 0), ((x1, y1 - r), (x1 - r, y1), 90),
                    ((x1 - r, y1), (x0 + r, y1), 0), ((x0 + r, y1), (x0, y1 - r), 90), ((x0, y1 - r), (x0, y0 + r), 0), ((x0, y0 + r), (x0 + r, y0), 90)]
        else:
            segs = [((x0, y0), (x1, y0), 0), ((x1, y0), (x1, y1), 0), ((x1, y1), (x0, y1), 0), ((x0, y1), (x0, y0), 0)]
    elif 'points' in o:
        pts = [(number(a), number(b)) for a, b in o['points']]
        if len(pts) < 3: fail('bad_args', 'A polygon outline needs at least three points')
        segs = [(pts[i], pts[(i + 1) % len(pts)], 0) for i in range(len(pts))]
    else:
        fail('bad_args', 'outline is {rect:[x0,y0,w,h], cornerRadius?} or {points:[[x,y]...]} (mm)')
    old = board.findall("plain/wire[@layer='20']")
    visible = _visible_layers(root); n_cmd = 0
    try:
        app.executeTextCommand('Electron.run DISPLAY NONE 20;')
        for wv in old:
            p1, p2, c = (_f(wv, 'x1'), _f(wv, 'y1')), (_f(wv, 'x2'), _f(wv, 'y2')), _f(wv, 'curve')
            pts_ = wire_points(p1, p2, c)
            mid = pts_[len(pts_) // 2] if c else ((p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2)
            app.executeTextCommand('Electron.run DELETE (%.6fmm %.6fmm);' % tuple(mid)); n_cmd += 1
        for a, b, curve in segs:
            # an arc is its signed angle bare (+90): '$90' and '@radius' are ignored without an error
            app.executeTextCommand('Electron.run LAYER 20; WIRE 0mm (%.6fmm %.6fmm) %s(%.6fmm %.6fmm);' % (a[0], a[1], ('%+g ' % curve) if curve else '', b[0], b[1])); n_cmd += 1
    finally:
        _restore_display(app, visible)
    app.executeTextCommand('Electron.run RATSNEST;')
    app.executeTextCommand('Electron.ImmediateRepaint /full')
    _, after, meta2 = snapshot(app, args)
    model, _ = board_model(after, meta2)
    drawn = after.findall("plain/wire[@layer='20']")
    ok = model['outlineClosed'] and len(drawn) == len(segs) and len(model['outlineEdges']) == len(segs)
    _, _, w0, v0 = objects(board); _, _, w1, v1 = objects(after)
    if copper_signature(w0 + v0) != copper_signature(w1 + v1) or not ok:
        _undo_until(app, args, meta['revision'], n_cmd + 2)
        fail('readback_mismatch', ('Copper changed while the outline was redrawn' if ok else
              'The outline read back was not the one asked (%d of %d edges, closed: %s)' % (len(drawn), len(segs), model['outlineClosed'])) + '; every command was undone')
    return {'success': True, 'mutated': True, **meta2, 'outlineEdges': model['outlineEdges'], 'vertices': len(model['outline']),
            'replaced': len(old), 'errorCode': None,
            '_hint': 'The outline is Fusion\'s Dimension layer (20), which its CAM output uses as the board edge. Check that parts and copper still sit inside it (fusion_routing_state outline, fusion_run_drc).'}


def board_outline_read(app, root, board, meta, args):
    """Fusion's board edge for the fab is its Dimension layer (20): there is no separate mechanical
    outline to draw, so this reports what is there."""
    model, _ = board_model(board, meta)
    n = len(board.findall("plain/wire[@layer='20']")) + len(board.findall("plain/circle[@layer='20']"))
    return {'success': bool(n), 'mutated': False, **meta, 'layer': 'Dimension (20)', 'alreadyThere': n, 'drawn': 0, 'outlineClosed': model['outlineClosed'],
            'errorCode': None if n else 'no_outline',
            '_hint': 'Fusion exports layer 20 (Dimension) as the board outline in its Gerbers, so nothing needs drawing. Change the shape with fusion_set_outline.' if n else 'The board has no outline: draw one with fusion_set_outline {outline:{rect:[x0,y0,w,h]}}.'}


def _schematic_of(app, board_doc):
    """The schematic document of the same design as the board: a saved design's documents share
    their cloud file name and folder; an unsaved one's are its same-named tabs opened together
    (the schematic comes right before the board)."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    def pt(d):
        try: return d.products.item(0).productType if d.products.count else ''
        except Exception: return ''
    if board_doc.dataFile is not None:
        ref = board_doc.dataFile
        for i, d in enumerate(docs):
            if pt(d) == 'SchematicProductType' and d.dataFile is not None and d.dataFile.name == ref.name and d.dataFile.parentFolder.id == ref.parentFolder.id:
                return i, d
        return None, None
    k = next(i for i, d in enumerate(docs) if d == board_doc)
    for i in range(k - 1, -1, -1):
        d = docs[i]
        if d.name != board_doc.name or d.dataFile is not None or pt(d) == 'EcadDesignProductType': break
        if pt(d) == 'SchematicProductType': return i, d
    return None, None


def run_erc(app, root, board, meta, args):
    """Fusion's own ERC on the board's schematic (the design's schematic tab is brought forward for
    the check and the board's tab put back), every message read from UL_SCHEMATIC.errors."""
    bdoc = app.activeDocument
    i, sdoc = _schematic_of(app, bdoc)
    if sdoc is None:
        fail('no_schematic', 'This board has no schematic open beside it: open the design (its project document), not the board alone')
    j = next(k for k in range(app.documents.count) if app.documents.item(k) == bdoc)
    with tempfile.TemporaryDirectory(prefix='adom-erc-') as d:
        out = Path(d) / 'erc.tsv'; ulp = Path(d) / 'erc.ulp'
        ulp.write_text('schematic(S) { output("' + out.as_posix() + '") { printf("CHECKED\\n"); S.errors(E) { printf("ERROR\\t%d\\t%d\\t%d\\t%d\\t%d\\t%d\\t%s\\n", E.code, E.type, E.state, E.sheet, E.x, E.y, E.description); } } }')
        try:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
            if app.activeDocument != sdoc: fail('wrong_document', 'Fusion did not bring the schematic tab forward for the ERC')
            app.executeTextCommand('Electron.run ERC;')
            app.executeTextCommand("Electron.run RUN '" + ulp.as_posix() + "';")
        finally:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
        lines = out.read_text().splitlines() if out.exists() else []
    if not lines or lines[0] != 'CHECKED': fail('erc_failed', 'Fusion ran the ERC but its result could not be read')
    msgs = []
    for line in lines[1:]:
        _, code, kind, state_, sheet, x, y, text = line.split('\t', 7)
        cls = 'warning' if kind == '1' else 'error'
        m = {'class': cls, 'source': 'ERC', 'document': sdoc.name, 'text': text, 'code': int(code), 'sheet': int(sheet), 'approved': state_ == '2'}
        if int(sheet): m.update(x=round(int(x) / 320000.0, 4), y=round(int(y) / 320000.0, 4), unit='mm')
        msgs.append(m)
    by = {}
    for m in msgs: by[m['class']] = by.get(m['class'], 0) + 1
    kinds = {}
    for m in msgs: kinds[m['code']] = kinds.get(m['code'], 0) + 1
    return {'success': True, 'mutated': False, **meta, 'messages': msgs, 'messageCount': len(msgs), 'byClass': by,
            'errorCount': sum(m['class'] == 'error' and not m['approved'] for m in msgs), 'warningCount': sum(m['class'] == 'warning' and not m['approved'] for m in msgs),
            'byCode': kinds, 'schematic': sdoc.name,
            '_hint': 'Fusion\'s ERC messages, each with its code, sheet and position (mm, on that sheet). Code 104 "POWER pin connected to" is informational on a supply net; 105 "No pins on net" and 109 "Close but unconnected wires" are the ones that usually mean a wiring mistake. Fusion keeps the result in the schematic, so it reads as modified.'}


def _copper_in_use(board):
    used = set()
    for w in board.findall('signals/signal/wire') + board.findall('signals/signal/polygon') + board.findall('plain/wire') + board.findall('plain/polygon'):
        n = int(w.get('layer', '0'))
        if n in COPPER: used.add(n)
    return used


def autoroute(app, root, board, meta, args):
    """Fusion's autorouter (EAGLE AUTO) on the given nets, or every net with unconnected copper.
    Refused before any command when a copper layer in use is switched off in the board's autorouter
    setup: Fusion then asks a Yes/No question (layers "used but not enabled") that neither SET
    CONFIRM nor AUTO LOAD answers, so the bridge never runs into it. Existing copper is kept
    (read back: every track and via from before is still there, or the run is undone)."""
    if args.get('expectedRevision') not in (None, meta['revision']): fail('stale_board', 'expectedRevision must match the current live board')
    if args.get('lockPreroutes') is False:
        fail('unsupported', "Fusion's autorouter keeps existing tracks; to reroute a net, rip it up first (fusion_ripup_nets) and autoroute it")
    pref = {int(p.get('name').split('.')[1]): p.get('value') for p in board.findall("autorouter/pass[@name='Default']/param") if p.get('name', '').startswith('PrefDir.')}
    L = Layers(board)
    off = sorted(n for n in _copper_in_use(board) if pref.get(n, 'a') == '0')
    if off:
        fail('layers_not_enabled', "The board's autorouter setup has %s switched off while they carry copper, and Fusion would stop to ask whether to run anyway (a dialog this bridge never lets it draw). "
             "Turn them on once in Fusion's Autorouter dialog (ROUTE > Autorouter, layer directions; Fusion keeps the setting in the board), or route these nets with fusion_route_net." % ', '.join('%s (%d)' % (L.name(n), n) for n in off))
    st = state(board, meta)
    names = {n.lower(): n for n in st['nets']}
    if args.get('nets'):
        bad = [n for n in args['nets'] if n.lower() not in names]
        if bad: fail('no_such_net', 'No net named %s' % ', '.join(bad))
        nets = [names[n.lower()] for n in args['nets']]
    else:
        nets = [x['net'] for x in st.get('splitNets') or []]
    before = {'unconnected': st['unconnected']}
    if args.get('drc', True):
        before['errors'] = native_drc(app)['errorCount']
    if not nets:
        return {'success': True, 'mutated': False, **meta, 'nets': [], 'before': before, 'after': before, '_hint': 'Every net is already connected: nothing to route.'}
    _, _, w0, v0 = objects(board)
    app.executeTextCommand('Electron.run AUTO %s;' % ' '.join("'%s'" % identifier(n) for n in nets))
    _, after, meta2 = snapshot(app, args)
    _, _, w1, v1 = objects(after)
    k1 = {_wire_key(x) for x in w1}; p1 = {tuple(round(c, 4) for c in v['at']) for v in v1}
    if not all(_wire_key(x) in k1 for x in w0) or not all(tuple(round(c, 4) for c in v['at']) in p1 for v in v0):
        _undo_until(app, args, meta['revision'], 3)
        fail('readback_mismatch', 'The autorouter changed copper that was there before it ran; the run was undone')
    st2 = state(after, meta2)
    aft = {'unconnected': st2['unconnected']}
    if args.get('drc', True):
        aft['errors'] = native_drc(app)['errorCount']
    done = [n for n in nets if n not in {x['net'] for x in st2.get('splitNets') or []}]
    return {'success': True, 'mutated': len(w1) != len(w0) or len(v1) != len(v0), **meta2, 'nets': nets, 'routed': done,
            'unrouted': [n for n in nets if n not in done], 'added': {'tracks': len(w1) - len(w0), 'vias': len(v1) - len(v0)}, 'before': before, 'after': aft,
            '_hint': "Fusion's autorouter ran on the listed nets; existing copper was kept. unrouted are nets it could not finish: route them with fusion_route_net or rip up and retry. Check after.errors with fusion_run_drc."}


def load_board_file(path):
    """A board file on the Fusion machine: EAGLE .brd XML, or the .brd inside a Fusion .fbrd."""
    pth = Path(path)
    if not pth.exists():
        fail('file_not_found', 'No file at %s on the Fusion machine' % path)
    raw = pth.read_bytes()
    if zipfile.is_zipfile(pth):
        with zipfile.ZipFile(pth) as z:
            names = [n for n in z.namelist() if n.endswith('.brd')]
            if len(names) != 1:
                fail('invalid_file', 'Expected one board inside %s, found %d' % (path, len(names)))
            raw = z.read(names[0])
    try:
        root = ET.fromstring(raw)
    except ET.ParseError as e:
        fail('invalid_file', 'Not a board file: %s' % e)
    board = root.find('drawing/board')
    if board is None:
        fail('invalid_file', '%s holds no board' % path)
    return root, board


def _netlist_of(board):
    return {s.get('name'): sorted((c.get('element'), c.get('pad')) for c in s.findall('contactref')) for s in board.findall('signals/signal')}


def _elements_of(board):
    return {e.get('name'): {'x': round(float(e.get('x')), 4), 'y': round(float(e.get('y')), 4), 'rot': (e.get('rot') or 'R0').lstrip('S') or 'R0',
                            'package': e.get('package'), 'library': e.get('library')} for e in board.findall('elements/element')}


def _plain_wires(board, layers):
    return sorted((int(w.get('layer')), round(_f(w, 'x1'), 4), round(_f(w, 'y1'), 4), round(_f(w, 'x2'), 4), round(_f(w, 'y2'), 4),
                   round(_f(w, 'width'), 4), round(_f(w, 'curve'), 3)) for w in board.findall('plain/wire') if int(w.get('layer', '0')) in layers)


def _norm_wire(t):
    """A wire with its ends in a fixed order, so a wire drawn the other way compares equal."""
    layer, x1, y1, x2, y2, w, c = t
    if (x2, y2) < (x1, y1):
        return (layer, x2, y2, x1, y1, w, -c)
    return t


def _holes(board):
    return sorted((round(_f(h, 'x'), 4), round(_f(h, 'y'), 4), round(_f(h, 'drill'), 4)) for h in board.findall('plain/hole'))


def _pour_rows(board):
    """Each pour as (net, layer, width, isolate, rank, outline). An EAGLE file writes <polygon>
    with its vertices; Fusion's own export writes <polygonpour> with them under
    <polygonoutlineobjects> (and its fill beside them), and no pour, orphans or thermals flags:
    only what both carry is compared."""
    rows = []
    for s_ in board.findall('signals/signal'):
        for p in list(s_.findall('polygon')) + list(s_.findall('polygonpour')):
            vs = p.findall('vertex') if p.tag == 'polygon' else p.findall('polygonoutlineobjects/vertex')
            verts = tuple((round(_f(v, 'x'), 4), round(_f(v, 'y'), 4), round(_f(v, 'curve'), 3)) for v in vs)
            rows.append((s_.get('name'), int(p.get('layer')), round(_f(p, 'width'), 4), round(_f(p, 'isolate'), 4), int(p.get('rank', '1')), verts))
    return rows


def _pour_key(r):
    """A pour by net, layer, pen and outline, whichever vertex it starts on."""
    return r[:5] + (tuple(sorted(r[5])),)


def update_board_from_eagle(app, root, board, meta, args):
    """Make the open design's board the board in an EAGLE file, in place: parts moved and turned
    by name, tracks and vias ripped up and drawn again from the file, pours, the outline, holes and
    net-less copper replaced; then the whole board read back against the file. Saving is the
    caller's (save:true, after a verified read back), so the cloud design changes only when
    the board in Fusion is exactly the file's."""
    if args.get('expectedRevision') not in (None, meta['revision']):
        fail('stale_board', 'expectedRevision must match the current live board')
    path = args.get('brdPath') or args.get('filePath')
    if not path:
        fail('bad_args', 'brdPath: the .brd (or .fbrd) on the Fusion machine')
    _, new = load_board_file(path)
    # the parts and the nets come from the schematic: a file that changes them is a schematic change
    e0, e1 = _elements_of(board), _elements_of(new)
    added, removed = sorted(set(e1) - set(e0)), sorted(set(e0) - set(e1))
    swapped = sorted(n for n in set(e0) & set(e1) if (e0[n]['package'], e0[n]['library']) != (e1[n]['package'], e1[n]['library']))
    n0, n1 = _netlist_of(board), _netlist_of(new)
    nets_diff = sorted(n for n in set(n0) | set(n1) if n0.get(n) != n1.get(n))
    if added or removed or swapped or nets_diff:
        fail('schematic_differs', 'The file changes what the schematic decides: %s. Update the schematic first (or import the files as a new design)' % '; '.join(x for x in (
            ('parts added %s' % added[:10]) if added else '', ('parts removed %s' % removed[:10]) if removed else '',
            ('packages changed %s' % swapped[:10]) if swapped else '', ('nets with other pads %s' % nets_diff[:10]) if nets_diff else '') if x))
    L0 = Layers(board)
    moves = [n for n in sorted(e1) if (e0[n]['x'], e0[n]['y'], e0[n]['rot']) != (e1[n]['x'], e1[n]['y'], e1[n]['rot'])]
    _, _, w_old, v_old = objects(board)
    _, _, w_new, v_new = objects(new)
    copper_same = copper_signature(w_old + v_old) == copper_signature(w_new + v_new)
    pours_same = sorted(map(_pour_key, _pour_rows(board))) == sorted(map(_pour_key, _pour_rows(new)))
    cu_layers = set(COPPER)
    outline_same = sorted(map(_norm_wire, _plain_wires(board, {20}))) == sorted(map(_norm_wire, _plain_wires(new, {20})))
    loose_same = sorted(map(_norm_wire, _plain_wires(board, cu_layers))) == sorted(map(_norm_wire, _plain_wires(new, cu_layers)))
    holes_same = _holes(board) == _holes(new)
    # not synced: what is not copper, parts, outline or holes
    def unsynced_plain(b):
        keep = cu_layers | {20}
        return sorted(ET.tostring(x) for x in b.find('plain') if not (x.tag == 'hole' or (x.tag == 'wire' and int(x.get('layer', '0')) in keep)))
    others = {'silkscreen and documentation (plain texts, lines and shapes off the copper and outline layers)': unsynced_plain(board) != unsynced_plain(new),
              'design rules': (ET.tostring(board.find('designrules')) if board.find('designrules') is not None else b'') != (ET.tostring(new.find('designrules')) if new.find('designrules') is not None else b''),
              'name and value positions': sorted(ET.tostring(a) for e in board.findall('elements/element') for a in e.findall('attribute')) != sorted(ET.tostring(a) for e in new.findall('elements/element') for a in e.findall('attribute'))}
    plan = {'partsMoved': moves, 'copper': not copper_same, 'pours': not pours_same, 'outline': not outline_same, 'looseCopper': not loose_same, 'holes': not holes_same}
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'plan': plan, 'notSynced': [k for k, v in others.items() if v],
                'tracks': len(w_new), 'vias': len(v_new), 'pours': len(_pour_rows(new))}
    if not any([moves, not copper_same, not pours_same, not outline_same, not loose_same, not holes_same]):
        return {'success': True, 'mutated': False, **meta, 'plan': plan, 'note': 'The board already is the file\'s (parts, copper, pours, outline, holes)',
                'notSynced': [k for k, v in others.items() if v]}
    visible = _visible_layers(root)
    lines = []
    def batch(cmds, size=30):
        for k in range(0, len(cmds), size):
            lines.append(' '.join(cmds[k:k + size]))
    try:
        # 1. tracks and vias up (pours stay until 2): every net that has any, now or in the file
        if not copper_same:
            nets = sorted({o['net'] for o in w_old + v_old})
            batch(["RIPUP '%s';" % identifier(n) for n in nets])
        # 2. old pours out: shift-click on the outline point farthest from every pad
        if not pours_same:
            pad_pts = [p_['at'] for p_ in objects(board)[1].values()]
            cmds = []
            for e in pours(board):
                ol = e['outline']; best = None
                for a_, b_ in zip(ol, ol[1:] + ol[:1]):
                    for t_ in (0.25, 0.5, 0.75):
                        q = (a_[0] + (b_[0] - a_[0]) * t_, a_[1] + (b_[1] - a_[1]) * t_)
                        dmin = min((math.hypot(q[0] - pp[0], q[1] - pp[1]) for pp in pad_pts), default=99)
                        if best is None or dmin > best[0]:
                            best = (dmin, q)
                if best[0] < 0.5:
                    fail('unsafe_pick', 'No outline point of the %s pour on %s is 0.5 mm clear of a pad: nothing was changed' % (e['net'], L0.name(e['layer'])))
                cmds.append('DISPLAY NONE %d; DELETE (S %.6fmm %.6fmm);' % (e['layer'], best[1][0], best[1][1]))
            batch(cmds, 10)
        # 3. parts to their places, by name
        batch(["MOVE '%s' (%.6fmm %.6fmm); ROTATE =%s '%s';" % (identifier(n), e1[n]['x'], e1[n]['y'], e1[n]['rot'], identifier(n)) for n in moves])
        # 4. the outline, loose copper and holes: old ones picked with only their layer shown, new ones drawn
        def redo_wires(layers):
            cmds = []
            for wv in board.findall('plain/wire'):
                if int(wv.get('layer', '0')) not in layers:
                    continue
                p1, p2, c = (_f(wv, 'x1'), _f(wv, 'y1')), (_f(wv, 'x2'), _f(wv, 'y2')), _f(wv, 'curve')
                pts_ = wire_points(p1, p2, c)
                mid = pts_[len(pts_) // 2] if c else ((p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2)
                cmds.append('DISPLAY NONE %s; DELETE (%.6fmm %.6fmm);' % (wv.get('layer'), mid[0], mid[1]))
            for wv in new.findall('plain/wire'):
                if int(wv.get('layer', '0')) not in layers:
                    continue
                c = _f(wv, 'curve')
                cmds.append('LAYER %s; WIRE %.6fmm (%.6fmm %.6fmm) %s(%.6fmm %.6fmm);' % (wv.get('layer'), _f(wv, 'width'), _f(wv, 'x1'), _f(wv, 'y1'), ('%+g ' % c) if c else '', _f(wv, 'x2'), _f(wv, 'y2')))
            batch(cmds, 20)
        if not outline_same:
            redo_wires({20})
        if not loose_same:
            redo_wires(cu_layers)
        if not holes_same:
            cmds = ['DISPLAY NONE 45; DELETE (%.6fmm %.6fmm);' % (x, y) for x, y, _ in _holes(board)]
            cmds += ['CHANGE DRILL %.6fmm; HOLE (%.6fmm %.6fmm);' % (d, x, y) for x, y, d in _holes(new)]
            batch(cmds, 20)
        # 5. the file's tracks and vias, each on its own net by name
        if not copper_same:
            cmds = []
            for s_ in new.findall('signals/signal'):
                n = identifier(s_.get('name'))
                for wv in s_.findall('wire'):
                    if wv.get('layer') == '19':
                        continue
                    c = _f(wv, 'curve')
                    cmds.append("LAYER %s; WIRE '%s' %.6fmm (%.6fmm %.6fmm) %s(%.6fmm %.6fmm);" % (wv.get('layer'), n, _f(wv, 'width'), _f(wv, 'x1'), _f(wv, 'y1'), ('%+g ' % c) if c else '', _f(wv, 'x2'), _f(wv, 'y2')))
                for v in s_.findall('via'):
                    shape = {'square': 'SQUARE', 'octagon': 'OCTAGON'}.get(v.get('shape', 'round'), 'ROUND')
                    dia = _f(v, 'diameter') or 0
                    cmds.append("CHANGE DRILL %.6fmm; VIA '%s' %s%s %s (%.6fmm %.6fmm);" % (_f(v, 'drill'), n, ('%.6fmm ' % dia) if dia else '', shape, v.get('extent', '1-16'), _f(v, 'x'), _f(v, 'y')))
            batch(cmds)
        # 6. the file's pours, with their own settings
        if not pours_same:
            cmds = []
            for s_ in new.findall('signals/signal'):
                for p in s_.findall('polygon'):
                    verts = p.findall('vertex')
                    path_ = ''
                    for k, v in enumerate(verts + verts[:1]):
                        path_ += '(%.6fmm %.6fmm) ' % (_f(v, 'x'), _f(v, 'y'))
                        c = _f(v, 'curve')
                        if c and k < len(verts):
                            path_ += '%+g ' % c
                    cmds.append("CHANGE ISOLATE %.6fmm; CHANGE RANK %s; CHANGE POUR %s; CHANGE ORPHANS %s; CHANGE THERMALS %s; LAYER %s; POLYGON '%s' %.6fmm %s;" % (
                        _f(p, 'isolate'), p.get('rank', '1'), p.get('pour', 'solid').upper(), 'ON' if p.get('orphans') == 'yes' else 'OFF',
                        'OFF' if p.get('thermals') == 'no' else 'ON', p.get('layer'), identifier(s_.get('name')), _f(p, 'width'), path_.strip()))
            batch(cmds, 10)
        for ln in lines:
            app.executeTextCommand('Electron.run ' + ln)
    finally:
        _restore_display(app, visible)
    app.executeTextCommand('Commands.Start Electron::PolyShowFillAll')
    app.executeTextCommand('Electron.run RATSNEST;')
    app.executeTextCommand('Electron.ImmediateRepaint /full')
    _, after, meta2 = snapshot(app, args)
    _, _, w_a, v_a = objects(after)
    checks = {'parts': _elements_of(after) == e1 or {n: (v['x'], v['y'], v['rot']) for n, v in _elements_of(after).items()} == {n: (v['x'], v['y'], v['rot']) for n, v in e1.items()},
              'copper': copper_signature(w_a + v_a) == copper_signature(w_new + v_new),
              'pours': sorted(map(_pour_key, _pour_rows(after))) == sorted(map(_pour_key, _pour_rows(new))),
              'outline': sorted(map(_norm_wire, _plain_wires(after, {20}))) == sorted(map(_norm_wire, _plain_wires(new, {20}))),
              'looseCopper': sorted(map(_norm_wire, _plain_wires(after, cu_layers))) == sorted(map(_norm_wire, _plain_wires(new, cu_layers))),
              'holes': _holes(after) == _holes(new),
              'nets': _netlist_of(after) == n1}
    wrong = sorted(k for k, ok in checks.items() if not ok)
    st = state(after, meta2)
    out = {'success': not wrong, 'mutated': True, 'verified': not wrong, **meta2, 'plan': plan, 'commandLines': len(lines),
           'notSynced': [k for k, v in others.items() if v], 'unconnected': st['unconnected'], 'tracks': len(w_a), 'vias': len(v_a)}
    if wrong:
        out.update(errorCode='readback_mismatch', mismatched=wrong,
                   error='The board in Fusion is not the file\'s in: %s. Nothing was saved: close the design without saving to discard these edits' % ', '.join(wrong))
    return out


# ── board layers to DXF (the standard's export_dxf) ─────────────────────────────────────────────

class Dxf:
    """A minimal DXF R12 writer: LINE, ARC, CIRCLE, POLYLINE (widths and bulges), TEXT."""

    def __init__(self, scale):
        self.k, self.out = scale, []

    def _g(self, *pairs):
        for code, v in pairs:
            self.out.append('%d\n%s' % (code, ('%.6f' % v) if isinstance(v, float) else v))

    def line(self, a, b):
        self._g((0, 'LINE'), (8, 'L'), (10, a[0] * self.k), (20, a[1] * self.k), (30, 0.0), (11, b[0] * self.k), (21, b[1] * self.k), (31, 0.0))

    def circle(self, c, r):
        self._g((0, 'CIRCLE'), (8, 'L'), (10, c[0] * self.k), (20, c[1] * self.k), (30, 0.0), (40, r * self.k))

    def arc(self, c, r, a1, a2):
        self._g((0, 'ARC'), (8, 'L'), (10, c[0] * self.k), (20, c[1] * self.k), (30, 0.0), (40, r * self.k), (50, float(a1)), (51, float(a2)))

    def poly(self, pts, closed=False, width=0.0, bulges=None):
        self._g((0, 'POLYLINE'), (8, 'L'), (66, 1), (10, 0.0), (20, 0.0), (30, 0.0), (70, 1 if closed else 0), (40, width * self.k), (41, width * self.k))
        for k, p in enumerate(pts):
            self._g((0, 'VERTEX'), (8, 'L'), (10, p[0] * self.k), (20, p[1] * self.k), (30, 0.0))
            if bulges and bulges[k]:
                self._g((42, float(bulges[k])))
        self._g((0, 'SEQEND'), (8, 'L'))

    H = {'left': 0, 'center': 1, 'right': 2}
    V = {'bottom': 1, 'center': 2, 'top': 3}

    def text(self, p, h, txt, rot=0.0, align='bottom-left', spin=False):
        """A text as Fusion shows it: alignment as EAGLE's (bottom-left, center, top-right...), and
        turned the readable way up unless it spins (Fusion turns a text past 90 degrees half round,
        swapping its alignment, so it never reads upside down). An R12 TEXT is one line: each line
        of a multi-line text is its own, stepped down."""
        parts = (align or 'bottom-left').split('-')
        v, hz = (parts[0], parts[1]) if len(parts) == 2 else (('center', 'center') if parts[0] == 'center' else ('bottom', 'left'))
        rot = float(rot) % 360
        if not spin and 90 < rot <= 270:
            rot = (rot - 180) % 360
            hz = {'left': 'right', 'right': 'left'}.get(hz, hz)
            v = {'bottom': 'top', 'top': 'bottom'}.get(v, v)
        a = math.radians(rot)
        for i, line in enumerate((txt or '').replace('\r', '').split('\n')):
            if not line.strip():
                continue
            dx, dy = math.sin(a) * h * 1.6 * i, -math.cos(a) * h * 1.6 * i
            x, y = (p[0] + dx) * self.k, (p[1] + dy) * self.k
            line = ''.join(c if ord(c) < 128 else '\\U+%04X' % ord(c) for c in line)  # R12 is not UTF-8: an ohm sign is written \\U+03A9
            tags = [(0, 'TEXT'), (8, 'L'), (10, x), (20, y), (30, 0.0), (40, h * self.k), (1, line), (50, rot)]
            if (hz, v) != ('left', 'bottom'):
                tags += [(72, self.H.get(hz, 0)), (11, x), (21, y), (31, 0.0), (73, self.V.get(v, 1))]
            self._g(*tags)

    def text_out(self):
        return '0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1009\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n' + '\n'.join(self.out) + '\n0\nENDSEC\n0\nEOF\n'


SIDE_LAYERS = {'F.SilkS': (21, 25), 'B.SilkS': (22, 26), 'F.Fab': (51,), 'B.Fab': (52,), 'Edge.Cuts': (20, 46), 'Dwgs.User': (47, 48)}
FLIP = {21: 22, 22: 21, 25: 26, 26: 25, 51: 52, 52: 51, 1: 16, 16: 1, 29: 30, 30: 29, 31: 32, 32: 31}


def _bulge(curve):
    return math.tan(math.radians(curve) / 4) if curve else 0.0


def _place(el):
    """A footprint-frame point to the board: rotate, then mirror x for a bottom part, then move."""
    mirrored, deg = 'M' in (el.get('rot') or ''), float((re.search(r'R(-?[\d.]+)', el.get('rot') or 'R0') or [0, 0])[1])
    a, ex, ey = math.radians(deg), float(el.get('x')), float(el.get('y'))
    def f(x, y):
        x, y = x * math.cos(a) - y * math.sin(a), x * math.sin(a) + y * math.cos(a)
        if mirrored:
            x = -x
        return (ex + x, ey + y)
    return f, mirrored, deg


def board_dxf(app, root, board, meta, args):
    """Chosen board layers to DXF, one file per layer: the outline (Edge.Cuts: Dimension 20 and
    Milling 46), copper (tracks with their width, arcs, vias, pads, pour fills), silkscreen and fab
    graphics (footprint and board lines, circles, rectangles, polygons, texts)."""
    want = args.get('layers')
    if not isinstance(want, list) or not want:
        fail('bad_args', 'layers is required: ["Edge.Cuts", "F.Cu", ...] (standard names) or Fusion layer numbers')
    units = args.get('units', 'mm')
    k = 1 / 25.4 if units == 'in' else 1.0
    out_dir = Path(args.get('outputDir') or (Path(tempfile.gettempdir()) / 'adom-dxf'))
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', meta.get('documentName') or 'board')
    model, _ = board_model(board, meta)
    L = Layers(board)
    num = {nm: n for n, nm in L.names.items()}
    pk = {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library') for p in l.findall('packages/package')}
    files, empty = [], []
    for lay in want:
        if isinstance(lay, int) or (isinstance(lay, str) and lay.isdigit()):
            eagle = (int(lay),)
            label = 'L%s' % lay
        elif lay in SIDE_LAYERS:
            eagle, label = SIDE_LAYERS[lay], lay
        elif lay in num:
            eagle, label = (num[lay],), lay
        else:
            fail('invalid_layer', '%s: use Edge.Cuts, F.Cu, In<n>.Cu, B.Cu, F.SilkS, B.SilkS, F.Fab, B.Fab, Dwgs.User or a Fusion layer number' % lay)
        d = Dxf(k)
        n0 = 0
        if 20 in eagle:
            edges = list(model['outlineEdges']) + [e for c in model['outlineCutouts'] for e in c['edges']]
            if args.get('contours'):
                for loop in [model['outline']] + [c['outline'] for c in model['outlineCutouts']]:
                    d.poly([tuple(p) for p in loop], closed=True)
            else:
                for e in edges:
                    if e['kind'] == 'arc':
                        d.arc(e['center'], e['radius'], e['angle1'], e['angle2'])
                    else:
                        d.line(e['start'], e['end'])
        cu = [n for n in eagle if n in COPPER]
        for n in cu:
            name = L.name(n)
            for t in model['tracks']:
                if t['layerId'] == n:
                    d.poly([(t['start']['x'], t['start']['y']), (t['end']['x'], t['end']['y'])], width=t['width'])
            for a in model['arcs']:
                if a['layerId'] == n:
                    c, r_ = (a['center']['x'], a['center']['y']), a['radius']
                    p1 = (c[0] + r_ * math.cos(math.radians(a['startAngle'])), c[1] + r_ * math.sin(math.radians(a['startAngle'])))
                    p2 = (c[0] + r_ * math.cos(math.radians(a['endAngle'])), c[1] + r_ * math.sin(math.radians(a['endAngle'])))
                    d.poly([p1, p2], width=a['width'], bulges=[_bulge((a['endAngle'] - a['startAngle']) % 360), 0])
            for v in model['vias']:
                if name in v['layers']:
                    d.circle((v['position']['x'], v['position']['y']), v['size'] / 2)
            for p_ in model['pads']:
                if p_['layer'] not in (name, '*.Cu'):
                    continue
                if p_.get('copperPoints'):
                    d.poly([tuple(q) for q in p_['copperPoints']], closed=True)
                elif p_.get('shape') in ('circle', 'round') or (p_.get('drill') and abs(p_['sizeX'] - p_['sizeY']) < 1e-6 and p_.get('shape') != 'rect'):
                    d.circle((p_['x'], p_['y']), p_['sizeX'] / 2)
                else:
                    hx, hy, a = p_['sizeX'] / 2, p_['sizeY'] / 2, math.radians(p_.get('rotation') or 0)
                    pts = [(p_['x'] + cx * math.cos(a) - cy * math.sin(a), p_['y'] + cx * math.sin(a) + cy * math.cos(a)) for cx, cy in ((-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy))]
                    d.poly(pts, closed=True)
            for z in zones(board):
                if z['layerId'] != n:
                    continue
                # the copper Fusion poured, island by island with its holes; a file with no fill
                # (an EAGLE board never poured) gives the pour as drawn
                for isl in z['fills']:
                    d.poly([tuple(q) for q in isl['outline']], closed=True)
                    for h in isl['holes']:
                        d.poly([tuple(q) for q in h], closed=True)
                if not z['fills']:
                    d.poly([tuple(q) for q in z['outline']], closed=True)
        other = [n for n in eagle if n not in COPPER and n != 20]
        if other or 20 in eagle:
            gl = set(other) | ({46} if 20 in eagle else set())
            for w in board.findall('plain/wire'):
                if int(w.get('layer', '0')) in gl:
                    d.poly([(_f(w, 'x1'), _f(w, 'y1')), (_f(w, 'x2'), _f(w, 'y2'))], width=_f(w, 'width'), bulges=[_bulge(_f(w, 'curve')), 0])
            for t in board.findall('plain/text'):
                if int(t.get('layer', '0')) in gl:
                    d.text((_f(t, 'x'), _f(t, 'y')), _f(t, 'size', 1.27), t.text or '', float((re.search(r'R(-?[\d.]+)', t.get('rot') or 'R0') or [0, 0])[1]),
                           t.get('align', 'bottom-left'), 'S' in (t.get('rot') or ''))
            for el in board.findall('elements/element'):
                pkg = pk.get((el.get('library'), el.get('package')))
                if pkg is None:
                    continue
                f, mirrored, deg = _place(el)
                for g in pkg:
                    lay_ = int(g.get('layer', '0') or 0)
                    if mirrored:
                        lay_ = FLIP.get(lay_, lay_)
                    if lay_ not in gl:
                        continue
                    if g.tag == 'wire':
                        c = _f(g, 'curve')
                        d.poly([f(_f(g, 'x1'), _f(g, 'y1')), f(_f(g, 'x2'), _f(g, 'y2'))], width=_f(g, 'width'), bulges=[_bulge(-c if mirrored else c), 0])
                    elif g.tag == 'circle':
                        d.circle(f(_f(g, 'x'), _f(g, 'y')), _f(g, 'radius'))
                    elif g.tag == 'rectangle':
                        x1, y1, x2, y2 = _f(g, 'x1'), _f(g, 'y1'), _f(g, 'x2'), _f(g, 'y2')
                        d.poly([f(x1, y1), f(x2, y1), f(x2, y2), f(x1, y2)], closed=True)
                    elif g.tag == 'polygon':
                        d.poly([f(_f(v, 'x'), _f(v, 'y')) for v in g.findall('vertex')], closed=True)
                    elif g.tag == 'text':
                        raw = (g.text or '').strip()
                        if raw.startswith('>'):
                            # a placeholder: the part's name, value or attribute (any case); none, nothing drawn
                            key = raw[1:].upper()
                            attrs = {a.get('name', '').upper(): a for a in el.findall('attribute')}
                            txt = {'NAME': el.get('name'), 'VALUE': el.get('value') or ''}.get(key)
                            if txt is None:
                                txt = attrs[key].get('value') if key in attrs else None
                            if not txt:
                                continue
                            sm = attrs.get(key)
                            if sm is not None and sm.get('display') == 'off':
                                continue
                            if sm is not None and sm.get('x') is not None:
                                d.text((_f(sm, 'x'), _f(sm, 'y')), _f(sm, 'size', 1.27), txt, float((re.search(r'R(-?[\d.]+)', sm.get('rot') or 'R0') or [0, 0])[1]),
                                       sm.get('align', 'bottom-left'), 'S' in (sm.get('rot') or ''))
                                continue
                        else:
                            txt = g.text or ''
                        trot = float((re.search(r'R(-?[\d.]+)', g.get('rot') or 'R0') or [0, 0])[1])
                        d.text(f(_f(g, 'x'), _f(g, 'y')), _f(g, 'size', 1.27), txt, (deg + (-trot if mirrored else trot)) % 360,
                               g.get('align', 'bottom-left'), 'S' in (g.get('rot') or ''))
        path = out_dir / ('%s-%s.dxf' % (stem, label.replace('.', '_')))
        path.write_text(d.text_out(), encoding='ascii')
        n_ent = len([x for x in d.out if x.startswith('0\n') and not x.startswith('0\nVERTEX') and not x.startswith('0\nSEQEND')])
        files.append({'layer': label, 'path': path.as_posix(), 'entities': n_ent})
        if not n_ent:
            empty.append(label)
    return {'success': True, 'mutated': False, **meta, 'files': files, 'units': units, 'empty': empty,
            '_hint': 'One DXF (R12) per layer, in %s with the board\'s origin and y up. Tracks are polylines carrying their width; contours:true joins the outline into closed polylines.%s' % (units, (' Empty: %s.' % ', '.join(empty)) if empty else '')}


# ── part and graphic edits (the standard's edit_parts, align_parts, remove_graphics, remove_parts) ──

def _element(board, ref):
    return next((e for e in board.findall('elements/element') if e.get('name') == ref), None)


def _guard(meta, args):
    if args.get('expectedRevision') not in (None, meta.get('revision')):
        fail('stale_board', 'expectedRevision must match the current live board')


def edit_parts(app, root, board, meta, args):
    """Value, designator, fields, the lock, do-not-populate, BOM and position exclusion and mounting type
    of placed parts; each edit read back. Fusion keeps board and schematic in step, so a value or name
    changed here is changed in the schematic too."""
    _guard(meta, args)
    edits = args.get('edits') or []
    if not edits:
        fail('bad_args', 'edits is [{ref, value?, newRef?, fields?: {name: value | null}, locked?}]')
    names = {e.get('name') for e in board.findall('elements/element')}
    plan, failed = [], []
    for ed in edits:
        ref = ed.get('ref')
        el = _element(board, ref)
        if el is None:
            failed.append({'ref': ref, 'error': 'no part %s on the board' % ref})
            continue
        # do-not-populate, BOM and position exclusion and mounting type: attributes on the part, which
        # fusion_board_bom, fusion_bom and fusion_export_pos honour. Fusion's assembly variants cannot be
        # changed from a command (VARIANT does nothing on 2706), so its own CAM outputs do not read them.
        have = {a.get('name') for a in el.findall('attribute')}
        flags = dict(ed.get('fields') or {})
        for key, attr in (('dnp', 'DNP'), ('excludeFromBom', 'EXCLUDE_FROM_BOM'), ('excludeFromPos', 'EXCLUDE_FROM_POS')):
            if key in ed:
                if ed[key]: flags[attr] = 'yes'
                elif attr in have: flags[attr] = None
        if 'mount' in ed:
            if ed['mount'] not in ('smd', 'through_hole', 'other'):
                failed.append({'ref': ref, 'error': 'mount is smd, through_hole or other'})
            else: flags['MOUNT'] = ed['mount']
        if flags: ed = dict(ed, fields=flags)
        cmds, cur = [], ref
        if ed.get('value') is not None:
            cmds.append("SET CONFIRM YES; VALUE '%s' %s; SET CONFIRM OFF;" % (identifier(cur), "'" + str(ed['value']).replace("'", "''") + "'"))
        for k, v in (ed.get('fields') or {}).items():
            if k.upper() in ('NAME', 'VALUE'):
                failed.append({'ref': ref, 'error': '%s is required: use newRef or value' % k})
                continue
            if v is None:
                cmds.append("SET CONFIRM YES; ATTRIBUTE '%s' '%s' DELETE; SET CONFIRM OFF;" % (identifier(cur), k.replace("'", "''")))
            else:
                cmds.append("SET CONFIRM YES; ATTRIBUTE '%s' '%s' '%s'; SET CONFIRM OFF;" % (identifier(cur), k.replace("'", "''"), str(v).replace("'", "''")))
        if ed.get('locked') is True:
            cmds.append("LOCK '%s';" % identifier(cur))
        elif ed.get('locked') is False:
            # a shift-click LOCK on the part's origin unlocks it ("LOCK -" is not a command)
            cmds.append('LOCK (S%.6fmm %.6fmm);' % (float(el.get('x')), float(el.get('y'))))
        if ed.get('newRef') and ed['newRef'] != ref:
            if ed['newRef'] in names:
                failed.append({'ref': ref, 'error': 'designator %s is already taken' % ed['newRef']})
            else:
                cmds.append("SET CONFIRM YES; NAME '%s' '%s'; SET CONFIRM OFF;" % (identifier(cur), identifier(ed['newRef'])))
                names.add(ed['newRef'])
        if cmds:
            plan.append({'ref': ref, 'edit': ed, 'cmds': cmds})
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'plan': [{'ref': p_['ref'], 'commands': p_['cmds']} for p_ in plan], 'failed': failed}
    for p_ in plan:
        for c in p_['cmds']:
            app.executeTextCommand('Electron.run ' + c)
    _, after, meta2 = snapshot(app, args)
    edited = []
    for p_ in plan:
        ed, ref = p_['edit'], p_['ref']
        name = ed.get('newRef') or ref
        el = _element(after, name)
        attrs = {a.get('name'): a.get('value') for a in el.findall('attribute')} if el is not None else {}
        bad = []
        if el is None:
            bad.append('part %s not found after the edit' % name)
        else:
            if ed.get('value') is not None and el.get('value') != str(ed['value']):
                bad.append('value is %s' % el.get('value'))
            for k, v in (ed.get('fields') or {}).items():
                if k.upper() in ('NAME', 'VALUE'):
                    continue
                got = next((val for an, val in attrs.items() if an.upper() == k.upper()), None)
                if v is None and got is not None:
                    bad.append('field %s is still %s: a field the library part defines cannot be removed from the board (set it to another value instead)' % (k, got))
                elif v is not None and got != str(v):
                    bad.append('field %s is %s' % (k, got))
            if 'locked' in ed and (el.get('locked') == 'yes') != bool(ed['locked']):
                bad.append('locked is %s' % (el.get('locked') == 'yes'))
        if bad:
            failed.append({'ref': ref, 'error': '; '.join(bad)})
        else:
            edited.append({'ref': ref, 'after': {'ref': name, 'value': el.get('value'), 'fields': attrs, 'locked': el.get('locked') == 'yes'}})
    return {'success': not failed, 'mutated': bool(plan), **meta2, 'edited': edited, 'failed': failed,
            '_hint': 'Fusion keeps board and schematic in step: these edits are in the schematic too.'}


def part_boxes(board):
    """Each part's footprint box on the board (pads, placement and documentation outlines,
    keepouts), from its package through the part's turn and mirror."""
    pk = {(l.get('name'), p.get('name')): p for l in board.findall('libraries/library') for p in l.findall('packages/package')}
    out = {}
    for el in board.findall('elements/element'):
        pkg = pk.get((el.get('library'), el.get('package')))
        f, _, _ = _place(el)
        xs, ys = [], []
        for g in (pkg if pkg is not None else []):
            pts = []
            if g.tag in ('smd',):
                hx, hy = _f(g, 'dx') / 2, _f(g, 'dy') / 2
                pts = [(_f(g, 'x') + a, _f(g, 'y') + b) for a in (-hx, hx) for b in (-hy, hy)]
            elif g.tag == 'pad':
                r_ = max(_f(g, 'diameter'), _f(g, 'drill') * 1.6) / 2
                pts = [(_f(g, 'x') + a, _f(g, 'y') + b) for a in (-r_, r_) for b in (-r_, r_)]
            elif g.tag in ('wire', 'rectangle') and g.get('layer') in ('21', '22', '39', '40', '51', '52'):
                pts = [(_f(g, 'x1'), _f(g, 'y1')), (_f(g, 'x2'), _f(g, 'y2'))]
            elif g.tag == 'circle' and g.get('layer') in ('21', '22', '39', '40', '51', '52'):
                r_ = _f(g, 'radius')
                pts = [(_f(g, 'x') + a, _f(g, 'y') + b) for a in (-r_, r_) for b in (-r_, r_)]
            for x, y in pts:
                X, Y = f(x, y)
                xs.append(X)
                ys.append(Y)
        ex, ey = float(el.get('x')), float(el.get('y'))
        out[el.get('name')] = (min(xs + [ex]), min(ys + [ey]), max(xs + [ex]), max(ys + [ey]))
    return out


def align_parts(app, root, board, meta, args):
    _guard(meta, args)
    refs = args.get('refs') or []
    mode = args.get('mode')
    if len(refs) < 2 or mode not in ('left', 'right', 'top', 'bottom', 'centerX', 'centerY', 'distributeX', 'distributeY'):
        fail('bad_args', 'refs (two or more) and mode left|right|top|bottom|centerX|centerY|distributeX|distributeY are required')
    els = {r_: _element(board, r_) for r_ in refs}
    missing = [r_ for r_, e in els.items() if e is None]
    if missing:
        fail('designator_not_found', 'No part %s on the board' % ', '.join(missing))
    by = args.get('by', 'courtyard')
    boxes = part_boxes(board)
    anch = {r_: (float(e.get('x')), float(e.get('y'))) for r_, e in els.items()}
    box = {r_: boxes[r_] if by == 'courtyard' else (anch[r_][0], anch[r_][1], anch[r_][0], anch[r_][1]) for r_ in refs}
    to = args.get('to')
    targets = {}
    if mode in ('left', 'right', 'top', 'bottom', 'centerX', 'centerY'):
        key = {'left': lambda b: b[0], 'right': lambda b: b[2], 'bottom': lambda b: b[1], 'top': lambda b: b[3],
               'centerX': lambda b: (b[0] + b[2]) / 2, 'centerY': lambda b: (b[1] + b[3]) / 2}[mode]
        vals = [key(box[r_]) for r_ in refs]
        line = float(to) if to is not None else {'left': min, 'bottom': min, 'right': max, 'top': max}.get(mode, lambda v: sum(v) / len(v))(vals)
        for r_ in refs:
            d = line - key(box[r_])
            x, y = anch[r_]
            targets[r_] = (x + d, y) if mode in ('left', 'right', 'centerX') else (x, y + d)
    else:
        ax = 0 if mode == 'distributeX' else 1
        order = sorted(refs, key=lambda r_: (box[r_][ax] + box[r_][ax + 2]) / 2)
        sizes = [box[r_][ax + 2] - box[r_][ax] for r_ in order]
        first, last = box[order[0]], box[order[-1]]
        if args.get('gapMm') is not None:
            gap = float(args['gapMm'])
        else:
            gap = ((last[ax + 2] - first[ax]) - sum(sizes)) / (len(order) - 1)
        pos = first[ax]
        for r_, sz in zip(order, sizes):
            d = pos - box[r_][ax]
            x, y = anch[r_]
            targets[r_] = (x + d, y) if ax == 0 else (x, y + d)
            pos += sz + gap
    # the checks move_parts makes: on the board, not over another part
    model, _ = board_model(board, meta)
    xs_ = [p[0] for p in model['outline']] or [-1e9, 1e9]
    ys_ = [p[1] for p in model['outline']] or [-1e9, 1e9]
    problems = []
    moved_box = {}
    for r_ in refs:
        dx, dy = targets[r_][0] - anch[r_][0], targets[r_][1] - anch[r_][1]
        b = boxes[r_]
        moved_box[r_] = (b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy)
        if not args.get('allowOutside') and (moved_box[r_][0] < min(xs_) - 1e-6 or moved_box[r_][2] > max(xs_) + 1e-6 or moved_box[r_][1] < min(ys_) - 1e-6 or moved_box[r_][3] > max(ys_) + 1e-6):
            problems.append('%s would leave the board' % r_)
    if not args.get('allowOverlap'):
        # only overlaps the move makes: footprint boxes of a dense board already touch
        def hit(b1, b2):
            return b1[0] < b2[2] - 1e-6 and b2[0] < b1[2] - 1e-6 and b1[1] < b2[3] - 1e-6 and b2[1] < b1[3] - 1e-6
        after_b = dict(boxes); after_b.update(moved_box)
        names_ = sorted(after_b)
        for i_, n1 in enumerate(names_):
            for n2 in names_[i_ + 1:]:
                if (n1 in refs or n2 in refs) and hit(after_b[n1], after_b[n2]) and not hit(boxes[n1], boxes[n2]):
                    problems.append('%s would overlap %s' % (n1, n2))
    tgt = [{'ref': r_, 'x': round(targets[r_][0], 4), 'y': round(targets[r_][1], 4)} for r_ in refs]
    if problems:
        fail('placement_refused', '; '.join(sorted(set(problems))[:10]) + ' (allowOverlap / allowOutside to allow)')
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'targets': tgt}
    for t in tgt:
        if abs(t['x'] - anch[t['ref']][0]) > 1e-6 or abs(t['y'] - anch[t['ref']][1]) > 1e-6:
            app.executeTextCommand("Electron.run MOVE '%s' (%.6fmm %.6fmm);" % (identifier(t['ref']), t['x'], t['y']))
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    mism = []
    for t in tgt:
        e = _element(after, t['ref'])
        got = (round(float(e.get('x')), 4), round(float(e.get('y')), 4))
        if abs(got[0] - t['x']) > 1e-3 or abs(got[1] - t['y']) > 1e-3:
            mism.append({'ref': t['ref'], 'asked': [t['x'], t['y']], 'got': list(got)})
    return {'success': not mism, 'mutated': True, **meta2, 'targets': tgt, 'mismatches': mism}


def plain_graphics(board, L):
    """Board-level graphics with ids built from kind, layer and position (Fusion stores none)."""
    out = []
    for g in board.find('plain'):
        layer = int(g.get('layer', '0') or 0)
        name = L.name(layer)
        if g.tag == 'wire':
            c = _f(g, 'curve')
            kind = 'arc' if c else 'line'
            pts = [[_f(g, 'x1'), _f(g, 'y1')], [_f(g, 'x2'), _f(g, 'y2')]]
            pp = wire_points(tuple(pts[0]), tuple(pts[1]), c)
            pick = pp[len(pp) // 2] if c else ((pts[0][0] + pts[1][0]) / 2, (pts[0][1] + pts[1][1]) / 2)
        elif g.tag == 'rectangle':
            kind, pts = 'rect', [[_f(g, 'x1'), _f(g, 'y1')], [_f(g, 'x2'), _f(g, 'y2')]]
            pick = (pts[0][0], (pts[0][1] + pts[1][1]) / 2)
        elif g.tag == 'circle':
            kind, pts = 'circle', [[_f(g, 'x'), _f(g, 'y')]]
            pick = (_f(g, 'x') + _f(g, 'radius'), _f(g, 'y'))
        elif g.tag == 'polygon':
            vs = g.findall('vertex')
            kind, pts = 'polygon', [[_f(v, 'x'), _f(v, 'y')] for v in vs]
            pick = ((pts[0][0] + pts[1][0]) / 2, (pts[0][1] + pts[1][1]) / 2) if len(pts) > 1 else tuple(pts[0])
        elif g.tag == 'text':
            kind, pts = 'text', [[_f(g, 'x'), _f(g, 'y')]]
            pick = tuple(pts[0])
        elif g.tag == 'dimension':
            kind, pts = 'dimension', [[_f(g, 'x1'), _f(g, 'y1')], [_f(g, 'x2'), _f(g, 'y2')]]
            pick = tuple(pts[0])
        else:
            continue
        gid = '%s:%d:%s' % (kind, layer, ','.join('%g,%g' % (round(p[0], 4), round(p[1], 4)) for p in pts))
        out.append({'id': gid, 'kind': kind, 'layer': name, 'layerId': layer, 'points': [[round(a, 4), round(b, 4)] for a, b in pts],
                    'text': g.text if g.tag == 'text' else None, 'pick': pick, 'shift': g.tag == 'polygon', 'node': g})
    return out


def remove_graphics(app, root, board, meta, args):
    _guard(meta, args)
    L = Layers(board)
    allg = plain_graphics(board, L)
    want_ids = args.get('ids')
    if want_ids:
        unknown = [i for i in want_ids if i not in {g['id'] for g in allg}]
        if unknown:
            fail('unknown_ids', 'No board graphic has id %s (a dry run with a layer lists them)' % ', '.join(unknown[:5]))
        hits = [g for g in allg if g['id'] in want_ids]
    else:
        hits = allg
    if args.get('layer') is not None:
        lay = args['layer']
        n = SIDE_LAYERS.get(lay, (None,))[0] if lay in SIDE_LAYERS else _layer_number(board, lay)
        hits = [g for g in hits if g['layerId'] == n or (lay in SIDE_LAYERS and g['layerId'] in SIDE_LAYERS[lay])]
    if args.get('kinds'):
        hits = [g for g in hits if g['kind'] in args['kinds']]
    if not want_ids and args.get('layer') is None and not args.get('kinds'):
        fail('bad_args', 'Give ids, or a layer and/or kinds to remove every match')
    rows = [{k: g[k] for k in ('id', 'kind', 'layer', 'points', 'text')} for g in hits]
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'removed': rows, 'count': len(rows)}
    if not hits:
        return {'success': True, 'mutated': False, **meta, 'removed': [], 'note': 'nothing matched'}
    visible = _visible_layers(root)
    try:
        for g in hits:
            app.executeTextCommand('Electron.run DISPLAY NONE %d; DELETE (%s%.6fmm %.6fmm);' % (g['layerId'], 'S' if g['shift'] else '', g['pick'][0], g['pick'][1]))
    finally:
        _restore_display(app, visible)
    _, after, meta2 = snapshot(app, args)
    left = {g['id'] for g in plain_graphics(after, Layers(after))}
    should = {g['id'] for g in allg} - {g['id'] for g in hits}
    _, _, w0, v0 = objects(board)
    _, _, w1, v1 = objects(after)
    if left != should or copper_signature(w0 + v0) != copper_signature(w1 + v1) or _elements_of(after) != _elements_of(board):
        for _ in hits:
            app.executeTextCommand('Electron.run UNDO;')
        fail('readback_mismatch', 'The board graphics read back were not the old ones minus the asked ones (still there %s, gone too %s); every DELETE was undone'
             % (sorted({g['id'] for g in hits} & left)[:4], sorted(should - left)[:4]))
    return {'success': True, 'mutated': True, **meta2, 'removed': rows}


# ── import_svg ──────────────────────────────────────────────────────────────────────────────────

def _svg_floats(t):
    return [float(x) for x in re.findall(r'[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?', t or '')]


def _svg_matrix(t):
    """An SVG transform attribute as a 2x3 affine (a, b, c, d, e, f)."""
    M = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
    def mul(A, B):
        a1, b1, c1, d1, e1, f1 = A; a2, b2, c2, d2, e2, f2 = B
        return (a1 * a2 + c1 * b2, b1 * a2 + d1 * b2, a1 * c2 + c1 * d2, b1 * c2 + d1 * d2, a1 * e2 + c1 * f2 + e1, b1 * e2 + d1 * f2 + f1)
    for name, body in re.findall(r'(matrix|translate|scale|rotate|skewX|skewY)\s*\(([^)]*)\)', t or ''):
        v = _svg_floats(body)
        if name == 'matrix' and len(v) == 6: T = tuple(v)
        elif name == 'translate': T = (1, 0, 0, 1, v[0], v[1] if len(v) > 1 else 0)
        elif name == 'scale': T = (v[0], 0, 0, v[1] if len(v) > 1 else v[0], 0, 0)
        elif name == 'rotate':
            a = math.radians(v[0]); c, s_ = math.cos(a), math.sin(a)
            T = (c, s_, -s_, c, 0, 0)
            if len(v) == 3:
                T = mul(mul((1, 0, 0, 1, v[1], v[2]), T), (1, 0, 0, 1, -v[1], -v[2]))
        elif name == 'skewX': T = (1, 0, math.tan(math.radians(v[0])), 1, 0, 0)
        else: T = (1, math.tan(math.radians(v[0])), 0, 1, 0, 0)
        M = mul(M, T)
    return M


def _svg_apply(M, p):
    a, b, c, d, e, f = M
    return (a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f)


def _bezier(p0, p1, p2, p3, tol):
    n = max(2, min(64, int(math.ceil(math.sqrt((math.hypot(p1[0] - p0[0], p1[1] - p0[1]) + math.hypot(p2[0] - p1[0], p2[1] - p1[1]) + math.hypot(p3[0] - p2[0], p3[1] - p2[1])) / max(tol, 1e-6))))))
    out = []
    for k in range(1, n + 1):
        t = k / n; u = 1 - t
        out.append((u ** 3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t ** 3 * p3[0],
                    u ** 3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t ** 3 * p3[1]))
    return out


def _arc(p0, rx, ry, phi, large, sweep, p1, tol):
    """SVG elliptical arc (endpoint form) flattened."""
    if rx == 0 or ry == 0:
        return [p1]
    rx, ry = abs(rx), abs(ry)
    c, s_ = math.cos(math.radians(phi)), math.sin(math.radians(phi))
    dx, dy = (p0[0] - p1[0]) / 2, (p0[1] - p1[1]) / 2
    x1, y1 = c * dx + s_ * dy, -s_ * dx + c * dy
    lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry)
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
    den = rx * rx * y1 * y1 + ry * ry * x1 * x1
    co = math.sqrt(max(0.0, num / den)) if den else 0.0
    if large == sweep:
        co = -co
    cx1, cy1 = co * rx * y1 / ry, -co * ry * x1 / rx
    cx, cy = c * cx1 - s_ * cy1 + (p0[0] + p1[0]) / 2, s_ * cx1 + c * cy1 + (p0[1] + p1[1]) / 2
    ang = lambda ux, uy, vx, vy: math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
    t1 = ang(1, 0, (x1 - cx1) / rx, (y1 - cy1) / ry)
    dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry)
    if not sweep and dt > 0: dt -= 2 * math.pi
    elif sweep and dt < 0: dt += 2 * math.pi
    n = max(2, min(128, int(math.ceil(abs(dt) / max(1e-3, 2 * math.acos(max(-1.0, min(1.0, 1 - tol / max(rx, ry)))))))))
    out = []
    for k in range(1, n + 1):
        t = t1 + dt * k / n
        x, y = rx * math.cos(t), ry * math.sin(t)
        out.append((c * x - s_ * y + cx, s_ * x + c * y + cy))
    return out


def _svg_path_rings(d, tol):
    toks = re.findall(r'[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?', d or '')
    rings, cur, pos, start, last_ctrl, cmd, i = [], [], (0.0, 0.0), (0.0, 0.0), None, None, 0
    def num():
        nonlocal i
        v = float(toks[i]); i += 1
        return v
    def isnum(k):
        return k < len(toks) and not toks[k].isalpha()
    while i < len(toks):
        if toks[i].isalpha():
            cmd = toks[i]; i += 1
        elif cmd is None:
            break
        rel = cmd.islower(); C = cmd.upper()
        if C == 'Z':
            if cur: rings.append(cur)
            cur, pos, last_ctrl = [], start, None
            continue
        if not isnum(i):
            continue
        base = pos if rel else (0.0, 0.0)
        if C == 'M':
            if cur: rings.append(cur)
            pos = (base[0] + num(), base[1] + num()); start = pos; cur = [pos]; last_ctrl = None
            cmd = 'l' if rel else 'L'
        elif C == 'L':
            pos = (base[0] + num(), base[1] + num()); cur.append(pos); last_ctrl = None
        elif C == 'H':
            pos = ((pos[0] if rel else 0) + num(), pos[1]); cur.append(pos); last_ctrl = None
        elif C == 'V':
            pos = (pos[0], (pos[1] if rel else 0) + num()); cur.append(pos); last_ctrl = None
        elif C in 'CS':
            if C == 'C':
                p1 = (base[0] + num(), base[1] + num())
            else:
                p1 = (2 * pos[0] - last_ctrl[0], 2 * pos[1] - last_ctrl[1]) if last_ctrl and last_ctrl[2] == 'c' else pos
            p2 = (base[0] + num(), base[1] + num()); p3 = (base[0] + num(), base[1] + num())
            cur += _bezier(pos, p1[:2], p2, p3, tol); pos = p3; last_ctrl = (p2[0], p2[1], 'c')
        elif C in 'QT':
            if C == 'Q':
                q = (base[0] + num(), base[1] + num())
            else:
                q = (2 * pos[0] - last_ctrl[0], 2 * pos[1] - last_ctrl[1]) if last_ctrl and last_ctrl[2] == 'q' else pos
            p3 = (base[0] + num(), base[1] + num())
            p1 = (pos[0] + 2 / 3 * (q[0] - pos[0]), pos[1] + 2 / 3 * (q[1] - pos[1])); p2 = (p3[0] + 2 / 3 * (q[0] - p3[0]), p3[1] + 2 / 3 * (q[1] - p3[1]))
            cur += _bezier(pos, p1, p2, p3, tol); pos = p3; last_ctrl = (q[0], q[1], 'q')
        elif C == 'A':
            rx, ry, phi, large, sweep = num(), num(), num(), num(), num()
            p3 = (base[0] + num(), base[1] + num())
            cur += _arc(pos, rx, ry, phi, bool(large), bool(sweep), p3, tol); pos = p3; last_ctrl = None
        else:
            i += 1
    if cur: rings.append(cur)
    return [r for r in rings if len(r) >= 3]


def _svg_style(el, inherited):
    st = dict(inherited)
    for k in ('fill', 'stroke', 'display', 'visibility', 'fill-rule'):
        if el.get(k) is not None: st[k] = el.get(k).strip()
    for part in (el.get('style') or '').split(';'):
        if ':' in part:
            k, v = part.split(':', 1); st[k.strip()] = v.strip()
    return st


def svg_shapes(text, tol=0.02):
    """Every filled shape in an SVG as rings in its own user units (y down), with what was skipped.
    Curves and arcs are flattened to tol (in user units); transforms on groups and shapes applied."""
    try:
        root = ET.fromstring(text)
    except ET.ParseError as e:
        fail('invalid_svg', 'Not an SVG: %s' % e)
    tag = lambda e: e.tag.split('}')[-1]
    if tag(root) != 'svg':
        fail('invalid_svg', 'The root element is <%s>, not <svg>' % tag(root))
    shapes, skipped = [], []
    def visit(el, M, st):
        t = tag(el)
        if el.get('transform'):
            A = _svg_matrix(el.get('transform'))
            a1, b1, c1, d1, e1, f1 = M; a2, b2, c2, d2, e2, f2 = A
            M = (a1 * a2 + c1 * b2, b1 * a2 + d1 * b2, a1 * c2 + c1 * d2, b1 * c2 + d1 * d2, a1 * e2 + c1 * f2 + e1, b1 * e2 + d1 * f2 + f1)
        st = _svg_style(el, st)
        if st.get('display') == 'none' or st.get('visibility') == 'hidden' or t in ('defs', 'clipPath', 'mask', 'symbol', 'title', 'desc', 'metadata', 'style'):
            return
        if t in ('svg', 'g', 'a'):
            for c in el:
                visit(c, M, st)
            return
        f = lambda k: float(_svg_floats(el.get(k) or '0')[0]) if _svg_floats(el.get(k) or '0') else 0.0
        rings = []
        if t == 'path':
            rings = _svg_path_rings(el.get('d'), tol)
        elif t == 'rect':
            x, y, w, h = f('x'), f('y'), f('width'), f('height')
            rings = [[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]] if w > 0 and h > 0 else []
        elif t in ('circle', 'ellipse'):
            cx, cy = f('cx'), f('cy')
            rx = f('r') if t == 'circle' else f('rx'); ry = f('r') if t == 'circle' else f('ry')
            if rx > 0 and ry > 0:
                n = max(12, min(180, int(math.ceil(math.pi / max(1e-3, math.acos(max(-1.0, 1 - tol / max(rx, ry))))))))
                rings = [[(cx + rx * math.cos(2 * math.pi * k / n), cy + ry * math.sin(2 * math.pi * k / n)) for k in range(n)]]
        elif t in ('polygon', 'polyline'):
            v = _svg_floats(el.get('points'))
            pts = list(zip(v[0::2], v[1::2]))
            rings = [pts] if len(pts) >= 3 else []
        elif t in ('text', 'image', 'use', 'line', 'foreignObject'):
            skipped.append({'element': t, 'id': el.get('id'), 'why': {'text': 'text is not a shape: convert it to paths first', 'image': 'a bitmap is not a shape',
                            'use': 'a <use> reference is not followed: expand it first', 'line': 'a line has no fill'}.get(t, 'not a shape')})
            return
        else:
            return
        if st.get('fill', 'black') in ('none', 'transparent'):
            skipped.append({'element': t, 'id': el.get('id'), 'why': 'no fill (stroke-only artwork imports nothing)'})
            return
        if not rings:
            return
        shapes.append([[_svg_apply(M, p) for p in r] for r in rings])
    visit(root, (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), {})
    w_attr, h_attr = root.get('width'), root.get('height')
    def mm_of(v):
        m = re.fullmatch(r'\s*([-+0-9.eE]+)\s*(mm|cm|in|pt|pc|px)?\s*', v or '')
        if not m: return None
        return float(m[1]) * {'mm': 1, 'cm': 10, 'in': 25.4, 'pt': 25.4 / 72, 'pc': 25.4 / 6, 'px': 25.4 / 96, None: 25.4 / 96}[m[2]]
    vb = _svg_floats(root.get('viewBox'))
    unit = None
    if len(vb) == 4 and vb[2] > 0 and mm_of(w_attr):
        unit = mm_of(w_attr) / vb[2]
    elif mm_of(w_attr) and not vb:
        unit = 25.4 / 96 if not re.search(r'(mm|cm|in|pt|pc)', w_attr) else (mm_of(w_attr) / float(_svg_floats(w_attr)[0]))
    return shapes, skipped, unit or 25.4 / 96


def _ring_area(r):
    return sum(r[k][0] * r[(k + 1) % len(r)][1] - r[(k + 1) % len(r)][0] * r[k][1] for k in range(len(r))) / 2


def _point_in(p, r):
    x, y, inside = p[0], p[1], False
    for k in range(len(r)):
        (x1, y1), (x2, y2) = r[k], r[(k + 1) % len(r)]
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / ((y2 - y1) or 1e-30) + x1:
            inside = not inside
    return inside


def svg_rects(rings, pitch):
    """Filled rings (even-odd, so a ring inside another is a hole) as rectangles: one scan row per
    pitch, each row's spans cut where the row's centre line crosses the outlines, and a span that
    repeats on the next row grown into one taller rectangle. Fusion's POLYGON asks for a signal
    name (a modal, live 2026-10-27) and has no holes, so the artwork is drawn with RECT."""
    pts = [p for r in rings for p in r]
    if not pts:
        return []
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    edges = [(r[k], r[(k + 1) % len(r)]) for r in rings for k in range(len(r))]
    rows = max(1, int(math.ceil((y1 - y0) / pitch)))
    h = (y1 - y0) / rows
    out, open_ = [], {}
    for row in range(rows):
        yc = y0 + (row + 0.5) * h
        xs = sorted(a[0] + (yc - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) for a, b in edges if (a[1] > yc) != (b[1] > yc))
        spans = {(round(xs[k], 4), round(xs[k + 1], 4)) for k in range(0, len(xs) - 1, 2) if xs[k + 1] - xs[k] > 1e-6}
        for key in list(open_):
            if key not in spans:
                out.append((key[0], open_.pop(key), key[1], y0 + row * h))
        for key in spans:
            open_.setdefault(key, y0 + row * h)
    for key, ys in open_.items():
        out.append((key[0], ys, key[1], y1))
    return out


def import_svg(app, root, board, meta, args):
    _guard(meta, args)
    text = args.get('svg')
    if not text and args.get('svgPath'):
        sp = Path(args['svgPath'])
        if not sp.exists():
            fail('file_not_found', 'No SVG at %s on the Fusion machine' % args['svgPath'])
        text = sp.read_text(encoding='utf-8', errors='replace')
    if not text:
        fail('bad_args', 'svgPath or svg (the SVG text) is required')
    at = args.get('at')
    if not (isinstance(at, (list, tuple)) and len(at) == 2):
        fail('bad_args', 'at: [x, y] mm is required')
    lay = args.get('layer') or 'F.SilkS'
    n = SIDE_LAYERS[lay][0] if lay in SIDE_LAYERS else ({'F.Cu': 1, 'B.Cu': 16, 'F.Mask': 29, 'B.Mask': 30, 'F.Paste': 31, 'B.Paste': 32}.get(lay) or _layer_number(board, lay))
    if root.find("drawing/layers/layer[@number='%d']" % n) is None:
        fail('invalid_layer', 'This board has no layer %d' % n)
    if 1 <= n <= 16:
        # POLYGON on copper opens Fusion's "Signal Name" modal (live 2026-10-27); RECT on copper is
        # unproven, and a modal must never be risked, so copper artwork is refused, not attempted
        fail('not_supported', 'Artwork on copper is not imported: Fusion draws copper shapes as signal pours and asks for the name in a dialog. Import onto F.SilkS, B.SilkS, F.Mask, B.Mask or a documentation layer')
    bottom = n in (16, 22, 26, 30, 32, 52)
    mirror = args.get('mirror', bottom)
    tol_mm = float(args.get('toleranceMm') or 0.02)
    shapes0, skipped, unit = svg_shapes(text, 1.0)
    pts = [p for sh in shapes0 for r in sh for p in r]
    if not pts:
        fail('nothing_to_import', 'The SVG has no filled shape to import (skipped: %s)' % '; '.join('%s: %s' % (k['element'], k['why']) for k in skipped[:5]))
    x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    if args.get('widthMm'):
        k = float(args['widthMm']) / max(x1 - x0, 1e-9)
    else:
        k = unit * float(args.get('scale') or 1)
    shapes, _, _ = svg_shapes(text, tol_mm / k)
    w, h = (x1 - x0) * k, (y1 - y0) * k
    ax, ay = float(at[0]), float(at[1])
    if (args.get('anchor') or 'topleft') == 'center':
        ax, ay = ax - w / 2, ay + h / 2
    def to_board(p):
        u = (p[0] - x0) * k
        return (ax + (w - u if mirror else u), ay - (p[1] - y0) * k)
    pitch = max(tol_mm, 0.01)
    rects = []
    for sh in shapes:
        rects += [r for r in svg_rects([[to_board(p) for p in ring] for ring in sh], pitch) if r[2] - r[0] >= 0.005 and r[3] - r[1] >= 0.005]
    plan = {'shapes': len(shapes), 'rects': len(rects), 'pitchMm': pitch, 'sizeMm': [round(w, 3), round(h, 3)], 'layer': n, 'mirrored': bool(mirror), 'skipped': skipped,
            'boxMm': [round(ax, 3), round(ay - h, 3), round(ax + w, 3), round(ay, 3)]}
    if len(rects) > 4000:
        fail('too_detailed', 'The artwork needs %d rectangles at %.3f mm: give a larger toleranceMm or a smaller widthMm' % (len(rects), pitch))
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, **plan}
    before = {g['id'] for g in plain_graphics(board, Layers(board))}
    visible = _visible_layers(root)
    done = 0
    try:
        for b in range(0, len(rects), 60):
            chunk = rects[b:b + 60]
            app.executeTextCommand('Electron.run DISPLAY NONE %d; LAYER %d; %s' % (n, n, ' '.join('RECT R0 (%.4fmm %.4fmm) (%.4fmm %.4fmm);' % r for r in chunk)))
            done += len(chunk)
    finally:
        _restore_display(app, visible)
    _, after, meta2 = snapshot(app, args)
    added = [g for g in plain_graphics(after, Layers(after)) if g['id'] not in before and g['kind'] == 'rect']
    _, _, w0, v0 = objects(board)
    _, _, w1, v1 = objects(after)
    if len(added) != len(rects) or _elements_of(after) != _elements_of(board) or copper_signature(w0 + v0) != copper_signature(w1 + v1):
        for _ in range(done):
            app.executeTextCommand('Electron.run UNDO;')
        fail('readback_mismatch', 'The board read back did not show %d new rectangles on layer %d with nothing else changed (found %d); every RECT was undone' % (len(rects), n, len(added)))
    return {'success': True, 'mutated': True, **meta2, **plan, 'ids': [g['id'] for g in added], 'undoSteps': done,
            '_hint': 'Filled shapes became rectangles row by row (%.3f mm rows; rows that repeat merged), since Fusion\'s POLYGON asks for a signal name in a dialog. A shape inside another is a hole. Remove the artwork with fusion_remove_graphics {ids}. Text and images are listed in skipped (convert text to paths first).' % pitch}


# ── copy_routing ────────────────────────────────────────────────────────────────────────────────

def copy_routing_plan(board, args):
    """What copy_routing would draw: the tracks with both ends in the box and the through vias with
    their centre in it, moved by the offset, each on its mapped net. Nets map through paired parts
    (refMap, refPattern: each source pad's net goes to the partner pad's net), then netMap. A
    destination end that lands on another net's pad, via or track end is a conflict: Fusion would
    ask whether to join the two nets, so the copy is refused before anything is drawn."""
    import fnmatch
    box = args.get('box'); off = args.get('offset')
    if not (isinstance(box, (list, tuple)) and len(box) == 4) or not (isinstance(off, (list, tuple)) and len(off) == 2):
        fail('bad_args', 'box [x1, y1, x2, y2] and offset [dx, dy] (mm) are required')
    x1, y1, x2, y2 = min(box[0], box[2]), min(box[1], box[3]), max(box[0], box[2]), max(box[1], box[3])
    dx, dy = float(off[0]), float(off[1])
    L = Layers(board)
    want_layers = {_layer_number(board, l) for l in args.get('layers') or []} or None
    _, pads, wires, vias = objects(board)
    inside = lambda p: x1 - 1e-6 <= p[0] <= x2 + 1e-6 and y1 - 1e-6 <= p[1] <= y2 + 1e-6
    # parts paired by designator
    refs = sorted({k.split('.')[0] for k in pads})
    pairs = dict(args.get('refMap') or {})
    pat = args.get('refPattern') or {}
    if pat.get('from') and pat.get('to'):
        f_, t_ = pat['from'], pat['to']
        for r in refs:
            if fnmatch.fnmatchcase(r, f_):
                m = re.fullmatch(re.escape(f_).replace(r'\*', '(.*)').replace(r'\?', '(.)'), r)
                if m:
                    target = t_
                    for g in m.groups():
                        target = re.sub(r'[*?]', lambda _: g, target, count=1)
                    if target in refs:
                        pairs.setdefault(r, target)
    missing = [r for pair in pairs.items() for r in pair if r not in refs]
    if missing:
        fail('designator_not_found', 'No part %s on the board' % ', '.join(sorted(set(missing))[:5]))
    net_map, conflicts = {}, []
    for src, dst in pairs.items():
        for key, p_ in pads.items():
            ref, pin = key.split('.', 1)
            if ref != src or not p_['net']:
                continue
            q = pads.get(dst + '.' + pin)
            if q is None or not q['net']:
                continue
            if net_map.get(p_['net'], q['net']) != q['net']:
                conflicts.append({'net': p_['net'], 'to': sorted({net_map[p_['net']], q['net']})})
            net_map.setdefault(p_['net'], q['net'])
    for k, v in (args.get('netMap') or {}).items():
        net_map[k] = v
        conflicts = [c for c in conflicts if c['net'] != k]
    if conflicts:
        fail('net_map_conflict', 'A source net pairs to two target nets: %s. Settle it with netMap' % '; '.join('%s -> %s' % (c['net'], ' or '.join(c['to'])) for c in conflicts[:5]))
    nets_on_board = {s_.get('name') for s_ in board.findall('signals/signal')}
    keep = bool(args.get('keepUnmappedNets'))
    new_w, new_v, skipped = [], [], []
    def target_net(n):
        if n in net_map:
            return net_map[n]
        return n if keep else None
    for w in wires:
        if want_layers and w['layer'] not in want_layers:
            continue
        if not (inside(w['start']) and inside(w['end'])):
            continue
        tn = target_net(w['net'])
        if tn is None:
            skipped.append({'item': 'track %s %s' % (w['net'], L.name(w['layer'])), 'why': 'its net is not mapped (pair the parts, give netMap, or keepUnmappedNets)'})
            continue
        if tn not in nets_on_board:
            skipped.append({'item': 'track %s' % w['net'], 'why': 'target net %s is not on the board' % tn}); continue
        new_w.append(dict(w, net=tn, start=[w['start'][0] + dx, w['start'][1] + dy], end=[w['end'][0] + dx, w['end'][1] + dy], src=w['net']))
    for v in vias:
        if not inside(v['at']):
            continue
        if v['extent'] not in (None, '', '1-16'):
            skipped.append({'item': 'via %s at %s' % (v['net'], v['at']), 'why': 'blind or buried (%s): only through vias are copied' % v['extent']}); continue
        tn = target_net(v['net'])
        if tn is None:
            skipped.append({'item': 'via %s' % v['net'], 'why': 'its net is not mapped'}); continue
        if tn not in nets_on_board:
            skipped.append({'item': 'via %s' % v['net'], 'why': 'target net %s is not on the board' % tn}); continue
        new_v.append(dict(v, net=tn, at=[v['at'][0] + dx, v['at'][1] + dy], src=v['net']))
    # destination conflicts: an end on another net's copper would raise Fusion's join question
    def touches(pt, layer, net):
        for key, p_ in pads.items():
            if p_['net'] == net or (layer is not None and layer not in p_['layers'] and p_['layers'] != [1, 16]):
                continue
            if p_['poly']:
                if _point_in(pt, p_['poly']): return key
            elif math.hypot(pt[0] - p_['at'][0], pt[1] - p_['at'][1]) <= p_['radius']: return key
        for v in vias + new_v:
            if v['net'] != net and math.hypot(pt[0] - v['at'][0], pt[1] - v['at'][1]) <= (v['diameter'] or v['drill'] * 2) / 2:
                return 'via %s' % v['net']
        for w in wires + new_w:
            if w['net'] != net and (layer is None or w['layer'] == layer):
                for e in (w['start'], w['end']):
                    if math.hypot(pt[0] - e[0], pt[1] - e[1]) <= w['width'] / 2 + 1e-6:
                        return 'track %s' % w['net']
        return None
    clashes = []
    for w in new_w:
        for e in (w['start'], w['end']):
            hit = touches(e, w['layer'], w['net'])
            if hit: clashes.append({'item': 'track %s' % w['net'], 'at': [round(e[0], 4), round(e[1], 4)], 'touches': hit})
    for v in new_v:
        hit = touches(v['at'], None, v['net'])
        if hit: clashes.append({'item': 'via %s' % v['net'], 'at': [round(v['at'][0], 4), round(v['at'][1], 4)], 'touches': hit})
    used = sorted({(w['src'], w['net']) for w in new_w} | {(v['src'], v['net']) for v in new_v})
    return {'tracks': new_w, 'vias': new_v, 'skipped': skipped, 'clashes': clashes, 'pairs': pairs, 'netMap': {a: b for a, b in used}}


def copy_routing(app, root, board, meta, args):
    _guard(meta, args)
    plan = copy_routing_plan(board, args)
    L = Layers(board)
    summary = {'copied': {'tracks': len(plan['tracks']), 'vias': len(plan['vias'])}, 'netMap': plan['netMap'], 'pairs': plan['pairs'],
               'skipped': plan['skipped'], 'clashes': plan['clashes']}
    if plan['clashes']:
        fail('destination_conflict', 'Copied copper would land on another net (%s): place the target parts at the same offset first, or fix the pairing. Nothing was drawn'
             % '; '.join('%s at %s touches %s' % (c['item'], c['at'], c['touches']) for c in plan['clashes'][:4]))
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, **summary}
    if not plan['tracks'] and not plan['vias']:
        return {'success': True, 'mutated': False, **meta, **summary, 'note': 'nothing to copy in the box'}
    validate = args.get('validate', True)
    drc0 = native_drc(app) if validate else None
    _, _, w0, v0 = objects(board)
    cmds = []
    for w in plan['tracks']:
        c = w['curve']
        cmds.append("LAYER %d; WIRE '%s' %.6fmm (%.6fmm %.6fmm) %s(%.6fmm %.6fmm);" % (w['layer'], identifier(w['net']), w['width'], w['start'][0], w['start'][1], ('%+g ' % c) if c else '', w['end'][0], w['end'][1]))
    for v in plan['vias']:
        cmds.append("CHANGE DRILL %.6fmm; VIA '%s' %s1-16 (%.6fmm %.6fmm);" % (v['drill'], identifier(v['net']), ('%.6fmm ' % v['diameter']) if v['diameter'] else '', v['at'][0], v['at'][1]))
    done = 0
    for k in range(0, len(cmds), 25):
        chunk = cmds[k:k + 25]
        # SET CONFIRM NO on the same line: a join question, if one is ever asked, is answered No
        app.executeTextCommand('Electron.run SET CONFIRM NO; ' + ' '.join(chunk) + ' SET CONFIRM OFF;')
        done += len(chunk)
    _, after, meta2 = snapshot(app, args)
    _, _, w1, v1 = objects(after)
    expect = copper_signature(w0 + v0 + [dict(w) for w in plan['tracks']] + [dict(v) for v in plan['vias']])
    def undo_all(why):
        for _ in range(done):
            app.executeTextCommand('Electron.run UNDO;')
        fail(why[0], why[1])
    if copper_signature(w1 + v1) != expect or _elements_of(after) != _elements_of(board):
        undo_all(('readback_mismatch', 'The copper read back is not the old copper plus the copy; every command was undone'))
    drc = {'ran': False}
    if validate:
        drc1 = native_drc(app)
        new_errors = drc1['errorCount'] - drc0['errorCount']
        drc = {'ran': True, 'newErrors': max(0, new_errors), 'before': drc0['errorCount'], 'after': drc1['errorCount']}
        if new_errors > 0:
            undo_all(('drc_failed', 'The copy adds %d DRC error(s) (%s); every command was undone. Dry-run and check the target placement, or pass validate:false' % (
                new_errors, '; '.join(list(drc1['summary'])[:3]))))
    return {'success': True, 'mutated': True, **meta2, **summary, 'drc': drc,
            '_hint': 'Tracks and through vias copied by the offset on their mapped nets; read back exactly. Run fusion_routing_state to see what is still unrouted.'}


# ── set_layer_count ─────────────────────────────────────────────────────────────────────────────

LAYER_SETS = {2: [1, 16], 4: [1, 2, 15, 16], 6: [1, 2, 3, 14, 15, 16], 8: [1, 2, 3, 4, 13, 14, 15, 16]}
LAYER_SETUP = {2: '(1*16)', 4: '(1+2*15+16)', 6: '(1+2*3+14*15+16)', 8: '(1+2*3+4*13+14*15+16)'}


def layers_used(board, numbers):
    """Copper and graphics per layer for the layers named: tracks, pours, pads, plain items."""
    used = {}
    for n in numbers:
        c = 0
        for s_ in board.findall('signals/signal'):
            c += sum(1 for w in s_.findall('wire') if w.get('layer') == str(n))
            c += sum(1 for p_ in list(s_.findall('polygon')) + list(s_.findall('polygonpour')) if p_.get('layer') == str(n))
        c += sum(1 for g in board.find('plain') if g.get('layer') == str(n)) if board.find('plain') is not None else 0
        for pkg in board.findall('libraries/library/packages/package'):
            c += sum(1 for g in pkg.findall('smd') if g.get('layer') == str(n))
        if c:
            used[n] = c
    return used


def stackup_for(count, total_mm, copper_mm, template):
    """A Fusion layerstackup for the count: copper keeps each layer's thickness where it had one
    (outer 0.035, inner 0.0175 mm otherwise); prepreg 0.2 mm next to the outer layers and between
    core pairs, the cores sharing what is left of the board thickness. Fusion numbers the cores
    Dielectric-1.. first, then the prepregs, top down."""
    import uuid
    sig = LAYER_SETS[count]
    names = {1: 'Top', 16: 'Bottom'}
    cu = [copper_mm.get(n, 0.035 if n in (1, 16) else 0.0175) for n in sig]
    kinds = ['Core'] if count == 2 else ['Prepreg' if i % 2 == 0 else 'Core' for i in range(count - 1)]
    n_pre, n_core = kinds.count('Prepreg'), kinds.count('Core')
    core = (total_mm - sum(cu) - 0.2 * n_pre) / n_core
    if core < 0.1:
        fail('stackup_too_thin', 'A %d-layer board %.3f mm thick leaves %.3f mm per core: give the board more thickness first' % (count, total_mm, core))
    root = ET.Element('layerstackup', dict(template.attrib) if template is not None else {'name': 'Default Layer Stack', 'type': 'Standard Stackup', 'display_unit': 'mic', 'roughness_model': '0'})
    gid = lambda: '{%s}' % uuid.uuid4()
    core_no, pre_no = 0, n_core
    for i, n in enumerate(sig):
        d = ET.SubElement(root, 'layerdef', {'type': 'Signal', 'id': gid(), 'layer': str(n), 'name': names.get(n, 'Route%d' % n), 'locally_modified_material': 'yes'})
        ET.SubElement(d, 'material', {'name': 'User Defined', 'thickness': '%gmm' % cu[i], 'weight': '1oz', 'process': 'Electro Deposited', 'source': 'User'})
        if i < len(kinds):
            k = kinds[i]
            if k == 'Core':
                core_no += 1; num = core_no; th = core
                mat = {'name': 'User Defined', 'construction': '', 'thickness': '%.4fmm' % th}
            else:
                pre_no += 1; num = pre_no; th = 0.2
                mat = {'name': 'User Defined', 'glass_style': '', 'thickness': '0.2mm'}
            mat.update({'resin': '50%', 'dielectric_constant_1g': '4.5', 'dissipation_factor_1g': '0.02', 'glass_transition_temperature': '125degC', 'source': 'User'})
            d = ET.SubElement(root, 'layerdef', {'type': k, 'id': gid(), 'layer': str(num), 'name': 'Dielectric-%d' % num, 'locally_modified_material': 'yes'})
            ET.SubElement(d, 'material', mat)
    # the shape Fusion writes itself. Without startlayer and stoplayer, Fusion raises LayerId::IsSignalLayer
    # on load and a VIA with a layer range then lands nothing (a VIA without a range still lands)
    ET.SubElement(root, 'viadef', {'name': 'Thru 1:16', 'startlayer': '1', 'stoplayer': '16', 'complement': 'no'})
    return root


def stack_has_broken_viadef(board):
    """True for a stack written by fusion_set_layer_count before 1.13.61. Its via definition had no start or stop
    layer, and Fusion drops such a definition on load, so what is left is a stack with no through via at all
    (live 2026-11-03). A via with a layer range lands nothing on it."""
    for s_ in board.findall('.//layerstackup'):
        defs = s_.findall('viadef')
        if not defs or any(not (v.get('startlayer') and v.get('stoplayer')) for v in defs):
            return True
    return False


def _invalid_via_nets(board):
    """Nets with a via drawn while the stack had no through via: Fusion writes its extent as 'invalid', and
    it stays that way after the stack is repaired."""
    return sorted(s_.get('name') for s_ in board.findall('signals/signal') if any(not re.fullmatch(r'[0-9]+-[0-9]+', v.get('extent') or '1-16') for v in s_.findall('via')))


def _reland_invalid_via_nets(app, args, board):
    """Land those nets again, as nothing but RIPUP by name reaches such a via (live: CHANGE VIA, CHANGE EXTENT,
    MOVE, DELETE and RIPUP at a point all leave it). Same tracks and vias, each via now through. Read back; a
    net that doesn't come back as it was is undone."""
    nets = _invalid_via_nets(board)
    _, _, w0, v0 = objects(board)
    before = copper_signature(w0 + v0)
    want = copper_signature(w0 + [dict(v, extent='1-16') if v['net'] in nets else v for v in v0])
    cmds = []
    for n_ in nets:
        s_ = board.find("signals/signal[@name='%s']" % identifier(n_)); n = identifier(n_)
        cmds.append("RIPUP '%s';" % n)
        for wv in s_.findall('wire'):
            if wv.get('layer') == '19':
                continue
            c = _f(wv, 'curve')
            cmds.append("SET WIRE_BEND 2; LAYER %s; WIRE '%s' %.6fmm (%.6fmm %.6fmm) %s(%.6fmm %.6fmm);" % (wv.get('layer'), n, _f(wv, 'width'), _f(wv, 'x1'), _f(wv, 'y1'), ('%+g ' % c) if c else '', _f(wv, 'x2'), _f(wv, 'y2')))
        for v in s_.findall('via'):
            shape = {'square': 'SQUARE', 'octagon': 'OCTAGON'}.get(v.get('shape', 'round'), 'ROUND')
            dia = _f(v, 'diameter') or 0
            cmds.append("CHANGE DRILL %.6fmm; VIA '%s' %s%s 1-16 (%.6fmm %.6fmm);" % (_f(v, 'drill'), n, ('%.6fmm ' % dia) if dia else '', shape, _f(v, 'x'), _f(v, 'y')))
    for c in cmds:
        app.executeTextCommand('Electron.run ' + c)
    # RIPUP by name also hides the net's pour fills, and RATSNEST will not refill a hidden pour
    app.executeTextCommand('Commands.Start Electron::PolyShowFillAll')
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    _, _, w1, v1 = objects(after)
    if copper_signature(w1 + v1) != want:
        undone, _, _ = undo_route(app, args, cmds, before)
        fail('readback_mismatch', 'Landing %s again did not bring the same copper back; %s' % (', '.join(nets), 'it was undone' if undone else 'the undo did not restore the copper exactly: read fusion_routing_state'))
    return nets, after, meta2


def set_layer_count(app, root, board, meta, args):
    _guard(meta, args)
    try:
        count = int(args.get('count'))
    except (TypeError, ValueError):
        count = 0
    if count not in LAYER_SETS:
        fail('bad_args', 'count is 2, 4, 6 or 8')
    L = Layers(board)
    now = list(L.order)
    want = LAYER_SETS[count]
    stack = layer_stack(board)
    plan = {'from': [L.name(n) for n in now], 'to': ['F.Cu'] + ['In%d.Cu' % i for i in range(1, count - 1)] + ['B.Cu'], 'layerSetup': LAYER_SETUP[count],
            'thicknessMm': round((stack.get('totalUm') or 1600) / 1000.0, 4)}
    broken, bad_nets = now == want and stack_has_broken_viadef(board), _invalid_via_nets(board)
    if now == want and not broken and not bad_nets:
        return {'success': True, 'mutated': False, **meta, **plan, 'note': 'the board already has %d copper layers' % count}
    if broken:
        plan['repairedViaDefinition'] = True
    if bad_nets:
        plan['netsLandedAgain'] = bad_nets
    gone = [n for n in now if n not in want]
    used = layers_used(board, gone)
    if used:
        fail('layers_in_use', 'Layers to be removed still carry copper or graphics: %s. Move or delete it first' % ', '.join('%s (%d)' % (L.name(n), c) for n, c in used.items()))
    copper_mm = {}
    for row in stack.get('layers') or []:
        n = row.get('layerId')
        if n in want and row.get('copperUm'):
            copper_mm[n] = row['copperUm'] / 1000.0
    total = (stack.get('totalUm') or 1600.0) / 1000.0
    new_stack = stackup_for(count, total, copper_mm, board.find('.//layerstackup'))
    plan['stackup'] = [{'type': d.get('type'), 'name': d.get('name'), 'thickness': d.find('material').get('thickness')} for d in new_stack.findall('layerdef')]
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, **plan}
    after, meta2 = board, meta
    got = now
    st2 = stack
    with tempfile.TemporaryDirectory(prefix='adom-layers-') as d:
        orig, new = Path(d) / 'before.edru', Path(d) / 'after.edru'
        if now == want and not broken:
            return _layer_count_reply(app, args, after, meta2, plan, got, st2, bad_nets)
        app.executeTextCommand("Electron.run DRC SAVE '%s';" % orig.as_posix())
        if not orig.exists():
            fail('rules_unreadable', 'Fusion did not save the board\'s design rules, so the layer count was not changed')
        tree = ET.parse(str(orig))
        rules = tree.getroot().find('designrules') if tree.getroot().tag != 'designrules' else tree.getroot()
        for p_ in rules.findall('param'):
            if p_.get('name') == 'layerSetup':
                p_.set('value', LAYER_SETUP[count])
        old = rules.find('layerstackup')
        if old is not None:
            rules.remove(old)
        rules.append(new_stack)
        tree.write(str(new), encoding='utf-8', xml_declaration=True)
        app.executeTextCommand("Electron.run SET CONFIRM NO; DRC LOAD '%s'; SET CONFIRM OFF;" % new.as_posix())
        _, after, meta2 = snapshot(app, args)
        got = list(Layers(after).order)
        st2 = layer_stack(after)
        same_parts = _elements_of(after) == _elements_of(board)
        if got != want or not same_parts or stack_has_broken_viadef(after):
            app.executeTextCommand("Electron.run SET CONFIRM NO; DRC LOAD '%s'; SET CONFIRM OFF;" % orig.as_posix())
            fail('readback_mismatch', 'Fusion read back copper layers %s (asked %s); the board\'s own rules were loaded back' % ([Layers(after).name(n) for n in got], plan['to']))
    return _layer_count_reply(app, args, after, meta2, plan, got, st2, bad_nets)


def _layer_count_reply(app, args, after, meta2, plan, got, st2, bad_nets):
    if bad_nets:
        _, after, meta2 = _reland_invalid_via_nets(app, args, after)
    return {'success': True, 'mutated': True, **meta2, **plan, 'layers': [Layers(after).name(n) for n in got], 'stackupRead': st2,
            '_hint': 'New inner layers are signal layers with generic prepreg and core (Dk 4.5); the board thickness is kept. Copper stays on its Fusion layer, so the standard names move with the count: Route15 is In2.Cu on 4 layers and In4.Cu on 6. Re-check an impedance-controlled stack with fusion_layer_stack. Net classes and other design rules are unchanged.'}


def remove_parts(app, root, board, meta, args):
    """Parts are removed in the design's schematic: Fusion keeps the board in step with it, and a
    DELETE of a part on the board does nothing (the board follows the schematic)."""
    _guard(meta, args)
    refs = args.get('refs') or []
    if not refs:
        fail('bad_args', 'refs is required')
    nets, failed, todo = objects(board)[0], [], []
    for r_ in refs:
        el = _element(board, r_)
        if el is None:
            failed.append({'ref': r_, 'error': 'no part %s on the board' % r_})
        elif el.get('locked') == 'yes' and not args.get('allowLocked'):
            failed.append({'ref': r_, 'error': 'part_locked (allowLocked:true to remove it)'})
        else:
            todo.append(r_)
    plan = [{'ref': r_, 'nets': sorted({n for n, s_ in nets.items() for c in s_.findall('contactref') if c.get('element') == r_})} for r_ in todo]
    if args.get('dryRun'):
        return {'success': True, 'mutated': False, **meta, 'removed': plan, 'failed': failed}
    if not todo:
        return {'success': False, 'errorCode': 'nothing_to_do', 'error': 'No part could be removed', 'failed': failed, **meta}
    bdoc = app.activeDocument
    i, sdoc = _schematic_of(app, bdoc)
    if sdoc is None:
        fail('no_schematic', 'The board has no schematic open beside it: open the design (its project document)')
    j = next(k for k in range(app.documents.count) if app.documents.item(k) == bdoc)
    try:
        for r_ in todo:
            el = _element(board, r_)
            if el.get('locked') == 'yes':
                app.executeTextCommand('Electron.run LOCK (S%.6fmm %.6fmm);' % (float(el.get('x')), float(el.get('y'))))
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
        for r_ in todo:
            app.executeTextCommand("Electron.run SET CONFIRM NO; DELETE '%s'; SET CONFIRM OFF;" % identifier(r_))
    finally:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, j))
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    still = [r_ for r_ in todo if _element(after, r_) is not None]
    gone_too = sorted(set(_elements_of(board)) - set(_elements_of(after)) - set(todo))
    for r_ in still:
        failed.append({'ref': r_, 'error': 'still on the board after the delete in the schematic'})
    if gone_too:
        failed.append({'ref': None, 'error': 'other parts went too: %s' % gone_too})
    return {'success': not failed, 'mutated': True, **meta2, 'removed': [p_ for p_ in plan if p_['ref'] not in still], 'failed': failed,
            '_hint': 'Removed from the design (schematic and board). Tracks and vias that met their pads stay: fusion_routing_state shows what is left dangling.'}


def _mm_value(v):
    m = re.fullmatch(r'\s*([0-9.]+)\s*(mm|mil|in)?\s*', str(v or ''))
    if not m:
        return None
    x = float(m[1])
    return x * {'mil': 0.0254, 'in': 25.4}.get(m[2], 1.0)


def check_inputs(app, root, board, meta, args):
    """Everything the shared checks engine reads, from one export: the board model, the stack-up,
    Fusion's pour fills, the rules (copper clearances also in the standard's {kind: clearance,
    clearanceMm}, same-net spacing left out: it does not separate two nets) and the silkscreen."""
    rules = rd_read_rules(root, board, {})
    std = []
    for r_ in rules.get('rules', []):
        if r_.get('kind') == 'Copper Clearance' and r_.get('enabled', True) and 'same' not in (r_.get('name') or '').lower():
            v = _mm_value(r_.get('value'))
            if v is not None:
                std.append({'kind': 'clearance', 'clearanceMm': round(v, 4), 'name': r_.get('name'), 'enabled': True})
    dia, drill = _via_rule(board)
    std.append({'kind': 'via', 'sizeMm': round(dia, 4), 'drillMm': round(drill, 4), 'name': 'via (drill and restring rules)', 'enabled': True})
    # the board's minimum track width and drill (EAGLE's msWidth, msDrill) in the standard's form: a
    # minimum only, so a planner's default width is not taken from it
    std.append({'kind': 'width', 'minMm': round(_rule_len(board, 'msWidth', 0.2), 4), 'name': 'minimum width (msWidth)', 'enabled': True})
    std.append({'kind': 'hole size', 'minMm': round(_rule_len(board, 'msDrill', 0.3), 4), 'name': 'minimum drill (msDrill)', 'enabled': True})
    return {'success': True, 'mutated': False, **meta, 'board': state(board, meta), 'stack': layer_stack(board),
            # only the copper clearances: Fusion's other rule kinds also say "Clearance" (Dimension,
            # Solder-Mask, Restrict, Keepout) and the engine takes any such kind's value
            'pours': {'zones': zones(board)}, 'rules': {'rules': std},
            # what route_net's own preflight holds a route to: the strictest copper clearance and the
            # copper-to-edge rule, so a plan the engine finds is one route_net accepts
            'routeLimits': {'clearanceMm': max([r_['clearanceMm'] for r_ in std if r_['kind'] == 'clearance'] or [0.2]),
                            'edgeClearanceMm': round(_rule_len(board, 'mdCopperDimension', 0.5), 4)},
            'silk': rd_silk_state(root, board, {})}


def _cam_job(layers):
    """The add-in's CAM job for the board's layer count (its JLCPCB jobs carry an ODB++ output)."""
    import os
    name = 'jlcpcb-%dlayers.cam' % (2 if layers <= 2 else 4)
    for base in (r'%APPDATA%\Autodesk\FusionAddins\AdomBridge', r'%APPDATA%\Autodesk\Autodesk Fusion 360\API\AddIns\AdomBridge',
                 r'%APPDATA%\Autodesk\Autodesk Fusion\API\AddIns\AdomBridge'):
        p_ = os.path.join(os.path.expandvars(base), 'resources', 'cam', name)
        if os.path.exists(p_):
            return p_.replace('\\', '/')
    fail('no_cam_job', 'The add-in\'s CAM job %s is not installed (fusion_addin_status / reinstall the add-in)' % name)


def export_odb(app, root, board, meta, args):
    """ODB++ through Fusion's own exporter (Electron.mfgodb with the add-in's CAM job). Fusion writes a
    complete ODB++ tree (matrix, steps/pcb with each layer's features, eda/data, components) and then
    reports 'ODB++ output directory does not exist or is empty' from a later step (live 2026-10-27):
    the tree is found by its matrix file, checked, and packed as asked."""
    import os, shutil, tarfile
    layers = len(Layers(board).order) or 2
    cam = _cam_job(layers)
    work = Path(tempfile.mkdtemp(prefix='adom-odb-'))
    note = None
    try:
        app.executeTextCommand('Electron.mfgodb %s %s' % (work.as_posix(), cam))
    except Exception as e:
        note = str(e)
    trees = [Path(d) for d, _, fs in os.walk(work) if d.endswith('.odb') and (Path(d) / 'matrix' / 'matrix').exists()]
    if not trees:
        shutil.rmtree(work, ignore_errors=True)
        fail('export_failed', 'Fusion wrote no ODB++ tree%s' % ((': ' + note) if note else ''))
    tree = trees[0]
    matrix = (tree / 'matrix' / 'matrix').read_text(errors='replace')
    names = re.findall(r'NAME=(\S+)', matrix)
    n_files = sum(len(fs) for _, _, fs in os.walk(tree))
    eda = (tree / 'steps' / 'pcb' / 'eda' / 'data')
    nets = len(re.findall(r'^NET ', eda.read_text(errors='replace'), re.M)) if eda.exists() else 0
    out_dir = Path(args.get('outputDir') or (Path(tempfile.gettempdir()) / 'adom-odb'))
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', meta.get('documentName') or 'board')
    comp = (args.get('compression') or 'zip').lower()
    if comp == 'zip':
        dest = out_dir / ('%s.odb.zip' % stem)
        if dest.exists(): dest.unlink()
        with zipfile.ZipFile(dest, 'w', zipfile.ZIP_DEFLATED) as z:
            for d, _, fs in os.walk(tree):
                for f in fs:
                    full = Path(d) / f
                    z.write(full, (Path(stem + '.odb') / full.relative_to(tree)).as_posix())
    elif comp == 'tgz':
        dest = out_dir / ('%s.odb.tgz' % stem)
        with tarfile.open(dest, 'w:gz') as t:
            t.add(tree, arcname=stem + '.odb')
    elif comp == 'none':
        dest = out_dir / ('%s.odb' % stem)
        if dest.exists(): shutil.rmtree(dest)
        shutil.copytree(tree, dest)
    else:
        shutil.rmtree(work, ignore_errors=True)
        fail('bad_args', 'compression is zip, tgz or none')
    shutil.rmtree(work, ignore_errors=True)
    return {'success': True, 'mutated': False, **meta, 'path': dest.as_posix(), 'format': 'ODB++', 'compression': comp,
            'layers': names, 'files': n_files, 'nets': nets, 'camJob': cam.rsplit('/', 1)[-1],
            '_hint': 'Fusion\'s own ODB++ (version from its CAM job). The matrix lists %d layers; eda/data carries the netlist (%d nets) and the component layers the placement.' % (len(names), nets)}


def ipc356_lines(model, job):
    """IPC-D-356A in its fixed 80-column form, metric (UNITS CUST 1: 0.001 mm): a 317 record per
    drilled pad and via, a 327 record per SMD pad, net names over 14 characters as NNAME aliases."""
    u = lambda v: int(round(float(v) * 1000))
    lines = ['C  IPC-D-356A netlist from Fusion (fusion-bridge)', 'P  JOB   %s' % ascii_text(job)[:60], 'P  UNITS CUST 1', 'P  DIM   N']
    alias, n_alias = {}, 0
    def net_field(n):
        nonlocal n_alias
        n = ascii_text(n or 'N/C')
        if len(n) <= 14:
            return n
        if n not in alias:
            n_alias += 1
            alias[n] = 'NNAME%d' % n_alias
            lines.append('P  %s %s' % (alias[n], n))
        return alias[n]
    def rec(code, net, ref, pin, drill, plated, access, x, y, sx, sy, rot, mask):
        r = [' '] * 80
        def put(col, txt):
            for k, ch in enumerate(txt):
                if col - 1 + k < 80:
                    r[col - 1 + k] = ch
        put(1, code)
        put(4, net_field(net).ljust(14)[:14])
        put(21, ref.ljust(6)[:6])
        put(27, '-')
        put(28, str(pin).ljust(4)[:4])
        if drill:
            put(33, 'D%04d' % min(u(drill), 9999))
            put(38, 'P' if plated else 'U')
        put(39, 'A%02d' % access)
        put(42, 'X%s%06d' % ('-' if x < 0 else '+', min(abs(u(x)), 999999)))
        put(50, 'Y%s%06d' % ('-' if y < 0 else '+', min(abs(u(y)), 999999)))
        put(58, 'X%04d' % min(u(sx), 9999))
        put(63, 'Y%04d' % min(u(sy), 9999))
        put(68, 'R%03d' % (int(round(rot or 0)) % 360))
        put(73, 'S%d' % mask)
        return ''.join(r).rstrip()
    body = []
    for p_ in model['pads']:
        drilled = bool(p_.get('drill'))
        side = 0 if drilled else (1 if p_.get('layer') == 'F.Cu' else 2)
        body.append(rec('317' if drilled else '327', p_.get('net'), p_.get('ref') or '', p_.get('name') or '', p_.get('drill') or 0, True,
                        side, p_['x'], p_['y'], p_.get('sizeX') or 0, p_.get('sizeY') or 0, p_.get('rotation') or 0, 0 if drilled else side))
    for v in model['vias']:
        body.append(rec('317', v.get('net'), 'VIA', '', v.get('drill') or 0, True, 0, v['position']['x'], v['position']['y'], v.get('size') or 0, v.get('size') or 0, 0, 3))
    return lines + body + ['999']


def export_ipc356(app, root, board, meta, args):
    model, _ = board_model(board, meta)
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', meta.get('documentName') or 'board')
    out_dir = Path(args.get('outputDir') or (Path(tempfile.gettempdir()) / 'adom-ipc356'))
    out_dir.mkdir(parents=True, exist_ok=True)
    lines = ipc356_lines(model, stem)
    path = out_dir / ('%s.ipc' % stem)
    path.write_text('\r\n'.join(lines) + '\r\n', encoding='ascii')
    n317 = sum(1 for l in lines if l.startswith('317')); n327 = sum(1 for l in lines if l.startswith('327'))
    return {'success': True, 'mutated': False, **meta, 'path': path.as_posix(), 'format': 'IPC-D-356A', 'units': 'mm (CUST 1)',
            'records': {'throughHole': n317, 'smd': n327}, 'nets': len({p_.get('net') for p_ in model['pads'] if p_.get('net')}),
            '_hint': 'One record per pad and via: 317 drilled (vias as VIA), 327 SMD, with net, position, size and access side; net names over 14 characters are NNAME aliases. For flying-probe or bed-of-nails test.'}


def ipc2581_xml(board, meta, model=None, stack=None, version=''):
    """IPC-2581 revision C (mm) for assembly and test: the layers and stack-up, the outline, one
    package per footprint with its pins, the components and BOM, the nets with their pins, and on
    each copper layer the pads (with their pin), traces, arcs and vias, plus the drills. Pours are
    left out: Fusion's ULP cannot read a computed pour (S.polygons fails on a polygonpour, live
    2026-10-27), and a pour drawn as its outline would be wrong copper."""
    NS = 'http://webstds.ipc.org/2581'
    E = ET.Element
    S = ET.SubElement
    f = lambda v: ('%.4f' % float(v)).rstrip('0').rstrip('.') or '0'
    name = re.sub(r'[^A-Za-z0-9_.-]+', '_', ascii_text(meta.get('documentName') or 'board')) or 'board'
    model = model or board_model(board, meta)[0]
    stack = stack or layer_stack(board)
    copper = list(model['copperLayers'])
    lname = {}
    for i, c in enumerate(copper):
        lname[c] = 'TOP' if i == 0 else ('BOTTOM' if i == len(copper) - 1 else 'INNER%d' % i)
    root = E('IPC-2581', {'revision': 'C', 'xmlns': NS})
    content = S(root, 'Content', roleRef='Owner')
    S(content, 'FunctionMode', mode='ASSEMBLY')
    S(content, 'StepRef', name=name)
    layers = [(lname[c], 'SIGNAL', 'TOP' if i == 0 else ('BOTTOM' if i == len(copper) - 1 else 'INTERNAL')) for i, c in enumerate(copper)]
    layers += [('DRILL_TOP_BOTTOM', 'DRILL', 'ALL')]
    for ln, _, _ in layers:
        S(content, 'LayerRef', name=ln)
    S(content, 'BomRef', name=name + '_BOM')
    dict_std = S(content, 'DictionaryStandard', units='MILLIMETER')
    prims = {}

    def prim(shape, w, h=None):
        if shape == 'round' or h is None:
            key = ('C', round(float(w), 4))
        else:
            key = ('R', round(float(w), 4), round(float(h), 4))
        if key not in prims:
            pid = ('CIRCLE_%s' % f(key[1]) if key[0] == 'C' else 'RECT_%sx%s' % (f(key[1]), f(key[2]))).replace('.', '_')
            prims[key] = pid
            ent = S(dict_std, 'EntryStandard', id=pid)
            if key[0] == 'C':
                S(ent, 'Circle', diameter=f(key[1]))
            else:
                S(ent, 'RectCenter', width=f(key[1]), height=f(key[2]))
        return prims[key]

    lh = S(root, 'LogisticHeader')
    S(lh, 'Role', id='Owner', roleFunction='SENDER')
    S(lh, 'Enterprise', id='Adom', code='NONE')
    S(lh, 'Person', name='fusion-bridge', enterpriseRef='Adom', roleRef='Owner')
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()
    hr = S(root, 'HistoryRecord', number='1', origination=now, software='fusion-bridge', lastChange=now)
    fr = S(hr, 'FileRevision', fileRevisionId='1', comment='from Autodesk Fusion')
    sw = S(fr, 'SoftwarePackage', name='fusion-bridge', revision=version or '1', vendor='Adom')
    S(sw, 'Certification', certificationStatus='SELFTEST')

    comps = model['components']
    bom = S(root, 'Bom', name=name + '_BOM')
    bh = S(bom, 'BomHeader', assembly=name, revision='1')
    S(bh, 'StepRef', name=name)
    groups = {}
    for c in comps:
        groups.setdefault((c.get('pattern') or '', c.get('value') or ''), []).append(c)
    for (pat, val), cs in sorted(groups.items()):
        bi = S(bom, 'BomItem', OEMDesignNumberRef=ascii_text('%s_%s' % (pat, val)).strip('_') or 'PART', quantity=str(len(cs)), category='ELECTRICAL',
               description=ascii_text(val))
        for c in cs:
            S(bi, 'RefDes', name=ascii_text(c['ref']), packageRef=ascii_text(pat), populate='true', layerRef='TOP' if c.get('side') != 'bottom' else 'BOTTOM')
        ch = S(bi, 'Characteristics', category='ELECTRICAL')
        if val:
            S(ch, 'Textual', definitionSource='fusion', textualCharacteristicName='Value', textualCharacteristicValue=ascii_text(val))

    ecad = S(root, 'Ecad', name=name)
    S(ecad, 'CadHeader', units='MILLIMETER')
    cad = S(ecad, 'CadData')
    for ln, fn, side in layers:
        S(cad, 'Layer', name=ln, layerFunction=fn, side=side, polarity='POSITIVE')
    total = (stack.get('totalUm') or 1600.0) / 1000.0
    st = S(cad, 'Stackup', name='PRIMARY', overallThickness=f(total), tolPlus='0', tolMinus='0', whereMeasured='METAL', stackupStatus='PROPOSED')
    sg = S(st, 'StackupGroup', name='PRIMARY', thickness=f(total), tolPlus='0', tolMinus='0')
    seq = 0
    by_name = {l['name']: l for l in stack.get('layers') or []}
    for i, c in enumerate(copper):
        info = by_name.get(c, {})
        seq += 1
        S(sg, 'StackupLayer', layerOrGroupRef=lname[c], thickness=f((info.get('copperUm') or 35.0) / 1000.0), tolPlus='0', tolMinus='0', sequence=str(seq))
    step = S(cad, 'Step', name=name)

    vstacks = {}
    for v in model['vias']:
        key = (round(v.get('size') or 0, 4), round(v.get('drill') or 0, 4))
        if key not in vstacks:
            vn = ('VIA_%s_%s' % (f(key[0]), f(key[1]))).replace('.', '_')
            vstacks[key] = vn
            ps = S(step, 'PadStackDef', name=vn)
            S(ps, 'PadstackHoleDef', name='H' + vn, diameter=f(key[1]), platingStatus='VIA', plusTol='0', minusTol='0', x='0', y='0')
            for c in copper:
                pd = S(ps, 'PadstackPadDef', layerRef=lname[c], padUse='REGULAR')
                S(pd, 'Location', x='0', y='0')
                S(pd, 'StandardPrimitiveRef', id=prim('round', key[0]))
    S(step, 'Datum', x='0', y='0')
    prof = S(step, 'Profile')

    def poly(parent, edges, tag='Polygon'):
        pg = S(parent, tag)
        if not edges:
            return pg
        S(pg, 'PolyBegin', x=f(edges[0]['start'][0]), y=f(edges[0]['start'][1]))
        for e in edges:
            if e['kind'] == 'arc':
                cw = e.get('clockwise')
                if cw is None:
                    (sx, sy), (ex, ey), (cx, cy) = e['start'], e['end'], e['center']
                    cw = ((sx - cx) * (ey - cy) - (sy - cy) * (ex - cx)) < 0
                S(pg, 'PolyStepCurve', x=f(e['end'][0]), y=f(e['end'][1]), centerX=f(e['center'][0]), centerY=f(e['center'][1]), clockwise='true' if cw else 'false')
            else:
                S(pg, 'PolyStepSegment', x=f(e['end'][0]), y=f(e['end'][1]))
        return pg

    poly(prof, list(model['outlineEdges']))
    for cut in model['outlineCutouts']:
        poly(prof, cut['edges'], 'Cutout')

    pk = {(l.get('name'), p_.get('name')): p_ for l in board.findall('libraries/library') for p_ in l.findall('packages/package')}
    el_key = {e.get('name'): (e.get('library'), e.get('package')) for e in board.findall('elements/element')}
    made = {}
    for c in comps:
        key = el_key.get(c['ref'])
        pkg = pk.get(key)
        pat = ascii_text(c.get('pattern') or (key or ('', ''))[1])
        if pkg is None or pat in made:
            continue
        pins = [g for g in pkg if g.tag in ('pad', 'smd')]
        th = any(g.tag == 'pad' for g in pins)
        made[pat] = 'THMT' if th else 'SMT'
        P = S(step, 'Package', name=pat, type='OTHER', pinOneOrientation='OTHER')
        xs = [_f(g, 'x') for g in pins] or [0]
        ys = [_f(g, 'y') for g in pins] or [0]
        half = max([max(_f(g, 'dx'), _f(g, 'dy'), _f(g, 'diameter'), _f(g, 'drill') * 2) / 2 for g in pins] or [0.5])
        x0, x1, y0, y1 = min(xs) - half, max(xs) + half, min(ys) - half, max(ys) + half
        out = S(P, 'Outline')
        pg = S(out, 'Polygon')
        S(pg, 'PolyBegin', x=f(x0), y=f(y0))
        for x, y in ((x1, y0), (x1, y1), (x0, y1), (x0, y0)):
            S(pg, 'PolyStepSegment', x=f(x), y=f(y))
        S(out, 'LineDesc', lineEnd='ROUND', lineWidth='0.1')
        for g in pins:
            pin = S(P, 'Pin', number=ascii_text(g.get('name')), type='THRU' if g.tag == 'pad' else 'SURFACE')
            rot = float((re.search(r'R(-?[\d.]+)', g.get('rot') or 'R0') or [0, 0])[1]) % 360
            if rot:
                S(pin, 'Xform', rotation=f(rot))
            S(pin, 'Location', x=f(_f(g, 'x')), y=f(_f(g, 'y')))
            if g.tag == 'smd':
                S(pin, 'StandardPrimitiveRef', id=prim('rect', _f(g, 'dx'), _f(g, 'dy')))
            else:
                S(pin, 'StandardPrimitiveRef', id=prim('round', max(_f(g, 'diameter'), _f(g, 'drill') * 1.5)))
    for c in comps:
        pat = ascii_text(c.get('pattern') or '')
        if pat not in made:
            continue
        bottom = c.get('side') == 'bottom'
        C = S(step, 'Component', refDes=ascii_text(c['ref']), packageRef=pat, part=ascii_text('%s_%s' % (c.get('pattern') or '', c.get('value') or '')).strip('_') or 'PART',
              layerRef='BOTTOM' if bottom else 'TOP', mountType=made[pat])
        rot = float(c.get('rotation') or 0) % 360
        if rot or bottom:
            xf = S(C, 'Xform')
            if rot:
                xf.set('rotation', f(rot))
            if bottom:
                xf.set('mirror', 'true')
        S(C, 'Location', x=f(c['x']), y=f(c['y']))
    nets = {}
    for p_ in model['pads']:
        if p_.get('net') and p_.get('ref'):
            nets.setdefault(p_['net'], []).append(p_)
    for n in sorted(nets):
        ln_ = S(step, 'LogicalNet', name=ascii_text(n))
        for p_ in nets[n]:
            S(ln_, 'PinRef', componentRef=ascii_text(p_['ref']), pin=ascii_text(p_['name']))
    counts = {'pads': 0, 'traces': 0, 'arcs': 0, 'vias': 0, 'holes': 0, 'pourIslands': 0, 'poursUnfilled': 0}
    pours = zones(board)
    counts['poursUnfilled'] = sum(1 for z in pours if not z['filled'])
    for c in copper:
        lf = S(step, 'LayerFeature', layerRef=lname[c])
        sets = {}

        def set_for(net, kind):
            k = (net or '', kind)
            if k not in sets:
                a = {'net': ascii_text(net)} if net else {}
                if kind == 'pad':
                    a['padUsage'] = 'TERMINATION'
                elif kind == 'via':
                    a['padUsage'] = 'VIA'
                sets[k] = S(lf, 'Set', a)
            return sets[k]

        for p_ in model['pads']:
            if p_.get('layer') not in (c, '*.Cu'):
                continue
            pd = S(set_for(p_.get('net'), 'pad'), 'Pad')
            rot = float(p_.get('rotation') or 0) % 360
            if rot:
                S(pd, 'Xform', rotation=f(rot))
            S(pd, 'Location', x=f(p_['x']), y=f(p_['y']))
            S(pd, 'StandardPrimitiveRef', id=prim(p_.get('shape'), p_.get('sizeX') or 0, p_.get('sizeY') or 0) if p_.get('shape') != 'round' else prim('round', p_.get('sizeX') or 0))
            if p_.get('ref'):
                S(pd, 'PinRef', componentRef=ascii_text(p_['ref']), pin=ascii_text(p_['name']))
            counts['pads'] += 1
        for v in model['vias']:
            if c not in (v.get('layers') or copper) and not (len(v.get('layers') or []) == 2 and copper.index(c) >= copper.index(v['layers'][0]) and copper.index(c) <= copper.index(v['layers'][1])):
                continue
            key = (round(v.get('size') or 0, 4), round(v.get('drill') or 0, 4))
            pd = S(set_for(v.get('net'), 'via'), 'Pad', padstackDefRef=vstacks[key])
            S(pd, 'Location', x=f(v['position']['x']), y=f(v['position']['y']))
            S(pd, 'StandardPrimitiveRef', id=prim('round', key[0]))
            counts['vias'] += 1
        for t in model['tracks']:
            if t.get('layer') != c:
                continue
            fe = S(set_for(t.get('net'), 'trace'), 'Features')
            ln = S(fe, 'Line', startX=f(t['start']['x']), startY=f(t['start']['y']), endX=f(t['end']['x']), endY=f(t['end']['y']))
            S(ln, 'LineDesc', lineEnd='ROUND', lineWidth=f(t['width']))
            counts['traces'] += 1
        for a in model['arcs']:
            if a.get('layer') != c:
                continue
            cx, cy, rr = a['center']['x'], a['center']['y'], a['radius']
            s0, s1 = math.radians(a['startAngle']), math.radians(a['endAngle'])
            fe = S(set_for(a.get('net'), 'trace'), 'Features')
            ar = S(fe, 'Arc', startX=f(cx + rr * math.cos(s0)), startY=f(cy + rr * math.sin(s0)), endX=f(cx + rr * math.cos(s1)), endY=f(cy + rr * math.sin(s1)),
                   centerX=f(cx), centerY=f(cy), clockwise='false')
            S(ar, 'LineDesc', lineEnd='ROUND', lineWidth=f(a['width']))
            counts['arcs'] += 1
        for z in pours:
            if z['layer'] != c:
                continue
            for fill in z['fills']:
                if len(fill['outline']) < 3:
                    continue
                fe = S(set_for(z['net'], 'pour'), 'Features')
                ct = S(fe, 'Contour')
                for tag, ring in [('Polygon', fill['outline'])] + [('Cutout', h) for h in fill['holes'] if len(h) >= 3]:
                    pg = S(ct, tag)
                    S(pg, 'PolyBegin', x=f(ring[0][0]), y=f(ring[0][1]))
                    for x, y in ring[1:] + ring[:1]:
                        S(pg, 'PolyStepSegment', x=f(x), y=f(y))
                counts['pourIslands'] += 1
    lf = S(step, 'LayerFeature', layerRef='DRILL_TOP_BOTTOM')
    hn = 0
    drills = [(p_.get('net'), p_['x'], p_['y'], p_['drill'], 'PLATED') for p_ in model['pads'] if p_.get('drill')]
    drills += [(v.get('net'), v['position']['x'], v['position']['y'], v['drill'], 'VIA') for v in model['vias']]
    drills += [(None, h['x'], h['y'], h.get('drill') or h.get('diameter') or 0, 'NONPLATED') for h in model['holes']]
    hsets = {}
    for net, x, y, d, plating in drills:
        k = net or ''
        if k not in hsets:
            hsets[k] = S(lf, 'Set', {'net': ascii_text(net)} if net else {})
        hn += 1
        S(hsets[k], 'Hole', name='H%d' % hn, diameter=f(d), platingStatus=plating, plusTol='0', minusTol='0', x=f(x), y=f(y))
    counts['holes'] = hn
    ET.indent(root) if hasattr(ET, 'indent') else None
    text = '<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(root, encoding='unicode') + '\n'
    return text, dict(counts, components=len(comps), nets=len(nets), packages=len(made), layers=len(layers))


def export_ipc2581(app, root, board, meta, args):
    text, counts = ipc2581_xml(board, meta, version=args.get('_bridgeVersion') or '')
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', meta.get('documentName') or 'board')
    out_dir = Path(args.get('outputDir') or (Path(tempfile.gettempdir()) / 'adom-ipc2581'))
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / ('%s.xml' % stem)
    path.write_text(text, encoding='utf-8')
    hint = ('IPC-2581 revision C, valid against the IPC-2581C schema: layers and stack-up, outline, packages and components, BOM, nets, '
            'and on each copper layer the pads, traces, vias and the poured copper island by island, plus the drills. Solder mask, paste and '
            'silkscreen are not in it, so a fab order goes through fusion_export_odb (the native ODB++) or the Gerbers.')
    if counts['poursUnfilled']:
        hint += ' %d pour(s) were not filled in this board, so their copper is missing: run fusion_repour and export again.' % counts['poursUnfilled']
    return {'success': True, 'mutated': False, **meta, 'path': path.as_posix(), 'format': 'IPC-2581 revision C', 'mode': 'ASSEMBLY', 'units': 'mm', **counts, '_hint': hint}


ASCII_SUBS = {'\u03a9': 'ohm', '\u2126': 'ohm', '\u00b5': 'u', '\u03bc': 'u', '\u00b1': '+/-', '\u00b0': 'deg', '\u2013': '-', '\u2014': '-'}


def ascii_text(t):
    """Fab formats are read by ASCII tools: name the usual unit signs (10k\u03a9 -> 10kohm), '?' for the rest."""
    return ''.join(c if ord(c) < 128 else ASCII_SUBS.get(c, '?') for c in str(t))


def gencad_text(board, meta):
    """GenCAD 1.4 (mm) from the board: the outline, pads and padstacks, footprint shapes with their
    pins, components, devices, signals with their nodes, and the routes (tracks and vias)."""
    L = Layers(board)
    q = lambda t: '"%s"' % ascii_text(t).replace('"', "'")
    f = lambda v: ('%.4f' % float(v)).rstrip('0').rstrip('.') if '.' in ('%.4f' % float(v)) else '%.4f' % float(v)
    out = ['$HEADER', 'GENCAD 1.4', 'USER "fusion-bridge"', 'DRAWING %s' % q(meta.get('documentName') or 'board'), 'REVISION "1"',
           'UNITS MM', 'ORIGIN 0 0', 'INTERTRACK 0', '$ENDHEADER', '$BOARD']
    model, _ = board_model(board, meta)
    for e in list(model['outlineEdges']) + [x for c in model['outlineCutouts'] for x in c['edges']]:
        if e['kind'] == 'arc':
            out.append('ARC %s %s %s %s %s %s' % (f(e['start'][0]), f(e['start'][1]), f(e['end'][0]), f(e['end'][1]), f(e['center'][0]), f(e['center'][1])))
        else:
            out.append('LINE %s %s %s %s' % (f(e['start'][0]), f(e['start'][1]), f(e['end'][0]), f(e['end'][1])))
    out.append('$ENDBOARD')
    pk = {(l.get('name'), p_.get('name')): p_ for l in board.findall('libraries/library') for p_ in l.findall('packages/package')}
    used = {(e.get('library'), e.get('package')) for e in board.findall('elements/element')}
    pads, stacks, shapes = {}, {}, []
    rv = board.find("designrules/param[@name='rvPadTop']")
    try: rv = float(rv.get('value')) if rv is not None else 0.25
    except ValueError: rv = 0.25
    def pad_of(g):
        if g.tag == 'smd':
            w, h = _f(g, 'dx'), _f(g, 'dy')
            key = ('R', round(w, 4), round(h, 4), 0)
            if key not in pads:
                pads[key] = ('P%d' % (len(pads) + 1), ['PAD %s RECTANGULAR 0' % 'P%d' % (len(pads) + 1), 'RECTANGLE %s %s %s %s' % (f(-w / 2), f(-h / 2), f(w), f(h))])
        else:
            drill = _f(g, 'drill')
            dia = max(_f(g, 'diameter'), drill * (1 + 2 * rv))
            shape = g.get('shape', 'round')
            key = (shape, round(dia, 4), round(drill, 4))
            if key not in pads:
                n = 'P%d' % (len(pads) + 1)
                body = ['RECTANGLE %s %s %s %s' % (f(-dia / 2), f(-dia / 2), f(dia), f(dia))] if shape == 'square' else ['CIRCLE 0 0 %s' % f(dia / 2)]
                pads[key] = (n, ['PAD %s %s %s' % (n, 'RECTANGULAR' if shape == 'square' else ('OCTAGON' if shape == 'octagon' else 'ROUND'), f(drill))] + body)
        return pads[key][0]
    def stack_of(g):
        pn = pad_of(g)
        if g.tag == 'smd':
            layer = 'TOP' if g.get('layer', '1') == '1' else 'BOTTOM'
            key = (pn, layer, 0)
            if key not in stacks:
                sn = 'S%d' % (len(stacks) + 1)
                stacks[key] = (sn, ['PADSTACK %s 0' % sn, 'PAD %s %s 0 0' % (pn, layer)])
        else:
            key = (pn, 'ALL', _f(g, 'drill'))
            if key not in stacks:
                sn = 'S%d' % (len(stacks) + 1)
                stacks[key] = (sn, ['PADSTACK %s %s' % (sn, f(_f(g, 'drill'))), 'PAD %s TOP 0 0' % pn, 'PAD %s BOTTOM 0 0' % pn])
        return stacks[key][0]
    shape_names = {}
    for key in sorted(used, key=lambda k: (k[0] or '', k[1] or '')):
        pkg = pk.get(key)
        if pkg is None:
            continue
        nm = re.sub(r'\s+', '_', '%s' % key[1])
        if nm in shape_names.values():
            nm = '%s_%s' % (nm, re.sub(r'\W+', '_', key[0] or ''))
        shape_names[key] = nm
        body = ['SHAPE %s' % q(nm)]
        th = any(g.tag == 'pad' for g in pkg)
        body.append('INSERT %s' % ('TH' if th else 'SMD'))
        for g in pkg:
            if g.tag in ('pad', 'smd'):
                rot = float((re.search(r'R(-?[\d.]+)', g.get('rot') or 'R0') or [0, 0])[1])
                layer = 'TOP' if g.tag == 'pad' or g.get('layer', '1') == '1' else 'BOTTOM'
                body.append('PIN %s %s %s %s %s %s 0' % (q(g.get('name')), stack_of(g), f(_f(g, 'x')), f(_f(g, 'y')), layer, f(rot)))
        shapes += body
    out += ['$PADS'] + [l for _, (n, lines) in sorted(pads.items(), key=lambda kv: int(kv[1][0][1:])) for l in lines] + ['$ENDPADS']
    out += ['$PADSTACKS'] + [l for _, (n, lines) in sorted(stacks.items(), key=lambda kv: int(kv[1][0][1:])) for l in lines] + ['$ENDPADSTACKS']
    out += ['$SHAPES'] + shapes + ['$ENDSHAPES']
    comps, devs = [], {}
    for el in board.findall('elements/element'):
        key = (el.get('library'), el.get('package'))
        if key not in shape_names:
            continue
        mirrored = 'M' in (el.get('rot') or '')
        rot = float((re.search(r'R(-?[\d.]+)', el.get('rot') or 'R0') or [0, 0])[1])
        dev = '%s_%s' % (shape_names[key], re.sub(r'\W+', '_', el.get('value') or ''))
        devs[dev] = (shape_names[key], el.get('value') or '')
        comps += ['COMPONENT %s' % q(el.get('name')), 'PLACE %s %s' % (f(el.get('x')), f(el.get('y'))), 'LAYER %s' % ('BOTTOM' if mirrored else 'TOP'),
                  'ROTATION %s' % f(rot), 'SHAPE %s %s %s' % (q(shape_names[key]), 'MIRRORX' if mirrored else '0', 'FLIP' if mirrored else '0'), 'DEVICE %s' % q(dev)]
    out += ['$COMPONENTS'] + comps + ['$ENDCOMPONENTS']
    out += ['$DEVICES'] + [l for d, (shp, val) in sorted(devs.items()) for l in ('DEVICE %s' % q(d), 'PART %s' % q(shp), 'VALUE %s' % q(val))] + ['$ENDDEVICES']
    sig = ['$SIGNALS']
    for s_ in board.findall('signals/signal'):
        refs = s_.findall('contactref')
        if not refs:
            continue
        sig.append('SIGNAL %s' % q(s_.get('name')))
        sig += ['NODE %s %s' % (q(c.get('element')), q(c.get('pad'))) for c in refs]
    out += sig + ['$ENDSIGNALS']
    widths = sorted({round(_f(w, 'width'), 4) for s_ in board.findall('signals/signal') for w in s_.findall('wire') if w.get('layer') != '19'})
    tno = {w: i + 1 for i, w in enumerate(widths)}
    out += ['$TRACKS'] + ['TRACK %d %s' % (tno[w], f(w)) for w in widths] + ['$ENDTRACKS']
    via_stacks = {}
    routes = ['$ROUTES']
    for s_ in board.findall('signals/signal'):
        ws = [w for w in s_.findall('wire') if w.get('layer') != '19']
        vs = s_.findall('via')
        if not ws and not vs:
            continue
        routes.append('ROUTE %s' % q(s_.get('name')))
        for w in sorted(ws, key=lambda w: (int(w.get('layer')), _f(w, 'width'))):
            lay = int(w.get('layer'))
            routes += ['TRACK %d' % tno[round(_f(w, 'width'), 4)], 'LAYER %s' % ('TOP' if lay == 1 else ('BOTTOM' if lay == 16 else 'INNER%d' % (L.order.index(lay) if lay in L.order else lay)))]
            if _f(w, 'curve'):
                pts = wire_points((_f(w, 'x1'), _f(w, 'y1')), (_f(w, 'x2'), _f(w, 'y2')), _f(w, 'curve'))
                routes += ['LINE %s %s %s %s' % (f(a[0]), f(a[1]), f(b[0]), f(b[1])) for a, b in zip(pts, pts[1:])]
            else:
                routes.append('LINE %s %s %s %s' % (f(_f(w, 'x1')), f(_f(w, 'y1')), f(_f(w, 'x2')), f(_f(w, 'y2'))))
        for v in vs:
            dia = _f(v, 'diameter') or _f(v, 'drill') * 2
            key = (round(dia, 4), round(_f(v, 'drill'), 4))
            if key not in via_stacks:
                vn = 'VIA%d' % (len(via_stacks) + 1)
                pn = 'PV%d' % (len(via_stacks) + 1)
                via_stacks[key] = (vn, ['PAD %s ROUND %s' % (pn, f(key[1])), 'CIRCLE 0 0 %s' % f(dia / 2)], ['PADSTACK %s %s' % (vn, f(key[1])), 'PAD %s TOP 0 0' % pn, 'PAD %s BOTTOM 0 0' % pn])
            routes.append('VIA %s %s %s ALL %s' % (via_stacks[key][0], f(_f(v, 'x')), f(_f(v, 'y')), f(_f(v, 'drill'))))
    routes.append('$ENDROUTES')
    # via pads and padstacks go in their sections
    i_pads, i_stacks = out.index('$ENDPADS'), None
    out[i_pads:i_pads] = [l for _, (vn, pl, sl) in via_stacks.items() for l in pl]
    i_stacks = out.index('$ENDPADSTACKS')
    out[i_stacks:i_stacks] = [l for _, (vn, pl, sl) in via_stacks.items() for l in sl]
    return '\n'.join(out + routes) + '\n', {'components': len(comps) // 6, 'signals': sum(1 for l in sig if l.startswith('SIGNAL')),
                                           'pads': len(pads) + len(via_stacks), 'shapes': len(shape_names)}


def export_gencad(app, root, board, meta, args):
    text, counts = gencad_text(board, meta)
    stem = re.sub(r'[^A-Za-z0-9_.-]+', '_', meta.get('documentName') or 'board')
    out_dir = Path(args.get('outputDir') or (Path(tempfile.gettempdir()) / 'adom-gencad'))
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / ('%s.cad' % stem)
    path.write_text(text, encoding='ascii')
    return {'success': True, 'mutated': False, **meta, 'path': path.as_posix(), 'format': 'GenCAD 1.4', 'units': 'mm', **counts,
            '_hint': 'Outline, pads and padstacks, footprint shapes, components (bottom parts mirrored), devices, signals with their nodes, and routes with vias. uniquePins, flipBottomPads and useDrillOrigin are not offered.'}


def board_info_compat(board, meta):
    """Fusion 2706 dropped the Electron.boardInfo text command (live 2026-10-28). This writes the
    same summary and an XML in its layout from the board's own export, so fusion_board_info,
    fusion_board_summary and electronics_read's board fallback keep answering on any Fusion."""
    from xml.sax.saxutils import quoteattr as qa
    L = Layers(board)
    stack = layer_stack(board)
    order = list(L.order)
    pk = {(l.get('name'), p_.get('name')): p_ for l in board.findall('libraries/library') for p_ in l.findall('packages/package')}
    lib_urn = {l.get('name'): l.get('urn', '') for l in board.findall('libraries/library')}
    rows = stack.get('layers') or []
    cu_um = lambda i: (rows[i].get('copperUm') if len(rows) > i and rows[i].get('copperUm') else 35.0) / 1000.0
    setup = board.find("designrules/param[@name='layerSetup']")
    head = '<g name=%s unit="mm" boardThickness="%g" topCopperThickness="%g" bottomCopperThickness="%g" layersetup=%s topLayer="%d" bottomLayer="%d" viaLimit="%d">' % (
        qa(meta.get('documentName') or ''), round((stack.get('totalUm') or 1600) / 1000.0, 4), cu_um(0), cu_um(len(rows) - 1 if rows else 0),
        qa(setup.get('value') if setup is not None else ''), order[0] if order else 1, order[-1] if order else 16, len(order))
    parts = ['<boardinfo>', head, '<libraries>']
    comps = []
    groups = {}
    for el in board.findall('elements/element'):
        groups.setdefault((el.get('library'), el.get('package')), []).append(el)
        comps.append({'name': el.get('name'), 'library': el.get('library'), 'package': el.get('package'), 'value': el.get('value', ''),
                      'x': el.get('x'), 'y': el.get('y'), 'rotation': el.get('rot', 'R0'), 'locked': el.get('locked') == 'yes'})
    for (lib, pkg), els in groups.items():
        p_ = pk.get((lib, pkg))
        parts.append('<g library=%s package=%s library_urn=%s package3d_urn=%s>' % (qa(lib or ''), qa(pkg or ''), qa(lib_urn.get(lib, '')), qa(els[0].get('package3d_urn', ''))))
        for el in els:
            parts.append('<g name=%s x=%s y=%s rot=%s LOCKED="%s"/>' % (qa(el.get('name')), qa(el.get('x')), qa(el.get('y')), qa(el.get('rot', 'R0')), 'true' if el.get('locked') == 'yes' else 'false'))
        parts.append('</g>')
    parts.append('</libraries>')
    nets = set()
    by_layer = {}
    for s_ in board.findall('signals/signal'):
        for w in s_.findall('wire'):
            if w.get('layer') == '19':
                continue
            by_layer.setdefault(int(w.get('layer')), []).append((s_.get('name'), '%s %s 0 %s %s 0' % (w.get('x1'), w.get('y1'), w.get('x2'), w.get('y2'))))
            nets.add(s_.get('name'))
        if s_.findall('contactref'):
            nets.add(s_.get('name'))
    k = 0
    for n in sorted(by_layer):
        parts.append('<g id="%d_copper">' % n)
        for net, d in by_layer[n]:
            k += 1
            parts.append('<copper name=%s id="w%d" d=%s/>' % (qa(net), k, qa(d)))
        parts.append('</g>')
    parts += ['<drc/>', '</g>', '</boardinfo>']
    summary = {'name': meta.get('documentName') or '', 'unit': 'mm', 'boardThickness': str(round((stack.get('totalUm') or 1600) / 1000.0, 4)),
               'layerSetup': setup.get('value') if setup is not None else '', 'components': comps, 'componentCount': len(comps),
               'nets': sorted(nets), 'netCount': len(nets)}
    return {'success': True, 'output': 'Board info: %s, %d components, %d nets' % (summary['name'], len(comps), len(nets)),
            'data': {'summary': summary, 'rawXml': '\n'.join(parts), 'source': 'board export (Electron.boardInfo is gone in this Fusion)'}}


def footprint_primitives(board, meta, args):
    """The standard's footprint_primitives for one placed part: its package's pads (mask and paste
    as EAGLE draws them), wires, arcs, circles, polygons and texts, placed on the board (mm, the
    board's frame; the standard's mil and layer ids come with them)."""
    ref = args.get('designator')
    el = _element(board, ref) if ref else None
    if el is None:
        fail('designator_not_found', 'No part %s on the board' % ref)
    pkg = next((p_ for l in board.findall('libraries/library') if l.get('name') == el.get('library') for p_ in l.findall('packages/package') if p_.get('name') == el.get('package')), None)
    if pkg is None:
        fail('invalid_export', 'The board does not embed %s' % el.get('package'))
    place, mirrored, _ = _place(el)
    L = Layers(board)
    flip = (lambda n: FLIP.get(n, n)) if mirrored else (lambda n: n)
    prims, other = [], {}
    for g in pkg:
        t = g.tag
        if t in ('smd', 'pad'):
            x, y = place(_f(g, 'x'), _f(g, 'y'))
            layer = flip(int(g.get('layer', '1'))) if t == 'smd' else 17
            row = {'kind': 'pad', 'name': g.get('name'), 'layerId': layer, 'layer': L.name(layer), 'xMm': round(x, 4), 'yMm': round(y, 4),
                   'xMil': round(x / 0.0254, 2), 'yMil': round(y / 0.0254, 2)}
            if t == 'smd':
                row.update(sizeMm=[_f(g, 'dx'), _f(g, 'dy')], roundness=_f(g, 'roundness'), solderMask=g.get('stop', 'yes') != 'no', paste=g.get('cream', 'yes') != 'no')
            else:
                row.update(drillMm=_f(g, 'drill'), diameterMm=_f(g, 'diameter'), shape=g.get('shape', 'round'), solderMask=g.get('stop', 'yes') != 'no')
            prims.append(row)
        elif t == 'wire':
            a, b = place(_f(g, 'x1'), _f(g, 'y1')), place(_f(g, 'x2'), _f(g, 'y2'))
            n = flip(int(g.get('layer')))
            prims.append({'kind': 'arc' if _f(g, 'curve') else 'track', 'layerId': n, 'layer': L.name(n), 'start': [round(a[0], 4), round(a[1], 4)], 'end': [round(b[0], 4), round(b[1], 4)],
                          'widthMm': _f(g, 'width'), 'curve': _f(g, 'curve') * (-1 if mirrored else 1)})
        elif t == 'text':
            x, y = place(_f(g, 'x'), _f(g, 'y'))
            n = flip(int(g.get('layer')))
            prims.append({'kind': 'text', 'layerId': n, 'layer': L.name(n), 'text': (g.text or ''), 'at': [round(x, 4), round(y, 4)], 'sizeMm': _f(g, 'size')})
        elif t != 'description':
            other[t] = other.get(t, 0) + 1
    return {'success': True, 'mutated': False, **meta, 'designator': ref, 'pattern': el.get('package'), 'side': 'bottom' if mirrored else 'top',
            'count': len(prims), 'otherPrimitives': other, 'primitives': prims}


# ── board furniture: thermal vias, via fences, mounting holes, fiducials, keepouts, stitching ──

def _seg_dist(p, a, b):
    ax, ay = a; bx, by = b; dx, dy = bx - ax, by - ay
    L = dx * dx + dy * dy
    t = 0.0 if L == 0 else max(0.0, min(1.0, ((p[0] - ax) * dx + (p[1] - ay) * dy) / L))
    return math.hypot(p[0] - ax - t * dx, p[1] - ay - t * dy)


def _copper_env(board):
    """Everything a new via or hole must keep clear of: pads (their real shape), tracks, vias,
    holes, and the outline."""
    _, pads, wires, vias = objects(board)
    model, _ = board_model(board, {})
    loops = [[tuple(e['start']) for e in model['outlineEdges']]]
    edges = [(tuple(e['start']), tuple(e['end'])) for e in model['outlineEdges']] + [(tuple(e['start']), tuple(e['end'])) for c in model['outlineCutouts'] for e in c['edges']]
    holes = [((_f(h, 'x'), _f(h, 'y')), _f(h, 'drill') / 2) for h in board.findall('plain/hole')]
    return {'pads': pads, 'wires': wires, 'vias': vias, 'holes': holes, 'outline': loops[0], 'edges': edges}


def _spot_clear(env, pt, radius, net, clearance, edge, layers=(1, 16), new=()):
    """Is a through via (or hole) of this radius at pt clear: of other nets' pads and tracks by the
    clearance, of every via and hole (same net too, by the clearance), and inside the outline by edge?"""
    if not _point_in(pt, env['outline']):
        return 'outside the board'
    if any(_seg_dist(pt, a, b) < radius + edge for a, b in env['edges']):
        return 'too close to the edge'
    for key, p_ in env['pads'].items():
        if p_['net'] == net and net:
            continue
        if not any(l in p_['layers'] for l in layers):
            continue
        if p_['poly']:
            inside = _point_in(pt, p_['poly'])
            d = 0.0 if inside else min(_seg_dist(pt, p_['poly'][k], p_['poly'][(k + 1) % 4]) for k in range(4))
        else:
            d = max(0.0, math.hypot(pt[0] - p_['at'][0], pt[1] - p_['at'][1]) - p_['radius'])
        if d < radius + clearance:
            return 'pad %s' % key
    for w in env['wires']:
        if w['net'] == net and net:
            continue
        if _seg_dist(pt, w['start'], w['end']) < radius + w['width'] / 2 + clearance:
            return 'track %s' % w['net']
    for v in list(env['vias']) + list(new):
        if math.hypot(pt[0] - v['at'][0], pt[1] - v['at'][1]) < radius + (v['diameter'] or v['drill'] * 2) / 2 + clearance:
            return 'via %s' % v['net']
    for at, r in env['holes']:
        if math.hypot(pt[0] - at[0], pt[1] - at[1]) < radius + r + clearance:
            return 'hole'
    return None


def _via_rule(board):
    """The board's routing via: diameter from the drill and the via restring, drill from mlViaMin... or 0.3."""
    drill = 0.3
    rv = board.find("designrules/param[@name='rvViaOuter']")
    try: ring = float(rv.get('value')) if rv is not None else 0.25
    except ValueError: ring = 0.25
    mn = _rule_len(board, 'rlMinViaOuter', 0.2)
    return max(drill + 2 * max(drill * ring, mn), 0.6), drill


def _place_and_check(app, args, board, items, label):
    """VIAs, HOLEs and plain shapes, then a read back: every via and hole must be where asked;
    otherwise every command is undone."""
    cmds = []
    for it in items:
        if it['kind'] == 'via':
            cmds.append("CHANGE DRILL %.6fmm; VIA '%s' %.6fmm ROUND 1-16 (%.6fmm %.6fmm);" % (it['drill'], identifier(it['net']), it['diameter'], it['at'][0], it['at'][1]))
        elif it['kind'] == 'hole':
            cmds.append("CHANGE DRILL %.6fmm; HOLE (%.6fmm %.6fmm);" % (it['drill'], it['at'][0], it['at'][1]))
        elif it['kind'] == 'circle':
            cmds.append("LAYER %d; CIRCLE %.6fmm (%.6fmm %.6fmm) (%.6fmm %.6fmm);" % (it['layer'], it.get('width', 0), it['at'][0], it['at'][1], it['at'][0] + it['radius'], it['at'][1]))
        elif it['kind'] == 'rect':
            x1, y1, x2, y2 = it['rect']
            cmds.append("LAYER %d; RECT R0 (%.6fmm %.6fmm) (%.6fmm %.6fmm);" % (it['layer'], x1, y1, x2, y2))
        elif it['kind'] == 'polygon':
            # named, so Fusion never asks for the signal in its modal; solid, rank 1 and no thermals:
            # a teardrop is copper of its own net that nothing else's pour may cut
            path = ' '.join('(%.6fmm %.6fmm)' % (x, y) for x, y in it['points'] + it['points'][:1])
            cmds.append("CHANGE ISOLATE 0mm; CHANGE RANK 1; CHANGE POUR SOLID; CHANGE ORPHANS ON; CHANGE THERMALS OFF; LAYER %d; POLYGON '%s' %.6fmm %s;" % (it['layer'], identifier(it['net']), it.get('width', 0.0254), path))
    done = 0
    for k in range(0, len(cmds), 25):
        app.executeTextCommand('Electron.run ' + ' '.join(cmds[k:k + 25]))
        done += len(cmds[k:k + 25])
    app.executeTextCommand('Electron.run RATSNEST;')
    _, after, meta2 = snapshot(app, args)
    _, _, _, vias2 = objects(after)
    holes2 = [(_f(h, 'x'), _f(h, 'y')) for h in after.findall('plain/hole')]
    missing = []
    for it in items:
        if it['kind'] == 'via' and not any(v['net'] == it['net'] and abs(v['at'][0] - it['at'][0]) < 0.002 and abs(v['at'][1] - it['at'][1]) < 0.002 for v in vias2):
            missing.append(it)
        if it['kind'] == 'hole' and not any(abs(h[0] - it['at'][0]) < 0.002 and abs(h[1] - it['at'][1]) < 0.002 for h in holes2):
            missing.append(it)
        if it['kind'] == 'polygon':
            x0, y0 = it['points'][0]
            sig = after.find("signals/signal[@name='%s']" % identifier(it['net']))
            # Fusion writes a drawn polygon as <polygonpour> with its outline under polygonoutlineobjects
            pgs = [] if sig is None else sig.findall('polygon') + sig.findall('polygonpour')
            if not any(int(pg.get('layer', 0)) == it['layer'] and any(abs(_f(v, 'x') - x0) < 0.002 and abs(_f(v, 'y') - y0) < 0.002 for v in pg.findall('vertex') + pg.findall('polygonoutlineobjects/vertex')) for pg in pgs):
                missing.append(dict(it, at=(x0, y0)))
    if missing:
        for _ in range(done):
            app.executeTextCommand('Electron.run UNDO;')
        fail('readback_mismatch', '%s: %d placed item(s) did not read back (first at %s); every command was undone' % (label, len(missing), missing[0]['at']))
    return after, meta2, done


def place_planned_vias(app, root, board, meta, args):
    """The vias of an engine plan (thermal_vias, via_fence: {net, x, y, size, drill}), placed and read
    back; a via that does not read back undoes every command."""
    _guard(meta, args)
    want = args.get('vias')
    if not isinstance(want, list) or not want:
        fail('invalid_args', 'vias must be a non-empty list of {net, x, y, size, drill}')
    nets = {s.get('name') for s in board.findall('signals/signal')}
    items = []
    for v in want:
        net = identifier(v.get('net'))
        if net not in nets:
            fail('unknown_net', 'No net named %s on the board' % v.get('net'))
        items.append({'kind': 'via', 'net': net, 'at': (number(v.get('x')), number(v.get('y'))),
                      'diameter': number(v.get('size', v.get('diameter', 0.6)), True), 'drill': number(v.get('drill', 0.3), True)})
    after, meta2, _ = _place_and_check(app, args, board, items, 'planned vias')
    return {'success': True, 'mutated': True, **meta2, 'applied': len(items), 'failed': 0}


def place_teardrops(app, root, board, meta, args):
    """The teardrops of an engine plan ({net, layer, points}: filled polygons where a track meets a
    pad or via), drawn as named solid polygons and read back; one that does not read back undoes all."""
    _guard(meta, args)
    want = args.get('teardrops')
    if not isinstance(want, list) or not want:
        fail('invalid_args', 'teardrops must be a non-empty list of {net, layer, points}')
    nets = {s.get('name') for s in board.findall('signals/signal')}
    items = []
    for t in want:
        net = identifier(t.get('net'))
        if net not in nets:
            fail('unknown_net', 'No net named %s on the board' % t.get('net'))
        pts = [(number(p[0]), number(p[1])) for p in (t.get('points') or [])]
        if len(pts) < 3:
            fail('invalid_args', 'a teardrop needs three points at least')
        items.append({'kind': 'polygon', 'net': net, 'layer': _layer_number(board, t.get('layer')), 'points': pts})
    after, meta2, _ = _place_and_check(app, args, board, items, 'teardrops')
    return {'success': True, 'mutated': True, **meta2, 'applied': len(items), 'failed': 0}


def board_features(app, root, board, meta, args):
    _guard(meta, args)
    env = _copper_env(board)
    items, results = [], []
    xs = [p[0] for p in env['outline']]; ys = [p[1] for p in env['outline']]
    mh = args.get('mountingHoles') or []
    if isinstance(mh, dict) and mh.get('corners') is not None:
        ins = float((mh.get('corners') or {}).get('inset', 4))
        spec = {k: v for k, v in mh.items() if k != 'corners'}
        mh = [dict(spec, x=x, y=y) for x, y in ((min(xs) + ins, min(ys) + ins), (max(xs) - ins, min(ys) + ins), (max(xs) - ins, max(ys) - ins), (min(xs) + ins, max(ys) - ins))]
    for h in mh:
        d = float(h.get('drill', 3.2)); at = (float(h['x']), float(h['y']))
        if h.get('plated'):
            pad = float(h.get('pad', d + 2.8)); net = h.get('net') or 'MOUNT'
            why = _spot_clear(env, at, pad / 2, net if h.get('net') else None, 0.3, 0.3)
            it = {'kind': 'via', 'net': net, 'at': at, 'diameter': pad, 'drill': d}
        else:
            why = _spot_clear(env, at, d / 2, None, 0.3, 0.3)
            it = {'kind': 'hole', 'at': at, 'drill': d}
        if why: results.append('err: mounting hole at %s: %s' % (list(at), why)); continue
        items.append(it); results.append('ok')
    for fd in args.get('fiducials') or []:
        at = (float(fd['x']), float(fd['y'])); size = float(fd.get('size', 1)); opening = float(fd.get('opening', 2))
        bottom = fd.get('side') == 'bottom'
        why = _spot_clear(env, at, opening / 2, None, 0.1, 0.5, layers=(16,) if bottom else (1,))
        if why: results.append('err: fiducial at %s: %s' % (list(at), why)); continue
        items.append({'kind': 'circle', 'layer': 16 if bottom else 1, 'at': at, 'radius': size / 2, 'width': 0})
        items.append({'kind': 'circle', 'layer': 30 if bottom else 29, 'at': at, 'radius': opening / 2, 'width': 0})
        results.append('ok')
    layer_of = {'all': (41, 42, 43), 'F.Cu': (41,), 'B.Cu': (42,)}
    for ko in args.get('keepouts') or []:
        r_ = [float(v) for v in ko['rect']]
        lay = ko.get('layer', 'all')
        for n in layer_of.get(lay, (43,)):
            items.append({'kind': 'rect', 'layer': n, 'rect': r_})
        results.append('ok')
    st = args.get('stitching')
    stitched = []
    if st:
        net = st.get('net')
        if not net or board.find("signals/signal[@name='%s']" % net) is None:
            fail('bad_args', 'stitching.net must be a net on the board')
        pitch = float(st.get('pitch', 5)); dia = float(st.get('size', 0.6)); drill = float(st.get('drill', 0.3))
        clr = float(st.get('clearance', 0.3)); edge = float(st.get('edge', 1))
        region = st.get('region')
        rx = [p[0] for p in region] if region else xs; ry = [p[1] for p in region] if region else ys
        keep = [tuple(ko['rect']) for ko in args.get('keepouts') or []]
        new = [{'at': it['at'], 'diameter': it.get('diameter') or it.get('drill', 0) or 0.1, 'drill': it.get('drill', 0.1), 'net': it.get('net')} for it in items if it['kind'] in ('via', 'hole')]
        y = min(ry) + pitch / 2
        while y < max(ry) and len(stitched) < int(st.get('max', 400)):
            x = min(rx) + pitch / 2
            while x < max(rx) and len(stitched) < int(st.get('max', 400)):
                p = (round(x, 4), round(y, 4))
                inside_region = _point_in(p, [tuple(q) for q in region]) if region else True
                in_keep = any(min(k[0], k[2]) <= p[0] <= max(k[0], k[2]) and min(k[1], k[3]) <= p[1] <= max(k[1], k[3]) for k in keep)
                if inside_region and not in_keep and _spot_clear(env, p, dia / 2, net, clr, edge, new=new + [{'at': q, 'diameter': dia, 'drill': drill, 'net': net} for q in stitched]) is None:
                    stitched.append(p)
                x += pitch
            y += pitch
        items += [{'kind': 'via', 'net': net, 'at': p, 'diameter': dia, 'drill': drill} for p in stitched]
    planned = {'mountingHoles': sum(1 for it in items if it['kind'] == 'hole' or (it['kind'] == 'via' and it['diameter'] >= 2)),
               'fiducials': sum(1 for it in items if it['kind'] == 'circle') // 2, 'keepouts': len(args.get('keepouts') or []),
               'stitching': {'count': len(stitched), 'points': stitched}}
    if args.get('dryRun') or not items:
        return {'success': True, 'mutated': False, **meta, 'planned': planned, 'results': results, 'applied': 0}
    after, meta2, n = _place_and_check(app, args, board, items, 'board features')
    return {'success': True, 'mutated': True, **meta2, 'applied': len(items), 'planned': planned, 'results': results,
            '_hint': 'Stitching keeps a pad clearance from each pad\'s real shape and ignores pours (other nets\' pours repour around the vias). Unplated holes are bare; plated ones are via-holes on their net.'}


def handle(app,args,action):
    attempted=False; commands=[]
    try:
        if action=='schematic_run': return sch_run(app,args)
        if action=='schematic_plan': return schematic_plan(args)
        if action=='board_seed': return board_seed(args)
        if action=='board_info_compat' and not args.get('expectDocument') and app.activeDocument is not None:
            args['expectDocument']=app.activeDocument.name
        root,board,meta=snapshot(app,args)
        if action=='board_info_compat': return board_info_compat(board,meta)
        if action=='footprint_primitives': return footprint_primitives(board,meta,args)
        if action=='place_planned_vias': return place_planned_vias(app,root,board,meta,args)
        if action=='place_teardrops': return place_teardrops(app,root,board,meta,args)
        if action=='board_features': return board_features(app,root,board,meta,args)
        if action=='routing_state':
            st=state(board,meta); n,fa=st['unconnected'],st['fusionAirwires']
            hint='Edit verbs take boardId and this revision as expectedRevision. unconnected (%d) is computed from the copper itself.'%n
            if fa>n: hint+=" Fusion's own ratsnest shows %d airwire(s) more: it also flags a track end that overlaps a pad without landing on the pad's anchor point (a few micrometres off), which is copper-connected. fusion_routing_drc lists them with positions; reroute that end onto the pad centre to clear them in Fusion."%(fa-n)
            elif fa<n: hint+=" Fusion's own ratsnest shows fewer (%d): it is stale (an import or an edit since Fusion last computed it). fusion_routing_drc recomputes it."%fa
            return {'success':True,**st,'_hint':hint}
        if action=='pour_state': return {'success':True,'mutated':False,**meta,**pour_state(board)}
        if action=='board_bom': return {'success':True,'mutated':False,**meta,**board_bom(board,args)}
        if action in READS: return {'success':True,'mutated':False,**meta,**READS[action](root,board,args)}
        if action=='export_bom':
            out=args.get('outputPath') or (tempfile.gettempdir().replace('\\','/')+'/adom-bom.csv')
            text,info=export_bom_csv(board,args)
            Path(out).parent.mkdir(parents=True,exist_ok=True); Path(out).write_text(text,encoding='utf-8')
            n,u=info['componentCount'],info['uniquePartCount']
            return {'success':True,'mutated':False,**meta,'output':'BOM exported: %d components%s -> %s'%(n,' in %d unique parts'%u if info['grouped'] else '',out),
                    'data':{'outputPath':out,**info,'nextSteps':['Run fusion_export_cpl to generate the pick-and-place CSV','Run fusion_export_gerbers to generate the Gerber fabrication ZIP']},
                    'csv':text if len(text)<200000 else None,
                    '_hint':'Columns: %s, then MPN, Manufacturer, LCSC, Mouser, DigiKey from each part\'s attributes. %d of %d parts carry an MPN and %d an LCSC number; data.missingMpn / missingLcsc name the rest (set them in the library or the part\'s attributes).'%('Comment, Designator, Footprint, Quantity, Library' if info['grouped'] else 'Designator, Comment, Footprint, Library, Side',info['withMpn'],n,info['withLcsc'])}
        if action=='apply_silkscreen':
            return apply_silkscreen(app,root,board,meta,args)
        if action=='autoroute': return autoroute(app,root,board,meta,args)
        if action=='update_board_from_eagle': return update_board_from_eagle(app,root,board,meta,args)
        if action=='board_dxf': return board_dxf(app,root,board,meta,args)
        if action=='check_inputs': return check_inputs(app,root,board,meta,args)
        if action=='export_odb': return export_odb(app,root,board,meta,args)
        if action=='export_ipc356': return export_ipc356(app,root,board,meta,args)
        if action=='export_gencad': return export_gencad(app,root,board,meta,args)
        if action=='export_ipc2581': return export_ipc2581(app,root,board,meta,args)
        if action=='import_svg': return import_svg(app,root,board,meta,args)
        if action=='copy_routing': return copy_routing(app,root,board,meta,args)
        if action=='set_layer_count': return set_layer_count(app,root,board,meta,args)
        if action=='edit_parts': return edit_parts(app,root,board,meta,args)
        if action=='align_parts': return align_parts(app,root,board,meta,args)
        if action=='remove_graphics': return remove_graphics(app,root,board,meta,args)
        if action=='remove_parts': return remove_parts(app,root,board,meta,args)
        if action=='run_erc': return run_erc(app,root,board,meta,args)
        if action=='set_layer_visibility': return set_layer_visibility(app,root,board,meta,args)
        if action=='select_net': return select_net(app,root,board,meta,args)
        if action=='set_track_width': return set_track_width(app,root,board,meta,args)
        if action=='edit_tracks': return edit_tracks(app,root,board,meta,args)
        if action=='change_vias': return change_vias(app,root,board,meta,args)
        if action=='place_parts': return place_parts(app,root,board,meta,args)
        if action=='add_rule': return add_rule(app,root,board,meta,args)
        if action=='read_board': return read_board(app,root,board,meta,args)
        if action=='remove_items': return remove_items(app,root,board,meta,args)
        if action=='board_outline': return board_outline_read(app,root,board,meta,args)
        if action in ('set_outline','board_shape'):
            if action=='board_shape':
                args=dict(args, outline={'rect':[args.get('x0Mm',0),args.get('y0Mm',0),args.get('wMm',15),args.get('hMm',15)]})
            return set_outline(app,root,board,meta,args)
        if action=='run_drc':
            # the native check on the live board, no revision guard: it changes no copper, parts or
            # outline (the revision), though Fusion keeps the DRC result in the board, which marks the
            # document modified
            app.executeTextCommand('Electron.run RATSNEST;')
            report=native_drc(app); _,b2,m2=snapshot(app,args)
            limit=int(args.get('limit',200))
            v=[{'rule':e['description'],'code':e['code'],'severity':e['severity'],'layer':e['layer'],'at':[e['x'],e['y']],'approved':e['approved']}
               for e in report['errors'] if e['code']!=21]
            return {'success':report['checked'],'mutated':False,**m2,'violationCount':len(v),'violations':v[:limit],'truncated':len(v)>limit,
                    'errors':report['errorCount'],'warnings':report['warningCount'],'airwires':report['airwireCount'],'summary':report['summary'],'notes':report['notes'],
                    'windowEtiquette':'background','source':report['source'],
                    '_hint':'Each violation carries its rule text, severity, layer and position in mm. fusion_zoom_to {rect} frames one; fusion_routing_drc is the same check inside the routing guards. Fusion keeps the DRC result in the board, so the document now reads as modified: close it without saving if you only checked it.'}
        if action=='repour':
            app.executeTextCommand('Commands.Start Electron::PolyShowFillAll')
            app.executeTextCommand('Electron.run RATSNEST;')
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            _,b2,m2=snapshot(app,args); ps=pour_state(b2)
            return {'success':True,'mutated':True,**m2,'filledAreaByLayerMm2':ps['filledAreaByLayerMm2'],'filledAreaByNetMm2':ps['filledAreaByNetMm2'],
                    'unfilled':ps['unfilled'],'zones':ps['zones'],'_hint':'Every pour was refilled by Fusion (RATSNEST). The revision moved if the poured copper changed.'}
        if action=='zoom_to':
            model,_=board_model(board); margin=number(args.get('margin',1.0))
            pts=[]
            if args.get('rect'):
                x1,y1,x2,y2=[number(v) for v in args['rect']]; pts=[(x1,y1),(x2,y2)]
            refs=([args['ref']] if args.get('ref') else [])+list(args.get('refs') or [])
            for r_ in refs:
                mine=[(p['x'],p['y']) for p in model['pads'] if p['ref']==r_]+[(c['x'],c['y']) for c in model['components'] if c['ref']==r_]
                if not mine: fail('designator_not_found','No part %s on the board'%r_)
                pts+=mine
            if args.get('net'):
                n=args['net']; hit=[(p['x'],p['y']) for p in model['pads'] if p['net'].lower()==n.lower()]
                hit+=[(t[k]['x'],t[k]['y']) for t in model['tracks'] if t['net'].lower()==n.lower() for k in ('start','end')]
                hit+=[(v['position']['x'],v['position']['y']) for v in model['vias'] if v['net'].lower()==n.lower()]
                if not hit: fail('no_such_net','No copper on a net named %s'%n)
                pts+=hit
            if not pts: fail('bad_args','Give rect [x1,y1,x2,y2] (mm), ref, refs or net')
            box=[min(p[0] for p in pts)-margin,min(p[1] for p in pts)-margin,max(p[0] for p in pts)+margin,max(p[1] for p in pts)+margin]
            app.executeTextCommand('Electron.run WINDOW (%.4fmm %.4fmm) (%.4fmm %.4fmm);'%tuple(box))
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            return {'success':True,'mutated':False,**meta,'rect':[round(v,4) for v in box],'windowEtiquette':'background'}
        if action=='layer_stack':
            st=layer_stack(board)
            return {'success':True,'mutated':False,**meta,**st,
                    '_hint':'Copper top to bottom, each with the dielectric below it (dielectricUm, dk1000 = Dk x 1000). This is what impedance and current-capacity checks read. '+(' '.join(st['notes']))}
        if action=='netlist':
            nl=netlist(board)
            return {'success':True,'mutated':False,**meta,'nets':nl,'netCount':len(nl),'source':'the board (Fusion keeps board and schematic in sync): Designator.Pad',
                    '_hint':'Every net with its pads as Designator.Pad. Compare with the schematic intent, or feed it to a schematic review.'}
        # The live board as EAGLE XML (Fusion's own fills included), for analysis offline.
        if action=='board_source': return {'success':True,'mutated':False,**meta,'source':ET.tostring(root,encoding='unicode')}
        if action=='plan_route':
            # the standard's plan_route (read only): from/to, widthMm, viaSizeMm, viaDrillMm, gridMm;
            # boardId and expectedRevision are checked when given. routeNet is this bridge's
            # route_net arguments, to pass on unchanged.
            for a_,b_ in (('from','fromPad'),('to','toPad'),('widthMm','width'),('viaSizeMm','viaDiameter'),('viaDrillMm','viaDrill'),('gridMm','grid')):
                if args.get(a_) is not None and args.get(b_) is None: args[b_]=args[a_]
            if args.get('boardId') not in (None,meta['boardId']): fail('board_mismatch','boardId does not identify the active board')
            if args.get('expectedRevision') not in (None,meta['revision']): fail('stale_board','expectedRevision must match the current live board')
            try:
                p_=automatic_plan(board,args)
            except RoutingError as e_:
                if e_.code in ('no_route','search_budget'):
                    return {'success':True,'mutated':False,**meta,'found':False,'reason':str(e_),'errorCode':None}
                raise
            return {'success':True,'mutated':False,**meta,'plan':p_,'found':True,'routeNet':p_['route'],'lengthMm':p_['planner']['lengthMm'],'vias':p_['planner']['viaCount'],
                    '_hint':'Pass routeNet to fusion_route_net with expectedRevision set to revision; fusion_run_drc after the route is still the proof.'}
        if args.get('boardId')!=meta['boardId']: fail('board_mismatch','Mutations/checks require boardId from fusion_routing_state')
        if args.get('expectedRevision')!=meta['revision']: fail('stale_board','expectedRevision must match the current live board')
        if action=='plan_route':
            return {'success':True,'mutated':False,**meta,'plan':automatic_plan(board,args)}
        if action=='routing_drc':
            app.executeTextCommand('Electron.run RATSNEST;')
            report=native_drc(app)
            _,board,meta=snapshot(app,args)
            return {'success':report['checked'],**state(board,meta),'drc':report}
        if action=='move_parts':
            # one MOVE (and ROTATE) per part, absolute; each its own undo step and its own moment on camera
            want=args.get('parts')
            if not isinstance(want,list) or not want: fail('invalid_args','parts must be a non-empty list of {ref,x,y,rotation}')
            have=element_rows(board); done=[]; rots={}; flipped=[]
            for p in want:
                ref=identifier(p.get('ref')); number(p.get('x')); number(p.get('y'))
                if ref not in have: fail('unknown_part','No element named %s on the board'%ref)
                rots[ref]=rot_of(p.get('rotation',have[ref]['rot']),have[ref]['rot'])
                # the standard's side and flip: rotation and side are kept when left out
                if p.get('side') not in (None,'top','bottom'): fail('invalid_args','side is top or bottom')
                if p.get('side') is not None and p.get('flip'): fail('invalid_args','give side or flip, not both')
                if p.get('side') is not None or p.get('flip'):
                    if isinstance(p.get('rotation'),str): fail('invalid_args','%s: give the rotation as a number when also giving side or flip: an R or MR string names the side itself'%ref)
                    bottom=(p.get('side')=='bottom') if p.get('side') is not None else 'M' not in have[ref]['rot']
                    rots[ref]=('M' if bottom else '')+rots[ref].lstrip('SM')
                if ('M' in rots[ref])!=('M' in have[ref]['rot']): flipped.append(ref)
            for p in want:
                ref=identifier(p.get('ref')); x=number(p.get('x')); y=number(p.get('y'))
                rot=rots[ref]
                cmd="MOVE '%s' (%.6fmm %.6fmm); ROTATE =%s '%s';"%(ref,x,y,rot,ref)
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd); done.append(ref)
            app.executeTextCommand('Electron.run RATSNEST;')
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            _,after,after_meta=snapshot(app,args); got=element_rows(after); bad=[]
            for p in want:
                g=got.get(p['ref']); r=rots[p['ref']]
                if not g or abs(g['x']-p['x'])>0.001 or abs(g['y']-p['y'])>0.001 or g['rot'].lstrip('S')!=r: bad.append({'ref':p['ref'],'want':[p['x'],p['y'],r],'got':g})
            if bad:
                # all or none: parts that did move would stay moved, or on the wrong side, with no way to tell which
                undone=_undo_until(app,args,meta['revision'],2*len(commands)+2)
                app.executeTextCommand('Electron.run RATSNEST;')
                _,back,back_meta=snapshot(app,args)
                return {'success':False,'mutated':not undone,'verified':False,'errorCode':'readback_mismatch','undone':undone,'mismatched':bad,'moved':[],'commands':commands,
                        'error':'%d part(s) did not land as asked; %s'%(len(bad),'every move was undone, so the board is as it was' if undone else 'the undo did not restore the board exactly: read fusion_routing_state'),**state(back,back_meta)}
            return {'success':True,'mutated':True,'verified':True,'errorCode':None,'mismatched':[],'moved':done,'sideChanged':flipped,'commands':commands,**state(after,after_meta)}
        if action=='change_pours':
            # change an existing pour's pen width and isolation in place (the refinement a field
            # solve asks for: a throat the pen was too wide to fill). Each CHANGE picks the pour on
            # the midpoint of its longest outline edge; the readback proves that pour changed and
            # no other copper did.
            want=args.get('pours')
            if not isinstance(want,list) or not want: fail('invalid_args','pours must be a non-empty list of {net, layer, points, width?, isolate?, thermals?, rank?, orphans?}')
            # the standard's edit_pours names: minWidthMm, clearanceMm, thermals {on}, priority, keepIslands
            for q in want:
                if q.get('width') is None and q.get('minWidthMm') is not None: q['width']=q['minWidthMm']
                if q.get('isolate') is None and q.get('clearanceMm') is not None: q['isolate']=q['clearanceMm']
                if isinstance(q.get('thermals'),dict): q['thermals']=q['thermals'].get('on')
                if q.get('rank') is None and q.get('priority') is not None: q['rank']=q['priority']
                if q.get('orphans') is None and q.get('keepIslands') is not None: q['orphans']=q['keepIslands']
                if q.get('newNet') is not None and identifier(q['newNet']) not in objects(board)[0]: fail('unknown_net','No exact net named '+q['newNet'])
                if q.get('rank') is not None and not (1<=int(q['rank'])<=6): fail('invalid_args','rank is 1 (poured first) to 6')
            have=pours(board); before_cu=copper_signature(objects(board)[2]+objects(board)[3])
            targets=[]
            for q in want:
                if q.get('id'):
                    # the standard's handle: it says which pour, so net, layer and outline come from the pour itself
                    byid=next((e for e in have if e['id']==q['id']),None)
                    if byid is None: fail('unknown_pour','No pour with id %s: fusion_pour_state lists them'%q['id'])
                    if q.get('net') and q['net']!=byid['net'] and q.get('newNet') is None:
                        if identifier(q['net']) not in objects(board)[0]: fail('unknown_net','No exact net named '+q['net'])
                        q['newNet']=q['net']
                    q.update(net=byid['net'],layer=byid['layer'],points=byid['outline'])
                if q.get('points') is None:
                    # no outline: net and layer name the pour when that net has one pour on that layer
                    same=[e for e in have if e['net']==q.get('net') and e['layer']==eagle_layer(q.get('layer',1),board)]
                    if len(same)!=1: fail('unknown_pour' if not same else 'ambiguous_pour','%d %s pours on layer %s: give its points (pour_state lists each outline)'%(len(same),q.get('net'),q.get('layer')))
                    q['points']=same[0]['outline']
                pts=[[float(a),float(b)] for a,b in q.get('points',[])]
                hit=next((e for e in have if e['net']==q.get('net') and e['layer']==eagle_layer(q.get('layer',1),board) and same_outline(e['outline'],pts)),None)
                if not hit: fail('unknown_pour','No %s pour on layer %s with that outline'%(q.get('net'),q.get('layer')))
                own=_pour_pick(hit,have)
                if not own: fail('unsafe_pick','Every edge of the %s pour on layer %d lies along another pour, so a change could land on the wrong one'%(hit['net'],hit['layer']))
                at=own[0]
                if q.get('newNet') is not None and q['newNet']!=hit['net']:
                    # a polygon belongs to its signal, so a pour moves net by being drawn again: same outline, same settings
                    keep={'width':q.get('width',hit['width']),'isolate':q.get('isolate',hit['isolate']),'rank':q.get('rank',hit['rank']),
                          'thermals':q.get('thermals',hit['thermals']),'orphans':q.get('orphans',hit['orphans'])}
                    cmd=(_pour_delete_cmd(hit,[p_['at'] for p_ in objects(board)[1].values()],have)+' '+_pour_draw_cmd(q['newNet'],hit['layer'],hit['outline'],keep,hit.get('pour','solid')))
                    attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd); targets.append((q,hit)); continue
                cmd='LAYER %d;'%hit['layer']
                if q.get('width') is not None: cmd+=' CHANGE WIDTH %.6fmm (%.6fmm %.6fmm);'%(number(q['width'],True),*at)
                if q.get('isolate') is not None: cmd+=' CHANGE ISOLATE %.6fmm (%.6fmm %.6fmm);'%(number(q['isolate']),*at)
                if q.get('thermals') is not None: cmd+=' CHANGE THERMALS %s (%.6fmm %.6fmm);'%('ON' if q['thermals'] else 'OFF',*at)
                if q.get('rank') is not None: cmd+=' CHANGE RANK %d (%.6fmm %.6fmm);'%(int(q['rank']),*at)
                if q.get('orphans') is not None: cmd+=' CHANGE ORPHANS %s (%.6fmm %.6fmm);'%('ON' if q['orphans'] else 'OFF',*at)
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd); targets.append((q,hit))
            app.executeTextCommand('Electron.run RATSNEST;')
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            _,after,after_meta=snapshot(app,args); now=pours(after)
            bad=[]; changes=[]
            for q,hit in targets:
                moved=q.get('newNet') is not None and q['newNet']!=hit['net']
                n=next((e for e in now if e['net']==(q['newNet'] if moved else hit['net']) and e['layer']==hit['layer'] and same_outline(e['outline'],hit['outline'])),None)
                if moved and any(e['net']==hit['net'] and e['layer']==hit['layer'] and same_outline(e['outline'],hit['outline']) for e in now): n=None
                ok=n is not None and (q.get('width') is None or abs(n['width']-q['width'])<1e-4) and (q.get('isolate') is None or abs(n['isolate']-q['isolate'])<1e-4) \
                    and (q.get('thermals') is None or n['thermals']==bool(q['thermals'])) and (q.get('rank') is None or n['rank']==int(q['rank'])) and (q.get('orphans') is None or n['orphans']==bool(q['orphans']))
                changes.append({'net':hit['net'],'newNet':q['newNet'] if moved else None,'layer':hit['layer'],'width':[hit['width'],n and n['width']],'isolate':[hit['isolate'],n and n['isolate']],
                                'thermals':[hit['thermals'],n and n['thermals']],'rank':[hit['rank'],n and n['rank']],'orphans':[hit['orphans'],n and n['orphans']],'filledMm2':[hit['filledMm2'],n and n['filledMm2']]})
                if not ok: bad.append(hit['net'])
            other=copper_signature(objects(after)[2]+objects(after)[3])!=before_cu
            # the pours nobody asked about are as they were, so a pick that took a neighbour shows
            asked={id(h) for _,h in targets}
            sett=lambda e:(e['id'],round(e['isolate'],4),round(e['width'],4),e['rank'],e['thermals'],e['orphans'])
            rest_before=sorted(sett(e) for e in have if id(e) not in asked)
            rest_now=sorted(sett(e) for e in now if not any(e['layer']==h['layer'] and same_outline(e['outline'],h['outline']) for _,h in targets))
            if rest_before!=rest_now: bad.append('another pour changed')
            okall=not bad and not other
            if not okall:
                undone=_undo_until(app,args,meta['revision'],len(commands)+2)
                _,back,back_meta=snapshot(app,args)
                return {'success':False,'mutated':not undone,'verified':False,'errorCode':'readback_mismatch','undone':undone,
                        'error':('tracks or vias changed' if other else 'pours not as asked: %s'%bad)+('; it was undone' if undone else '; the undo did not restore them: read fusion_pour_state'),
                        'changes':changes,'commands':commands,**pour_state(back),**state(back,back_meta)}
            return {'success':okall,'mutated':True,'verified':okall,'errorCode':None if okall else 'readback_mismatch',
                    'error':None if okall else ('tracks or vias changed' if other else 'pours not changed: %s'%bad),
                    'changes':changes,'commands':commands,**pour_state(after),**state(after,after_meta)}
        if action=='remove_pours':
            # delete pours (all, or the ones named) so a board can be poured again. DELETE picks the
            # pour on the outline point farthest from every pad, so it can never take a part with it;
            # the readback refuses (and undoes) if a part or a track changed
            have=pours(board); _,pads_,ws_,vs_=objects(board)
            want=args.get('pours')
            if want in (None,'all'): targets=have
            else:
                ids=[q['id'] for q in want if q.get('id')]
                unknown=[i for i in ids if not any(e['id']==i for e in have)]
                if unknown: fail('unknown_pour','No pour with id %s: fusion_pour_state lists them'%', '.join(unknown))
                targets=[e for e in have if e['id'] in ids or any(not q.get('id') and e['net']==q.get('net') and e['layer']==eagle_layer(q.get('layer',1),board) and same_outline(e['outline'],[[float(a),float(b)] for a,b in q.get('points',[])]) for q in want)]
            if not targets: return {'success':True,'mutated':False,'removed':0,**meta,'note':'no pour to remove'}
            elements_before=len(board.findall('elements/element')); cu_before=copper_signature(ws_+vs_)
            pad_pts=[p_['at'] for p_ in pads_.values()]
            still=list(have)
            cmds_=[]
            for e in targets:
                cmds_.append(_pour_delete_cmd(e,pad_pts,still))   # all checked before anything is deleted
                still.remove(e)
            for cmd in cmds_:
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd)
            app.executeTextCommand('Electron.run RATSNEST;')
            _,after,after_meta=snapshot(app,args)
            _,_,ws2,vs2=objects(after)
            if len(after.findall('elements/element'))!=elements_before or copper_signature(ws2+vs2)!=cu_before:
                for _ in commands: app.executeTextCommand('Electron.run UNDO;')
                fail('readback_mismatch','Removing pours changed parts or tracks; every DELETE was undone')
            # by id: a count per net and layer can't see that the other pour went instead of the one asked for
            from collections import Counter
            expect=Counter(e['id'] for e in have)-Counter(t['id'] for t in targets); now_=Counter(e['id'] for e in pours(after))
            if expect!=now_:
                undone=_undo_until(app,args,meta['revision'],len(commands)+2)
                r_,back,back_meta=snapshot(app,args)
                return {'success':False,'mutated':not undone,'verified':False,'errorCode':'readback_mismatch','undone':undone,'removed':0,'commands':commands,
                        'removedByMistake':sorted((expect-now_).elements()),'stillThere':sorted((now_-expect).elements()),
                        'error':'Fusion did not remove exactly the pours asked for; %s'%('it was undone, so the board is as it was' if undone else 'the undo did not restore them: read fusion_pour_state'),**pour_state(back),**state(back,back_meta)}
            left=[]
            okp=True
            return {'success':okp,'mutated':True,'verified':okp,'errorCode':None if okp else 'readback_mismatch','removed':len(targets)-sum(x['count'] for x in left),
                    'left':left,'commands':commands,**pour_state(after),**state(after,after_meta)}
        if action=='ripup_nets':
            # take a net's tracks and vias up so it can be routed again (wider, or another way);
            # its pours stay. One RIPUP per net, each its own undo step.
            want=args.get('nets')
            if not isinstance(want,list) or not want: fail('invalid_args','nets must be a non-empty list of net names')
            nets_before=objects(board)[0]; pours_before=len(pours(board))
            filled_before={(e['net'],e['layer'],tuple(map(tuple,e['outline']))) for e in pours(board) if e['filledMm2']>0}
            for n in want:
                n=identifier(n)
                if n not in nets_before: fail('unknown_net','No exact net named '+n)
                cmd="RIPUP '%s';"%n
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd)
            # RIPUP by name also hides the net's pour fills (fillvisible=no), and RATSNEST will not
            # refill a hidden pour: show all fills again, then refill, then prove the fill is back
            app.executeTextCommand('Commands.Start Electron::PolyShowFillAll')
            app.executeTextCommand('Electron.run RATSNEST;')
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            _,after,after_meta=snapshot(app,args); _,_,ws,vs=objects(after)
            left=sorted({o['net'] for o in ws+vs if o['net'] in want})
            lost=pours_before-len(pours(after))
            unfilled=sorted({k[0] for k in filled_before if not any((e['net'],e['layer'],tuple(map(tuple,e['outline'])))==k and e['filledMm2']>0 for e in pours(after))})
            ok=not left and lost==0 and not unfilled
            return {'success':ok,'mutated':True,'verified':ok,'errorCode':None if ok else 'readback_mismatch',
                    'error':None if ok else 'copper left on %s, %d pour(s) lost, fill not restored on %s'%(left,lost,unfilled),'ripped':want,
                    '_hint':None if ok else 'If a pour lost its fill, run Fusion\'s Re-fill Polygon Pours (Electron::PolyShowFillAll, then RATSNEST) and read fusion_pour_state.','commands':commands,**state(after,after_meta)}
        if action=='place_vias':
            want=args.get('vias')
            if not isinstance(want,list) or not want: fail('invalid_args','vias must be a non-empty list of {net,x,y,diameter (or size),drill}')
            for v in want:
                cmd="CHANGE DRILL %.6fmm; VIA '%s' %.6fmm ROUND 1-16 (%.6fmm %.6fmm);"%(number(v.get('drill',0.3),True),identifier(v.get('net')),number(v.get('diameter',v.get('size',0.6)),True),number(v.get('x')),number(v.get('y')))
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd)
            app.executeTextCommand('Electron.run RATSNEST;')
            _,after,after_meta=snapshot(app,args); vs=objects(after)[3]
            missing=[v for v in want if not any(o['net']==identifier(v.get('net')) and abs(o['at'][0]-number(v.get('x')))<0.001 and abs(o['at'][1]-number(v.get('y')))<0.001 for o in vs)]
            return {'success':not missing,'mutated':True,'verified':not missing,'errorCode':None if not missing else 'readback_mismatch','missing':missing,'commands':commands,**state(after,after_meta)}
        if action=='apply_pours':
            # draw each planned pour that is not already on the board (same net, layer and outline),
            # then RATSNEST fills them; the reply is Fusion's own fill per pour
            want=args.get('pours')
            if not isinstance(want,list) or not want: fail('invalid_args','pours must be a non-empty list of {net,layer,points,isolate,rank}')
            existing=pours(board); added=[]; kept=[]
            cutouts=_cutout_rects(board,args.get('cutouts'))
            for c_ in cutouts:
                for b_ in range(0,len(c_['rects']),60):
                    cmd='LAYER %d; %s'%(c_['restrictLayer'],' '.join('RECT R0 (%.4fmm %.4fmm) (%.4fmm %.4fmm);'%r_ for r_ in c_['rects'][b_:b_+60]))
                    attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd)
            for p in want:
                # the standard's names: order is the pour order (1 first), connect is how pads join it
                if p.get('rank') is None and p.get('order') is not None: p['rank']=p['order']
                if p.get('thermals') is None and p.get('connect') is not None:
                    if p['connect'] not in ('direct','relief'): fail('not_supported',"connect is direct or relief: Fusion's pour always joins pads of its own net")
                    p['thermals']=p['connect']=='relief'
                net=identifier(p.get('net')); layer=eagle_layer(p.get('layer',1),board)
                pts=[[number(a),number(b)] for a,b in p.get('points') or []]
                if len(pts)<3: fail('invalid_geometry','a pour needs at least three points')
                if any(e['net']==net and e['layer']==layer and same_outline(e['outline'],pts) for e in existing): kept.append(net); continue
                width=number(p.get('width',0.25),True); iso=number(p.get('isolate',0.25),True); rank=int(p.get('rank',1))
                if not 1<=rank<=6: fail('invalid_args','rank must be 1..6 (1 fills first)')
                path=' '.join('(%.6fmm %.6fmm)'%(x,y) for x,y in pts+[pts[0]])
                cmd=("CHANGE ISOLATE %.6fmm; CHANGE RANK %d; CHANGE POUR SOLID; CHANGE ORPHANS %s; CHANGE THERMALS %s; "
                     "LAYER %d; POLYGON '%s' %.6fmm %s;")%(iso,rank,'ON' if p.get('orphans') else 'OFF','OFF' if p.get('thermals') is False else 'ON',layer,net,width,path)
                attempted=True; app.executeTextCommand('Electron.run '+cmd); commands.append(cmd); added.append(net)
            app.executeTextCommand('Electron.run RATSNEST;')
            app.executeTextCommand('Electron.ImmediateRepaint /full')
            _,after,after_meta=snapshot(app,args); ps=pour_state(after)
            missing=[p['net'] for p in want if not any(e['net']==p['net'] and e['layer']==eagle_layer(p.get('layer',1),board) and same_outline(e['outline'],[[float(a),float(b)] for a,b in p['points']]) for e in ps['pours'])]
            return {'success':not missing,'mutated':bool(added or cutouts),'verified':not missing,'errorCode':None if not missing else 'readback_mismatch',
                    'added':added,'kept':kept,'missing':missing,'cutouts':[{'layer':c_['layer'],'restrictLayer':c_['restrictLayer'],'rects':len(c_['rects'])} for c_ in cutouts],
                    'commands':commands,**ps,**state(after,after_meta)}
        if action!='route_net': fail('unknown_action','Unknown routing action')
        candidate=plan(board,args)
        if 'dryRun' in args and not isinstance(args['dryRun'],bool): fail('invalid_args','dryRun must be boolean')
        if args.get('dryRun') is True: return {'success':True,'mutated':False,**meta,'plan':candidate}
        before=objects(board); before_items=copper_signature(before[2]+before[3])
        # nets already in pieces before the route (a pour cut by new copper splits its net)
        split_before={x['net'] for x in state(board,meta).get('splitNets') or []}
        # Every emitted edit is complete (no mouse-bound interactive operation).
        # Explicit mm coordinates/width suffixes avoid changing the document grid.
        for item in candidate['items']:
            if item['kind']=='segment':
                a,b=item['start'],item['end']
                # SET WIRE_BEND 2 = straight: the default bend style splits an angled
                # segment into a 45-degree dogleg, so the copper is not what was planned.
                command="SET WIRE_BEND 2; LAYER %d; LINE '%s' %.6fmm (%.9fmm %.9fmm) (%.9fmm %.9fmm);"%(item['layer'],candidate['net'],item['width'],*a,*b)
            else:
                command="CHANGE DRILL %.6fmm; VIA '%s' %.6fmm ROUND 1-16 (%.9fmm %.9fmm);"%(item['drill'],candidate['net'],item['diameter'],*item['at'])
            attempted=True
            app.executeTextCommand('Electron.run '+command)
            commands.append(command)
        app.executeTextCommand('Electron.run RATSNEST;')
        app.executeTextCommand('Electron.ImmediateRepaint /full')
        _,after,after_meta=snapshot(app,args)
        after_objs=objects(after); actual=copper_signature(after_objs[2]+after_objs[3])
        expected=copper_signature(before[2]+before[3]+candidate['items'])
        # Compare full copper coverage: a branch may split an existing straight wire.
        verified=actual==expected
        after_state=state(after,after_meta)
        # Pours are not obstacles to a plan: Fusion repours round the new copper, and a track that
        # cuts another net's pour in two disconnects that net (BQ25792's VBAT, 2026-10-28). Undo it.
        split_new=sorted({x['net'] for x in after_state.get('splitNets') or []}-split_before-{candidate['net']})
        if verified and split_new and not args.get('allowSplit'):
            undone,back,back_meta=undo_route(app,args,commands,before_items)
            return {'success':False,'mutated':not undone,'verified':undone,'errorCode':'splits_net','splitNets':split_new,
                    'error':'The route cuts %s in two (a pour split by the new copper)%s'%(', '.join(split_new),'; it was undone' if undone else '; the undo did not restore the copper exactly: read fusion_routing_state'),
                    '_hint':'Plan it on another layer (fusion_plan_route layers:[...]), or around the pour; allowSplit:true keeps a route that splits a net.',
                    'commands':commands,'plan':candidate,**state(back,back_meta)}
        if not verified:
            # a route that did not land as planned comes off again, so a retry starts from the board it planned on
            vias_missing=[v for v in expected[1] if v not in actual[1]]; tracks_same=actual[0]==expected[0]
            undone,back,back_meta=undo_route(app,args,commands,before_items)
            return {'success':False,'mutated':not undone,'verified':False,'errorCode':'readback_mismatch','undone':undone,
                    'viasNotLanded':len(vias_missing),'tracksAsPlanned':tracks_same,
                    'error':'Fusion\'s copper was not the planned route%s'%('; it was undone, so the board is as it was' if undone else '; the undo did not restore the copper exactly: read fusion_routing_state before retrying'),
                    'commands':commands,'plan':candidate,**state(back,back_meta)}
        return {'success':True,'mutated':actual!=before_items,'verified':True,'errorCode':None,'error':None,
                'commands':commands,'plan':candidate,**after_state}
    except Exception as exc:
        return {'success':False,'errorCode':getattr(exc,'code','routing_failed'),'error':str(exc),
                'mutationAttempted':attempted,'completedCommands':commands,
                '_hint':'Read fusion_routing_state before retrying. No automatic replay or blind Undo is performed.'}
