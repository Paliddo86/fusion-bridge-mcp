
import sys, glob, os, importlib
import adsk.core, adsk.fusion

# Locate EPG across install conventions + versions (NEVER hardcode the webdeploy hash).
_bases = []
for env in ('ProgramFiles', 'ProgramFiles(x86)', 'ProgramW6432', 'LOCALAPPDATA'):
    root = os.environ.get(env)
    if root:
        _bases += glob.glob(os.path.join(root, 'Autodesk', 'webdeploy', 'production', '*',
                                         'Api', 'InternalAddins', 'ElectronicsPackageGenerator'))
if not _bases:
    raise RuntimeError('EPG_NOT_FOUND: ElectronicsPackageGenerator not present in this Fusion install')
epg_dir = max(_bases, key=os.path.getmtime)
parent = os.path.dirname(epg_dir)
if parent not in sys.path:
    sys.path.insert(0, parent)

mod = importlib.import_module('ElectronicsPackageGenerator.Scripts3d.@@RAW_PKG_TYPE@@')

doc = app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
design = adsk.fusion.Design.cast(app.activeProduct)
mod.runWithInput(@@PARAMS@@, design)
root = design.rootComponent

inv = [{'name': b.name, 'vol': round(b.volume, 6)} for b in root.bRepBodies]
etched = None
if @@ETCH@@:
    # FIND THE TOP SURFACE the robust way (John's algorithm, 2026-07-04):
    # 1. bounding box of the WHOLE chip (all bodies);
    # 2. only faces whose centroid sits in the TOP 5% band of that bbox count;
    # 3. among those upward planar faces take the LARGEST AREA (the molded body
    #    top beats terminal tops that tie it on height);
    # 4. lay the text out with a 10% margin inside that face.
    gmin, gmax = 1e9, -1e9
    for b in root.bRepBodies:
        bb = b.boundingBox
        gmin = min(gmin, bb.minPoint.z)
        gmax = max(gmax, bb.maxPoint.z)
    band_z = gmax - 0.05 * (gmax - gmin)
    top_face, top_area = None, -1.0
    for b in root.bRepBodies:
        for f in b.faces:
            if isinstance(f.geometry, adsk.core.Plane) and f.geometry.normal.z > 0.9                     and f.centroid.z >= band_z and f.area > top_area:
                top_face, top_area = f, f.area
    if top_face is None:
        raise RuntimeError('ETCH_NO_TOP_FACE: no upward planar face in the top 5% of the chip bbox')
    body = top_face.body
    sk = root.sketches.add(top_face)
    center = sk.modelToSketchSpace(top_face.centroid)
    # Measure the face in SKETCH space (model-space bbox axes can be SWAPPED vs
    # the sketch axes the text lays out in). Project the model bbox corners.
    bb = top_face.boundingBox
    p_min = sk.modelToSketchSpace(bb.minPoint)
    p_max = sk.modelToSketchSpace(bb.maxPoint)
    face_w = abs(p_max.x - p_min.x)   # extent along sketch-x (the default text baseline)
    face_h = abs(p_max.y - p_min.y)   # extent along sketch-y
    # ALWAYS run the text along the LONGEST axis of the top face (most room for
    # the marking - John's rule, 2026-07-04). If the long axis is sketch-y,
    # rotate the text 90deg rather than squeezing it onto the short axis.
    import math as _math
    rot_deg = 90 if face_h > face_w else 0
    long_len = max(face_w, face_h)
    short_len = min(face_w, face_h)
    w = long_len * 0.80               # 10% margin each side along the long axis
    box_h_cap = short_len * 0.80      # 10% margin each side across it
    # multi-line support (e.g. MPN + variant on line 2): fit per-line
    lines = @@ETCH@@.split(chr(10))
    n_lines = max(1, len(lines))
    max_chars = max(1, max(len(l) for l in lines))
    # width-aware auto-fit: glyph ~0.75*h wide; line block ~1.35*h per line
    h = (@@ETCH_HEIGHT_MM@@ / 10.0) if @@ETCH_HEIGHT_MM@@ else         max(0.015, min(box_h_cap / (1.35 * n_lines), w / (0.75 * max_chars)))
    if rot_deg:
        c1 = adsk.core.Point3D.create(center.x - box_h_cap/2, center.y - w/2, 0)
        c2 = adsk.core.Point3D.create(center.x + box_h_cap/2, center.y + w/2, 0)
    else:
        c1 = adsk.core.Point3D.create(center.x - w/2, center.y - box_h_cap/2, 0)
        c2 = adsk.core.Point3D.create(center.x + w/2, center.y + box_h_cap/2, 0)
    ti = sk.sketchTexts.createInput2(@@ETCH@@, h)
    ti.setAsMultiLine(c1, c2,
                      adsk.core.HorizontalAlignments.CenterHorizontalAlignment,
                      adsk.core.VerticalAlignments.MiddleVerticalAlignment, 0)
    if rot_deg:
        try:
            ti.angle = _math.pi / 2.0
        except Exception:
            rot_deg = 0  # older API without angle - fall back to unrotated
    txt = sk.sketchTexts.add(ti)
    # MEASURE-AND-CORRECT (the margin is a CONTRACT): font metrics vary, so the
    # 0.75h glyph estimate can under-shoot ('ADOM-A' rendered edge-to-edge). After
    # placing, measure the text's real bbox; if it busts the 10%-margin box,
    # recreate at the scaled-down height. Never trust an estimate you can measure.
    fit_iterations = 0
    measured_w = None
    for _pass in range(2):
        tb = txt.boundingBox
        t_ext_x = abs(tb.maxPoint.x - tb.minPoint.x)
        t_ext_y = abs(tb.maxPoint.y - tb.minPoint.y)
        measured_w = max(t_ext_x, t_ext_y)   # the baseline-axis extent
        measured_c = min(t_ext_x, t_ext_y)   # the cross-axis extent (line stack)
        if measured_w <= w and measured_c <= box_h_cap:
            break
        scale = min(w / max(measured_w, 1e-6), box_h_cap / max(measured_c, 1e-6)) * 0.95
        h = max(0.008, h * scale)
        txt.deleteMe()
        ti = sk.sketchTexts.createInput2(@@ETCH@@, h)
        ti.setAsMultiLine(c1, c2,
                          adsk.core.HorizontalAlignments.CenterHorizontalAlignment,
                          adsk.core.VerticalAlignments.MiddleVerticalAlignment, 0)
        if rot_deg:
            try:
                ti.angle = _math.pi / 2.0
            except Exception:
                pass
        txt = sk.sketchTexts.add(ti)
        fit_iterations += 1
    marking_geom = {
        'chipBBox': {'min': [round(gmin,5), 0, 0], 'maxZ': round(gmax,5)},
        'chipBBoxZ': {'min': round(gmin,5), 'max': round(gmax,5)},
        'topBandZ': round(band_z,5),
        'topFace': {'area': round(top_area,6), 'centroidZ': round(top_face.centroid.z,5),
                    'sketchExtents': {'x': round(face_w,5), 'y': round(face_h,5)},
                    'body': body.name},
        'longAxis': 'sketch-y' if face_h > face_w else 'sketch-x',
        'rotatedDeg': rot_deg,
        'textBox': {'c1': [round(c1.x,5), round(c1.y,5)], 'c2': [round(c2.x,5), round(c2.y,5)],
                    'marginPct': 10},
        'textHeightMm': round(h*10,4), 'lines': n_lines, 'maxLineChars': max_chars,
        'heightAuto': not bool(@@ETCH_HEIGHT_MM@@),
        'textWidthMeasuredMm': round(measured_w*10,4) if measured_w else None,
        'fitIterations': fit_iterations,
        'why': {'band': 'top 5% of whole-chip bbox excludes terminal tops that tie the body',
                'largestArea': 'molded body top beats small terminal faces in the band',
                'longAxis': 'text runs along the longest face axis for maximum room',
                'fit': 'h = min(boxH/(1.35*lines), boxW/(0.75*maxChars)) - glyphs ~0.75h wide'},
    }
    if @@ETCH_STYLE@@ == 'engraved':
        # engraved (sunken) cut - the pre-2026-07-04 default
        ext_in = root.features.extrudeFeatures.createInput(txt, adsk.fusion.FeatureOperations.CutFeatureOperation)
        ext_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-@@ETCH_DEPTH_CM@@))
        ext_in.participantBodies = [body]
        root.features.extrudeFeatures.add(ext_in)
    else:
        # RAISED WHITE marking (default): humans need CONTRAST - a thin positive
        # extrude painted white on the dark body reads like real silkscreen ink,
        # far better than a same-color emboss. Reuses EPG's own rgb-appearance
        # utility so the white survives Fusion rendering (+ colored STEP).
        ext_in = root.features.extrudeFeatures.createInput(txt, adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
        ext_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(@@ETCH_DEPTH_CM@@))
        mark = root.features.extrudeFeatures.add(ext_in)
        from ElectronicsPackageGenerator.Utilities import addin_utility as _au
        for i in range(mark.bodies.count):
            mb = mark.bodies.item(i)
            mb.name = 'Marking' if i == 0 else 'Marking' + str(i + 1)
            _au.apply_rgb_appearance(app, design, mb, 255, 255, 255, 'AdomMarkingWhite')
    etched = @@ETCH@@

