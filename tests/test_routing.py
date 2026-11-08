import copy
import importlib.util
from pathlib import Path
import math
import unittest
import xml.etree.ElementTree as ET

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('routing',ROOT/'rust/bridge/src/routing_script.py')
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
exec((ROOT/'rust/bridge/src/routing_planner.py').read_text(),r.__dict__)
exec((ROOT/'rust/bridge/src/board_model.py').read_text(),r.__dict__)

class RoutingTests(unittest.TestCase):
    def setUp(self):
        self.board=ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').find('drawing/board')
        self.args={'net':'NET_1','points':[{'pad':'J1.1'},[14,7],[40,7],{'pad':'J2.1'}]}
    def rejected(self,code):
        with self.assertRaises(r.RoutingError) as e:r.plan(self.board,self.args)
        self.assertEqual(code,e.exception.code)
    def test_six_net_fixture(self):
        import json
        for a in json.loads((ROOT/'demo/routing/fixture/plan.json').read_text()):
            p=r.plan(self.board,a)
            self.assertGreater(len(p['items']),0)
            if a['net']=='NET_3':self.assertEqual(2,sum(x['kind']=='via' for x in p['items']))
            if a['net']=='NET_6':self.assertEqual(5,len(p['items']))
    def test_crossbar_all_nets_and_shared_vias(self):
        import json
        board=ET.parse(ROOT/'demo/routing/crossbar/fusion-crossbar-24.brd').find('drawing/board')
        totals={'segment':0,'via':0}
        for args in json.loads((ROOT/'demo/routing/crossbar/plan.json').read_text()):
            plan=r.plan(board,args)
            self.assertEqual(2,sum(i['kind']=='via' for i in plan['items']))
            self.assertEqual(5,sum(i['kind']=='segment' for i in plan['items']))
            signal=board.find("signals/signal[@name='%s']"%args['net'])
            for item in plan['items']:
                totals[item['kind']]+=1
                if item['kind']=='segment':
                    ET.SubElement(signal,'wire',x1=str(item['start'][0]),y1=str(item['start'][1]),x2=str(item['end'][0]),y2=str(item['end'][1]),width=str(item['width']),layer=str(item['layer']))
                else:
                    ET.SubElement(signal,'via',x=str(item['at'][0]),y=str(item['at'][1]),diameter=str(item['diameter']),drill=str(item['drill']),extent=item['extent'])
        self.assertEqual({'segment':120,'via':48},totals)

    def test_obstacle_planner_chooses_vias(self):
        b=ET.parse(ROOT/'demo/routing/obstacles/fusion-obstacle-lab.brd').find('drawing/board')
        args={'net':'ROUTE_1','fromPad':'L1.1','toPad':'R1.1','timeoutMs':5000}
        before=ET.tostring(b)
        p=r.automatic_plan(b,args)
        self.assertEqual(2,p['planner']['viaCount'])
        self.assertEqual(40,p['planner']['lengthMm'])
        self.assertEqual(before,ET.tostring(b))
        self.assertFalse(p['planner']['externalRouter'])

    def test_obstacle_planner_same_layer_detour(self):
        b=ET.parse(ROOT/'demo/routing/obstacles/fusion-obstacle-lab.brd').find('drawing/board')
        p=r.automatic_plan(b,{'net':'ROUTE_1','fromPad':'L1.1','toPad':'R1.1','allowVias':False,'timeoutMs':5000})
        self.assertEqual(0,p['planner']['viaCount'])
        self.assertGreater(p['planner']['lengthMm'],40)
        self.assertTrue(all(i.get('layer')==1 for i in p['items']))

    def test_planner_budget_is_read_only(self):
        b=ET.parse(ROOT/'demo/routing/obstacles/fusion-obstacle-lab.brd').find('drawing/board');before=ET.tostring(b)
        with self.assertRaises(r.RoutingError) as e:
            r.automatic_plan(b,{'net':'ROUTE_1','fromPad':'L1.1','toPad':'R1.1','maxNodes':1})
        self.assertEqual('search_budget',e.exception.code);self.assertEqual(before,ET.tostring(b))

    def test_via_insertion_margin_rejects_native_shove_trigger(self):
        b=ET.parse(ROOT/'demo/routing/obstacles/fusion-obstacle-lab.brd').find('drawing/board')
        with self.assertRaises(r.RoutingError) as e:
            r.plan(b,{'net':'ROUTE_2','points':[[21,20],{'x':21,'y':20,'layer':16}]})
        self.assertEqual('clearance_violation',e.exception.code)

    def test_no_route_when_same_layer_is_partitioned(self):
        b=ET.parse(ROOT/'demo/routing/obstacles/fusion-obstacle-lab.brd').find('drawing/board')
        barrier=b.find("signals/signal[@name='TOP_BARRIER']/wire");barrier.set('y1','0');barrier.set('y2','40')
        with self.assertRaises(r.RoutingError) as e:
            r.automatic_plan(b,{'net':'ROUTE_1','fromPad':'L1.1','toPad':'R1.1','allowVias':False,'timeoutMs':5000})
        self.assertEqual('no_route',e.exception.code)

    def test_planner_wrong_net(self):
        with self.assertRaises(r.RoutingError) as e:
            r.automatic_plan(self.board,{'net':'NET_1','fromPad':'J1.1','toPad':'J2.2'})
        self.assertEqual('net_mismatch',e.exception.code)

    def test_wrong_net_pad(self):
        self.args['points'][0]={'pad':'J1.2'};self.rejected('net_mismatch')
    def test_injected_command(self):
        self.args['net']="NET_1';RIPUP;";self.rejected('invalid_net')
    def test_wildcards(self):
        self.args['net']='NET_*';self.rejected('invalid_net')
    def test_nan(self):
        self.args['width']=float('nan');self.rejected('invalid_geometry')
    def test_bool(self):
        self.args['width']=True;self.rejected('invalid_geometry')
    def test_invalid_via(self):
        self.args.update(viaDrill=.7,viaDiameter=.6);self.rejected('invalid_geometry')
    def test_wrong_layer(self):
        self.args['layer']=19;self.rejected('invalid_layer')
    def test_crossing_other_pad(self):
        self.args['points']=[[7,7],[7,17]];self.rejected('clearance_violation')
    def test_crossing_other_trace(self):
        s=self.board.find("signals/signal[@name='NET_2']")
        ET.SubElement(s,'wire',x1='20',y1='5',x2='20',y2='10',width='.25',layer='1')
        self.rejected('clearance_violation')
    def test_crossing_opposite_layer(self):
        s=self.board.find("signals/signal[@name='NET_2']")
        ET.SubElement(s,'wire',x1='20',y1='5',x2='20',y2='10',width='.25',layer='16')
        self.assertEqual(3,len(r.plan(self.board,self.args)['items']))
    def test_board_edge(self):
        self.args['points']=[[7,7],[-1,7]];self.rejected('clearance_violation')
    def test_bad_outline(self):
        self.board.find("plain/wire").set('x2','53');self.rejected('unsupported_board')
    def test_rules(self):
        self.args['width']=.1;self.rejected('design_rule_violation')
    def test_pours_are_not_obstacles(self):
        # Fusion repours another net's pour round new copper (RATSNEST after the route), as the
        # standard's plan_route assumes; a pour no longer refuses the board
        poly=ET.SubElement(self.board.find("signals/signal[@name='NET_2']"),'polygon',width='.2',layer='1')
        for x,y in ((10,4),(45,4),(45,10),(10,10)): ET.SubElement(poly,'vertex',x=str(x),y=str(y))
        self.assertGreater(len(r.plan(self.board,self.args)['items']),0)
    def _four_layers(self):
        self.board.find("designrules/param[@name='layerSetup']").set('value','(1*2*15*16)')
    def test_inner_layer_by_name_on_a_four_layer_board(self):
        self._four_layers()
        self.args.update(viaSize=.7,points=[{'pad':'J1.1'},{'x':14,'y':7,'layer':'In1.Cu'},{'x':40,'y':7,'layer':'F.Cu'},{'pad':'J2.1'}])
        items=r.plan(self.board,self.args)['items']
        self.assertEqual([1,2,1],[i['layer'] for i in items if i['kind']=='segment'])
        self.assertEqual([.7,.7],[i['diameter'] for i in items if i['kind']=='via'],'viaSize is the standard routeNet name')
    def test_inner_copper_of_another_net_blocks(self):
        self._four_layers()
        ET.SubElement(self.board.find("signals/signal[@name='NET_2']"),'wire',x1='20',y1='5',x2='20',y2='10',width='.25',layer='2')
        self.args['points']=[{'pad':'J1.1'},{'x':14,'y':7,'layer':'In1.Cu'},{'x':40,'y':7,'layer':'F.Cu'},{'pad':'J2.1'}]
        self.rejected('clearance_violation')
    def test_a_through_pad_takes_an_inner_layer_track(self):
        self._four_layers()
        pad=next(k for k,v in r.objects(self.board)[1].items() if v['layers']==[1,16] and v['net']=='NET_1') if any(v['layers']==[1,16] and v['net']=='NET_1' for v in r.objects(self.board)[1].values()) else None
        if pad is None: self.skipTest('the fixture has no through pad on NET_1')
        at=r.objects(self.board)[1][pad]['at']
        self.args.update(layer='In1.Cu',points=[[at[0]+3,at[1]],{'pad':pad}])
        self.assertGreater(len(r.plan(self.board,self.args)['items']),0)
    def test_an_inner_layer_the_board_lacks_is_refused(self):
        self.args['layer']='In1.Cu';self.rejected('invalid_layer')
    def test_a_curved_track_is_an_obstacle_along_its_arc(self):
        # a half circle of another net rising from y 3 to y 8, across the straight path at y 7
        ET.SubElement(self.board.find("signals/signal[@name='NET_2']"),'wire',x1='22',y1='3',x2='32',y2='3',width='.25',layer='1',curve='-180')
        self.rejected('clearance_violation')
        self.board.find("signals/signal[@name='NET_2']")[-1].set('curve','-20')
        self.assertGreater(len(r.plan(self.board,self.args)['items']),0,'a shallow arc stays clear')
    def test_board_copper_and_keepouts_are_obstacles(self):
        plain=self.board.find('plain')
        dot=ET.SubElement(plain,'circle',x='20',y='7',radius='.5',width='0',layer='1')
        self.rejected('clearance_violation')
        dot.set('layer','16')
        self.assertGreater(len(r.plan(self.board,self.args)['items']),0,'bottom copper does not block a top track')
        dot.set('layer','41');self.rejected('clearance_violation')
        plain.remove(dot)
        poly=ET.SubElement(plain,'polygonshape',layer='42')
        outline=ET.SubElement(poly,'polygonoutlineobjects')
        for x,y in ((18,5),(22,5),(22,9),(18,9)): ET.SubElement(outline,'vertex',x=str(x),y=str(y))
        self.assertGreater(len(r.plan(self.board,self.args)['items']),0,'a bottom keepout does not block a top track')
        poly.set('layer','41');self.rejected('clearance_violation')
    def test_milling_is_still_refused(self):
        ET.SubElement(self.board.find('plain'),'wire',x1='1',y1='1',x2='2',y2='1',width='.5',layer='46')
        self.rejected('unsupported_board')
    def test_footprint_hole_is_an_obstacle(self):
        # a mounting hole in a part's footprint, placed with the part, under the track at y 7
        el=self.board.find("elements/element[@name='J1']")
        pkg=self.board.find("libraries/library[@name='%s']/packages/package[@name='%s']"%(el.get('library'),el.get('package')))
        xf,_,_=r.placement(el)
        # find the local point that lands on (20, 7): the transform is rigid, so solve with two probes
        ox,oy=xf(0,0); ux=xf(1,0); uy=xf(0,1)
        a,b=ux[0]-ox,uy[0]-ox; c,d=ux[1]-oy,uy[1]-oy; det=a*d-b*c
        lx=((20-ox)*d-b*(7-oy))/det; ly=(a*(7-oy)-c*(20-ox))/det
        ET.SubElement(pkg,'hole',x=str(lx),y=str(ly),drill='1.0')
        self.rejected('clearance_violation')
    def test_revision_guard_before_mutation(self):
        saved=r.snapshot
        r.snapshot=lambda app,args:(None,self.board,{'boardId':'board','revision':'now'})
        try:
            res=r.handle(None,{'boardId':'board','expectedRevision':'old'},'route_net')
            self.assertEqual('stale_board',res['errorCode']);self.assertFalse(res['mutationAttempted'])
        finally:r.snapshot=saved
    def test_identity_guard(self):
        saved=r.snapshot;r.snapshot=lambda app,args:(None,self.board,{'boardId':'board','revision':'now'})
        try:
            res=r.handle(None,{'boardId':'other','expectedRevision':'now'},'route_net')
            self.assertEqual('board_mismatch',res['errorCode']);self.assertFalse(res['mutationAttempted'])
        finally:r.snapshot=saved

