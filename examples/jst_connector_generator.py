"""Parametric JST connector generator — runs via fusion_run_modeling_script.

Builds a JST-family through-hole, top-entry header from a family table + pin count:
housing, shrouded mating cavity, the friction-lock window (polarization), and N
square pins. Dimensions are created as editable Fusion user parameters; the solid
is generated from those values (re-run to regenerate at new settings).

Configure by setting CONNECTOR before running (the bridge verb can prepend it):
    CONNECTOR = {"family": "XH", "pins": 4}
Supported families (pitch mm): SH 1.0 · GH 1.25 · ZH 1.5 · PH 2.0 · XH 2.5

Globals provided by the verb: adsk, app, ui, design.
"""

import adsk.core
import adsk.fusion

MM = 0.1  # Fusion internal length unit is cm

# family -> nominal dims (mm)
FAMILIES = {
    "SH": dict(pitch=1.00, bodyW=2.90, bodyH=4.25, wall=0.40, floor=0.50, end=0.90, pin=0.30, pinLen=3.0),
    "GH": dict(pitch=1.25, bodyW=3.00, bodyH=4.25, wall=0.45, floor=0.50, end=0.95, pin=0.30, pinLen=3.0),
    "ZH": dict(pitch=1.50, bodyW=3.50, bodyH=4.50, wall=0.50, floor=0.60, end=1.00, pin=0.40, pinLen=3.2),
    "PH": dict(pitch=2.00, bodyW=4.50, bodyH=4.50, wall=0.60, floor=0.60, end=1.35, pin=0.50, pinLen=3.4),
    "XH": dict(pitch=2.50, bodyW=5.75, bodyH=5.75, wall=0.70, floor=0.70, end=1.45, pin=0.64, pinLen=3.4),
}

cfg = dict(globals().get("CONNECTOR", {}))
fam = str(cfg.get("family", "PH")).upper()
if fam not in FAMILIES:
    raise ValueError(f"Unknown JST family '{fam}'. Choose from {list(FAMILIES)}.")
pins = max(1, int(cfg.get("pins", 2)))
d = FAMILIES[fam]
length_mm = (pins - 1) * d["pitch"] + 2 * d["end"]
part_name = f"JST-{fam} {pins}-pin"

root = design.rootComponent
extrudes = root.features.extrudeFeatures
planes = root.constructionPlanes

# --- editable user parameters (parametric in the Fusion UI) ---
up = design.userParameters
def _param(name, mm_val, comment):
    ex = up.itemByName(name)
    if ex:
        ex.expression = f"{mm_val} mm"
    else:
        up.add(name, adsk.core.ValueInput.createByString(f"{mm_val} mm"), "mm", comment)
for nm, val, cm in [("jst_pitch", d["pitch"], "Pin pitch"), ("jst_pin", d["pin"], "Pin square size"),
                    ("jst_pinLen", d["pinLen"], "Pin length below housing"), ("jst_bodyH", d["bodyH"], "Housing height"),
                    ("jst_bodyW", d["bodyW"], "Housing width"), ("jst_wall", d["wall"], "Shroud wall"),
                    ("jst_length", length_mm, "Housing length (derived)")]:
    _param(nm, val, cm)

L, W, H = length_mm * MM, d["bodyW"] * MM, d["bodyH"] * MM
wall, floor = d["wall"] * MM, d["floor"] * MM

def _rect(sketch, cx, cy, w, h):
    p0 = adsk.core.Point3D.create(cx - w / 2, cy - h / 2, 0)
    p1 = adsk.core.Point3D.create(cx + w / 2, cy + h / 2, 0)
    sketch.sketchCurves.sketchLines.addTwoPointRectangle(p0, p1)
    return sketch.profiles.item(0)

# 1) Housing solid.
sk_body = root.sketches.add(root.xYConstructionPlane)
housing = extrudes.addSimple(_rect(sk_body, 0, 0, L, W), adsk.core.ValueInput.createByReal(H),
                             adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
housing_body = housing.bodies.item(0)
housing_body.name = "housing"

# 2) Shrouded mating cavity (inset, cut down from the top, leaving a floor).
plane_in = planes.createInput()
plane_in.setByOffset(root.xYConstructionPlane, adsk.core.ValueInput.createByReal(H))
top_plane = planes.add(plane_in)
sk_pocket = root.sketches.add(top_plane)
cut_in = extrudes.createInput(_rect(sk_pocket, 0, 0, L - 2 * wall, W - 2 * wall),
                              adsk.fusion.FeatureOperations.CutFeatureOperation)
cut_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-(H - floor)))
cut_in.participantBodies = [housing_body]
extrudes.add(cut_in)

# 3) Friction-lock window through the front wall (polarization / mating feature).
made_window = False
try:
    front_in = planes.createInput()
    front_in.setByOffset(root.xZConstructionPlane, adsk.core.ValueInput.createByReal(-W / 2))
    front_plane = planes.add(front_in)
    sk_win = root.sketches.add(front_plane)
    win_w, win_h = L * 0.55, H * 0.42
    # sketch plane X->model X, sketch Y->model Z; raise the window toward the top.
    win = extrudes.createInput(_rect(sk_win, 0, H * 0.55, win_w, win_h),
                               adsk.fusion.FeatureOperations.CutFeatureOperation)
    win.setDistanceExtent(False, adsk.core.ValueInput.createByReal(wall * 1.2))
    win.participantBodies = [housing_body]
    extrudes.add(win)
    made_window = True
except Exception as _e:
    print("note: lock window skipped:", _e)

# 4) Pins: one square post extruded down, then rectangular-patterned across the row.
first_x = -((pins - 1) * d["pitch"] * MM) / 2.0
sk_pin = root.sketches.add(root.xYConstructionPlane)
pin_in = extrudes.createInput(_rect(sk_pin, first_x, 0, d["pin"] * MM, d["pin"] * MM),
                              adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
pin_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-(d["pinLen"] * MM)))
pin_feat = extrudes.add(pin_in)
pin_feat.bodies.item(0).name = "pin"
if pins > 1:
    patterns = root.features.rectangularPatternFeatures
    ents = adsk.core.ObjectCollection.create()
    ents.add(pin_feat)
    pat_in = patterns.createInput(ents, root.xConstructionAxis,
                                  adsk.core.ValueInput.createByReal(pins),
                                  adsk.core.ValueInput.createByReal((pins - 1) * d["pitch"] * MM),
                                  adsk.fusion.PatternDistanceType.ExtentPatternDistanceType)
    pat_in.quantityTwo = adsk.core.ValueInput.createByReal(1)
    pat_in.distanceTwo = adsk.core.ValueInput.createByReal(0)
    patterns.add(pat_in)

try:
    app.activeViewport.fit()
except Exception:
    pass

result = {"part": part_name, "family": fam, "pins": pins, "pitch_mm": d["pitch"],
          "length_mm": round(length_mm, 3), "bodies": root.bRepBodies.count, "lockWindow": made_window}
print(f"Built {part_name}: {pins}p @ {d['pitch']}mm, housing "
      f"{length_mm:.2f}x{d['bodyW']}x{d['bodyH']}mm, {root.bRepBodies.count} bodies"
      f"{' + lock window' if made_window else ''}.")
