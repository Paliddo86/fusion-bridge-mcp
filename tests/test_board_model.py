"""The board model (rust/bridge/src/board_model.py) against the EDA Bridge Standard's schema and
its rules, on the demo boards. Offline: python3 -m unittest tests.test_board_model"""
import copy
import importlib.util
import json
import math
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('routing', ROOT / 'rust/bridge/src/routing_script.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
exec((ROOT / 'rust/bridge/src/routing_planner.py').read_text(), r.__dict__)
exec((ROOT / 'rust/bridge/src/board_model.py').read_text(), r.__dict__)
SCHEMA = json.loads((ROOT / 'contracts/eda-bridge-standard/board.schema.json').read_text())
COMPANIONS = json.loads((ROOT / 'contracts/eda-bridge-standard/companions.schema.json').read_text())
DEMO = ROOT / 'demo/routing'
BQ = DEMO / 'bq25792/thermal-reroute/candidate-01/filled.brd'
COPPER_NAMES = {'F.Cu', 'B.Cu', '*.Cu'} | {'In%d.Cu' % i for i in range(1, 31)}


def board(path):
    return ET.parse(path).getroot().find('drawing/board')


def validate(value, schema, root, where='$'):
    """The subset of JSON Schema the standard uses: $ref, type, const, enum, required, properties,
    items, minItems, maxItems."""
    if '$ref' in schema:
        node = root
        for part in schema['$ref'].lstrip('#/').split('/'):
            node = node[part]
        return validate(value, node, root, where)
    t = schema.get('type')
    types = {'object': dict, 'array': list, 'string': str, 'number': (int, float), 'integer': int, 'boolean': bool, 'null': type(None)}
    if t:
        allowed = t if isinstance(t, list) else [t]
        ok = any(isinstance(value, types[x]) and not (x in ('number', 'integer') and isinstance(value, bool)) for x in allowed)
        assert ok, '%s: %r is not %s' % (where, value if not isinstance(value, (dict, list)) else type(value).__name__, t)
    if 'const' in schema:
        assert value == schema['const'], '%s: %r != %r' % (where, value, schema['const'])
    if 'enum' in schema:
        assert value in schema['enum'], '%s: %r not in %r' % (where, value, schema['enum'])
    if isinstance(value, dict):
        for k in schema.get('required', []):
            assert k in value, '%s: missing %s' % (where, k)
        for k, sub in schema.get('properties', {}).items():
            if k in value:
                validate(value[k], sub, root, where + '.' + k)
    if isinstance(value, list):
        assert len(value) >= schema.get('minItems', 0) and len(value) <= schema.get('maxItems', 10 ** 9), where
        if 'items' in schema:
            for i, v in enumerate(value):
                validate(v, schema['items'], root, '%s[%d]' % (where, i))


class BoardModelTests(unittest.TestCase):
    def test_every_demo_board_matches_the_schema(self):
        for f in sorted(DEMO.glob('**/*.brd')):
            with self.subTest(board=f.name):
                model, _ = r.board_model(board(f))
                validate(model, SCHEMA, SCHEMA)
                for kind in ('tracks', 'arcs', 'pads', 'polygons'):
                    for item in model[kind]:
                        self.assertTrue(item['layer'] in COPPER_NAMES, '%s layer %s' % (kind, item['layer']))
                for v in model['vias']:
                    self.assertTrue(set(v['layers']) <= COPPER_NAMES)

    def test_routed_boards_are_connected_and_unrouted_ones_count_what_is_left(self):
        expect = {'crossbar/take-01/fusion-crossbar-24-routed.brd': 0, 'crossbar/fusion-crossbar-24.brd': 48,
                  'fixture/fusion-routing.brd': 7, 'evidence/boards/fusion-routing-routed.brd': 0,
                  'obstacles/take-04/fusion-obstacle-live-routed.brd': 0, 'bq25792/native-routed/bq25792-astra-routed.brd': 0,
                  'bq25792/thermal-reroute/candidate-01/filled.brd': 0, 'bq25792/ablation/take-02/bq25792-ablation-routed.brd': 0}
        for f, n in expect.items():
            with self.subTest(board=f):
                model, _ = r.board_model(board(DEMO / f))
                self.assertEqual(n, model['unconnected'], model['splitNets'])

    def test_a_split_net_is_found_when_a_track_is_cut(self):
        b = board(DEMO / 'crossbar/take-01/fusion-crossbar-24-routed.brd')
        sig = b.find('signals/signal')
        sig.remove(sig.find('wire'))
        model, _ = r.board_model(b)
        self.assertEqual(1, model['unconnected'])
        self.assertEqual([{'net': sig.get('name'), 'groups': 2}], model['splitNets'])

    def test_isolated_copper_is_listed_not_counted(self):
        # standard 0.1.1: a group of only poured copper is isolatedCopper; a stray track still counts
        b = board(DEMO / 'bq25792/ablation/take-02/bq25792-ablation-routed.brd')
        model, _ = r.board_model(b)
        self.assertEqual(0, model['unconnected'])
        iso = model['isolatedCopper']
        self.assertGreater(len(iso), 10)
        one = iso[0]
        self.assertEqual({'net', 'layer', 'at', 'areaMm2'}, set(one))
        self.assertGreater(one['areaMm2'], 0)
        self.assertEqual(len(iso), model['floatingPourIslandCount'])
        b2 = board(DEMO / 'crossbar/take-01/fusion-crossbar-24-routed.brd')
        sig = b2.find('signals/signal')
        ET.SubElement(sig, 'wire', x1='1', y1='1', x2='2', y2='1', width='0.2', layer='1')   # a stray track
        self.assertEqual(1, r.board_model(b2)[0]['unconnected'])

    def test_custom_shape_pads_are_their_bounding_rectangle(self):
        # the power FET's drain is one footprint polygon joining pads 5..9 (0.127 mm stubs)
        model, _ = r.board_model(board(BQ))
        q1 = {p['name']: p for p in model['pads'] if p['ref'] == 'Q1'}
        for n in '56789':
            self.assertTrue(q1[n].get('customShape'))
            self.assertEqual('rect', q1[n]['shape'])
            self.assertEqual(0, q1[n]['rotation'])
            self.assertGreater(q1[n]['sizeX'] * q1[n]['sizeY'], 1.0)
        self.assertFalse(q1['1'].get('customShape'))
        for f in DEMO.glob('**/*.brd'):
            for p in r.board_model(board(f))[0]['pads']:
                self.assertGreater(p['sizeX'], 0, (f.name, p))
                self.assertGreater(p['sizeY'], 0, (f.name, p))

    def test_copper_layers_follow_the_stack_order(self):
        model, _ = r.board_model(board(BQ))
        self.assertEqual(['F.Cu', 'In1.Cu', 'In2.Cu', 'B.Cu'], model['copperLayers'])
        self.assertEqual({'F.Cu', 'In1.Cu', 'In2.Cu', 'B.Cu'}, {g['layer'] for g in model['polygons'] if g['net'] == 'PGND'})
        self.assertTrue(all(v['layers'] == ['F.Cu', 'B.Cu'] for v in model['vias']))

    def test_netless_copper_is_kept_with_an_empty_net(self):
        b = board(DEMO / 'fixture/fusion-routing.brd')
        ET.SubElement(b.find('plain'), 'wire', x1='1', y1='1', x2='3', y2='1', width='0.3', layer='16')
        ET.SubElement(b.find('plain'), 'rectangle', x1='4', y1='4', x2='6', y2='5', layer='1')
        model, _ = r.board_model(b)
        self.assertIn({'start': {'x': 1, 'y': 1}, 'end': {'x': 3, 'y': 1}, 'width': 0.3, 'layer': 'B.Cu', 'layerId': 16, 'net': ''},
                      [{k: t[k] for k in ('start', 'end', 'width', 'layer', 'layerId', 'net')} for t in model['tracks']])
        self.assertTrue(any(g['net'] == '' and g['layer'] == 'F.Cu' for g in model['polygons']))
        self.assertEqual(7, model['unconnected'])     # free copper is not a connection to make

    def test_curved_copper_is_an_arc(self):
        b = board(DEMO / 'fixture/fusion-routing.brd')
        sig = b.find('signals/signal')
        ET.SubElement(sig, 'wire', x1='10', y1='0', x2='0', y2='10', width='0.25', layer='1', curve='90')
        model, _ = r.board_model(b)
        a = model['arcs'][0]
        self.assertAlmostEqual(0, a['center']['x'], 6)
        self.assertAlmostEqual(0, a['center']['y'], 6)
        self.assertAlmostEqual(10, a['radius'], 6)
        self.assertEqual((0, 90), (a['startAngle'], a['endAngle']))
        ET.SubElement(sig, 'wire', x1='0', y1='10', x2='10', y2='0', width='0.25', layer='1', curve='-90')
        a = r.board_model(b)[0]['arcs'][1]
        self.assertEqual((0, 90), (a['startAngle'], a['endAngle']))      # the same arc walked clockwise

    def test_outline_keeps_its_arcs(self):
        b = board(DEMO / 'fixture/fusion-routing.brd')
        plain = b.find('plain')
        for w in plain.findall("wire[@layer='20']"):
            plain.remove(w)
        # a 40 x 20 board with 2 mm rounded corners, one corner drawn backwards
        for a in [('2', '0', '38', '0', '0'), ('40', '2', '38', '0', '-90'), ('40', '2', '40', '18', '0'), ('40', '18', '38', '20', '90'),
                  ('38', '20', '2', '20', '0'), ('2', '20', '0', '18', '90'), ('0', '18', '0', '2', '0'), ('0', '2', '2', '0', '90')]:
            ET.SubElement(plain, 'wire', x1=a[0], y1=a[1], x2=a[2], y2=a[3], width='0', layer='20', curve=a[4])
        model, _ = r.board_model(b)
        arcs = [e for e in model['outlineEdges'] if e['kind'] == 'arc']
        self.assertEqual(4, len(arcs))
        self.assertTrue(model['outlineClosed'])
        self.assertEqual({2}, {round(e['radius'], 6) for e in arcs})
        self.assertIn({'kind': 'arc', 'start': [38, 0], 'end': [40, 2], 'center': [38, 2], 'radius': 2, 'angle1': 270, 'angle2': 360}, arcs)
        # the vertex list samples each arc at 2 degrees or finer and stays on the arc
        for p in model['outline']:
            self.assertTrue(-1e-6 <= p[0] <= 40 + 1e-6 and -1e-6 <= p[1] <= 20 + 1e-6)
        self.assertEqual(4 + 4 * 45, len(model['outline']))

    def test_unclosed_outline_says_so(self):
        b = board(DEMO / 'fixture/fusion-routing.brd')
        plain = b.find('plain')
        plain.remove(plain.findall("wire[@layer='20']")[0])
        model, _ = r.board_model(b)
        self.assertFalse(model['outlineClosed'])
        self.assertIn('not closed', model['outlineProblem'])

    def test_revision_moves_with_copper_parts_and_outline_only(self):
        base = board(DEMO / 'fixture/fusion-routing.brd')
        rev = r.board_revision(base)
        self.assertEqual(rev, r.board_revision(copy.deepcopy(base)))

        def changed(edit):
            b = copy.deepcopy(base)
            edit(b)
            return r.board_revision(b) != rev
        self.assertTrue(changed(lambda b: ET.SubElement(b.find('signals/signal'), 'wire', x1='1', y1='1', x2='2', y2='1', width='0.2', layer='1')))
        self.assertTrue(changed(lambda b: b.find('elements/element').set('x', '1.5')))
        self.assertTrue(changed(lambda b: b.find("plain/wire[@layer='20']").set('x1', '0.5')))
        self.assertFalse(changed(lambda b: ET.SubElement(b.find('plain'), 'text', x='1', y='1', size='1', layer='21').__setattr__('text', 'silk')))
        self.assertFalse(changed(lambda b: ET.SubElement(b.find('signals/signal'), 'wire', x1='1', y1='1', x2='2', y2='1', width='0', layer='19')))
        self.assertFalse(changed(lambda b: b.find("designrules/param[@name='layerSetup']").set('value', '(1*16)') if b.find('designrules/param') is not None else ET.SubElement(b, 'designrules')))

    def test_state_keeps_the_keys_the_ai_flow_reads(self):
        b = board(DEMO / 'evidence/boards/fusion-routing-routed.brd')
        st = r.state(b, {'boardId': 'B', 'documentName': 'fusion-routing', 'revision': r.board_revision(b)})
        validate(st, SCHEMA, SCHEMA)
        for k in ('segments', 'elements', 'airwireCount', 'remainingNets', 'segmentCount', 'viaCount'):
            self.assertIn(k, st)
        p = st['pads'][0]
        self.assertEqual(p['ref'] + '.' + p['name'], p['key'])
        self.assertEqual([p['x'], p['y']], [round(c, 6) for c in p['at']])
        v = st['vias'][0]
        self.assertEqual([v['position']['x'], v['position']['y']], v['at'])
        self.assertTrue(all(isinstance(n, str) for n in st['nets']))
        self.assertEqual(6, len(st['netRows']))
        self.assertEqual('eda-bridge-standard/board 0.1', st['model'])

    def test_planner_and_model_agree_on_every_pad_position(self):
        for f in DEMO.glob('**/*.brd'):
            b = board(f)
            _, pads, _, _ = r.objects(b)
            model, std = r.board_model(b)
            for key, p in pads.items():
                s = std[key]
                if s.get('customShape') or s.get('holeAt'):
                    continue
                self.assertAlmostEqual(p['at'][0], s['x'], 5, (f.name, key))
                self.assertAlmostEqual(p['at'][1], s['y'], 5, (f.name, key))

    def test_layer_stack_from_the_layer_stack_manager(self):
        st = r.layer_stack(board(BQ))
        validate(st, COMPANIONS['$defs']['stack'], COMPANIONS)
        self.assertEqual(4, st['copperLayers'])
        self.assertEqual(['F.Cu', 'In1.Cu', 'In2.Cu', 'B.Cu'], [l['name'] for l in st['layers']])
        top = st['layers'][0]
        self.assertEqual((35, 200, 4500), (top['copperUm'], top['dielectricUm'], top['dk1000']))
        self.assertEqual(1065, st['layers'][1]['dielectricUm'])
        self.assertNotIn('dielectricUm', st['layers'][-1])
        self.assertAlmostEqual(35 + 200 + 17.5 + 1065 + 17.5 + 200 + 35, st['totalUm'], 3)

    def test_layer_stack_from_the_rules_says_what_it_lacks(self):
        st = r.layer_stack(board(DEMO / 'fixture/fusion-routing.brd'))
        validate(st, COMPANIONS['$defs']['stack'], COMPANIONS)
        self.assertEqual(['F.Cu', 'B.Cu'], [l['name'] for l in st['layers']])
        self.assertTrue(st['notes'])

    def test_no_name_is_shared_with_the_routing_scripts(self):
        # the three files run as ONE script inside Fusion: a shared top-level name silently
        # replaces the other file's function
        import ast
        def names(f):
            body = ast.parse((ROOT / 'rust/bridge/src' / f).read_text()).body
            return {n.name for n in body if isinstance(n, (ast.FunctionDef, ast.ClassDef))} | \
                   {t.id for n in body if isinstance(n, ast.Assign) for t in n.targets if isinstance(t, ast.Name)}
        self.assertEqual(set(), (names('routing_script.py') | names('routing_planner.py')) & names('board_model.py'))

    def test_pours_model_and_netlist(self):
        zs = r.zones(board(BQ))
        validate({'zones': zs}, COMPANIONS['$defs']['pours'], COMPANIONS)
        pgnd = [z for z in zs if z['net'] == 'PGND' and z['layer'] == 'In1.Cu'][0]
        self.assertTrue(pgnd['filled'])
        self.assertGreater(pgnd['filledMm2'], 100)
        self.assertTrue(any(f['holes'] for f in pgnd['fills']))
        nl = r.netlist(board(DEMO / 'fixture/fusion-routing.brd'))
        validate(nl, COMPANIONS['$defs']['netlist'], COMPANIONS)
        self.assertEqual(13, sum(len(v) for v in nl.values()))
        self.assertTrue(all('.' in m for v in nl.values() for m in v))



class BoardReadTests(unittest.TestCase):
    """The standard's read verbs served from the same export (rd_* in board_model.py)."""
    root = ET.parse(BQ).getroot()
    b = root.find('drawing/board')

    def test_component_data_and_pins(self):
        one = r.rd_component_data(self.root, self.b, {'designator': 'IC1'})['component']
        self.assertEqual(('BQ25792RQMR', 'top', 'F.Cu', 35.2, 13.2), (one['pattern'], one['side'], one['layer'], one['xMm'], one['yMm']))
        self.assertGreater(one['pads'], 20)
        # the box holds every pad (IC1's documentation outline makes it 6.5 mm for a 4 mm chip)
        x0, y0, x1, y1 = one['box']
        for p in r.rd_component_pins(self.root, self.b, {'designator': 'IC1'})['pins']:
            self.assertTrue(x0 <= p['x'] <= x1 and y0 <= p['y'] <= y1, p)
        self.assertEqual('BQ25792RQMR', one['partNumber'])
        everything = r.rd_component_data(self.root, self.b, {})
        self.assertEqual(109, everything['count'])
        self.assertEqual(['bottom'], [c['side'] for c in everything['components'] if c['designator'] == 'U$5'])
        pins = r.rd_component_pins(self.root, self.b, {'designator': 'Q1'})
        self.assertEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', 'P$1'], [p['name'] for p in pins['pins']][:10])
        with self.assertRaises(r.RoutingError) as e:
            r.rd_component_pins(self.root, self.b, {'designator': 'NOPE'})
        self.assertEqual('designator_not_found', e.exception.code)

    def test_nets_connections_and_designators(self):
        nets = r.rd_list_nets(self.root, self.b, {})
        self.assertEqual(50, nets['count'])
        pgnd = [n for n in nets['nets'] if n['name'] == 'PGND'][0]
        self.assertEqual(4, pgnd['pours'])
        nc = r.rd_net_connections(self.root, self.b, {'net': '!int'})
        self.assertEqual({'IC1.21', 'MP9.P$1', 'R19.2'}, {p['pad'] for p in nc['pads']})
        self.assertGreater(nc['nets'][0]['airlineMm'], 0)
        self.assertEqual(3.0, round(r.airline([(0, 0), (1, 0), (1, 2)]), 6))
        d = r.rd_list_designators(self.root, self.b, {})['designators']
        self.assertEqual(109, len(d))
        self.assertLess(d.index('C2'), d.index('C11'))     # natural order

    def test_layers(self):
        cu = r.rd_copper_layers(self.root, self.b, {})
        self.assertEqual(['F.Cu', 'In1.Cu', 'In2.Cu', 'B.Cu'], [l['name'] for l in cu['layers']])
        self.assertTrue(all(l['primitives'] > 0 for l in cu['layers']))
        ls = r.rd_layers(self.root, self.b, {})
        kinds = {l['name']: l['kind'] for l in ls['layers']}
        self.assertEqual('copper', kinds['In2.Cu'])
        self.assertEqual('silkscreen', kinds['SilkscreenTop'])     # Fusion's own layer names
        self.assertNotIn('In3.Cu', kinds)                   # not in this board's stack
        used = r.rd_layers_in_use(self.root, self.b, {})
        self.assertEqual({'F.Cu', 'In1.Cu', 'In2.Cu', 'B.Cu'}, {l['name'] for l in used['copperLayersInUse']})

    def test_rules_classes_and_silk(self):
        rr = r.rd_read_rules(self.root, self.b, {})
        self.assertTrue(rr['ruleSet'].endswith('.edru'))
        self.assertIn('Teardrop', {x['kind'] for x in rr['rules']})
        self.assertEqual('0.1mm', rr['params']['mdWireWire'])
        by = r.rd_rules_by_kind(self.root, self.b, {})['rulesByKind']
        widths = by['Minimum Copper Width']
        self.assertEqual(sorted(x['priority'] for x in widths), [x['priority'] for x in widths])
        nc = r.rd_net_classes(self.root, self.b, {})
        self.assertEqual(['default', 'pwr'], [c['name'] for c in nc['classes']])
        self.assertEqual(0.09, nc['classes'][1]['widthMm'])
        silk = r.rd_silk_state(self.root, self.b, {})
        ic1 = [c for c in silk['components'] if c['ref'] == 'IC1'][0]
        self.assertEqual('IC1', ic1['name']['text'])
        self.assertEqual('BQ25792RQMR', ic1['comment']['text'])
        self.assertIsNotNone(ic1['body'])
        self.assertTrue(all(c['name']['text'] == c['ref'] for c in silk['components']))


class BomTests(unittest.TestCase):
    """adom/fusion-bridge#105 (n105): the BOM export carries each part's MPN, manufacturer and LCSC."""
    b = ET.parse(BQ).getroot().find('drawing/board')

    def test_part_ids_read_every_naming(self):
        def el(**attrs):
            e = ET.Element('element')
            for k, v in attrs.items():
                ET.SubElement(e, 'attribute', name=k, value=v)
            return e
        self.assertEqual({'mpn': 'KT-0603R', 'manufacturer': 'Hubei KENTO Elec', 'lcsc': 'C2286', 'mouser': '', 'digikey': ''},
                         r.part_ids(el(MP='KT-0603R', MF='Hubei KENTO Elec', LCSC='C2286')))
        self.assertEqual('BQ25792RQMR', r.part_ids(el(MANUFACTURER_PART_NUMBER='BQ25792RQMR'))['mpn'])
        self.assertEqual('C1', r.part_ids(el(lcsc_pn='C1'))['lcsc'])
        self.assertEqual('X', r.part_ids(el(MPN='', MANUFACTURER_PART_NUMBER='X'))['mpn'])   # an empty one does not win

    def test_export_csv_carries_the_numbers_after_the_old_columns(self):
        import csv, io
        text, info = r.export_bom_csv(self.b, {})
        rows = list(csv.reader(io.StringIO(text)))
        self.assertEqual(['Comment', 'Designator', 'Footprint', 'Quantity', 'Library', 'MPN', 'Manufacturer', 'LCSC', 'Mouser', 'DigiKey'], rows[0])
        r2 = [x for x in rows if 'R2' in x[1].split()][0]
        self.assertEqual(('WSL2010R0100FEA18', 'Vishay-Dale', 'C844898'), (r2[5], r2[6], r2[7]))
        self.assertEqual(109, sum(int(x[3]) for x in rows[1:]))
        self.assertEqual(info['withLcsc'], 109 - len(info['missingLcsc']))
        text, info = r.export_bom_csv(self.b, {'grouped': False})
        rows = list(csv.reader(io.StringIO(text)))
        self.assertEqual(['Designator', 'Comment', 'Footprint', 'Library', 'Side', 'MPN', 'Manufacturer', 'LCSC', 'Mouser', 'DigiKey'], rows[0])
        self.assertEqual(110, len(rows))

if __name__ == '__main__':
    unittest.main()
