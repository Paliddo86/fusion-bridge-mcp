#!/usr/bin/env python3
"""Live drill of Glenn Jones's reports (#139, #141, #142, #143, #144, #145) on new test designs, on a machine with Fusion and its seat.
  python3 tests/glenn_drill.py --target Colby-ROG2 --dir C:/Users/<you>/Documents/fusion-lane-tests/glenn --unc /mnt/c/Users/<you>/Documents/fusion-lane-tests/glenn
"""
import argparse, json, os, subprocess, sys, time, zipfile
ap = argparse.ArgumentParser(); ap.add_argument('--target', required=True); ap.add_argument('--dir', required=True); ap.add_argument('--unc', required=True, help='the same folder as this machine sees it')
ap.add_argument('--only', default=''); a = ap.parse_args(); A = ['adom-bridge', '--target', a.target, '--ai-thread', 'fusion-lane']; D = a.dir
FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fixtures', 'sch_drill.lbr'); LBR = D + '/sch_drill.lbr'
def call(v, args=None, t=400):
    try: o = subprocess.run(A + [v, json.dumps(args or {})], capture_output=True, text=True, timeout=t).stdout
    except subprocess.TimeoutExpired: return {'success': False, 'error': 'drill timeout'}
    try: return json.loads(o)
    except Exception: return {'error': o[:300], 'success': False}
res = []
def check(n, ok, why=''):
    res.append(bool(ok)); print('%-58s %s %s' % (n, 'PASS' if ok else 'FAIL', '' if ok else str(why)[:300]), flush=True)
def active(): return call('fusion_get_app_state').get('data', {}).get('activeDocument')
def docs(): d = call('fusion_get_app_state'); d = d.get('data', d); return [x.get('name') for x in d.get('documents') or []]
def nodialog(t): n = call('fusion_check_dialogs'); check('no dialog ' + t, not n.get('count'), [(x.get('title'), (x.get('text') or '')[:90]) for x in n.get('dialogs') or []])
def script(s): r = call('fusion_run_modeling_script', {'script': s}); return (r.get('data') or {}).get('result'), r.get('error')
def closeall():
    for _ in range(8):
        ds = [n for n in docs()]
        if len(ds) <= 1: break
        call('fusion_close_document', {'name': ds[-1], 'save': False})
FIXED = [{'ref': 'R1', 'deviceset': 'RES', 'value': '1k', 'x': 12, 'y': 20, 'nets': {'A': 'MID', 'B': 'NB'}}, {'ref': 'R2', 'deviceset': 'RES', 'value': '1k', 'x': 28, 'y': 20, 'nets': {'A': 'MID', 'B': 'NC'}}]
def seed(name, n=FIXED):
    r = call('fusion_board_seed', {'libraryPath': LBR, 'outDir': D, 'name': name, 'title': 'glenn', 'outlineMm': [50, 40], 'fixed': n}); check('seed ' + name, r.get('success'), r.get('error'))
def board(name, n=FIXED):
    seed(name, n); r = call('fusion_open_board', {'filePath': '%s/%s.brd' % (D, name)}); check('open ' + name, r.get('success'), r.get('error')); time.sleep(4); return active()
def st(doc): return call('fusion_routing_state', {'expectDocument': doc})
def want(x): return not a.only or x in a.only.split(',')
os.makedirs(a.unc, exist_ok=True)
print('send lib', call('send_files', {'filePaths': [FIX], 'dest': D, 'destIsFolder': True, 'reason': 'glenn_drill: its test library'}).get('success'))
r = call('fusion_start', {'reason': 'glenn_drill'}, 300); print('start', r.get('success'), (r.get('error') or '')[:120])
for i in range(40):
    x = call('fusion_readiness', {'noAutoStart': True})
    if x.get('ready'): break
    time.sleep(5)
print('ready', x.get('ready'), x.get('bridgeVersion'), x.get('addinVersion'), flush=True)

