#!/usr/bin/env python3
"""Live drill of the schematic edit verbs: each one runs for real on a throwaway seeded design, and each
result is checked with a separate read. The daily battery skips these (they edit a design), which is how
fusion_sch_swap_part stayed broken from 1.13.8 to 1.13.53 with only its dry run ever exercised.

    python3 tests/sch_drill.py --target DESKTOP-BAR6S0F [--dir C:/Users/<you>/Documents/fusion-lane-tests/drill]

It opens a new design and closes it again, and touches nothing else. Run it when no other lane has this
Fusion in front. Exit 1 when any step fails.

Its own fixture is written the way the bridge expects, so a pass on it alone proves little: #136 passed every
test here and failed on a customer's real library. So it also runs real parts from libraries it did not make:
AdomBasicParts_full.lbr from this repo, and each --foreign library on the machine. Every check compares
before and after, so a verb that answers success and does nothing fails."""
import argparse
import json
import os
import subprocess
import sys
import time

ap = argparse.ArgumentParser()
ap.add_argument('--target', required=True)
ap.add_argument('--dir', default='C:/Users/olive/Documents/fusion-lane-tests/drill')
ap.add_argument('--keep', action='store_true', help='leave the design open')
ap.add_argument('--foreign', action='append', default=[], help='another .lbr ALREADY on the Fusion machine to exercise, e.g. a Fusion export or a customer library. It stays there: nothing of it is copied. Repeatable.')
opts = ap.parse_args()
A = ['adom-bridge', '--target', opts.target, '--ai-thread', 'fusion-lane']
D = opts.dir
LBR = D + '/sch_drill.lbr'
FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fixtures', 'sch_drill.lbr')
results = []


def call(verb, args=None):
    out = subprocess.run(A + [verb, json.dumps(args or {})], capture_output=True, text=True).stdout
    try:
        return json.loads(out)
    except Exception:
        return {'success': False, 'error': out[:300]}


def step(name, verb, args, check=None):
    """Run a verb for real, then the separate read. check returns None when good, else what is wrong."""
    r = call(verb, args)
    why = None
    if r.get('success') is not True:
        why = '%s: %s' % (r.get('errorCode'), str(r.get('error'))[:170])
    elif check:
        try:
            why = check(r)
        except Exception as e:
            why = 'check raised %r' % (e,)
    results.append((name, why))
    print('%-34s %s' % (name, 'PASS' if why is None else 'FAIL  ' + why), flush=True)
    return r


def pins(*refs):
    rows = call('fusion_sch_pins', {'designators': list(refs)}).get('pins') or []
    return {(p['ref'], p['pin']): p for p in rows}


def net(ref, pin):
    return pins(ref).get((ref, pin), {}).get('net')


def parts():
    return {c.get('designator'): c for c in call('fusion_schematic_data', {'pins': False}).get('components') or []}


def items(*kinds):
    return call('fusion_sch_items', {'kinds': list(kinds)})


def expect(cond, why):
    return None if cond else why


# ── the design ─────────────────────────────────────────────────────────────────────────────────────
print(json.dumps(call('send_files', {'filePaths': [FIXTURE], 'dest': D, 'destIsFolder': True, 'reason': 'sch_drill: its test library'}).get('success')), 'library sent')
fixed = [{'ref': 'R1', 'deviceset': 'RES', 'value': '10k', 'x': 10, 'y': 10, 'nets': {'A': 'NA', 'B': 'NB'}},
         {'ref': 'R2', 'deviceset': 'RES', 'value': '22k', 'x': 20, 'y': 10, 'nets': {'A': 'NC', 'B': 'ND'}}]
r = call('fusion_board_seed', {'libraryPath': LBR, 'outDir': D, 'name': 'Drill', 'title': 'drill', 'outlineMm': [80, 60], 'fixed': fixed})
if r.get('success') is not True:
    sys.exit('seed failed: %s' % r.get('error'))
