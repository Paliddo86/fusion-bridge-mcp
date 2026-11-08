import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('sch', ROOT / 'rust/bridge/src/sch_script.py')
s = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s)
FIXTURE = ROOT / 'demo/routing/fixture/fusion-routing.sch'


class SchReadTests(unittest.TestCase):
    def setUp(self):
        self.root = s.sch_root_from_file(str(FIXTURE))
        self.n, self.sheet = s.pick_sheet(self.root, {})

    def test_every_pin_sits_on_a_wire_end_of_its_own_segment(self):
        # The proof the transform is right: Fusion's ULP pin coordinates count the part's offset
        # twice (live 2026-10-27), but its wires do not, and a connected pin ends a wire.
        lib = s.Lib(self.root.find('drawing/schematic'))
        pins = {(p['ref'], p['gate'], p['pin']): p['at'] for p in s.sheet_pins(lib, self.sheet)}
        checked = 0
        for _, seg, _ in s.segments(self.sheet):
            ends = [(s._sf(w, 'x1'), s._sf(w, 'y1')) for w in seg.findall('wire')] + [(s._sf(w, 'x2'), s._sf(w, 'y2')) for w in seg.findall('wire')]
            for pr in seg.findall('pinref'):
                at = pins[(pr.get('part'), pr.get('gate'), pr.get('pin'))]
                self.assertTrue(any(abs(at[0] - x) < 1e-3 and abs(at[1] - y) < 1e-3 for x, y in ends), (pr.attrib, at))
                checked += 1
        self.assertEqual(13, checked)

    def test_items_and_ids(self):
        it = s.items(self.root, self.sheet)
        self.assertEqual(7, it['counts']['wires'])
        self.assertEqual(1, it['counts']['junctions'])
        w = it['wires'][0]
        self.assertEqual(s.wire_id(*w['from'], *w['to']), w['id'])
        self.assertEqual(s.wire_id(1, 2, 3, 4), s.wire_id(3, 4, 1, 2))

    def test_trace_point_and_nearest(self):
        r = s.trace_point(self.root, self.sheet, [7, 7], 0.01)
        self.assertEqual(['NET_1'], [n['name'] for n in r['nets']])
        self.assertIn('J1.1', r['nets'][0]['pins'])
        r = s.trace_point(self.root, self.sheet, [-50, -50], 0.01)
        self.assertEqual([], r['items'])
        self.assertIsNotNone(r['nearest'])

    def test_pin_side_follows_turn_and_mirror(self):
        self.assertEqual('left', s.pin_side('R0', 'R0'))
        self.assertEqual('right', s.pin_side('R0', 'MR0'))
        self.assertEqual('bottom', s.pin_side('R0', 'R90'))
        self.assertEqual('top', s.pin_side('R90', 'R180'))

    def test_sheet_choice(self):
        self.assertEqual(1, s.pick_sheet(self.root, {'sheet': '1'})[0])
        with self.assertRaises(s.SchError):
            s.pick_sheet(self.root, {'sheet': 2})


