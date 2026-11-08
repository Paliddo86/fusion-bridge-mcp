
import adsk.core, adsk.fusion

_des = adsk.fusion.Design.cast(app.activeProduct)
_WORLD = __WORLD__
_OCC = __OCC__

def _mm(v):      return round(v * 10.0, 4)      # cm  -> mm
def _mm3(v):     return round(v * 1000.0, 4)    # cm3 -> mm3
def _mm2(v):     return round(v * 100.0, 4)     # cm2 -> mm2

def _describe(body, owner):
    bb = body.boundingBox
    mn, mx = bb.minPoint, bb.maxPoint
    cyl = 0
    try:
        for f in body.faces:
            g = f.geometry
            if g and g.surfaceType == adsk.core.SurfaceTypes.CylinderSurfaceType:
                cyl += 1
    except Exception:
        cyl = -1
    def _nm(o):
        try:    return o.name
        except Exception: return None
    return {
        "name": body.name,
        "owner": owner,
        "bbox": {"x": [_mm(mn.x), _mm(mx.x)],
                 "y": [_mm(mn.y), _mm(mx.y)],
                 "z": [_mm(mn.z), _mm(mx.z)]},
        "sizeMm": [_mm(mx.x - mn.x), _mm(mx.y - mn.y), _mm(mx.z - mn.z)],
        "volumeMm3": _mm3(body.volume),
        "areaMm2": _mm2(body.area),
        "faceCount": body.faces.count,
        "cylindricalFaceCount": cyl,
        "appearance": _nm(body.appearance),
        "material": _nm(body.material),
        "isVisible": body.isVisible,
    }

rows = []
root = _des.rootComponent
for b in root.bRepBodies:
    rows.append(_describe(b, root.name))

if _WORLD:
    # Bodies fetched THROUGH an occurrence are proxies whose boundingBox is already resolved
    # into assembly space, so occurrence placement is verified without any matrix maths here.
    for occ in root.allOccurrences:
        if _OCC and _OCC.lower() not in occ.name.lower():
            continue
        try:
            for b in occ.bRepBodies:
                rows.append(_describe(b, occ.name))
        except Exception:
            pass

result = {"units": "mm", "worldSpace": bool(_WORLD),
          "documentName": app.activeDocument.name if app.activeDocument else None,
          "bodyCount": len(rows), "bodies": rows}