run = call('fusion_new_electronics_from_eagle', {'schPath': D + '/Drill.sch', 'brdPath': D + '/Drill.brd'}).get('runId')
for _ in range(40):
    time.sleep(6)
    st = call('fusion_eagle_import_status', {'runId': run})
    if st.get('stage') in ('done', 'failed'):
        break
if st.get('stage') != 'done':
    sys.exit('import failed: %s' % json.dumps(st.get('result'))[:300])
call('fusion_show_schematic', {'expectDocument': 'Untitled'})

# ── placing ────────────────────────────────────────────────────────────────────────────────────────
for ref, sym, x, y in (('R4', 'RES', 3000, 3000), ('R5', 'RES', 3000, 4000), ('R6', 'RES', 4500, 4000), ('R7', 'RES', 3000, 5000), ('R8', 'RES', 4500, 5000)):
    step('sch_place_part ' + ref, 'fusion_sch_place_part', {'library': LBR, 'symbol': sym, 'designator': ref, 'x': x, 'y': y},
         lambda _r, ref=ref: expect(ref in parts(), ref + ' not on the sheet'))
step('sch_place_part partId 2', 'fusion_sch_place_part', {'library': LBR, 'symbol': 'DUAL', 'designator': 'U2', 'partId': 2, 'x': 6000, 'y': 3000},
     lambda _r: expect([g.get('gate') for g in parts()['U2'].get('gates') or []] == ['B'], 'gate is not B'))
r = call('fusion_sch_place_part', {'library': LBR, 'symbol': 'RES', 'designator': 'R9', 'x': 100000, 'y': 1000})
results.append(('off the sheet refused', expect(r.get('errorCode') == 'off_the_sheet', str(r.get('errorCode')))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))

