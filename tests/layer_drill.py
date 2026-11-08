#!/usr/bin/env python3
"""Live drill of layer counts, vias, routes and pours on a new test board in Fusion (run on a shared laptop only when
it is free; never on anyone's design). It answers the questions unit tests can't: does a converted board take vias with a
layer range, does a stack in the old shape get repaired, does a route that doesn't land come off, do inner pours fill?
  python3 tests/layer_drill.py --target DESKTOP-BAR6S0F --dir C:/Users/olive/Documents/fusion-lane-tests/verbs --lib <a .lbr with RES>
"""
import argparse, json, subprocess, sys, tempfile, time
ap = argparse.ArgumentParser()
ap.add_argument('--target', default='DESKTOP-BAR6S0F'); ap.add_argument('--dir', required=True); ap.add_argument('--lib', required=True)
a = ap.parse_args()
A = ['adom-bridge', '--target', a.target, '--ai-thread', 'fusion-lane']
def call(v, args=None):
    o = subprocess.run(A + [v, json.dumps(args or {})], capture_output=True, text=True).stdout
    try: return json.loads(o)
    except Exception: return {'error': o[:300], 'success': False}
results = []
def check(name, ok, detail=''):
    results.append((name, bool(ok), detail)); print('%-46s %s %s' % (name, 'PASS' if ok else 'FAIL', '' if ok else str(detail)[:230]), flush=True)
def dialogs():
    d = call('fusion_check_dialogs'); check('no dialog', not d.get('count'), [x.get('title') for x in d.get('dialogs') or []])
    for x in d.get('dialogs') or []: call('fusion_dismiss_dialogs', {'hwnd': x.get('hwnd'), 'button': 'Cancel', 'reason': 'fusion-lane drill, my own test board'})
def seed(name):
    r = call('fusion_board_seed', {'libraryPath': a.lib, 'outDir': a.dir, 'name': name, 'title': 'drill', 'outlineMm': [40, 30], 'fixed': [
        {'ref': 'R1', 'deviceset': 'RES', 'value': '1k', 'x': 12, 'y': 20, 'nets': {'A': 'MID', 'B': 'NB'}},
        {'ref': 'R2', 'deviceset': 'RES', 'value': '1k', 'x': 28, 'y': 20, 'nets': {'A': 'MID', 'B': 'NC'}}]})
    check('seed ' + name, r.get('success'), r.get('error'))
    r = call('fusion_open_board', {'filePath': '%s/%s.brd' % (a.dir, name)}); check('open ' + name, r.get('success'), r.get('error')); time.sleep(4)
    return call('fusion_get_app_state').get('data', {}).get('activeDocument')
def st(doc): return call('fusion_routing_state', {'expectDocument': doc})
def via_lands(doc, cmd, label):
    b = len(st(doc).get('vias') or []); call('fusion_execute_text_command', {'command': 'Electron.run ' + cmd}); check(label, len(st(doc).get('vias') or []) > b, cmd)
def route(doc, y):
    s = st(doc)
    return call('fusion_route_net', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'net': 'NB', 'layer': 'F.Cu', 'points': [[2, y], {'x': 4, 'y': y, 'layer': 'B.Cu'}, [6, y]]}), s