class SchEditPlanTests(unittest.TestCase):
    def setUp(self):
        self.root = s.sch_root_from_file(str(FIXTURE))
        self.n, self.sheet = s.pick_sheet(self.root, {})

    def test_route_turns_once_along_the_first_leg(self):
        self.assertEqual([(0, 0), (10, 0), (10, 5)], s.route_points((0, 0), (10, 5), 'h', None))
        self.assertEqual([(0, 0), (0, 5), (10, 5)], s.route_points((0, 0), (10, 5), 'v', None))
        self.assertEqual([(0, 0), (10, 0)], s.route_points((0, 0), (10, 0), 'v', None))
        self.assertEqual([(0, 0), (3, 3), (10, 3), (10, 5)], s.route_points((0, 0), (10, 5), 'h', [[3, 3]]))

    def test_pins_by_number_then_name(self):
        p = s.find_pin(self.root, self.sheet, 'J1.1')
        self.assertEqual([7.0, 7.0], p['at'])
        for bad in ('J1', 'ZZ9.1', 'J1.99'):
            with self.assertRaises(s.SchError):
                s.find_pin(self.root, self.sheet, bad)

    def test_cover_ignores_how_a_wire_is_split(self):
        import copy, xml.etree.ElementTree as ET
        sh = copy.deepcopy(self.sheet)
        seg = sh.find('nets/net/segment')
        w = seg.find('wire')
        x1, y1, x2, y2 = (float(w.get(k)) for k in ('x1', 'y1', 'x2', 'y2'))
        mx, my = (x1 + x2) / 2, (y1 + y2) / 2
        w.set('x2', str(mx)); w.set('y2', str(my))
        ET.SubElement(seg, 'wire', x1=str(mx), y1=str(my), x2=str(x2), y2=str(y2), width='0.1524', layer='91')
        self.assertEqual(s.wire_cover(self.sheet), s.wire_cover(sh))

    def test_ends_at_counts_a_t(self):
        w = self.sheet.find('nets/net/segment/wire')
        a = (float(w.get('x1')), float(w.get('y1')))
        self.assertGreaterEqual(s.ends_at(self.sheet, a), 1)
        self.assertEqual(0, s.ends_at(self.sheet, (-500, -500)))

    def test_every_drawn_junction_counts_as_needed(self):
        # the rule sch_delete_wire cleans up with: a junction joining a pin and two wires is needed
        # (counting wires only once removed 25 good junctions on BQ25792, live 2026-10-27)
        pp = s.pin_points(self.root, self.sheet)
        for q in s.junction_points(self.sheet):
            self.assertGreaterEqual(s.ends_at(self.sheet, q, pins=pp), 3, q)

    def test_bus_members(self):
        self.assertEqual(['D0', 'D1', 'D2'], s.bus_members('D[0..2]'))
        self.assertEqual(['A3', 'A2', 'CLK'], s.bus_members('A[3..2],CLK'))
        self.assertEqual(['SDA', 'SCL'], s.bus_members('{SDA SCL}'))

    def test_field_home_matches_the_fields_left_in_place(self):
        bq = Path('/home/adom/project/fusion-lane/bq25792-charger/bq25792-charger.sch')
        if not bq.exists():
            self.skipTest('BQ25792 fixture not on this machine')
        root = s.sch_root_from_file(str(bq))
        _, sh = s.pick_sheet(root, {})
        lib = s.Lib(root.find('drawing/schematic'))
        n = 0
        for inst in sh.findall('instances/instance'):
            for a in inst.findall('attribute'):
                h = s.field_home(lib, inst, a.get('name'))
                if h and abs(h['at'][0] - float(a.get('x'))) < 1e-3 and abs(h['at'][1] - float(a.get('y'))) < 1e-3:
                    self.assertEqual(h['rot'], a.get('rot', 'R0'))
                    self.assertEqual(h['align'], a.get('align', 'bottom-left'))
                    n += 1
        self.assertGreater(n, 100)

    def test_quotes_are_doubled(self):
        self.assertEqual("'it''s'", s.quote("it's"))



class CrossProbeTests(unittest.TestCase):
    def test_pairs_by_designator(self):
        import xml.etree.ElementTree as ET
        sr = ET.parse(str(FIXTURE)).getroot()
        br = ET.parse(str(ROOT / 'demo/routing/fixture/fusion-routing.brd')).getroot()
        m, without = s.cross_probe_pairs(sr, br, {})
        self.assertTrue(m and all(x['linked'] for x in m))
        self.assertEqual([], without)
        j1 = s.cross_probe_pairs(sr, br, {'ref': 'J1'})[0]
        self.assertEqual(['J1'], [x['ref'] for x in j1])
        self.assertEqual('J1:G$1', j1[0]['schematic']['id'])
        self.assertEqual('top', j1[0]['board']['side'])
        miss = s.cross_probe_pairs(sr, br, {'ref': 'ZZ9'})[0]
        self.assertIs(False, miss[0]['found'])

    def test_no_board_means_unlinked(self):
        import xml.etree.ElementTree as ET
        sr = ET.parse(str(FIXTURE)).getroot()
        m, without = s.cross_probe_pairs(sr, None, {})
        self.assertTrue(m and not any(x['linked'] for x in m))
        self.assertEqual(sorted(x['ref'] for x in m), sorted(without))



class SchematicDataTests(unittest.TestCase):
    def test_parts_pins_and_params(self):
        root = s.sch_root_from_file(str(FIXTURE))
        d = s.schematic_data(root, {})
        self.assertGreater(d['count'], 0)
        j1 = [c for c in d['components'] if c['designator'] == 'J1'][0]
        self.assertTrue(j1['pins'] and all(p['number'] for p in j1['pins']))
        self.assertIn(j1['pins'][0]['electrical'], (0, 1, 2, 3, 4, 5, 7))
        names = s.part_parameters(root, {})['names']
        self.assertIsInstance(names, list)



class SwapPartCommand(unittest.TestCase):
    """REPLACE takes the part first, then the device. The other order answers "Device not found" from
    Fusion and changes nothing: the verb did nothing from 1.13.8 to 1.13.53, and only its dry run was tested."""
    def test_replace_names_the_part_first(self):
        import os, re
        src = open(os.path.join(os.path.dirname(__file__), '..', 'rust', 'bridge', 'src', 'sch_script.py'), encoding='utf-8').read()
        body = src[src.index('def sch_swap_part'):src.index('def sch_split_wire')]
        self.assertIn("REPLACE %s '%s@%s'; SET CONFIRM OFF;\" % (quote(ref), full, libname)", body)
        self.assertNotRegex(body, r"REPLACE '%s@%s' %s")
        # Fusion ignores NAME right after REPLACE until its events are pumped
        self.assertRegex(body, r"doEvents[\s\S]{0,120}e\.again\(\[\"SET CONFIRM YES; NAME")

if __name__ == '__main__':
    unittest.main()