class InnerLayerPourTests(unittest.TestCase):
    """Pours on any copper layer of the board, by number or name, and cutouts on the restrict layers."""
    def setUp(self):
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot()
        self.b = self.root.find('drawing/board')
        for p_ in self.b.findall('designrules/param'):
            if p_.get('name') == 'layerSetup': p_.set('value', r.LAYER_SETUP[4])
        self.sent = []
        test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App(); self.saved = r.snapshot; self.after = self.b
        r.snapshot = lambda app, args: (self.root, self.after, {'boardId': 'b', 'revision': 'r'})
        self.net = self.b.find('signals/signal').get('name')
        self.pts = [[1, 1], [20, 1], [20, 20], [1, 20]]
    def tearDown(self): r.snapshot = self.saved
    def call(self, action, **a): return r.handle(self.app, dict(a, boardId='b', expectedRevision='r'), action)
    def test_layer_names_follow_the_board(self):
        self.assertEqual([2, 15, 16], [r.eagle_layer(v, self.b) for v in ('In1.Cu', 'in2.cu', 'B.Cu')])
        self.assertEqual(15, r.eagle_layer(15, self.b))
        for bad in ('In3.Cu', 3, 'Top'):
            with self.assertRaises(r.RoutingError): r.eagle_layer(bad, self.b)
    def test_inner_pour_is_drawn_on_its_layer(self):
        out = self.call('apply_pours', pours=[{'net': self.net, 'layer': 'In1.Cu', 'points': self.pts}])
        self.assertTrue(any('LAYER 2; POLYGON' in c for c in out['commands']), out)
    def test_pour_on_a_layer_the_board_lacks_is_refused(self):
        out = self.call('apply_pours', pours=[{'net': self.net, 'layer': 3, 'points': self.pts}])
        self.assertEqual('invalid_layer', out['errorCode']); self.assertFalse(self.sent)
    def test_connect_and_order_take_the_standards_names(self):
        out = self.call('apply_pours', pours=[{'net': self.net, 'layer': 1, 'points': self.pts, 'connect': 'direct', 'order': 3}])
        c = [c for c in out['commands'] if 'POLYGON' in c][0]
        self.assertIn('CHANGE RANK 3', c); self.assertIn('CHANGE THERMALS OFF', c)
    def test_cutouts_go_on_the_restrict_layer_before_the_pours(self):
        out = self.call('apply_pours', pours=[{'net': self.net, 'layer': 'B.Cu', 'points': self.pts}], cutouts=[{'layer': 'B.Cu', 'points': [[5, 5], [8, 5], [8, 8], [5, 8]]}])
        self.assertTrue(out['commands'][0].startswith('LAYER 42; RECT'), out['commands'][0])
        self.assertEqual([{'layer': 'B.Cu', 'restrictLayer': 42, 'rects': 1}], out['cutouts'])
    def test_inner_cutout_is_refused_before_anything_is_drawn(self):
        out = self.call('apply_pours', pours=[{'net': self.net, 'layer': 'In1.Cu', 'points': self.pts}], cutouts=[{'layer': 'In1.Cu', 'points': [[5, 5], [8, 5], [8, 8]]}])
        self.assertEqual('not_supported', out['errorCode']); self.assertFalse(self.sent)
    def test_a_pours_net_moves_by_drawing_it_again_and_reads_back(self):
        nets = [s_.get('name') for s_ in self.b.findall('signals/signal')]
        import copy
        def with_pour(net):
            b = copy.deepcopy(self.b)
            sig = b.find("signals/signal[@name='%s']" % net)
            pg = ET.SubElement(sig, 'polygon', layer='15', width='0.3', isolate='0.2', rank='2', thermals='no', orphans='yes')
            for x, y in [(60, 60), (90, 60), (90, 90), (60, 90)]: ET.SubElement(pg, 'vertex', x=str(x), y=str(y))
            return b
        self.after = with_pour(nets[1]); self.b = with_pour(nets[0])
        r.snapshot = lambda app, args: (self.root, self.after if self.sent else self.b, {'boardId': 'b', 'revision': 'r'})
        out = self.call('change_pours', pours=[{'net': nets[0], 'layer': 'In2.Cu', 'newNet': nets[1]}])
        self.assertTrue(out['success'], out)
        c = out['commands'][0]
        self.assertIn('LAYER 15; DELETE (S ', c); self.assertIn("POLYGON '%s' 0.300000mm" % nets[1], c)
        self.assertIn('CHANGE RANK 2', c); self.assertIn('CHANGE THERMALS OFF', c); self.assertIn('CHANGE ORPHANS ON', c)
        self.assertEqual(nets[1], out['changes'][0]['newNet'])
    def test_a_move_where_the_old_pour_stays_is_a_mismatch(self):
        nets = [s_.get('name') for s_ in self.b.findall('signals/signal')]
        sig = self.b.find("signals/signal[@name='%s']" % nets[0])
        pg = ET.SubElement(sig, 'polygon', layer='15', width='0.3', isolate='0.2')
        for x, y in [(60, 60), (90, 60), (90, 90), (60, 90)]: ET.SubElement(pg, 'vertex', x=str(x), y=str(y))
        out = self.call('change_pours', pours=[{'net': nets[0], 'layer': 15, 'newNet': nets[1]}])
        self.assertEqual('readback_mismatch', out['errorCode'])


