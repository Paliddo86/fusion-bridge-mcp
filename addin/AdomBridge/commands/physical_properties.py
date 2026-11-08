"""Per-component physical properties (volume, mass, density, area, center of mass).

Fusion exposes physical properties only through the modeling API, so before this verb a
caller had to run getPhysicalProperties inside fusion_run_modeling_script. This verb reads
them directly, per component, so BOM/costing tools can price by volume or mass without a
custom script.
"""

import adsk.core
import adsk.fusion

_ACC = {
    "low": adsk.fusion.CalculationAccuracy.LowCalculationAccuracy,
    "medium": adsk.fusion.CalculationAccuracy.MediumCalculationAccuracy,
    "high": adsk.fusion.CalculationAccuracy.HighCalculationAccuracy,
    "veryhigh": adsk.fusion.CalculationAccuracy.VeryHighCalculationAccuracy,
}


def handle_physical_properties(app: adsk.core.Application, args: dict) -> dict:
    """Return physical properties per unique component in the active design.

    args:
      names      optional list of component names to measure (default: all).
      accuracy   optional "low" | "medium" | "high" | "veryhigh" (default: low).
    """
    design = adsk.fusion.Design.cast(app.activeProduct)
    if not design:
        return {"success": False, "error": "No active Fusion Design.",
                "_hint": "Open a design in the Design workspace, then retry."}

    args = args or {}
    names = set(args.get("names") or [])
    acc = _ACC.get((args.get("accuracy") or "low").lower(),
                   adsk.fusion.CalculationAccuracy.LowCalculationAccuracy)

    out = {}
    seen = set()
    for occ in design.rootComponent.allOccurrences:
        comp = occ.component
        if comp.name in seen:
            continue
        if names and comp.name not in names:
            continue
        seen.add(comp.name)
        try:
            pp = comp.getPhysicalProperties(acc)
            entry = {
                "volume_cm3": round(pp.volume, 4),
                "mass_kg": round(pp.mass, 6),
                "density": round(pp.density, 6),
                "area_cm2": round(pp.area, 4),
            }
            try:
                com = pp.centerOfMass
                entry["center_of_mass"] = [round(com.x, 4), round(com.y, 4), round(com.z, 4)]
            except Exception:
                pass
            out[comp.name] = entry
        except Exception as e:
            out[comp.name] = {"error": str(e)[:80]}

    return {"success": True, "count": len(out), "accuracy": (args.get("accuracy") or "low"),
            "properties": out}
