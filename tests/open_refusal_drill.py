#!/usr/bin/env python3
"""Live drill of what Fusion refuses, on test files of ours in --dir: a board with errors never reaches Fusion, one it refuses
anyway is reported, and a library name clash answers with both files rather than blocking Fusion.
  python3 tests/open_refusal_drill.py --target DESKTOP-BAR6S0F --dir <test dir> --lib <.lbr with RES> --clash <a .lbr> --other <its copy under the same name>
"""
import argparse, json, subprocess, sys, threading, time
ap = argparse.ArgumentParser()
for k in ('target', 'dir', 'lib', 'clash', 'other'): ap.add_argument('--' + k, required=True)
ap.add_argument('--symbol', default='3X1_JUMP', help='a part in the clash library')
a = ap.parse_args(); A = ['adom-bridge', '--target', a.target, '--ai-thread', 'fusion-lane']; D = a.dir
def call(v, args=None):
    o = subprocess.run(A + [v, json.dumps(args or {})], capture_output=True, text=True).stdout
    try: return json.loads(o)
    except Exception: return {'error': o[:300], 'success': False}
results = []
def check(name, ok, detail=''):
    results.append(bool(ok)); print('%-52s %s %s' % (name, 'PASS' if ok else 'FAIL', '' if ok else str(detail)[:260]), flush=True)
def docs():
    d = call('fusion_get_app_state'); d = d.get('data', d); return [x.get('name') for x in d.get('documents') or []]
def script(s): return (call('fusion_run_modeling_script', {'script': s}).get('data') or {}).get('result')
def no_dialogs(tag): n = call('fusion_check_dialogs'); check('no dialog ' + tag, not n.get('count'), [x.get('title') for x in n.get('dialogs') or []])
r = call('fusion_board_seed', {'libraryPath': a.lib, 'outDir': D, 'name': 'ORef', 'title': 'o', 'outlineMm': [40, 30], 'fixed': [{'ref': 'R1', 'deviceset': 'RES', 'value': '1k', 'x': 12, 'y': 20, 'nets': {'A': 'MID', 'B': 'NB'}}]}); check('seed', r.get('success'), r.get('error'))
print(script("""
import xml.etree.ElementTree as ET
for name, edit in (('OB1', lambda b: b.find('libraries/library/packages').remove(b.find('libraries/library/packages')[0])), ('OB2', lambda b: b.find('elements/element').set('library', 'NoSuchLibrary')), ('OB3', lambda b: ET.SubElement(b.find('signals/signal'), 'contactref', {'element': 'NOPE', 'pad': '1'}))):
    t = ET.parse(r'%s/ORef.brd'); edit(t.getroot().find('drawing/board')); t.write(r'%s/%%s.brd' %% name, encoding='utf-8', xml_declaration=True)
t = ET.parse(r'%s/ORef.sch'); s = t.getroot().find('drawing/schematic'); s.find('parts/part').set('library', 'NoSuchLibrary'); t.write(r'%s/OS1.sch', encoding='utf-8', xml_declaration=True)
result = 'written'
""" % (D, D, D, D)))
before = docs()
for n in ('OB1', 'OB2', 'OB3'):
    r = call('fusion_open_board', {'filePath': '%s/%s.brd' % (D, n)})
    check(n + ' refused before Fusion sees it', r.get('errorCode') == 'board_file_has_errors' and r.get('problems'), {k: r.get(k) for k in ('success', 'errorCode', 'error')}); print('   ', (r.get('error') or '')[:120])
check('nothing opened', docs() == before, docs()); no_dialogs('after the refusals')
r = call('fusion_open_board', {'filePath': D + '/ORef.brd'}); check('the good board still opens', r.get('success'), r.get('error')); time.sleep(3)
s = call('fusion_board_summary', {}); check('board_summary with no document named', s.get('success'), s.get('error'))
for n in docs()[len(before):]: call('fusion_close_document', {'name': n})
r = call('fusion_open_schematic', {'filePath': D + '/OS1.sch'}); print('   a schematic Fusion refuses:', r.get('success'), r.get('errorCode'), (r.get('error') or '')[:150])
check('it is reported, or Fusion took it and said nothing', r.get('errorCode') == 'file_refused_by_fusion' or r.get('success'), r.get('errorCode')); no_dialogs('after the schematic')
if r.get('errorCode') == 'file_refused_by_fusion': check('the empty design is closed', r.get('emptyDesignClosed'), docs())
for n in [x for x in docs()[len(before):]]: call('fusion_close_document', {'name': n})
print('--- a library name clash')
r = call('fusion_project_create', {'name': 'OClash', 'parentDir': D, 'overwrite': True}); check('new design', r.get('success'), r.get('error')); call('fusion_show_schematic', {})
call('fusion_execute_text_command', {'command': "Electron.run USE '%s';" % a.other})
t0 = time.time(); r = call('fusion_sch_place_part', {'library': a.clash, 'symbol': a.symbol, 'designator': 'A1', 'x': 3000, 'y': 3000})
print('    answered in %.0f s: %s %s' % (time.time() - t0, r.get('errorCode'), (r.get('error') or '')[:200]))
check('answers library_name_in_use with both files', r.get('errorCode') == 'library_name_in_use' and len(r.get('librariesInUse') or []) == 2, {k: r.get(k) for k in ('success', 'errorCode', 'librariesInUse', 'error')})
check('the picker closed', r.get('pickerClosed') is True, r.get('pickerClosed')); time.sleep(3); no_dialogs('after the clash')
g = call('fusion_get_app_state'); check('Fusion still answers', g.get('success') is not False, g.get('error'))
r = call('fusion_sch_place_part', {'library': a.lib, 'symbol': 'RES', 'designator': 'R9', 'x': 3000, 'y': 4000}); check('another library still places', r.get('success'), r.get('error'))
for n in docs():
    if n != 'Untitled' or docs().count('Untitled') > 1: call('fusion_close_document', {'name': n})
print('--- fusion_project_create is linked')
for layers in (2, 4):
    r = call('fusion_project_create', {'name': 'OLink%d' % layers, 'parentDir': D, 'layers': layers, 'overwrite': True}); check('%d layers: linked' % layers, r.get('linked') is True and r.get('documentName'), {k: r.get(k) for k in ('success', 'linked', 'whyNotLinked', 'error', 'stillRunning')})
    time.sleep(2); no_dialogs('after it')
    call('fusion_show_schematic', {}); r = call('fusion_sch_place_part', {'library': a.lib, 'symbol': 'RES', 'designator': 'R1', 'x': 3000, 'y': 3000}); check('a part goes on the sheet', r.get('success'), r.get('error'))
    call('fusion_show_2d_board', {}); s = call('fusion_board_summary', {}); names = [c.get('designator') for c in (s.get('summary') or s).get('components') or []]
    check('and is on the board', 'R1' in names, {k: s.get(k) for k in ('success', 'errorCode', 'error')})
    d0 = call('fusion_get_app_state').get('data', {}).get('activeDocument'); n = len(call('fusion_routing_state', {'expectDocument': d0}).get('copperLayers') or []); check('with %d copper layers' % layers, n == layers, n)
    call('fusion_close_document', {'name': d0})
print('%d PASS, %d FAIL' % (sum(results), len(results) - sum(results))); sys.exit(0 if all(results) else 1)
