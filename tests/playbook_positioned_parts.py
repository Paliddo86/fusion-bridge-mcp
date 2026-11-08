#!/usr/bin/env python3
"""Live check of what skills/fusion-design-efficiency item 4 claims, on a new unsaved design (never anyone's): a positioned part
stays where it is through a compute, and does not follow when the part it sits on changes size, while a jointed part does.
  python3 tests/playbook_positioned_parts.py --target DESKTOP-BAR6S0F
"""
import argparse, json, subprocess, sys
ap = argparse.ArgumentParser(); ap.add_argument('--target', default='DESKTOP-BAR6S0F'); a = ap.parse_args()
SCRIPT = r'''
import adsk.core, adsk.fusion
app = adsk.core.Application.get()
doc = app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
out = {}
try:
    d = adsk.fusion.Design.cast(app.activeProduct); d.designType = adsk.fusion.DesignTypes.ParametricDesignType
    root = d.rootComponent
    vi = adsk.core.ValueInput
    d.userParameters.add('base_h', vi.createByString('20 mm'), 'mm', 'fusion-lane playbook test')
    def box(name, x, y, side, height, z=0.0):
        occ = root.occurrences.addNewComponent(adsk.core.Matrix3D.create())
        c = occ.component; c.name = name
        sk = c.sketches.add(c.xYConstructionPlane)
        sk.sketchCurves.sketchLines.addTwoPointRectangle(adsk.core.Point3D.create(x, y, 0), adsk.core.Point3D.create(x + side, y + side, 0))
        ex = c.features.extrudeFeatures.addSimple(sk.profiles.item(0), height, adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
        return occ, ex.bodies.item(0)
    base, base_body = box('Base', 0, 0, 8.0, vi.createByString('base_h'))
    # positioned: placed by its transform on top of the base, no joint
    pos, _ = box('Positioned', 0.5, 0.5, 1.0, vi.createByReal(1.0))
    m = adsk.core.Matrix3D.create(); m.translation = adsk.core.Vector3D.create(0, 0, 2.0); pos.transform = m
    if d.snapshots.hasPendingSnapshot: d.snapshots.add()
    # jointed: rigid joint to the base's top face
    jnt, jb = box('Jointed', 4.0, 4.0, 1.0, vi.createByReal(1.0))
    top = max((f for f in base.bRepBodies.item(0).faces), key=lambda f: f.boundingBox.maxPoint.z + f.boundingBox.minPoint.z)
    low = min((f for f in jnt.bRepBodies.item(0).faces), key=lambda f: f.boundingBox.maxPoint.z + f.boundingBox.minPoint.z)
    ji = root.joints.createInput(adsk.fusion.JointGeometry.createByPlanarFace(low, None, adsk.fusion.JointKeyPointTypes.CenterKeyPoint),
                                 adsk.fusion.JointGeometry.createByPlanarFace(top, None, adsk.fusion.JointKeyPointTypes.CenterKeyPoint))
    ji.isFlipped = True   # the two faces look at each other, so the part sits on top rather than inside
    ji.setAsRigidJointMotion(); root.joints.add(ji)
    z = lambda o: (round(o.bRepBodies.item(0).boundingBox.minPoint.z * 10, 3), round(o.bRepBodies.item(0).boundingBox.maxPoint.z * 10, 3))
    out['before'] = {'baseTopMm': z(base)[1], 'positionedBottomMm': z(pos)[0], 'jointedBottomMm': z(jnt)[0]}
    d.computeAll()
    out['afterCompute'] = {'baseTopMm': z(base)[1], 'positionedBottomMm': z(pos)[0], 'jointedBottomMm': z(jnt)[0]}
    d.userParameters.itemByName('base_h').expression = '40 mm'
    d.computeAll(); adsk.doEvents()
    out['afterResize'] = {'baseTopMm': z(base)[1], 'positionedBottomMm': z(pos)[0], 'jointedBottomMm': z(jnt)[0]}
finally:
    doc.close(False)
result = out
'''
o = subprocess.run(['adom-bridge', '--target', a.target, '--ai-thread', 'fusion-lane', 'fusion_run_modeling_script', json.dumps({'script': SCRIPT})], capture_output=True, text=True).stdout
r = json.loads(o); print(json.dumps({k: r.get(k) for k in ('success', 'errorCode', 'error', 'result', 'data')}, indent=1)[:1800])
res = r.get('result') or (r.get('data') or {}).get('result') or {}
if isinstance(res, str): res = json.loads(res)
try:
    b, c, s = res['before'], res['afterCompute'], res['afterResize']
except Exception:
    print('FAIL: the test did not finish'); sys.exit(1)
checks = [('base starts 20 mm tall', abs(b['baseTopMm'] - 20) < .01), ('positioned part sits on it', abs(b['positionedBottomMm'] - 20) < .01), ('jointed part sits on it', abs(b['jointedBottomMm'] - 20) < .01),
          ('positioned part stays through a compute', abs(c['positionedBottomMm'] - 20) < .01), ('base grew to 40 mm', abs(s['baseTopMm'] - 40) < .01),
          ('positioned part did NOT follow', abs(s['positionedBottomMm'] - 20) < .01), ('jointed part DID follow', abs(s['jointedBottomMm'] - 40) < .01)]
for n, ok in checks: print('%-42s %s' % (n, 'PASS' if ok else 'FAIL'))
sys.exit(0 if all(ok for _, ok in checks) else 1)