class SchematicRunListTests(unittest.TestCase):
    """validateOnly checks a list of any length and runs nothing; a real run stays one batch's worth."""
    def setUp(self):
        self.sent = []
        test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App(); self.saved = r.sch_snapshot
        r.sch_snapshot = lambda app: ('doc', [], {})
    def tearDown(self): r.sch_snapshot = self.saved
    def test_a_long_list_is_checked_without_running(self):
        out = r.sch_run(self.app, {'commands': ['WINDOW FIT;'] * 560, 'validateOnly': True})
        self.assertTrue(out['success']); self.assertFalse(out['mutated']); self.assertEqual(560, out['commands']); self.assertFalse(self.sent)
    def test_a_bad_command_late_in_a_long_list_is_caught(self):
        with self.assertRaises(r.RoutingError) as e:
            r.sch_run(self.app, {'commands': ['WINDOW FIT;'] * 559 + ['RUN evil;'], 'validateOnly': True})
        self.assertEqual('command_not_allowed', e.exception.code); self.assertFalse(self.sent)
    def test_one_call_stays_a_batch(self):
        with self.assertRaises(r.RoutingError) as e:
            r.sch_run(self.app, {'commands': ['WINDOW FIT;'] * 401})
        self.assertEqual('invalid_args', e.exception.code)


class PourIdTests(unittest.TestCase):
    """#138: a pour's id is what it is, so it survives saves and other pours coming and going."""
    SQ = [[60, 60], [90, 60], [90, 90], [60, 90]]
    def setUp(self):
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); self.b = self.root.find('drawing/board')
        self.nets = [s_.get('name') for s_ in self.b.findall('signals/signal')]
        self.sent = []; test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App(); self.saved = r.snapshot; self.after = self.b
        r.snapshot = lambda app, args: (self.root, self.after, {'boardId': 'b', 'revision': 'r'})
    def tearDown(self): r.snapshot = self.saved
    def add(self, b, net, pts, layer='16', tag='polygon'):
        pg = ET.SubElement(b.find("signals/signal[@name='%s']" % net), tag, layer=layer, width='0.3', isolate='0.2')
        holder = pg if tag == 'polygon' else ET.SubElement(pg, 'polygonoutlineobjects')
        for x, y in pts: ET.SubElement(holder, 'vertex', x=str(x), y=str(y))
    def call(self, action, **a): return r.handle(self.app, dict(a, boardId='b', expectedRevision='r'), action)
    def test_the_id_ignores_where_the_outline_starts_and_changes_with_the_pour(self):
        a = r.pour_id('N', 16, self.SQ)
        self.assertEqual(a, r.pour_id('N', 16, self.SQ[2:] + self.SQ[:2]))
        self.assertNotEqual(a, r.pour_id('M', 16, self.SQ)); self.assertNotEqual(a, r.pour_id('N', 1, self.SQ))
        self.assertNotEqual(a, r.pour_id('N', 16, [[60, 60], [91, 60], [90, 90], [60, 90]]))
    def test_saved_and_drawn_pours_read_the_same_id_and_other_pours_dont_shift_it(self):
        self.add(self.b, self.nets[0], self.SQ); one = r.pours(self.b)[0]['id']
        self.add(self.b, self.nets[1], [[1, 1], [9, 1], [9, 9]])
        ids = {p['net']: p['id'] for p in r.pours(self.b)}
        self.assertEqual(one, ids[self.nets[0]])
        self.assertEqual(ids[self.nets[0]], next(z['id'] for z in r.zones(self.b) if z['net'] == self.nets[0]))
        import copy
        fus = copy.deepcopy(self.root).find('drawing/board'); [sg.remove(pg) for sg in fus.findall('signals/signal') for pg in sg.findall('polygon')]
        self.add(fus, self.nets[0], self.SQ, tag='polygonpour')
        self.assertEqual(one, r.pours(fus)[0]['id'])
    def test_remove_by_id_takes_only_that_pour(self):
        self.add(self.b, self.nets[0], self.SQ); self.add(self.b, self.nets[0], [[100, 60], [130, 60], [130, 90]])
        rows = r.pours(self.b)
        import copy
        self.after = copy.deepcopy(self.b); s_ = self.after.find("signals/signal[@name='%s']" % self.nets[0]); s_.remove(s_.findall('polygon')[0])
        after = self.after; r.snapshot = lambda app, args: (self.root, after if self.sent else self.b, {'boardId': 'b', 'revision': 'r'})
        out = self.call('remove_pours', pours=[{'id': rows[0]['id']}])
        self.assertEqual(1, out.get('removed'), {k: out.get(k) for k in ('errorCode', 'error', 'left', 'note')}); self.assertEqual(1, len(out['commands']))
    def test_an_unknown_id_is_refused_before_anything_runs(self):
        self.add(self.b, self.nets[0], self.SQ)
        for action in ('remove_pours', 'change_pours'):
            out = self.call(action, pours=[{'id': 'pour:x:1:nope', 'isolate': 0.3}])
            self.assertEqual('unknown_pour', out['errorCode']); self.assertFalse(self.sent)
    def overlap(self):
        small = [[1, 1], [19, 1], [19, 5], [1, 5]]; big = [[1, 1], [19, 1], [19, 14], [1, 14]]
        return small, big
    def test_a_pick_is_never_on_a_neighbours_edge(self):
        small, big = self.overlap()
        e = {'id': 's', 'net': 'G', 'layer': 16, 'outline': small}; o = {'id': 'b', 'net': 'G', 'layer': 16, 'outline': big}
        picks = r._pour_pick(e, [e, o])
        self.assertTrue(picks); self.assertTrue(all(abs(q[1] - 5) < 1e-6 for q in picks), picks)   # only its top edge is its own
        self.assertTrue(all(abs(q[1] - 14) < 1e-6 or q[1] > 5.2 for q in r._pour_pick(o, [e, o])))
        self.assertTrue(r._pour_pick(e, [dict(o, layer=1)]))                                       # another layer is no neighbour
        self.assertEqual([], r._pour_pick(dict(e, id='x'), [dict(e, id='y')]))                      # every edge shared
    def test_all_edges_shared_is_refused_before_anything_runs(self):
        small, _ = self.overlap(); self.add(self.b, self.nets[0], small); self.add(self.b, self.nets[1], small)
        rows = r.pours(self.b)
        out = self.call('remove_pours', pours=[{'id': rows[0]['id']}])
        self.assertEqual('unsafe_pick', out['errorCode']); self.assertFalse(self.sent)
    def test_the_wrong_pour_going_is_caught_by_id_and_undone(self):
        import copy
        small, big = self.overlap()
        # far from the fixture's pads, so the only question is which pour went
        small = [[x + 100, y + 100] for x, y in small]; big = [[x + 100, y + 100] for x, y in big]
        self.add(self.b, self.nets[0], small); self.add(self.b, self.nets[0], big)
        rows = r.pours(self.b); want = next(p for p in rows if p['outline'] == [[float(a), float(b)] for a, b in small])
        wrong = copy.deepcopy(self.b); s_ = wrong.find("signals/signal[@name='%s']" % self.nets[0]); s_.remove(s_.findall('polygon')[1])   # the big one went
        r.snapshot = lambda app, args: (self.root, (self.b if not self.sent or 'Electron.run UNDO;' in self.sent else wrong), {'boardId': 'b', 'revision': 'r'})
        out = self.call('remove_pours', pours=[{'id': want['id']}])
        self.assertEqual('readback_mismatch', out['errorCode']); self.assertFalse(out['verified'], out); self.assertIn('Electron.run UNDO;', self.sent)
        self.assertTrue(out['undone']); self.assertEqual(1, len(out['removedByMistake'])); self.assertEqual([want['id']], out['stillThere'])
    def test_edit_that_changes_a_neighbour_is_undone(self):
        import copy
        small, big = self.overlap(); small = [[x + 100, y + 100] for x, y in small]; big = [[x + 100, y + 100] for x, y in big]
        self.add(self.b, self.nets[0], small); self.add(self.b, self.nets[0], big)
        rows = r.pours(self.b); want = next(p for p in rows if p['outline'] == [[float(a), float(b)] for a, b in small])
        got = copy.deepcopy(self.b); pgs = got.find("signals/signal[@name='%s']" % self.nets[0]).findall('polygon'); pgs[0].set('isolate', '0.4'); pgs[1].set('isolate', '0.4')
        r.snapshot = lambda app, args: (self.root, (self.b if not self.sent or 'Electron.run UNDO;' in self.sent else got), {'boardId': 'b', 'revision': 'r'})
        out = self.call('change_pours', pours=[{'id': want['id'], 'isolate': 0.4}])
        self.assertEqual('readback_mismatch', out['errorCode']); self.assertIn('another pour changed', out['error']); self.assertIn('Electron.run UNDO;', self.sent)
    def test_edit_by_id_takes_the_pour_from_its_handle(self):
        self.add(self.b, self.nets[0], self.SQ); pid = r.pours(self.b)[0]['id']
        out = self.call('change_pours', pours=[{'id': pid, 'isolate': 0.4}])
        self.assertTrue(any('CHANGE ISOLATE 0.400000mm' in c for c in out['commands']), out)


