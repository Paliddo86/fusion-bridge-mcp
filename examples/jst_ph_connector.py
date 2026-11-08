"""Parametric JST-PH (2.0 mm) connector — runs via fusion_run_modeling_script.

Demonstrates the in-app modeling verb: builds a recognizable, parametric JST-PH
style top-entry connector (housing + shrouded mating cavity + N square pins) from
code, in the live Fusion session. FREE (no APS Fusion Automation billing).

Pin count: set `JST_PINS` in the exec namespace before running (the bridge verb
can prepend e.g. `JST_PINS = 4`). Defaults to 2. All dimensions are real JST-PH
nominal values, created as editable Fusion user parameters; the solid is then
generated from those parameter values (re-run to regenerate at new values).

Globals provided by the verb: adsk, app, ui, design.
"""

import adsk.core
import adsk.fusion

MM = 0.1  # Fusion internal length unit is cm; 1 mm = 0.1 cm

# --- parameters (JST-PH 2.0 mm nominal), in mm ---
pins = int(globals().get("JST_PINS", 2))
pitch_mm = 2.0
pin_mm = 0.5     # square pin cross-section
pin_len_mm = 3.4  # pin length below the housing
body_h_mm = 4.5   # housing height
body_w_mm = 4.5   # housing width (across the short axis)
wall_mm = 0.6     # shroud wall thickness
floor_mm = 0.6    # cavity floor thickness
end_mm = 1.35     # housing margin past the outer pins
length_mm = (pins - 1) * pitch_mm + 2 * end_mm

root = design.rootComponent
part_name = f"JST-PH {pins}-pin"  # root component can't be renamed; used for labels

# --- editable user parameters (so the model is parametric in the UI) ---
up = design.userParameters
def _param(name, mm_val, comment):
    existing = up.itemByName(name)
    vi = adsk.core.ValueInput.createByString(f"{mm_val} mm")
    if existing:
        existing.expression = f"{mm_val} mm"
        return existing
    return up.add(name, vi, "mm", comment)

_param("jst_pitch", pitch_mm, "Pin pitch")
_param("jst_pin", pin_mm, "Pin square size")
_param("jst_pinLen", pin_len_mm, "Pin length below housing")
_param("jst_bodyH", body_h_mm, "Housing height")
_param("jst_bodyW", body_w_mm, "Housing width")
_param("jst_wall", wall_mm, "Shroud wall thickness")
_param("jst_length", length_mm, "Housing length (derived)")

L = length_mm * MM
W = body_w_mm * MM
H = body_h_mm * MM
sk_planes = root.constructionPlanes
extrudes = root.features.extrudeFeatures

def _rect(sketch, cx, cy, w, h):
    p0 = adsk.core.Point3D.create(cx - w / 2, cy - h / 2, 0)
    p1 = adsk.core.Point3D.create(cx + w / 2, cy + h / 2, 0)
    sketch.sketchCurves.sketchLines.addTwoPointRectangle(p0, p1)
    return sketch.profiles.item(0)

# 1) Housing solid: centered box, extruded up by H.
sk_body = root.sketches.add(root.xYConstructionPlane)
prof_body = _rect(sk_body, 0, 0, L, W)
housing = extrudes.addSimple(prof_body, adsk.core.ValueInput.createByReal(H),
                             adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
housing_body = housing.bodies.item(0)
housing_body.name = "housing"

# 2) Shroud cavity: inset rectangle on the top face, cut downward leaving a floor.
plane_in = sk_planes.createInput()
plane_in.setByOffset(root.xYConstructionPlane, adsk.core.ValueInput.createByReal(H))
top_plane = sk_planes.add(plane_in)
sk_pocket = root.sketches.add(top_plane)
prof_pocket = _rect(sk_pocket, 0, 0, L - 2 * wall_mm * MM, W - 2 * wall_mm * MM)
cut_in = extrudes.createInput(prof_pocket, adsk.fusion.FeatureOperations.CutFeatureOperation)
cut_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-(H - floor_mm * MM)))
cut_in.participantBodies = [housing_body]
extrudes.add(cut_in)

# 3) One pin: square post at the first pin location, extruded DOWN by pin_len.
first_x = -((pins - 1) * pitch_mm * MM) / 2.0
sk_pin = root.sketches.add(root.xYConstructionPlane)
prof_pin = _rect(sk_pin, first_x, 0, pin_mm * MM, pin_mm * MM)
pin_in = extrudes.createInput(prof_pin, adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
pin_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-(pin_len_mm * MM)))
pin_feat = extrudes.add(pin_in)
pin_feat.bodies.item(0).name = "pin"

# 4) Rectangular-pattern the pin N times along X at the pitch.
if pins > 1:
    patterns = root.features.rectangularPatternFeatures
    ents = adsk.core.ObjectCollection.create()
    ents.add(pin_feat)
    qty = adsk.core.ValueInput.createByReal(pins)
    total = adsk.core.ValueInput.createByReal((pins - 1) * pitch_mm * MM)
    pat_in = patterns.createInput(ents, root.xConstructionAxis, qty, total,
                                  adsk.fusion.PatternDistanceType.ExtentPatternDistanceType)
    pat_in.quantityTwo = adsk.core.ValueInput.createByReal(1)
    pat_in.distanceTwo = adsk.core.ValueInput.createByReal(0)
    patterns.add(pat_in)

# Fit the camera so a screenshot shows the part.
try:
    app.activeViewport.fit()
except Exception:
    pass

result = {"part": part_name, "pins": pins, "pitch_mm": pitch_mm,
          "length_mm": round(length_mm, 3), "bodies": root.bRepBodies.count}
print(f"Built {part_name}: {pins} pins @ {pitch_mm}mm pitch, "
      f"housing {length_mm:.2f}x{body_w_mm}x{body_h_mm}mm, {root.bRepBodies.count} bodies.")