if want('139'):
    print('--- #139 read-back after an EAGLE import')
    seed('GImport')
    run = call('fusion_new_electronics_from_eagle', {'schPath': D + '/GImport.sch', 'brdPath': D + '/GImport.brd'}).get('runId')
    for _ in range(40):
        time.sleep(4); s = call('fusion_eagle_import_status', {'runId': run})
        if s.get('stage') in ('done', 'failed'): break
    r = s.get('result') or {}
    check('import counts every part', s.get('stage') == 'done' and r.get('partsPlaced') == 2 and not r.get('errorCode'), {k: r.get(k) for k in ('partsPlaced', 'partsExpected', 'errorCode', 'error')}); nodialog('after the import'); closeall()

if want('142') or want('143'):
    print('--- #142 gerbers on a 4-layer board whose inner layers are only pours')
    doc = board('GGerb'); r = call('fusion_set_layer_count', {'expectDocument': doc, 'count': 4}); check('4 layers', r.get('success'), r.get('error')); time.sleep(2)
    s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'In1.Cu', 'points': [[1, 1], [49, 1], [49, 39], [1, 39]]}, {'net': 'NB', 'layer': 'In2.Cu', 'points': [[1, 1], [49, 1], [49, 39], [1, 39]]}]}); check('inner pours', r.get('success'), r.get('error') or r.get('missing'))
    out = D + '/g4'; r = call('fusion_export_gerbers', {'outputDir': out}, 300); d = r.get('data') or r
    check('exports 4 layers with no layers argument', r.get('success') and d.get('layerCount') == 4, {k: d.get(k) for k in ('layerCount', 'detectionMethod', 'error')}); print('   ', r.get('layerSource'), d.get('detectionMethod'), d.get('boardCopperLayers'))
    z = (d.get('zipPath') or '').replace(D, a.unc); names = zipfile.ZipFile(z).namelist() if z and os.path.exists(z) else []
    print('    zip', len(names), sorted(os.path.splitext(n)[1].upper() for n in names)[:14])
    check('the zip has the inner layers', any(n.upper().endswith(('.G1', '.G2', '.GP1', '.GP2')) for n in names), names)
    if want('143'):
        print('--- #143 the panel')
        PV = "import adsk.core\nui=adsk.core.Application.get().userInterface\np=ui.palettes.itemById('AdomActivityPalette')\nresult=bool(p and p.isVisible)"
        call('fusion_export_gerbers', {'outputDir': out + 'b'}, 300); call('fusion_export_gerbers', {'outputDir': out + 'c'}, 300)
        print('    right after two exports in a row:', script(PV)[0]); time.sleep(8); check('panel gone 8 s after the verb answered', script(PV)[0] is False, script(PV))
    closeall()
    doc = board('GGerb2'); r = call('fusion_export_gerbers', {'outputDir': D + '/g2'}, 300); d = r.get('data') or r; check('a 2-layer board still exports 2', r.get('success') and d.get('layerCount') == 2, {k: d.get(k) for k in ('layerCount', 'error')}); closeall()

if want('141'):
    print('--- #141 a document with no tab')
    print('   ', script("""
import adsk.core
hit=[]
def walk(fol,depth):
    for k in range(fol.dataFiles.count):
        q=fol.dataFiles.item(k)
        if q.fileExtension=='f3d' and not hit: hit.append(q)
    if depth<2 and not hit:
        for j in range(min(fol.dataFolders.count,6)): walk(fol.dataFolders.item(j),depth+1)
walk(app.data.activeProject.rootFolder,0)
z=app.documents.open(hit[0],False) if hit else None
result={'hidden':z.name if z else None,'visible':z.isVisible if z else None}
"""))
    doc = board('GTab'); time.sleep(2)
    print('   ', script("result=[(app.documents.item(i).name,app.documents.item(i).isVisible) for i in range(app.documents.count)]"))
    print('   ', script("i=[k for k in range(app.documents.count) if app.documents.item(k).name=='%s'][0]\nvis=sum(1 for k in range(i) if app.documents.item(k).isVisible)\napp.executeTextCommand('UI.SwitchToTab %%d'%%i)\nold=app.activeDocument.name\napp.executeTextCommand('UI.SwitchToTab %%d'%%vis)\nresult={'oldIndexLandsOn':old,'fixLandsOn':app.activeDocument.name}" % doc))
    r = st(doc); check('routing_state reads the right board', r.get('success') is not False and r.get('documentName') == doc and len(r.get('elements') or []) == 2, {k: r.get(k) for k in ('errorCode', 'error', 'documentName')})
    r = call('fusion_board_source', {'expectDocument': doc}); check('board_source too', r.get('success') is not False, r.get('error'))
    print('   ', script("import adsk.core\nfor i in range(app.documents.count):\n    d=app.documents.item(i)\n    if not d.isVisible:\n        d.close(False)\n        break\nresult=app.documents.count")); closeall()