class MovePartsSideTests(unittest.TestCase):
    """adom-aiflow#142: a move keeps a part's side unless the caller names another."""
    def test_a_number_keeps_the_side_and_a_string_names_it(self):
        self.assertEqual('MR90', r.rot_of(90, 'MR180')); self.assertEqual('R90', r.rot_of(90, 'R180')); self.assertEqual('R90', r.rot_of(90))
        self.assertEqual('MR270', r.rot_of(-90, 'SMR0')); self.assertEqual('MR45.5', r.rot_of(45.5, 'MR0'))
        self.assertEqual('R90', r.rot_of('R90', 'MR180')); self.assertEqual('MR90', r.rot_of('MR90', 'R0')); self.assertEqual('MR90', r.rot_of('SMR90'))
        with self.assertRaises(r.RoutingError): r.rot_of('sideways')
    def setUp(self):
        import copy
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); self.b = self.root.find('drawing/board')
        self.els = self.b.findall('elements/element'); self.a, self.c = self.els[0], self.els[1]
        self.a.set('rot', 'MR0'); self.a.set('x', '10'); self.a.set('y', '10'); self.c.set('rot', 'R0'); self.c.set('x', '20'); self.c.set('y', '20')
        self.sent = []; test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App(); self.saved = r.snapshot; self.after = None; self.copy = copy
        def snap(app, args):
            if 'Electron.run UNDO;' in self.sent: return self.root, self.b, {'boardId': 'b', 'revision': 'r'}
            return self.root, (self.after if self.after is not None and self.sent else self.b), {'boardId': 'b', 'revision': 'r2' if self.sent else 'r'}
        r.snapshot = snap
    def tearDown(self): r.snapshot = self.saved
    def landed(self, **parts):
        b = self.copy.deepcopy(self.b)
        for ref, (x, y, rot) in parts.items():
            e = b.find("elements/element[@name='%s']" % ref); e.set('x', str(x)); e.set('y', str(y)); e.set('rot', rot)
        self.after = b
    def call(self, parts): return r.handle(self.app, {'boardId': 'b', 'expectedRevision': 'r', 'parts': parts}, 'move_parts')
    def test_a_bottom_part_given_a_number_stays_on_the_bottom(self):
        ref = self.a.get('name'); self.landed(**{ref: (11, 12, 'MR90')})
        out = self.call([{'ref': ref, 'x': 11, 'y': 12, 'rotation': 90}])
        self.assertTrue(out['success'], out.get('mismatched')); self.assertEqual([], out['sideChanged'])
        self.assertTrue(any("ROTATE =MR90 '%s'" % ref in c for c in self.sent))
    def test_naming_the_other_side_is_said_in_the_reply(self):
        ref = self.a.get('name'); self.landed(**{ref: (11, 12, 'R90')})
        out = self.call([{'ref': ref, 'x': 11, 'y': 12, 'rotation': 'R90'}])
        self.assertTrue(out['success'], out.get('mismatched')); self.assertEqual([ref], out['sideChanged'])
    def test_no_rotation_keeps_side_and_angle(self):
        ref = self.a.get('name'); self.landed(**{ref: (11, 12, 'MR0')})
        self.assertTrue(self.call([{'ref': ref, 'x': 11, 'y': 12}])['success'])
    def test_a_mismatch_undoes_the_parts_that_did_move(self):
        a, c = self.a.get('name'), self.c.get('name'); self.landed(**{a: (11, 12, 'MR90'), c: (20, 20, 'R0')})   # the second didn't land
        out = self.call([{'ref': a, 'x': 11, 'y': 12, 'rotation': 90}, {'ref': c, 'x': 25, 'y': 25, 'rotation': 0}])
        self.assertEqual('readback_mismatch', out['errorCode']); self.assertIn('Electron.run UNDO;', self.sent)
        self.assertTrue(out['undone']); self.assertFalse(out['mutated']); self.assertEqual([], out['moved'])
    def test_side_and_flip_put_a_part_where_asked(self):
        ref = self.a.get('name'); self.landed(**{ref: (11, 12, 'R90')})
        out = self.call([{'ref': ref, 'x': 11, 'y': 12, 'rotation': 90, 'side': 'top'}]); self.assertEqual([ref], out['sideChanged'], out.get('mismatched'))
        self.sent.clear(); self.landed(**{ref: (11, 12, 'R0')})
        out = self.call([{'ref': ref, 'x': 11, 'y': 12, 'flip': True}]); self.assertEqual([ref], out['sideChanged'], out.get('mismatched'))
        self.assertTrue(any("ROTATE =R0 '%s'" % ref in c for c in self.sent))
        c = self.c.get('name'); self.sent.clear(); self.landed(**{c: (20, 20, 'MR0')})
        out = self.call([{'ref': c, 'x': 20, 'y': 20, 'side': 'bottom'}]); self.assertEqual([c], out['sideChanged'], out.get('mismatched'))
    def test_side_with_a_string_or_with_flip_is_refused_before_anything_moves(self):
        ref = self.a.get('name')
        for bad in ({'rotation': 'MR90', 'side': 'top'}, {'side': 'top', 'flip': True}, {'side': 'left'}):
            out = self.call([dict({'ref': ref, 'x': 1, 'y': 1}, **bad)])
            self.assertEqual('invalid_args', out['errorCode']); self.assertFalse(self.sent)
    def test_a_bad_part_moves_nothing(self):
        out = self.call([{'ref': self.a.get('name'), 'x': 1, 'y': 1, 'rotation': 90}, {'ref': 'NOPE9', 'x': 1, 'y': 1}])
        self.assertEqual('unknown_part', out['errorCode']); self.assertFalse(self.sent)


class TabNumberTests(unittest.TestCase):
    """#141: a document with no visible tab doesn't take a place in the tab bar."""
    def test_a_hidden_document_shifts_nothing(self):
        class D:
            def __init__(self, v): self.isVisible = v
        class Docs:
            def __init__(self, v): self.v = [D(x) for x in v]; self.count = len(v)
            def item(self, i): return self.v[i]
        class App: documents = Docs([True, False, True, True])
        self.assertEqual([0, 1, 1, 2], [r._tab_number(App, i) for i in range(4)])


class ChangeViasTests(unittest.TestCase):
    """#145: only vias change, only as asked, and a read back that disagrees is undone."""
    def setUp(self):
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); self.b = self.root.find('drawing/board')
        self.sig = self.b.find('signals/signal'); self.net = self.sig.get('name')
        for x, d in ((60, '0.2'), (70, '0.2'), (80, '0.4')):
            ET.SubElement(self.sig, 'via', x=str(x), y='60', extent='1-16', drill=d, diameter='0.6')
        self.sent = []; test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App(); self.saved = r.snapshot; self.after = None
        r.snapshot = lambda app, args: (self.root, (self.after if self.after is not None and any('CHANGE' in c for c in self.sent) and 'Electron.run UNDO;' not in self.sent else self.b), {'boardId': 'b', 'revision': 'r'})
    def tearDown(self): r.snapshot = self.saved
    def call(self, **a): return r.handle(self.app, dict(a, boardId='b', expectedRevision='r'), 'change_vias')
    def landed(self, *changes):
        import copy
        b = copy.deepcopy(self.b)
        for x, attr, val in changes:
            next(v for v in b.iter('via') if v.get('x') == str(x)).set(attr, val)
        self.after = b
    def test_a_filter_changes_every_match_and_nothing_else(self):
        self.landed((60, 'drill', '0.3'), (70, 'drill', '0.3'))
        out = self.call(filter={'drill': 0.2}, set={'drill': 0.3})
        self.assertTrue(out['success'], out); self.assertEqual(2, out['changed'])
        runs = [c for c in self.sent if 'CHANGE DRILL' in c]; self.assertEqual(1, len(runs))
        self.assertIn('(60.000000mm 60.000000mm)', runs[0]); self.assertNotIn('(80.000000mm', runs[0])
        self.assertIn('Electron.run DISPLAY NONE 18;', self.sent)
    def test_by_position_each_via_gets_its_own_values(self):
        self.landed((80, 'diameter', '0.9'))
        out = self.call(vias=[{'x': 80, 'y': 60, 'diameter': 0.9}])
        self.assertTrue(out['success'], out); self.assertTrue(any('CHANGE DIAMETER 0.900000mm (80.000000mm' in c for c in self.sent))
    def test_a_point_with_no_via_is_reported(self):
        self.landed((60, 'drill', '0.3'))
        out = self.call(vias=[{'x': 60, 'y': 60, 'drill': 0.3}, {'x': 1, 'y': 1, 'drill': 0.3}])
        self.assertEqual(['ok', 'err:0 vias at (1, 1)'], out['results'])
    def test_a_drill_as_big_as_its_via_is_refused(self):
        out = self.call(filter={'drill': 0.2}, set={'drill': 0.6})
        self.assertEqual('invalid_geometry', out['errorCode']); self.assertFalse(self.sent)
    def test_a_read_back_that_disagrees_is_undone(self):
        self.landed((60, 'drill', '0.3'))   # the filter asked for two, one landed
        out = self.call(filter={'drill': 0.2}, set={'drill': 0.3})
        self.assertEqual('readback_mismatch', out['errorCode']); self.assertIn('Electron.run UNDO;', self.sent)
    def test_dry_run_sends_nothing(self):
        out = self.call(filter={'drill': 0.2}, set={'drill': 0.3}, dryRun=True)
        self.assertFalse(out['mutated']); self.assertEqual(2, len(out['changes'])); self.assertFalse(self.sent)
    def test_needs_one_way_of_naming_them(self):
        self.assertEqual('bad_args', self.call(set={'drill': 0.3})['errorCode'])
        self.assertEqual('bad_args', self.call(filter={'drill': 0.2})['errorCode'])


