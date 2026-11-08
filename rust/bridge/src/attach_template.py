import adsk.core, adsk.fusion, math
res={}
d=adsk.fusion.Design.cast(app.activeProduct); root=d.rootComponent
im=app.importManager
im.importToTarget(im.createSTEPImportOptions('{MODEL_PATH}'), root)
oc=root.occurrences.item(root.occurrences.count-1); bb=oc.boundingBox
dx=bb.maxPoint.x-bb.minPoint.x; dy=bb.maxPoint.y-bb.minPoint.y; dz=bb.maxPoint.z-bb.minPoint.z
{ORIENT_BLOCK}if axis:
    mat=oc.transform2.copy(); rot=adsk.core.Matrix3D.create()
    rot.setToRotation(math.pi/2, adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0,0,0))
    mat.transformBy(rot); oc.transform2=mat
    d.snapshots.add() if d.snapshots.hasPendingSnapshot else None
res['dims_mm']=[round(dx*10,2),round(dy*10,2),round(dz*10,2)]
names=[]; hidden=[]
def _layer(n):
    n=(n or '').split(':')[0]
    return n.startswith('Adom.') and n in LAYERS_OFF
def _walk(comp, group, hide):
    for i in range(comp.bRepBodies.count):
        b=comp.bRepBodies.item(i); names.append((group or '')+'/'+b.name)
        if hide or _layer(b.name):
            try:
                b.isLightBulbOn=False; hidden.append((group or '')+'/'+b.name)
            except Exception: pass
    for j in range(comp.occurrences.count):
        o=comp.occurrences.item(j); on=(o.component.name or o.name).split(':')[0]
        g=group or (on if on.startswith('Adom.') else '')
        h=hide or _layer(on)
        if h and not hide:
            try: o.isLightBulbOn=False
            except Exception: pass
        _walk(o.component, g, h)
LAYERS_OFF=set({LAYERS_OFF})
try:
    _walk(oc.component, '', False)
except Exception as _e:
    res['layersError']=str(_e)[:120]
res['bodies']=names; res['layersHidden']=hidden
result=res