if want('138'):
    print('--- #138 stable pour ids')
    doc = board('GPour'); s = st(doc); OUT = [[1, 1], [49, 1], [49, 39], [1, 39]]; OUT2 = [[2, 2], [48, 2], [48, 38], [2, 38]]
    r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NB', 'layer': 'B.Cu', 'points': OUT}]}); check('a pour', r.get('success'), r.get('error'))
    ps = call('fusion_pour_state', {'expectDocument': doc}).get('pours') or []; one = ps[0].get('id') if ps else None; check('it has an id', str(one).startswith('pour:'), ps[:1])
    s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'NC', 'layer': 'F.Cu', 'points': OUT2}]}); check('a second pour', r.get('success'), r.get('error'))
    ps = call('fusion_pour_state', {'expectDocument': doc}).get('pours') or []; check('the first keeps its id', one in [p.get('id') for p in ps] and len(ps) == 2, [p.get('id') for p in ps])
    s = st(doc); r = call('fusion_edit_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': one, 'isolate': 0.35}]}); ps = call('fusion_pour_state', {'expectDocument': doc}).get('pours') or []
    check('edit by id', r.get('success') and abs(next(p for p in ps if p.get('id') == one)['isolate'] - 0.35) < 1e-3, (r.get('errorCode'), r.get('error')))
    s = st(doc); r = call('fusion_remove_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': one}]}); ps = call('fusion_pour_state', {'expectDocument': doc}).get('pours') or []
    check('remove by id takes only that one', r.get('removed') == 1 and [p.get('net') for p in ps] == ['NC'], (r.get('errorCode'), r.get('error'), [p.get('net') for p in ps]))
    s = st(doc); r = call('fusion_remove_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': 'pour:x:1:nope'}]}); check('an unknown id is refused', r.get('errorCode') == 'unknown_pour', r.get('errorCode'))
    print('--- two pours sharing three edges, as AI Flow reported')
    SMALL = [[1, 1], [19, 1], [19, 5], [1, 5]]; BIG = [[1, 1], [19, 1], [19, 14], [1, 14]]
    for order, outline in (('the smaller', SMALL), ('the larger', BIG)):
        s = st(doc); r = call('fusion_apply_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'net': 'MID', 'layer': 'B.Cu', 'points': SMALL}, {'net': 'MID', 'layer': 'B.Cu', 'points': BIG}]}); check('both pours', r.get('success'), r.get('error') or r.get('missing'))
        ps = [p for p in call('fusion_pour_state', {'expectDocument': doc}).get('pours') or [] if p.get('net') == 'MID']
        rows = {tuple(map(tuple, p['outline'])): p['id'] for p in ps}; want_id = rows[tuple(map(tuple, [[float(a), float(b)] for a, b in outline]))]; other = [v for v in rows.values() if v != want_id]
        s = st(doc); r = call('fusion_edit_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': want_id, 'isolate': 0.33}]})
        ps = [p for p in call('fusion_pour_state', {'expectDocument': doc}).get('pours') or [] if p.get('net') == 'MID']; iso = {p['id']: round(p['isolate'], 3) for p in ps}
        check('edit %s changes only it' % order, r.get('success') and iso.get(want_id) == 0.33 and all(iso.get(o) != 0.33 for o in other), (r.get('errorCode'), r.get('error'), iso))
        s = st(doc); r = call('fusion_remove_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': want_id}]})
        left = [p['id'] for p in call('fusion_pour_state', {'expectDocument': doc}).get('pours') or [] if p.get('net') == 'MID']; print('   ', r.get('commands'))
        check('remove %s takes that one' % order, r.get('success') and r.get('removed') == 1 and left == other, (r.get('errorCode'), r.get('error'), left, want_id))
        s = st(doc); call('fusion_remove_pours', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'pours': [{'id': i} for i in left]})
    nodialog('after the pours'); closeall()