# ── a long command list (#137): 80 parts at 7 commands each, as AI Flow sends ──
long_cmds = []
for i in range(80):
    x, y = 300 + 25 * (i % 10), 300 + 25 * (i // 10)
    long_cmds += ["ADD 'RES@%s' 'L%d' R0 (%dmm %dmm);" % (LBR.rsplit('/', 1)[-1][:-4], i + 1, x, y), "VALUE 'L%d' '%dk';" % (i + 1, i + 1)]
    long_cmds += ["NET 'LN%d_%d' (%dmm %dmm) (%dmm %dmm);" % (i + 1, k, x + 12, y + 2 * k, x + 18, y + 2 * k) for k in range(5)]
bad_late = long_cmds[:300] + ["ADD 'RES@%s' 'LFAR' R0 (5000mm 300mm);" % LBR.rsplit('/', 1)[-1][:-4]] + long_cmds[300:]
before_long = set(parts())
r = call('fusion_schematic_run', {'libraryPath': LBR, 'commands': bad_late})
results.append(('long list, bad command late: nothing ran', expect(r.get('errorCode') == 'off_the_sheet' and set(parts()) == before_long, '%s, added %s' % (r.get('errorCode'), sorted(set(parts()) - before_long)[:5]))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
t_long = time.time()
r = call('fusion_schematic_run', {'libraryPath': LBR, 'commands': long_cmds})
print('   the call answered in %.0f s, stillRunning %s' % (time.time() - t_long, r.get('stillRunning')))
results.append(('long list: no false timeout', expect(r.get('success') is True and r.get('errorCode') != 'timed_out', '%s %s' % (r.get('errorCode'), str(r.get('error'))[:120]))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
while r.get('stillRunning') and time.time() - t_long < 1200:
    time.sleep(10)
    r = call('fusion_schematic_run', {'runId': r.get('runId')})
print('   560 commands took %.0f s, %s batches, %s commands' % (time.time() - t_long, r.get('batches'), r.get('commands')))
added_long = set(parts()) - before_long
results.append(('long list runs whole', expect(r.get('success') and r.get('batches') == 2 and r.get('commands') == 560 and len(added_long) == 80, '%s %s, %d parts' % (r.get('errorCode'), str(r.get('error'))[:120], len(added_long)))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))

# ── wiring ─────────────────────────────────────────────────────────────────────────────────────────
step('sch_connect label', 'fusion_sch_connect', {'connections': [{'designator': 'R4', 'pin': 'A', 'net': 'N1'}]},
     lambda _r: expect(net('R4', 'A') == 'N1', 'R4.A is on %s' % net('R4', 'A')))
step('sch_connect wire', 'fusion_sch_connect', {'connections': [{'designator': 'R4', 'pin': 'B', 'net': 'N2', 'kind': 'wire'}]},
     lambda _r: expect(net('R4', 'B') == 'N2', 'R4.B is on %s' % net('R4', 'B')))
step('sch_connect_pins', 'fusion_sch_connect_pins', {'pairs': [{'from': 'R5.1', 'to': 'R6.1'}]},
     lambda _r: expect(net('R5', 'A') and net('R5', 'A') == net('R6', 'A'), 'R5.A %s, R6.A %s' % (net('R5', 'A'), net('R6', 'A'))))
label_y = next((l['at'][1] for l in items('labels').get('labels') or [] if l.get('text') == 'N1'), None)
step('sch_edit_label', 'fusion_sch_edit_label', {'edits': [{'net': 'N1', 'by': [0, 2.54]}]},
     lambda _r: expect(any(l.get('text') == 'N1' and abs(l['at'][1] - label_y - 2.54) < 0.01 for l in items('labels').get('labels') or []), 'label did not move'))
step('sch_text', 'fusion_sch_text', {'texts': [{'text': 'DRILL NOTE', 'at': [20, 40]}]},
     lambda _r: expect('DRILL NOTE' in json.dumps(items('texts')), 'text not on the sheet'))
w = (pins('R4').get(('R4', 'B')) or {}).get('at')
wires = [x for x in items('wires').get('wires') or []]
stub = next((x for x in wires if w and ('%g,%g' % tuple(w)) in str(x.get('id'))), wires[0] if wires else {})
n_wires = len(wires)
step('sch_split_wire', 'fusion_sch_split_wire', {'id': stub.get('id')},
     lambda _r: expect(len(items('wires').get('wires') or []) == n_wires + 1, 'still %d wires' % n_wires))
junctions = items('junctions').get('junctions') or []
step('sch_split_wire left a junction', 'fusion_sch_items', {'kinds': ['junctions']}, lambda _r: expect(junctions, 'no junction on the sheet'))
step('sch_junction remove', 'fusion_sch_junction', {'remove': [(junctions or [{}])[0].get('id')]},
     lambda _r: expect(len(items('junctions').get('junctions') or []) == len(junctions) - 1, 'junction still there'))
wires = items('wires').get('wires') or []
step('sch_delete_wire', 'fusion_sch_delete_wire', {'ids': [(wires or [{}])[-1].get('id')]},
     lambda _r: expect(len(items('wires').get('wires') or []) < len(wires), 'wire still there'))

# ── wires and labels by point ──────────────────────────────────────────────────────────────────────
step('sch_wire', 'fusion_sch_wire', {'points': [[20, 200], [40, 200], [40, 220]], 'net': 'WIRED'},
     lambda r: expect(any(w.get('net') == 'WIRED' for w in items('wires').get('wires') or []) and len(r.get('wires') or []) == 2, 'wire not on the sheet'))
step('sch_wire diagonal', 'fusion_sch_wire', {'points': [[60, 200], [75, 215]], 'net': 'DIAG'},
     lambda _r: expect(sum(w.get('net') == 'DIAG' for w in items('wires').get('wires') or []) == 1, 'a diagonal wire came back in pieces'))
step('sch_label add', 'fusion_sch_label', {'text': 'WIRED', 'at': [30, 200]},
     lambda _r: expect(any(l.get('text') == 'WIRED' for l in items('labels').get('labels') or []), 'label not on the sheet'))
r = call('fusion_sch_label', {'text': 'OTHER', 'at': [30, 200]})
results.append(('sch_label wrong net refused', expect(r.get('errorCode') == 'net_mismatch', str(r.get('errorCode')))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
lab = next((l for l in items('labels').get('labels') or [] if l.get('text') == 'WIRED'), {})
step('sch_label remove', 'fusion_sch_label', {'remove': True, 'id': lab.get('id')},
     lambda _r: expect(not any(l.get('id') == lab.get('id') for l in items('labels').get('labels') or []), 'label still there'))
r = call('fusion_sch_no_connect', {'pin': 'R4.1'})
results.append(('sch_no_connect not_supported', expect(r.get('errorCode') in ('not_supported', 'unsupported') or r.get('supported') is False, json.dumps({k: r.get(k) for k in ('errorCode', 'success')}))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))

# ── moving and editing ─────────────────────────────────────────────────────────────────────────────
a0 = pins('R6')[('R6', 'A')]['at']
step('sch_move_region', 'fusion_sch_move_region', {'box': [a0[0] - 15, a0[1] - 6, a0[0] + 40, a0[1] + 6], 'by': [10.16, 0]},
     lambda _r: expect(abs(pins('R6')[('R6', 'A')]['at'][0] - a0[0] - 10.16) < 0.01, 'R6 did not move'))
step('sch_edit rotate', 'fusion_sch_edit', {'edits': [{'designator': 'R7', 'action': 'rotate', 'degrees': 90}]},
     lambda _r: expect((parts()['R7'].get('gates') or [{}])[0].get('rotation') in (90, 90.0), 'R7 not turned'))
step('sch_reset_fields', 'fusion_sch_reset_fields', {'references': ['R7']},
     lambda r: expect(r.get('reset') or r.get('untouched'), 'answered with neither reset nor untouched parts: %s' % json.dumps({k: r.get(k) for k in ('mutated', 'reset', 'untouched')})))
step('sch_fix_connectivity dry', 'fusion_sch_fix_connectivity', {},
     lambda r: expect(r.get('mutated') is False, 'a dry run changed the sheet'))

# ── buses ──────────────────────────────────────────────────────────────────────────────────────────
# a long bus under R8, whose pins are on a row above it, so an entry from each pin lands on the bus
r8 = pins('R8')
bus_y = r8[('R8', 'A')]['at'][1] - 20.32
step('sch_bus', 'fusion_sch_bus', {'label': 'D[0..1]', 'points': [[60, bus_y], [190, bus_y]]},
     lambda _r: expect(items('buses').get('buses'), 'no bus on the sheet'))
step('sch_connect_to_bus', 'fusion_sch_connect_to_bus', {'pins': ['R8.1', 'R8.2'], 'prefix': 'D', 'entry': 'down'},
     lambda _r: expect(net('R8', 'A') == 'D0' and net('R8', 'B') == 'D1', 'R8 on %s, %s' % (net('R8', 'A'), net('R8', 'B'))))
step('sch_bus_entry', 'fusion_sch_bus_entry', {'entries': [{'at': [70, bus_y + 2.54], 'direction': 'down-right', 'member': 'D0'}]},
     lambda _r: expect(items('busEntries').get('busEntries'), 'no bus entry on the sheet'))

# ── swapping ───────────────────────────────────────────────────────────────────────────────────────
n1 = (net('R1', 'A'), net('R1', 'B'))
step('sch_swap_part moved pins', 'fusion_sch_swap_part', {'library': LBR, 'symbol': 'RES2', 'designator': 'R1'},
     lambda r: expect(parts()['R1'].get('libReference') == 'RES2' and (net('R1', 'A'), net('R1', 'B')) == n1 and len(r.get('pinsJoined') or []) == 2, 'R1 %s %s' % (parts()['R1'].get('libReference'), r.get('pinsJoined'))))
step('sch_swap_part pin off its wire', 'fusion_sch_swap_part', {'library': LBR, 'symbol': 'RESY', 'designator': 'R1'},
     lambda r: expect((net('R1', 'A'), net('R1', 'B')) == n1, 'nets now %s' % ((net('R1', 'A'), net('R1', 'B')),)))
step('sch_swap_part prefix, keepComment', 'fusion_sch_swap_part', {'library': LBR, 'symbol': 'CAP', 'designator': 'R2', 'keepComment': True},
     lambda _r: expect(parts().get('R2', {}).get('libReference') == 'CAP' and parts()['R2'].get('comment') == '22k', 'R2 %s' % (parts().get('R2'),)))
step('board kept in step', 'fusion_cross_check', {},
     lambda r: expect(not [x for x in r.get('onSheetNotOnBoard') or [] if x != 'U2'] and not r.get('onBoardNotOnSheet'), json.dumps({k: r.get(k) for k in ('onSheetNotOnBoard', 'onBoardNotOnSheet')})))

# ── the rest ───────────────────────────────────────────────────────────────────────────────────────
step('sch_batch', 'fusion_sch_batch', {'ops': [{'op': 'sch_text', 'texts': [{'at': [25, 45], 'text': 'BATCH NOTE'}]}]},
     lambda _r: expect('BATCH NOTE' in json.dumps(items('texts')), 'batch text not on the sheet'))
def sheet_count():
    return len((call('fusion_sch_hierarchy', {}).get('tree') or {}).get('sheets') or [])


n = sheet_count()
step('sch_create', 'fusion_sch_create', {}, lambda _r: expect(sheet_count() == n + 1, 'sheets %d, was %d' % (sheet_count(), n)))

# ── real parts from libraries this drill did not make ──────────────────────────────────────────────
def foreign(path, on_machine):
    """Place real parts, wire one, swap it, and prove each change on the sheet AND on the board, by package."""
    import re
    import xml.etree.ElementTree as ET
    tag = os.path.basename(path)
    if on_machine:
        got = call('read_file', {'path': path})
        text = got.get('content') or (got.get('data') or {}).get('content')
        if not text:
            results.append(('%s readable' % tag, 'cannot read it on the Fusion machine'))
            print('%-34s FAIL  cannot read it' % tag)
            return
    else:
        text = open(path, encoding='utf-8').read()
        call('send_files', {'filePaths': [path], 'dest': D, 'destIsFolder': True, 'reason': 'sch_drill: a public library from its repo'})
        path = D + '/' + tag
    # a name already in use from another path is refused, by design: that's not this drill's failure to report
    probe = call('fusion_schematic_run', {'libraryPath': path, 'commands': ['WINDOW FIT;']})
    if probe.get('errorCode') == 'library_name_in_use':
        print('%-34s SKIP  %s' % (tag[:34], str(probe.get('error'))[:120]))
        return
    root = ET.fromstring(text.encode('utf-8'))
    sets = [d for d in root.iter('deviceset') if any(v.get('package') for v in d.iter('device')) and len(d.findall('gates/gate')) == 1
            and not any('(' in (v.get('name') or '') for v in d.iter('device'))]
    picks = [sets[i] for i in sorted({0, len(sets) // 2, len(sets) - 1})] if sets else []
    if not picks:
        results.append(('%s has parts' % tag, 'no single-gate part with a footprint'))
        return
    pkg_of = {d.get('name'): next(v.get('package') for v in d.iter('device') if v.get('package')) for d in sets}
    base = 9000 + 500 * len(results)
    for k, d in enumerate(picks):
        name, ref = d.get('name'), 'F%d' % (len(results) + k)
        had = set(parts())
        y = 8000 + 300 * ((len(results) + k) % 20)

        def placed(_r, ref=ref, name=name, had=had):
            p_ = parts()
            if ref not in p_ or ref in had:
                return '%s is not a new part on the sheet' % ref
            if p_[ref].get('libReference', '').split('@')[0].upper() != name.upper():
                return '%s is %s, not %s' % (ref, p_[ref].get('libReference'), name)
            if not pins(ref):
                return '%s has no pins' % ref
            return None
        step('%s: place %s' % (tag[:12], name[:14]), 'fusion_sch_place_part', {'library': path, 'symbol': name, 'designator': ref, 'x': base + 700 * k, 'y': y}, placed)
    ref = None
    rows = parts()
    for d in picks:
        ref = next((r for r, c in rows.items() if r.startswith('F') and c.get('libReference', '').split('@')[0].upper() == d.get('name').upper()), None)
        if ref and pins(ref):
            break
    if not ref:
        return
    first = sorted(pins(ref))[0][1]
    step('%s: connect' % tag[:12], 'fusion_sch_connect', {'connections': [{'designator': ref, 'pin': first, 'net': 'FN%d' % len(results)}]},
         lambda _r: expect(net(ref, first), '%s.%s is on no net' % (ref, first)))
    held = net(ref, first)
    pins_now = {p_[1] for p_ in pins(ref)}
    sym = {s_.get('name'): {q.get('name') for q in s_.iter('pin')} for s_ in root.iter('symbol')}
    same = next((d for d in sets if d.get('name').upper() != rows[ref].get('libReference', '').split('@')[0].upper()
                 and sym.get(d.find('gates/gate').get('symbol')) == pins_now), None)
    if same is not None:
        step('%s: swap to %s' % (tag[:12], same.get('name')[:12]), 'fusion_sch_swap_part', {'library': path, 'symbol': same.get('name'), 'designator': ref},
             lambda _r: expect(parts().get(ref, {}).get('libReference', '').split('@')[0].upper() == same.get('name').upper() and net(ref, first) == held,
                               '%s is %s on net %s, was %s' % (ref, parts().get(ref, {}).get('libReference'), net(ref, first), held)))
    # on the board too, with the footprint that device names: a part with the wrong package or none at all fails here
    call('fusion_show_2d_board', {'expectDocument': 'Untitled'})
    comps = {c.get('ref'): c for c in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('components') or []}
    call('fusion_show_schematic', {'expectDocument': 'Untitled'})
    wrong = []
    for r_, c in parts().items():
        if not r_.startswith('F'):
            continue
        want = pkg_of.get(c.get('libReference', '').split('@')[0]) or next((v for k_, v in pkg_of.items() if k_.upper() == c.get('libReference', '').split('@')[0].upper()), None)
        have = (comps.get(r_) or {}).get('pattern')
        if not comps.get(r_) or (want and have and have.upper() != want.upper()):
            wrong.append('%s: board has %s, device names %s' % (r_, have, want))
    results.append(('%s: on the board, right package' % tag[:12], expect(not wrong, '; '.join(wrong)[:170])))
    print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))


foreign(os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'AdomBasicParts_full.lbr'), False)
for extra in opts.foreign:
    foreign(extra, True)

# ── the board: tracks, models, add-ins ─────────────────────────────────────────────────────────────
call('fusion_show_2d_board', {'expectDocument': 'Untitled'})
step('model_check', 'fusion_model_check', {'expectDocument': 'Untitled'},
     lambda r: expect(r.get('checked', 0) >= 2 and all('models' in p_ for p_ in r.get('parts') or []), json.dumps(r.get('parts'))[:120]))
step('extensions list', 'fusion_extensions', {},
     lambda r: expect(any(x.get('id') == 'AdomBridge' for x in r.get('extensions') or []), 'AdomBridge not listed'))
r = call('fusion_extensions', {'action': 'install', 'id': 'x'})
results.append(('extensions install refused', expect(r.get('errorCode') == 'not_supported', str(r.get('errorCode')))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
r = call('fusion_place_parts', {'expectDocument': 'Untitled', 'parts': [{'library': LBR, 'footprint': 'R0603', 'ref': 'R50', 'x': 5, 'y': 5}]})
results.append(('place_parts linked refused', expect(r.get('errorCode') == 'linked_board', '%s %s' % (r.get('errorCode'), str(r.get('error'))[:90]))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
# a track to edit: R1 and R2 share no net yet, so draw one on a part's net with a plain wire on Top
pad = next((p_ for p_ in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('pads') or [] if p_.get('net') == 'NB'), None)
if pad:
    x0, y0 = pad.get('x'), pad.get('y')
    call('fusion_electron_run', {'command': "LAYER 1; CHANGE WIDTH 0.2mm; WIRE 'NB' (%gmm %gmm) (%gmm %gmm);" % (x0, y0, x0, y0 + 4)})
tracks = [t for t in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('tracks') or [] if t.get('net') == 'NB']
tid = tracks[0].get('id') if tracks else None
results.append(('a track to edit, with an id', expect(tid, 'no track on NB: %d tracks' % len(tracks))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))


def track_widths():
    return {t.get('id'): t.get('width') for t in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('tracks') or [] if t.get('net') == 'NB'}


step('edit_tracks width', 'fusion_edit_tracks', {'expectDocument': 'Untitled', 'tracks': [{'id': tid, 'width': 0.4}]},
     lambda r: expect(r.get('verified') and abs((track_widths().get(tid) or 0) - 0.4) < 1e-6, 'width now %s' % track_widths().get(tid)))
step('edit_tracks end, along its line', 'fusion_edit_tracks', {'expectDocument': 'Untitled', 'tracks': [{'id': tid, 'end': [x0, y0 + 6] if pad else [0, 0]}]},
     lambda r: expect(r.get('verified') and any(abs(t['end']['y'] - y0 - 6) < 1e-3 or abs(t['start']['y'] - y0 - 6) < 1e-3 for t in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('tracks') or [] if t.get('net') == 'NB'), 'end did not move'))
now = next((t.get('id') for t in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('tracks') or [] if t.get('net') == 'NB'), None)
before_angle = len(track_widths())
r = call('fusion_edit_tracks', {'expectDocument': 'Untitled', 'tracks': [{'id': now, 'end': [x0 + 3, y0 + 9]}]})
tracks_now = [t for t in call('fusion_routing_state', {'expectDocument': 'Untitled'}).get('tracks') or [] if t.get('net') == 'NB']
results.append(('edit_tracks end, at an angle', expect(r.get('verified') and len(track_widths()) == before_angle and any(abs(t['end']['x'] - x0 - 3) < 1e-3 and abs(t['end']['y'] - y0 - 9) < 1e-3 or abs(t['start']['x'] - x0 - 3) < 1e-3 and abs(t['start']['y'] - y0 - 9) < 1e-3 for t in tracks_now), '%s, %d tracks, NB %s' % (r.get('errorCode') or r.get('error'), len(track_widths()), tracks_now[:2]))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))
r = call('fusion_edit_tracks', {'expectDocument': 'Untitled', 'tracks': [{'id': 'track:NOPE:1:0,0,1,1', 'width': 0.3}]})
results.append(('edit_tracks unknown id', expect(r.get('errorCode') == 'nothing_to_do', str(r.get('errorCode')))))
print('%-34s %s' % (results[-1][0], 'PASS' if results[-1][1] is None else 'FAIL  ' + str(results[-1][1])))

dialogs = call('fusion_check_dialogs', {}).get('count')
results.append(('no dialog left', expect(not dialogs, '%s dialog(s) open' % dialogs)))
if not opts.keep:
    for _ in range(4):
        call('fusion_close_document', {'name': 'Untitled'})
bad = [n for n, why in results if why]
print('\n%d PASS, %d FAIL%s' % (len(results) - len(bad), len(bad), ': ' + ', '.join(bad) if bad else ''))
sys.exit(1 if bad else 0)