print('--- a board converted to 4 layers'); doc = seed('LDrill2')
r = call('fusion_set_layer_count', {'expectDocument': doc, 'count': 4}); check('set_layer_count 4, no assert', r.get('success') and not r.get('fusionAsserts'), {k: r.get(k) for k in ('errorCode', 'error', 'fusionAsserts')}); time.sleep(2); dialogs()
check('four copper layers', len(st(doc).get('layers') or []) == 4 or len(call('fusion_copper_layers', {'expectDocument': doc}).get('layers') or []) == 4)
via_lands(doc, "CHANGE DRILL 0.3mm; VIA 'MID' 0.6mm ROUND 1-16 (2mm 10mm);", 'via 1-16 lands')
via_lands(doc, "VIA 'MID' (4mm 10mm);", 'via with no range lands')
b = len(st(doc).get('vias') or []); call('fusion_execute_text_command', {'command': "Electron.run VIA 'MID' 0.6mm ROUND 1-2 (6mm 10mm);"}); print('   a blind via 1-2:', 'lands' if len(st(doc).get('vias') or []) > b else 'does not land (the layer setup has no blind stage)')
s = st(doc); r = call('fusion_place_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'vias': [{'net': 'MID', 'x': 8, 'y': 10}]}); check('fusion_place_vias', r.get('success'), r.get('error') or r.get('missing'))
print('--- pours on an inner layer')
s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'In1.Cu', 'points': [[1, 1], [39, 1], [39, 29], [1, 29]], 'connect': 'direct', 'order': 2}]})
check('apply_pours on In1.Cu', r.get('success'), r.get('error') or r.get('errorCode')); inner = [p for p in r.get('pours') or [] if p.get('layer') == 2]
check('it has copper in it', inner and inner[0].get('filledMm2', 0) > 100, inner)
s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'B.Cu', 'points': [[1, 1], [39, 1], [39, 29], [1, 29]]}]})
whole = next((p.get('filledMm2') for p in r.get('pours') or [] if p.get('layer') == 16), 0); check('pour on B.Cu', whole > 100, r.get('error'))
s = st(doc); r = call('fusion_remove_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'B.Cu', 'points': [[1, 1], [39, 1], [39, 29], [1, 29]]}]}); check('remove it', r.get('success'), r.get('error'))
s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'B.Cu', 'points': [[1, 1], [39, 1], [39, 29], [1, 29]]}], 'cutouts': [{'layer': 'B.Cu', 'points': [[30, 3], [38, 3], [38, 11], [30, 11]]}]})
cut = next((p.get('filledMm2') for p in r.get('pours') or [] if p.get('layer') == 16), 0); check('a cutout takes about 64 mm2 out of the pour', 50 < whole - cut < 90, '%s -> %s, %s' % (whole, cut, r.get('error')))
s = st(doc); r = call('fusion_change_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'In1.Cu', 'newNet': 'NB'}]})
check("edit_pours moves a pour's net", r.get('success') and any(p.get('net') == 'NB' and p.get('layer') == 2 for p in r.get('pours') or []), r.get('error') or r.get('errorCode')); dialogs()
call('fusion_close_document', {'name': doc})

print('--- a board with the stack the old conversion wrote'); doc = seed('LDrillOld')
r = call('fusion_set_layer_count', {'expectDocument': doc, 'count': 4}); check('convert it first', r.get('success'), r.get('error')); time.sleep(2)
READ = """
import tempfile, os, xml.etree.ElementTree as ET
p = os.path.join(tempfile.gettempdir(), 'ldrill2.edru').replace(chr(92), '/'); app.executeTextCommand("Electron.run DRC SAVE '%s';" % p)
r = ET.parse(p).getroot(); r = r.find('designrules') if r.tag != 'designrules' else r
result = {'viadefs': [dict(v.attrib) for v in r.iter('viadef')], 'setup': [q.get('value') for q in r.findall('param') if q.get('name') == 'layerSetup']}
"""
r = call('fusion_run_modeling_script', {'script': """
import tempfile, os, xml.etree.ElementTree as ET
p = os.path.join(tempfile.gettempdir(), 'ldrill.edru').replace(chr(92), '/'); app.executeTextCommand("Electron.run DRC SAVE '%s';" % p)
t = ET.parse(p); r = t.getroot(); r = r.find('designrules') if r.tag != 'designrules' else r
v = r.find('layerstackup').find('viadef'); v.attrib.clear(); v.set('name', 'Thru'); v.set('layers', '1:16'); t.write(p, encoding='utf-8', xml_declaration=True)
app.executeTextCommand("Electron.run SET CONFIRM NO; DRC LOAD '%s'; SET CONFIRM OFF;" % p); result = 'ok'
"""}); print('   old stack loaded:', r.get('success'), (r.get('error') or '')[:150]); time.sleep(2); dialogs()
print('   Fusion now holds:', (call('fusion_run_modeling_script', {'script': READ}).get('data') or {}).get('result'))
b0 = len(st(doc).get('vias') or []); call('fusion_execute_text_command', {'command': "Electron.run CHANGE DRILL 0.4mm; VIA 'MID' 0.8mm ROUND 1-16 (6mm 6mm);"})
broken = len(st(doc).get('vias') or []) == b0; print('   the bug reproduces (a via 1-16 lands nothing):', broken)
b = len(st(doc).get('tracks') or []); v0 = len(st(doc).get('vias') or [])
s = st(doc); r = call('fusion_route_net', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'net': 'NB', 'layer': 'F.Cu', 'width': 0.3, 'viaDiameter': 0.8, 'viaDrill': 0.4, 'points': [[2, 25], [4, 25], {'x': 6, 'y': 25, 'layer': 'B.Cu'}, [8, 25]]})
print('   route on the old stack:', r.get('success'), r.get('errorCode'), 'undone', r.get('undone'), 'viasNotLanded', r.get('viasNotLanded'), (r.get('error') or '')[:130])
if broken:
    check('a route that does not land comes off', r.get('errorCode') == 'readback_mismatch' and r.get('undone') and len(st(doc).get('tracks') or []) == b, {k: r.get(k) for k in ('errorCode', 'undone', 'error')})
elif r.get('success'):
    call('fusion_execute_text_command', {'command': 'Electron.run UNDO;'})
call('fusion_execute_text_command', {'command': "Electron.run CHANGE DRILL 0.4mm; VIA 'MID' 0.8mm ROUND (14mm 6mm);"})
stuck = [v for v in st(doc).get('vias') or [] if v.get('extent') == 'invalid']; check('a via drawn now is marked invalid', stuck, st(doc).get('vias'))
check('routing_state names its net', st(doc).get('invalidViaNets') == ['MID'], st(doc).get('invalidViaNets'))
r = call('fusion_set_layer_count', {'expectDocument': doc, 'count': 4}); check('it lands the net again', r.get('netsLandedAgain') == ['MID'], {k: r.get(k) for k in ('errorCode', 'error', 'netsLandedAgain')})
check('no via is invalid now, and it is still there', not [v for v in st(doc).get('vias') or [] if v.get('extent') == 'invalid'] and any(abs(v['at'][0] - 14) < .01 for v in st(doc).get('vias') or []), st(doc).get('vias'))
check("set_layer_count repairs it", r.get("success") and r.get("repairedViaDefinition") == True or not broken, {k: r.get(k) for k in ('errorCode', 'error', 'note', 'mutated', 'repairedViaDefinition')}); time.sleep(2)
print('   Fusion now holds:', (call('fusion_run_modeling_script', {'script': READ}).get('data') or {}).get('result'))
via_lands(doc, "CHANGE DRILL 0.3mm; VIA 'MID' 0.6mm ROUND 1-16 (10mm 5mm);", 'via 1-16 lands after the repair')
r2 = call('fusion_set_layer_count', {'expectDocument': doc, 'count': 4}); check('a second call has nothing to repair', r2.get('success') and not r2.get('mutated'), r2.get('mutated'))
dialogs(); call('fusion_close_document', {'name': doc})

fails = [r for r in results if not r[1]]
print('%d PASS, %d FAIL' % (len(results) - len(fails), len(fails)))
sys.exit(1 if fails else 0)
