import adsk.core, adsk.fusion, math
app=adsk.core.Application.get(); doc=app.activeDocument
d=adsk.fusion.Design.cast(app.activeProduct); root=d.rootComponent
im=app.importManager
im.importToTarget(im.createSTEPImportOptions('{MODEL_PATH}'), root)
oc=root.occurrences.item(root.occurrences.count-1); bb=oc.boundingBox
dx=bb.maxPoint.x-bb.minPoint.x; dy=bb.maxPoint.y-bb.minPoint.y; dz=bb.maxPoint.z-bb.minPoint.z
{ORIENT_BLOCK}PIN1={PIN1}
PIN1_SKIP={PIN1_SKIP}
res_pin1={'pad1_mm': {PAD1}, 'mark_mm': None, 'rotatedDeg': 0, 'status': 'unchecked'}
def _mark(occ):
    bb2=occ.boundingBox; zmax=bb2.maxPoint.z; best=None
    def _w(comp, depth):
        nonlocal best
        if depth>6: return
        for i in range(comp.bRepBodies.count):
            b=comp.bRepBodies.item(i)
            try: vol=b.volume
            except Exception: vol=0
            if vol*1000 < 5: continue
            for e in b.edges:
                try:
                    g=e.geometry
                    if g.objectType not in (adsk.core.Circle3D.classType(), adsk.core.Arc3D.classType()): continue
                    c=g.center
                    if abs(c.z-zmax)>0.03 or g.radius>0.1: continue
                    score=abs(c.x)+abs(c.y)
                    if best is None or score>best[0]: best=(score,c.x,c.y)
                except Exception: pass
        for j in range(comp.occurrences.count): _w(comp.occurrences.item(j).component, depth+1)
    _w(occ.component, 0)
    return None if best is None else (best[1],best[2])
try:
    mk=_mark(oc)
    if mk: res_pin1['mark_mm']=[round(mk[0]*10,2), round(mk[1]*10,2)]
    if mk and PIN1:
        import math as _m
        want=_m.atan2(PIN1[1],PIN1[0]); have=_m.atan2(mk[1],mk[0])
        k=int(round(((want-have)/(_m.pi/2)))) % 4
        if k:
            mat=oc.transform2.copy(); r=adsk.core.Matrix3D.create()
            r.setToRotation(k*_m.pi/2, adsk.core.Vector3D.create(0,0,1), adsk.core.Point3D.create(0,0,0))
            mat.transformBy(r); oc.transform2=mat
            res_pin1['rotatedDeg']=k*90
        res_pin1['status']='rotated' if k else 'aligned'
    elif PIN1: res_pin1['status']='no_mark_on_chip'
    else: res_pin1['status']='no_pad1_in_lbr'
except Exception as _e: res_pin1['status']='error: '+str(_e)[:80]
if PIN1_SKIP: res_pin1['status']=PIN1_SKIP
try:
 app.activeViewport.fit()
except: pass
def _sub(p,nm):
 import time as _ts
 for _try in range(4):
  for i in range(p.dataFolders.count):
   if str(p.dataFolders.item(i).name).lower()==str(nm).lower(): return p.dataFolders.item(i)
  try: return p.dataFolders.add(nm)
  except Exception as _fe:
   if 'CB_NAE' not in str(_fe) and 'same name' not in str(_fe): raise
   adsk.doEvents(); _ts.sleep(2.0)
 raise RuntimeError('cloud folder %s exists but cannot be resolved' % nm)
res={}
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
res['pin1']=res_pin1
res['placement']=globals().get('PLACEMENT_RES')
proj=None
try: proj=app.data.activeProject
except Exception as e: res['projErr']=str(e)[:60]
if not proj:
 try:
  hubs=app.data.dataHubs
  for hi in range(hubs.count):
   ps=hubs.item(hi).dataProjects
   for pj2 in range(ps.count):
    p2=ps.item(pj2)
    ok=False
    try: ok=(p2.id=='{DEFAULT_PROJECT}')
    except Exception: ok=False
    if not ok:
     try: ok=(p2.name=='Main')
     except Exception: continue
    if ok: proj=p2; break
   if proj: break
 except Exception as e: res['projErr']=str(e)[:60]
if not proj:
 try:
  hubs=app.data.dataHubs
  for hi in range(hubs.count):
   ps=hubs.item(hi).dataProjects
   for pj2 in range(ps.count):
    try:
     p2=ps.item(pj2); p2.rootFolder
     proj=p2; break
    except Exception: continue
   if proj: break
  if not proj:
   for hi in range(hubs.count):
    try:
     proj=hubs.item(hi).dataProjects.add('Adom'); break
    except Exception as e: res['createErr']=str(e)[:60]
 except Exception as e: res['projErr2']=str(e)[:60]
if not proj: res['errorCode']='no_cloud_project'
try:
 if proj is None: raise RuntimeError('no usable cloud project on this account')
 rf=proj.rootFolder
 wsf=_sub(_sub(rf,'Adom AI Workspace'),'{TASK}')
 try:
  doc.saveAs('{NAME}', wsf, '3d package (footprint+chip)', '')
 except Exception as _se:
  if 'CB_NAE' in str(_se) or 'same name' in str(_se):
   import time as _t0; _nm='{NAME}-'+_t0.strftime('%m%d%H%M%S')
   doc.saveAs(_nm, wsf, '3d package (footprint+chip)', ''); res['renamedTo']=_nm
  else: raise
 import time as _tt
 fid=None
 for w in range(40):
  try: fid=doc.dataFile.id if doc.dataFile else None
  except Exception: fid=None
  if fid and 'dm.lineage:' in str(fid): break
  adsk.doEvents(); _tt.sleep(1.5)
 res['path']=fid
 res['dims']=[round(dx*10,2),round(dy*10,2),round(dz*10,2)]
except Exception as e: res['err']=str(e)[:80]
result=res