if want('move'):
    print('--- adom-aiflow#142 a move keeps the side')
    for linked in (False, True):
        if linked:
            seed('GMoveL'); run = call('fusion_new_electronics_from_eagle', {'schPath': D + '/GMoveL.sch', 'brdPath': D + '/GMoveL.brd'}).get('runId')
            for _ in range(40):
                time.sleep(4); x = call('fusion_eagle_import_status', {'runId': run})
                if x.get('stage') in ('done', 'failed'): break
            call('fusion_show_2d_board', {}); time.sleep(2); doc = active(); tag = 'linked board: '
        else:
            doc = board('GMove'); tag = 'no schematic: '
        rot = lambda ref: next(e['rot'] for e in st(doc).get('elements') or [] if e['name'] == ref).lstrip('S')
        def mv(**p):
            s = st(doc); return call('fusion_move_parts', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'parts': [dict({'ref': 'R1', 'x': 12, 'y': 20}, **p)]})
        r = mv(side='bottom'); check(tag + 'side bottom flips it', r.get('success') and r.get('sideChanged') == ['R1'] and rot('R1').startswith('MR'), (r.get('errorCode'), r.get('error'), r.get('mismatched')))
        r = mv(x=14, rotation=90); check(tag + 'a number keeps it on the bottom', r.get('success') and r.get('sideChanged') == [] and rot('R1') == 'MR90', (r.get('errorCode'), r.get('mismatched'), rot('R1')))
        r = mv(x=15); check(tag + 'no rotation keeps side and angle', r.get('success') and rot('R1') == 'MR90', (r.get('errorCode'), rot('R1')))
        r = mv(rotation='MR180'); check(tag + 'an MR string still works', r.get('success') and r.get('sideChanged') == [] and rot('R1') == 'MR180', (r.get('errorCode'), rot('R1')))
        r = mv(flip=True); check(tag + 'flip turns it back', r.get('success') and r.get('sideChanged') == ['R1'] and not rot('R1').startswith('M'), (r.get('errorCode'), r.get('mismatched'), rot('R1')))
        r = mv(rotation=270); check(tag + 'a number keeps a top part on top', r.get('success') and rot('R1') == 'R270', (r.get('errorCode'), rot('R1')))
        nodialog(tag + 'after the moves'); closeall()