class SilkSmashTests(unittest.TestCase):
    """#144: a part marked smashed with no text of its own still needs SMASH, or MOVE picks nothing."""
    def test_smashes_by_the_text_not_the_flag(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        a, c = b.findall('elements/element')[:2]
        a.set('smashed', 'yes')                                                                      # the flag, no text
        c.set('smashed', 'yes'); ET.SubElement(c, 'attribute', name='NAME', x='1', y='1', size='1', layer='25')   # both
        sent = []
        class App:
            def executeTextCommand(self, cmd): sent.append(cmd)
        saved = r.snapshot; r.snapshot = lambda app, args: (root, b, {'boardId': 'b', 'revision': 'r'})
        try:
            r.apply_silkscreen(App(), root, b, {'boardId': 'b', 'revision': 'r'}, {'keepPartial': True, 'names': [{'ref': a.get('name'), 'x': 1, 'y': 2}, {'ref': c.get('name'), 'x': 1, 'y': 2}]})
        finally:
            r.snapshot = saved
        self.assertIn("Electron.run SMASH '%s';" % a.get('name'), sent)
        self.assertNotIn("Electron.run SMASH '%s';" % c.get('name'), sent)


class SilkUndoTests(unittest.TestCase):
    """#144: a failed batch is really undone, new free texts included, or says what's left."""
    def setUp(self):
        import copy
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); self.b = self.root.find('drawing/board')
        el = self.b.find('elements/element'); el.set('smashed', 'yes')
        ET.SubElement(el, 'attribute', name='NAME', x='1', y='1', size='1', layer='25')
        self.ref = el.get('name')
        # the name stayed shown, and the new text landed
        self.landed = copy.deepcopy(self.b); ET.SubElement(self.landed.find('plain'), 'text', x='5', y='5', size='1', layer='21').text = 'NEW'
        self.undos = 0; test = self; self.limit = 1
        class App:
            def executeTextCommand(self, cmd):
                if cmd == 'Electron.run UNDO;': test.undos += 1
        self.app = App(); self.saved = r.snapshot
    def tearDown(self): r.snapshot = self.saved
    def run_batch(self, undo_works):
        def snap(app, args):
            gone = undo_works and self.undos >= 2
            return self.root, (self.b if gone or self.calls['n'] == 0 else self.landed), {'boardId': 'b', 'revision': 'r'}
        self.calls = {'n': 0}
        r.snapshot = snap
        self.calls['n'] = 1
        return r.apply_silkscreen(self.app, self.root, self.b, {'boardId': 'b', 'revision': 'r'}, {'names': [{'ref': self.ref, 'x': 1, 'y': 1, 'show': False}], 'texts': [{'text': 'NEW', 'x': 5, 'y': 5}]})
    def test_undo_goes_on_until_the_free_text_is_gone_too(self):
        out = self.run_batch(True)
        self.assertEqual('readback_mismatch', out['errorCode']); self.assertGreaterEqual(self.undos, 2)
        self.assertFalse(out['mutated']); self.assertIn('back as it was', out['error'])
    def test_an_undo_that_leaves_a_text_says_so(self):
        out = self.run_batch(False)
        self.assertTrue(out['mutated']); self.assertIn('did not restore', out['error'])


class RouteMismatchTests(unittest.TestCase):
    """A route whose copper does not land as planned comes off again, without taking an earlier edit with it."""
    def run_route(self, undo_steps_needed):
        import copy
        board = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').find('drawing/board')
        landed = copy.deepcopy(board)      # the wire landed; nothing else asked for did
        ET.SubElement(landed.find("signals/signal[@name='NET_1']"), 'wire', x1='1', y1='1', x2='2', y2='1', width='0.2', layer='1')
        sent = []
        class App:
            def executeTextCommand(self, cmd): sent.append(cmd)
        saved = r.snapshot
        calls = {'n': 0}
        def snap(app, args):
            calls['n'] += 1
            undos = sum(c == 'Electron.run UNDO;' for c in sent)
            if calls['n'] == 1: return None, board, {'boardId': 'b', 'revision': 'r'}
            return None, (board if undos >= undo_steps_needed else landed), {'boardId': 'b', 'revision': 'r2'}
        r.snapshot = snap
        try:
            out = r.handle(App(), {'boardId': 'b', 'expectedRevision': 'r', 'net': 'NET_1', 'layer': 1, 'points': [{'pad': 'J1.1'}, [14, 7], [40, 7], {'pad': 'J2.1'}]}, 'route_net')
        finally:
            r.snapshot = saved
        return out, sum(c == 'Electron.run UNDO;' for c in sent), sum('LINE ' in c for c in sent)

    def test_mismatch_is_undone_and_stops_when_the_copper_is_back(self):
        out, undos, lines = self.run_route(1)
        self.assertEqual('readback_mismatch', out['errorCode'])
        self.assertTrue(out['undone']); self.assertFalse(out['mutated']); self.assertFalse(out['success'])
        self.assertGreater(lines, 1); self.assertEqual(1, undos)   # not one per command: the rest would hit earlier edits

    def test_an_undo_that_cannot_restore_says_so(self):
        out, undos, lines = self.run_route(99)
        self.assertEqual('readback_mismatch', out['errorCode'])
        self.assertFalse(out['undone']); self.assertTrue(out['mutated']); self.assertEqual(lines, undos)
        self.assertIn('did not restore', out['error'])


class NativeDrcTests(unittest.TestCase):
    def test_severity_follows_fusions_constants(self):
        class App:
            def executeTextCommand(self, cmd):
                if cmd.startswith("Electron.run RUN '"):
                    ulp = Path(cmd[len("Electron.run RUN '"):-2]).read_text()
                    out = ulp.split('output("', 1)[1].split('"', 1)[0]
                    # type 1 warning, 2 error, 3 consistency; state 2 approved; code 21 airwire
                    Path(out).write_text('CHECKED\t1\n' + '\n'.join('ERROR\t%d\t%d\t%d\t1\t320000\t640000\t%s' % row for row in
                        [(23, 1, 0, 'Wire Stub'), (19, 2, 0, 'Overlap'), (19, 2, 2, 'Overlap'), (5, 3, 0, 'Consistency'), (21, 2, 0, 'Airwire')]))
        rep = r.native_drc(App())
        self.assertEqual((3, 1, 1), (rep['errorCount'], rep['warningCount'], rep['airwireCount']))
        self.assertEqual(['warning', 'error', 'error', 'error', 'error'], [e['severity'] for e in rep['errors']])
        self.assertEqual([False, False, True, False, False], [e['approved'] for e in rep['errors']])
        self.assertEqual([1.0, 2.0], [rep['errors'][0]['x'], rep['errors'][0]['y']])


class LayerArgTests(unittest.TestCase):
    def test_standard_layer_names_are_accepted(self):
        self.assertEqual([1, 16, 1, 16], [r.eagle_layer(v) for v in ('F.Cu', 'b.cu', 1, '16')])
        for v in (True, 'Top', None, 2.5):
            with self.assertRaises(r.RoutingError):
                r.eagle_layer(v)

    def test_route_net_plans_with_layer_names(self):
        b = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').find('drawing/board')
        by_num = r.plan(b, {'net': 'NET_1', 'layer': 1, 'points': [{'pad': 'J1.1'}, [14, 7], [40, 7], {'pad': 'J2.1'}]})
        by_name = r.plan(b, {'net': 'NET_1', 'layer': 'F.Cu', 'points': [{'pad': 'J1.1'}, [14, 7], [40, 7], {'pad': 'J2.1'}]})
        self.assertEqual(by_num['items'], by_name['items'])


class NetNameTests(unittest.TestCase):
    def test_active_low_nets_are_accepted(self):
        for n in ('!CE', '!INT', 'N$12', 'VBUS', 'A/B.C:D+'):
            self.assertEqual(n, r.identifier(n))
        for n in ("A'B", 'A;B', 'A*', 'A?', 'A B', ''):
            with self.assertRaises(r.RoutingError):
                r.identifier(n)


