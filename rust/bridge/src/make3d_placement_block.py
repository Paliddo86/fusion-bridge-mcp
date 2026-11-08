import math as _pmath
_pt=({TX},{TY},{TZ}); _pr=({RX},{RY},{RZ})
_pmat=adsk.core.Matrix3D.create()
for _ax,_deg in ((adsk.core.Vector3D.create(1,0,0),_pr[0]),(adsk.core.Vector3D.create(0,1,0),_pr[1]),(adsk.core.Vector3D.create(0,0,1),_pr[2])):
    if _deg:
        _prot=adsk.core.Matrix3D.create(); _prot.setToRotation(_deg*_pmath.pi/180.0,_ax,adsk.core.Point3D.create(0,0,0)); _pmat.transformBy(_prot)
_pmat.translation=adsk.core.Vector3D.create(_pt[0]/10.0,_pt[1]/10.0,_pt[2]/10.0)
oc.transform2=_pmat
_pgot=oc.transform2.asArray(); _pwant=_pmat.asArray()
_pok=all(abs(a-b)<1e-6 for a,b in zip(_pgot,_pwant))
PLACEMENT_RES={'requested':{'translationMm':list(_pt),'rotationDeg':list(_pr),'scale':1,'order':'rotate about X, then Y, then Z (extrinsic, about the origin), then translate'},
 'appliedMatrix':[[round(_pgot[i*4+j]*(10.0 if j==3 and i<3 else 1.0),6) for j in range(4)] for i in range(4)],
 'matrixUnits':'row-major 4x4; rotation part unitless, translation column in mm',
 'verification':'verified' if _pok else 'mismatch','heuristics':{'orient':False,'centering':False,'pin1':'skipped'}}
try: d.snapshots.add() if d.snapshots.hasPendingSnapshot else None
except Exception: pass