if want('145'):
    print('--- #145 fusion_change_vias')
    doc = board('GVia'); s = st(doc)
    r = call('fusion_place_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'vias': [{'net': 'MID', 'x': 5, 'y': 5, 'drill': 0.3, 'diameter': 0.7}, {'net': 'MID', 'x': 10, 'y': 5, 'drill': 0.3, 'diameter': 0.7}, {'net': 'NB', 'x': 15, 'y': 5, 'drill': 0.4, 'diameter': 0.8}]}); check('place 3', r.get('success'), r.get('error'))
    s = st(doc); call('fusion_execute_text_command', {'command': "Electron.run LAYER 1; SET WIRE_BEND 2; LINE 'MID' 0.3mm (5mm 5mm) (10mm 5mm);"})
    s = st(doc); tr0 = len(s.get('tracks') or [])
    vz = lambda: {tuple(v['at']): (round(v['drill'], 3), round(v['diameter'], 3)) for v in st(doc).get('vias') or []}
    r = call('fusion_change_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'filter': {'drill': 0.3}, 'set': {'drill': 0.35}, 'dryRun': True}); check('dry run lists two and changes none', r.get('success') and len(r.get('changes') or []) == 2 and vz()[(5.0, 5.0)][0] == 0.3, r.get('error'))
    s = st(doc); r = call('fusion_change_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'filter': {'drill': 0.3}, 'set': {'drill': 0.35}})
    v = vz(); check('filter changes both, leaves the third', r.get('verified') and v[(5.0, 5.0)][0] == 0.35 and v[(10.0, 5.0)][0] == 0.35 and v[(15.0, 5.0)][0] == 0.4, (r.get('errorCode'), r.get('error'), v))
    s = st(doc); r = call('fusion_change_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'vias': [{'x': 5, 'y': 5, 'diameter': 0.9}]})
    v = vz(); check('by position, on a track end, changes the via', r.get('verified') and v[(5.0, 5.0)][1] == 0.9 and v[(10.0, 5.0)][1] == 0.7, (r.get('errorCode'), r.get('error'), v)); check('tracks untouched', len(st(doc).get('tracks') or []) == tr0, tr0)
    s = st(doc); r = call('fusion_change_vias', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'vias': [{'x': 40, 'y': 40, 'drill': 0.3}]}); check('no via there: nothing changes', r.get('errorCode') == 'nothing_to_do', r.get('errorCode')); nodialog('after the vias'); closeall()

if want('144'):
    print('--- #144 hiding rotated names')
    parts = [dict(FIXED[0], ref='R1', x=10, y=10), dict(FIXED[1], ref='R2', x=25, y=10), {'ref': 'R3', 'deviceset': 'RES', 'value': '1k', 'x': 40, 'y': 10, 'nets': {'A': 'MID', 'B': 'ND'}}]
    doc = board('GSilk', parts); s = st(doc)
    r = call('fusion_move_parts', {'expectDocument': doc, 'boardId': s.get('boardId'), 'expectedRevision': s.get('revision'), 'parts': [{'ref': 'R2', 'x': 25, 'y': 10, 'rotation': 90}, {'ref': 'R3', 'x': 40, 'y': 10, 'rotation': 270}]}); check('rotate two parts', r.get('success'), r.get('error'))
    silk = call('fusion_silk_state', {'expectDocument': doc}); comps = {c['ref']: c for c in silk.get('components') or []}
    print('    names', {k: {x: v['name'].get(x) for x in ('x', 'y', 'rotation', 'shown')} for k, v in comps.items()})
    r = call('fusion_apply_silkscreen', {'expectDocument': doc, 'names': [{'ref': k, 'x': comps[k]['name']['x'], 'y': comps[k]['name']['y'] + 2, 'show': False} for k in ('R1', 'R2', 'R3')], 'keepPartial': True})
    print('    results', r.get('results'), r.get('errorCode'), (r.get('error') or '')[:140])
    check('hides unrotated and rotated names', r.get('results') == ['ok', 'ok', 'ok'], r.get('results'))
    json.dump({'doc': doc}, open(os.path.join(a.unc, 'silk.json'), 'w'))
    print('--- #144 a failed batch is undone')
    s0 = call('fusion_silk_state', {'expectDocument': doc}); t0 = len(s0.get('texts') or [])
    r = call('fusion_apply_silkscreen', {'expectDocument': doc, 'names': [{'ref': 'R1', 'x': 1000, 'y': 1000, 'size': -1}] if False else [{'ref': 'R1', 'x': comps['R1']['name']['x'], 'y': comps['R1']['name']['y'], 'rotation': 33.3}], 'texts': [{'text': 'T%d' % i, 'x': 5 + 3 * i, 'y': 30} for i in range(4)]})
    s1 = call('fusion_silk_state', {'expectDocument': doc}); print('   ', r.get('errorCode'), (r.get('error') or '')[:120], 'texts', t0, '->', len(s1.get('texts') or []), r.get('results'))
    if r.get('errorCode'):
        check('after a failure the new texts are gone', len(s1.get('texts') or []) == t0 and not r.get('mutated'), (t0, len(s1.get('texts') or []), r.get('mutated')))
    else:
        print('    (that batch landed, so the failure path was not exercised)')
    nodialog('after the silkscreen'); closeall()
print('%d PASS, %d FAIL' % (sum(res), len(res) - sum(res))); sys.exit(0 if all(res) else 1)