class EditVerbTests(unittest.TestCase):
    """The board-edit verbs against a recording app: the commands sent, the display restored one
    layer per command, and a read back that does not match undone."""
    def setUp(self):
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot()
        self.board = self.root.find('drawing/board')
        sig = self.board.find("signals/signal[@name='NET_1']")
        for x1, x2, layer in ((0, 10, 1), (10, 20, 1), (20, 30, 16)):
            ET.SubElement(sig, 'wire', x1=str(x1), y1='5', x2=str(x2), y2='5', width='0.2', layer=str(layer))
        ET.SubElement(sig, 'via', x='20', y='5', extent='1-16', drill='0.3', diameter='0.6')
        self.sent, self.after = [], self.board
        test = self
        class App:
            def executeTextCommand(self, cmd): test.sent.append(cmd)
        self.app = App()
        self.saved = r.snapshot
        r.snapshot = lambda app, args: (self.root, self.after, {'boardId': 'b', 'revision': 'r1' if self.after is self.board else 'r2'})
    def tearDown(self): r.snapshot = self.saved
    def runs(self): return [c[len('Electron.run '):] for c in self.sent if c.startswith('Electron.run ')]
    def edited(self, fn):
        b = copy.deepcopy(self.board); fn(b); self.after = b
    def test_layer_names(self):
        self.assertEqual([1, 16, 16, 20], [r._layer_number(self.board, v) for v in ('F.Cu', 'bottom', 'B.Cu', '20')])
        with self.assertRaises(r.RoutingError): r._layer_number(self.board, 'In9.Cu')
    def test_track_width_changes_only_the_asked_wires(self):
        def widen(b):
            for w in b.findall("signals/signal[@name='NET_1']/wire[@layer='1']"): w.set('width', '0.5')
        self.edited(widen)
        out = r.set_track_width(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'nets': ['NET_1', 'NOPE'], 'layer': 'F.Cu', 'widthMm': 0.5})
        self.assertTrue(out['success']); self.assertEqual(2, out['changed']); self.assertEqual(['NOPE'], out['notOnBoard'])
        runs = self.runs()
        picks = [tuple(map(float, __import__('re').findall(r'([\d.]+)mm', c)[1:])) for c in runs if c.startswith('CHANGE WIDTH')]
        # one pick per wire, on it, and away from the junction the two share at x=10
        self.assertEqual(2, len(picks)); self.assertTrue(0 < picks[0][0] < 10 < picks[1][0] < 20, picks)
        self.assertTrue(all(abs(p[0] - 10) > 4 and p[1] == 5 for p in picks), picks)
        self.assertNotIn('DISPLAY 23;', runs); self.assertIn('DISPLAY 1;', runs); self.assertIn('DISPLAY 21;', runs)
    def test_track_end_along_its_line_moves_and_at_an_angle_is_drawn_again(self):
        wires = r.objects(self.board)[2]
        x = next(w for w in wires if w['start'] in ([0.0, 5.0], [10.0, 5.0]) and w['end'] in ([10.0, 5.0], [0.0, 5.0]))
        tid = 'track:%s:%d:%g,%g,%g,%g' % (x['net'], x['layer'], *x['start'], *x['end'])
        def moved(to):
            def go(b):
                for w in b.findall("signals/signal[@name='NET_1']/wire"):
                    if {w.get('x1'), w.get('x2')} == {'0', '10'} and w.get('layer') == '1':
                        w.set('x1', str(x['start'][0])); w.set('y1', str(x['start'][1])); w.set('x2', str(to[0])); w.set('y2', str(to[1]))
            return go
        far = [x['end'][0] + (2 if x['end'][0] > x['start'][0] else -2), 5.0]
        self.edited(moved(far))
        args = {'boardId': 'b', 'revision': 'r1'}
        out = r.edit_tracks(self.app, self.root, self.board, args, {'tracks': [{'id': tid, 'end': far}]})
        self.assertTrue(out['success'], out); self.assertTrue(any(c.startswith('MOVE') for c in self.runs()))
        self.sent.clear()
        angled = [x['end'][0], 9.0]
        self.edited(moved(angled))
        out = r.edit_tracks(self.app, self.root, self.board, args, {'tracks': [{'id': tid, 'end': angled, 'width': 0.2}]})
        self.assertTrue(out['success'], out)
        runs = self.runs()
        self.assertFalse(any(c.startswith('MOVE') for c in runs))
        ln = next(i for i, c in enumerate(runs) if 'LINE ' in c); de = next(i for i, c in enumerate(runs) if c.startswith('DELETE'))
        self.assertLess(ln, de)   # the new track first: DELETE of a net's last wire takes the whole signal
    def test_track_width_mismatch_is_undone(self):
        def wrong(b):
            for w in b.findall("signals/signal[@name='NET_1']/wire"): w.set('width', '0.5')
        self.edited(wrong)
        with self.assertRaises(r.RoutingError) as e:
            r.set_track_width(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'nets': ['NET_1'], 'layer': 1, 'widthMm': 0.5})
        self.assertEqual('readback_mismatch', e.exception.code); self.assertIn('UNDO;', self.runs())
    def test_remove_items(self):
        def drop(b):
            s = b.find("signals/signal[@name='NET_1']")
            s.remove(s.find("wire[@x1='20']")); s.remove(s.find('via'))
        self.edited(drop)
        out = r.remove_items(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'},
                             {'tracks': [{'start': {'x': 30, 'y': 5}, 'end': {'x': 20, 'y': 5}, 'layer': 'B.Cu'}, {'start': [1, 1], 'end': [2, 2], 'layer': 'F.Cu'}],
                              'vias': [{'position': {'x': 20, 'y': 5}}]})
        self.assertEqual(['ok', 'err:not found', 'ok'], out['results']); self.assertEqual({'tracks': 1, 'vias': 1}, out['removed'])
        self.assertIn('DELETE (25.000000mm 5.000000mm);', self.runs())
    def test_stale_revision(self):
        with self.assertRaises(r.RoutingError) as e:
            r.remove_items(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'expectedRevision': 'old', 'vias': [[20, 5]]})
        self.assertEqual('stale_board', e.exception.code); self.assertEqual([], self.sent)
    def test_rounded_rect_outline(self):
        def redraw(b):
            for w in b.findall("plain/wire[@layer='20']"): b.find('plain').remove(w)
            for c in self.runs():
                m = __import__('re').match(r'LAYER 20; WIRE 0mm \(([-\d.]+)mm ([-\d.]+)mm\) (?:\+(\d+) )?\(([-\d.]+)mm ([-\d.]+)mm\);', c)
                if m: ET.SubElement(b.find('plain'), 'wire', x1=m[1], y1=m[2], x2=m[4], y2=m[5], width='0', layer='20', **({'curve': m[3]} if m[3] else {}))
        out_ = {}
        real = self.app.executeTextCommand
        def cmd(c):
            real(c)
            if c == 'Electron.run RATSNEST;': self.edited(redraw)
        self.app.executeTextCommand = cmd
        out = r.set_outline(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'outline': {'rect': [0, 0, 60, 40], 'cornerRadius': 2}})
        self.assertTrue(out['success'], out); self.assertEqual(8, len(out['outlineEdges'])); self.assertEqual(4, out['replaced'])
        self.assertEqual(4, sum(') +90 (' in c for c in self.runs()))
        for bad in ({'rect': [0, 0, 10, 10], 'cornerRadius': 6}, {'points': [[0, 0], [1, 1]]}, {}):
            with self.assertRaises(r.RoutingError): r.set_outline(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'outline': bad})

    def test_autoroute_refuses_disabled_layers_before_any_command(self):
        sig = self.board.find("signals/signal[@name='NET_2']")
        ET.SubElement(sig, 'wire', x1='1', y1='1', x2='2', y2='1', width='0.2', layer='2')
        ar = self.board.find('autorouter') or ET.SubElement(self.board, 'autorouter')
        ps = ET.SubElement(ar, 'pass', name='Default')
        for n in range(1, 17): ET.SubElement(ps, 'param', name='PrefDir.%d' % n, value='a' if n in (1, 16) else '0')
        with self.assertRaises(r.RoutingError) as e:
            r.autoroute(self.app, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {})
        self.assertEqual('layers_not_enabled', e.exception.code); self.assertIn('(2)', str(e.exception)); self.assertEqual([], self.sent)