step_path = None
if @@OUTPUT_STEP@@:
    # Fusion's STEP export writes nothing into a folder that does not exist: make it first
    os.makedirs(os.path.dirname(os.path.abspath(@@OUTPUT_STEP@@)), exist_ok=True)
    em = design.exportManager
    opts = em.createSTEPExportOptions(@@OUTPUT_STEP@@)
    em.execute(opts)
    if not os.path.isfile(@@OUTPUT_STEP@@):
        raise RuntimeError('Fusion did not write the STEP file %s' % @@OUTPUT_STEP@@)
    step_path = @@OUTPUT_STEP@@

# SIDECAR MANIFEST: every setting we picked + WHY + the bboxes we calculated,
# so a marking refresh (e.g. adding a variant name as a 2nd line) can rework the
# text without re-deriving anything. Written next to the STEP as <step>.manifest.json.
manifest = {
    'schema': 'adom-fusion-generate-package/1',
    'bridgeVersion': @@BRIDGE_VERSION@@,
    'type': @@PKG_TYPE@@,
    'paramsCm': @@PARAMS@@,
    'paramsNote': 'paramsCm are EPG-native cm (mm inputs were /10)',
    'marking': ({'text': @@ETCH@@, 'style': @@ETCH_STYLE@@,
                'depthMm': @@ETCH_DEPTH_CM@@ * 10, **marking_geom} if etched else None),
    'bodies': inv,
    'outputs': {'step': step_path},
    'refresh': {
        'howTo': 'To re-mark (e.g. add a variant on line 2): call fusion_generate_package again '
                 'with the SAME type+paramsCm (unitsCm:true) and the new multi-line etch text '
                 '(join lines with a newline). The generator is deterministic, so geometry and '
                 'placement reproduce exactly; this manifest carries the boxes/height the last '
                 'run chose for comparison.',
        'example': {'type': @@PKG_TYPE@@, 'unitsCm': True, 'params': 'paramsCm from this file',
                    'etch': (@@ETCH@@ + chr(10) + 'VARIANT') if etched else 'MPN' + chr(10) + 'VARIANT'},
    },
}
manifest_path = None
if step_path:
    manifest_path = step_path + '.manifest.json'
    import json as _json
    with open(manifest_path, 'w', encoding='utf-8') as _f:
        _json.dump(manifest, _f, indent=2)

vp = app.activeViewport
cam = vp.camera
cam.isSmoothTransition = False
cam.target = adsk.core.Point3D.create(0, 0, 0)
cam.eye = adsk.core.Point3D.create(0.25, -0.35, 0.45)
cam.upVector = adsk.core.Vector3D.create(0, 0, 1)
cam.isFitView = True
vp.camera = cam
vp.refresh()

result = {'type': @@PKG_TYPE@@, 'bodies': inv, 'etched': etched, 'stepPath': step_path,
           'manifest': manifest, 'manifestPath': manifest_path,
           'epgDir': epg_dir, 'doc': app.activeDocument.name}