class BoardFromEagleTests(unittest.TestCase):
    """update_board_from_eagle plans from a file and refuses a file that changes the schematic."""
    def setUp(self):
        import tempfile
        self.tmp = tempfile.mkdtemp()
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot()
        self.board = self.root.find('drawing/board')
        self.saved = r.snapshot
        r.snapshot = lambda app, args: (self.root, self.board, {'boardId': 'b', 'revision': 'r1'})
    def tearDown(self): r.snapshot = self.saved
    def write(self, fn):
        root = copy.deepcopy(self.root); fn(root.find('drawing/board'))
        p = Path(self.tmp) / 'new.brd'; ET.ElementTree(root).write(p); return str(p)
    def test_plan_moves_and_copper(self):
        def edit(b):
            el = b.find("elements/element[@name='J2']"); el.set('x', str(float(el.get('x')) + 2.54))
            ET.SubElement(b.find("signals/signal[@name='NET_1']"), 'wire', x1='7', y1='7', x2='20', y2='7', width='0.25', layer='1')
        out = r.update_board_from_eagle(None, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'brdPath': self.write(edit), 'dryRun': True})
        self.assertEqual(['J2'], out['plan']['partsMoved'])
        self.assertTrue(out['plan']['copper']); self.assertFalse(out['plan']['outline']); self.assertFalse(out['plan']['pours'])
    def test_same_file_changes_nothing(self):
        out = r.update_board_from_eagle(None, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'brdPath': self.write(lambda b: None)})
        self.assertFalse(out['mutated'])
    def test_fusion_and_eagle_pours_compare_equal(self):
        # Fusion exports a pour as <polygonpour> with <polygonoutlineobjects>; a file has <polygon>
        fus = ET.fromstring('<board><signals><signal name="N"><polygonpour layer="16" isolate="0.3" width="0.254"><polygonoutlineobjects>'
                            '<vertex x="52" y="20"/><vertex x="52" y="30"/><vertex x="2" y="30"/><vertex x="2" y="20"/></polygonoutlineobjects></polygonpour></signal></signals></board>')
        eag = ET.fromstring('<board><signals><signal name="N"><polygon width="0.254" layer="16" isolate="0.3" rank="1">'
                            '<vertex x="2" y="20"/><vertex x="52" y="20"/><vertex x="52" y="30"/><vertex x="2" y="30"/></polygon></signal></signals></board>')
        self.assertEqual(sorted(map(r._pour_key, r._pour_rows(fus))), sorted(map(r._pour_key, r._pour_rows(eag))))
    def test_schematic_changes_are_refused(self):
        def edit(b):
            c = b.find("signals/signal[@name='NET_1']/contactref"); c.set('pad', '99')
        with self.assertRaises(r.RoutingError) as e:
            r.update_board_from_eagle(None, self.root, self.board, {'boardId': 'b', 'revision': 'r1'}, {'brdPath': self.write(edit)})
        self.assertEqual('schematic_differs', e.exception.code)
        self.assertIn('NET_1', str(e.exception))


class PlanRouteStandardTests(unittest.TestCase):
    """plan_route answers the standard's shape: from/to in, found and routeNet out."""
    def setUp(self):
        self.root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot()
        self.board = self.root.find('drawing/board')
        self.saved = r.snapshot
        r.snapshot = lambda app, args: (self.root, self.board, {'boardId': 'b', 'revision': 'r1'})
    def tearDown(self): r.snapshot = self.saved
    def test_standard_names(self):
        out = r.handle(None, {'net': 'NET_1', 'from': 'J1.1', 'to': 'J2.1', 'widthMm': 0.25}, 'plan_route')
        self.assertTrue(out['found'], out)
        self.assertEqual('NET_1', out['routeNet']['net'])
        self.assertGreater(out['lengthMm'], 39)
    def test_stale_revision_still_refused(self):
        out = r.handle(None, {'net': 'NET_1', 'from': 'J1.1', 'to': 'J2.1', 'expectedRevision': 'old'}, 'plan_route')
        self.assertEqual('stale_board', out['errorCode'])


class BoardDxfTests(unittest.TestCase):
    def test_layers_to_dxf(self):
        import tempfile
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        d = tempfile.mkdtemp()
        out = r.board_dxf(None, root, b, {'documentName': 'fixture'}, {'layers': ['Edge.Cuts', 'F.Cu', 'F.SilkS'], 'outputDir': d, 'contours': True})
        got = {f['layer']: f for f in out['files']}
        self.assertEqual({'Edge.Cuts', 'F.Cu', 'F.SilkS'}, set(got))
        self.assertGreater(got['F.Cu']['entities'], 0)
        text = Path(got['Edge.Cuts']['path']).read_text()
        self.assertTrue(text.startswith('0\nSECTION') and text.rstrip().endswith('EOF'))
        try:
            import ezdxf
        except ImportError:
            return
        for f in out['files']:
            self.assertEqual(0, len(ezdxf.readfile(f['path']).audit().errors), f)
    def test_unknown_layer_refused(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        with self.assertRaises(r.RoutingError):
            r.board_dxf(None, root, b, {}, {'layers': ['Nope.Layer']})


class CheckInputsTests(unittest.TestCase):
    def test_clearances_in_the_standard_form(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        out = r.check_inputs(None, root, b, {'boardId': 'b', 'revision': 'r'}, {})
        for k in ('board', 'stack', 'pours', 'rules', 'silk'):
            self.assertIn(k, out)
        std = [x for x in out['rules']['rules'] if x['kind'] == 'clearance']
        self.assertTrue(all(isinstance(x['clearanceMm'], float) for x in std))
        self.assertEqual(0.0254 * 10, r._mm_value('10mil'))
        self.assertEqual(0.2, r._mm_value('0.2mm'))


class Ipc356Tests(unittest.TestCase):
    def test_fixed_columns(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        m, _ = r.board_model(b, {})
        m['nets']; lines = r.ipc356_lines(m, 'fixture')
        self.assertEqual('999', lines[-1])
        self.assertIn('P  UNITS CUST 1', lines)
        recs = [l for l in lines if l[:3] in ('317', '327')]
        self.assertEqual(len(m['pads']) + len(m['vias']), len(recs))
        for l in recs:
            self.assertEqual('-', l[26]); self.assertEqual('X', l[41]); self.assertEqual('Y', l[49]); self.assertEqual('R', l[67])
        long_ = dict(m, pads=[dict(m['pads'][0], net='A_VERY_LONG_NET_NAME_1')])
        out = r.ipc356_lines(long_, 'x')
        self.assertIn('P  NNAME1 A_VERY_LONG_NET_NAME_1', out)



class GencadTests(unittest.TestCase):
    def test_sections_and_references(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        text, counts = r.gencad_text(b, {'documentName': 'fixture'})
        L = text.splitlines()
        def sec(n): return L[L.index('$' + n) + 1:L.index('$END' + n)]
        for n in ('HEADER', 'BOARD', 'PADS', 'PADSTACKS', 'SHAPES', 'COMPONENTS', 'DEVICES', 'SIGNALS', 'TRACKS', 'ROUTES'):
            sec(n)
        pads = {l.split()[1] for l in sec('PADS') if l.startswith('PAD ')}
        stacks = {l.split()[1] for l in sec('PADSTACKS') if l.startswith('PADSTACK ')}
        shapes = {l.split(None, 1)[1] for l in sec('SHAPES') if l.startswith('SHAPE ')}
        self.assertTrue(all(l.split()[1] in pads for l in sec('PADSTACKS') if l.startswith('PAD ')))
        self.assertTrue(all(l.split()[2] in stacks for l in sec('SHAPES') if l.startswith('PIN ')))
        self.assertTrue(all(l.split()[1] in stacks for l in sec('ROUTES') if l.startswith('VIA ')))
        self.assertTrue(all(l.split(None, 1)[1].rsplit(' ', 2)[0] in shapes for l in sec('COMPONENTS') if l.startswith('SHAPE ')))
        self.assertEqual(len(b.findall('elements/element')), counts['components'])
        self.assertGreaterEqual(len(sec('BOARD')), 4)



class Ipc2581Tests(unittest.TestCase):
    def test_valid_against_the_c_schema(self):
        try:
            from lxml import etree
        except ImportError:
            self.skipTest('lxml not installed')
        schema = etree.XMLSchema(etree.parse(str(ROOT/'tests/fixtures/IPC-2581C.xsd')))
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        text, counts = r.ipc2581_xml(b, {'documentName': 'fixture'}, version='test')
        doc = etree.fromstring(text.encode())
        self.assertTrue(schema.validate(doc), [e.message for e in schema.error_log][:5])
        self.assertEqual(len(b.findall('elements/element')), counts['components'])
        self.assertEqual(['ASSEMBLY'], [e.get('mode') for e in doc.iter('{http://webstds.ipc.org/2581}FunctionMode')])

    def test_ascii_text(self):
        self.assertEqual('10kohm +/-1% 4.7uF', r.ascii_text('10k\u03a9 \u00b11% 4.7\u00b5F'))



class ImportSvgTests(unittest.TestCase):
    def test_shapes_holes_and_skips(self):
        text = (ROOT/'tests/fixtures/import-test.svg').read_text()
        shapes, skipped, unit = r.svg_shapes(text, 0.1)
        self.assertAlmostEqual(0.1, unit)  # 20 mm over a 200-unit viewBox
        self.assertEqual(3, len(shapes))
        self.assertEqual({'text', 'path'}, {k['element'] for k in skipped})
        square = r.svg_rects(shapes[0], 1.0)
        # the square's hole is cut out: 80x80 minus 40x40, in 4 rectangles (below, the two sides, above)
        self.assertAlmostEqual(80 * 80 - 40 * 40, sum((x2 - x1) * (y2 - y1) for x1, y1, x2, y2 in square), places=3)
        self.assertEqual(4, len(square))
        disc = r.svg_rects(shapes[1], 0.5)
        self.assertAlmostEqual(math.pi * 30 * 30, sum((x2 - x1) * (y2 - y1) for x1, y1, x2, y2 in disc), delta=60)

    def test_arc_sweep_follows_svg(self):
        up = r._arc((110, 70), 30, 30, 0, False, True, (170, 70), 0.5)
        self.assertAlmostEqual(40, min(p[1] for p in up), places=0)
        down = r._arc((110, 70), 30, 30, 0, False, False, (170, 70), 0.5)
        self.assertAlmostEqual(100, max(p[1] for p in down), places=0)

    def test_transform(self):
        M = r._svg_matrix('translate(10,0) rotate(90)')
        x, y = r._svg_apply(M, (1, 0))
        self.assertAlmostEqual(10, x); self.assertAlmostEqual(1, y)



class CopyRoutingTests(unittest.TestCase):
    BQ = Path('/home/adom/project/fusion-lane/bq25792-charger/bq25792-charger.brd')

    def setUp(self):
        if not self.BQ.exists():
            self.skipTest('BQ25792 fixture not on this machine')
        self.b = ET.parse(self.BQ).getroot().find('drawing/board')

    def test_unmapped_nets_are_skipped(self):
        pl = r.copy_routing_plan(self.b, {'box': [20, 13, 30, 16], 'offset': [0, -10]})
        self.assertEqual([], pl['tracks'])
        self.assertTrue(pl['skipped'])

    def test_clash_with_another_net_is_found_before_drawing(self):
        pl = r.copy_routing_plan(self.b, {'box': [20, 13, 30, 16], 'offset': [0, -10], 'keepUnmappedNets': True})
        self.assertTrue(pl['clashes'])
        self.assertTrue(all(c['touches'] for c in pl['clashes']))

    def test_pairs_map_nets_pad_by_pad(self):
        # C12 and C13 are the same package: pairing them maps each of C12's nets to C13's
        pl = r.copy_routing_plan(self.b, {'box': [0, 0, 0.1, 0.1], 'offset': [1, 0], 'refMap': {'C12': 'C13'}})
        self.assertEqual({'C12': 'C13'}, pl['pairs'])
        with self.assertRaises(r.RoutingError):
            r.copy_routing_plan(self.b, {'box': [0, 0, 1, 1], 'offset': [1, 0], 'refMap': {'C12': 'NOPE9'}})



class LayerCountTests(unittest.TestCase):
    def test_stackup_keeps_thickness_and_reads_back(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        for n in (2, 4, 6, 8):
            st = r.stackup_for(n, 1.6, {}, None)
            b2 = ET.fromstring(ET.tostring(b))
            old = b2.find('.//layerstackup')
            if old is not None:
                [p for p in b2.iter() if old in list(p)][0].remove(old)
            b2.find('designrules').append(st)
            for p_ in b2.findall('designrules/param'):
                if p_.get('name') == 'layerSetup': p_.set('value', r.LAYER_SETUP[n])
            self.assertEqual(r.LAYER_SETS[n], r.Layers(b2).order)
            self.assertAlmostEqual(1600, r.layer_stack(b2)['totalUm'], delta=1)

    def test_too_thin_is_refused(self):
        with self.assertRaises(r.RoutingError):
            r.stackup_for(8, 0.8, {}, None)

    def test_a_via_marked_invalid_is_landed_again_with_its_net(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        sig = b.find('signals/signal'); name = sig.get('name')
        ET.SubElement(sig, 'wire', x1='1', y1='1', x2='5', y2='1', width='0.3', layer='1')
        ET.SubElement(sig, 'via', x='5', y='1', extent='invalid', drill='0.4', diameter='0.8')
        self.assertEqual([name], r._invalid_via_nets(b))
        import copy
        fixed = copy.deepcopy(b); fixed.find('signals/signal').find('via').set('extent', '1-16')
        self.assertEqual([], r._invalid_via_nets(fixed))
        sent = []
        class App:
            def executeTextCommand(self, cmd): sent.append(cmd)
        saved = r.snapshot; r.snapshot = lambda app, args: (root, fixed, {'boardId': 'b', 'revision': 'r2'})
        try:
            nets, after, _ = r._reland_invalid_via_nets(App(), {}, b)
        finally:
            r.snapshot = saved
        self.assertEqual([name], nets)
        cmds = [c[len('Electron.run '):] for c in sent if c.startswith('Electron.run ')]
        self.assertEqual("RIPUP '%s';" % name, cmds[0])
        self.assertTrue(any("WIRE '%s' 0.300000mm" % name in c for c in cmds)); self.assertTrue(any(' 1-16 (5.000000mm 1.000000mm)' in c for c in cmds))
    def test_landing_again_undoes_when_the_copper_differs(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        sig = b.find('signals/signal')
        ET.SubElement(sig, 'wire', x1='1', y1='1', x2='5', y2='1', width='0.3', layer='1')
        ET.SubElement(sig, 'via', x='5', y='1', extent='invalid', drill='0.4', diameter='0.8')
        import copy
        lost = copy.deepcopy(b); s2 = lost.find('signals/signal'); s2.remove(s2.find('via'))
        sent = []
        class App:
            def executeTextCommand(self, cmd): sent.append(cmd)
        saved = r.snapshot
        r.snapshot = lambda app, args: (root, b if 'Electron.run UNDO;' in sent else lost, {'boardId': 'b', 'revision': 'r2'})
        try:
            with self.assertRaises(r.RoutingError) as e:
                r._reland_invalid_via_nets(App(), {}, b)
        finally:
            r.snapshot = saved
        self.assertEqual('readback_mismatch', e.exception.code); self.assertIn('undone', str(e.exception)); self.assertIn('Electron.run UNDO;', sent)

    def test_stackup_has_the_shape_fusion_writes(self):
        # against a 4-layer stack Fusion wrote itself: same elements and attributes, including the via definition
        real = ET.parse(ROOT/'addin/AdomBridge/resources/dru/Adom-3rdPartyFab-4-Layer.edru').getroot().find('.//layerstackup')
        ours = r.stackup_for(4, 1.6, {}, real)
        shape = lambda s_: [(c.tag, c.get('type'), c.get('layer'), c.get('name'), sorted(c.attrib), [sorted(m.attrib) for m in c]) for c in s_]
        self.assertEqual(shape(real), shape(ours))
        self.assertEqual(dict(real.find('viadef').attrib), dict(ours.find('viadef').attrib))

    def test_a_stack_from_the_old_conversion_is_seen_as_broken(self):
        b = ET.fromstring('<board><designrules/></board>')
        self.assertFalse(r.stack_has_broken_viadef(b))
        st = r.stackup_for(4, 1.6, {}, None); b.find('designrules').append(st)
        self.assertFalse(r.stack_has_broken_viadef(b))
        v = st.find('viadef'); v.attrib.clear(); v.set('name', 'Thru'); v.set('layers', '1:16')
        self.assertTrue(r.stack_has_broken_viadef(b))
        st.remove(v)   # what Fusion actually holds after loading that: the definition dropped
        self.assertTrue(r.stack_has_broken_viadef(b))



class FootprintPrimitivesTests(unittest.TestCase):
    def test_pads_land_where_the_board_model_has_them(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        ref = b.find('elements/element').get('name')
        out = r.footprint_primitives(b, {}, {'designator': ref})
        m, _ = r.board_model(b, {})
        model_pads = {(p_['name']): (round(p_['x'], 3), round(p_['y'], 3)) for p_ in m['pads'] if p_['ref'] == ref}
        for p_ in out['primitives']:
            if p_['kind'] == 'pad':
                self.assertEqual(model_pads[p_['name']], (round(p_['xMm'], 3), round(p_['yMm'], 3)))
        with self.assertRaises(r.RoutingError):
            r.footprint_primitives(b, {}, {'designator': 'NOPE9'})

    def test_gencad_bottom_parts_mirror_x(self):
        bq = Path('/home/adom/project/fusion-lane/bq25792-charger/bq25792-charger.brd')
        if not bq.exists():
            self.skipTest('BQ25792 fixture not on this machine')
        b = ET.parse(bq).getroot().find('drawing/board')
        text, _ = r.gencad_text(b, {'documentName': 'x'})
        self.assertIn('MIRRORX FLIP', text)
        self.assertNotIn('MIRRORY', text)



class BoardFurnitureTests(unittest.TestCase):
    def test_spot_clear_and_grid(self):
        root = ET.parse(ROOT/'demo/routing/fixture/fusion-routing.brd').getroot(); b = root.find('drawing/board')
        env = r._copper_env(b)
        _, pads, _, _ = r.objects(b)
        k, p_ = next(iter(pads.items()))
        self.assertIsNotNone(r._spot_clear(env, tuple(p_['at']), 0.3, None, 0.2, 0.5), 'a spot on a pad of another net is refused')
        self.assertEqual('outside the board', r._spot_clear(env, (-50, -50), 0.3, None, 0.2, 0.5))
        r._guard = lambda m, a: None
        o = r.board_features(None, None, b, {'documentName': 'x'}, {'stitching': {'net': p_['net'] or 'GND', 'pitch': 3}, 'dryRun': True})
        for pt in o['planned']['stitching']['points']:
            self.assertIsNone(r._spot_clear(env, tuple(pt), 0.3, p_['net'], 0.3, 1.0))


if __name__=='__main__':unittest.main()
