"""Design info command — component tree, bodies, document info."""

import adsk.core
import adsk.fusion


def handle_get_design_info(app: adsk.core.Application, args: dict) -> dict:
    """Get information about the active design.

    Returns component tree, bodies, user parameters, and document info.
    """
    product = app.activeProduct
    if not product or product.productType != "DesignProductType":
        return {
            "success": False,
            "error": "No active design open in Fusion 360",
            "_hint": "Call fusion_open_cloud_file to open a Design document, or fusion_show_3d_board if an electronics board is open, then retry.",
        }

    design = adsk.fusion.Design.cast(product)
    doc = app.activeDocument

    root = design.rootComponent
    components = _walk_component(root)

    # User parameters
    params = []
    for i in range(design.userParameters.count):
        param = design.userParameters.item(i)
        params.append({
            "name": param.name,
            "expression": param.expression,
            "value": param.value,
            "unit": param.unit,
        })

    return {
        "success": True,
        "data": {
            "document": {
                "name": doc.name if doc else "Untitled",
                "isSaved": doc.isSaved if doc else False,
            },
            "designType": "Parametric" if design.designType == adsk.fusion.DesignTypes.ParametricDesignType else "Direct",
            "components": components,
            "userParameters": params,
        },
    }


def _walk_component(comp: adsk.fusion.Component) -> dict:
    """Recursively walk a component and its children."""
    bodies = []
    for i in range(comp.bRepBodies.count):
        body = comp.bRepBodies.item(i)
        bodies.append({
            "name": body.name,
            "isVisible": body.isVisible,
            "isSolid": body.isSolid,
            "volume_cm3": body.volume if body.isSolid else None,
        })

    # Also check mesh bodies (STL/3MF imports create these)
    mesh_bodies = []
    try:
        for i in range(comp.meshBodies.count):
            mb = comp.meshBodies.item(i)
            mesh_bodies.append({
                "name": mb.name,
                "isVisible": mb.isVisible,
            })
    except Exception:
        pass  # meshBodies may not exist in older API versions

    children = []
    for i in range(comp.occurrences.count):
        occ = comp.occurrences.item(i)
        children.append({
            "name": occ.name,
            "isVisible": occ.isVisible,
            "component": _walk_component(occ.component),
        })

    result = {
        "name": comp.name,
        "bodies": bodies,
        "children": children,
    }
    if mesh_bodies:
        result["meshBodies"] = mesh_bodies

    # Debug info: feature count tells us if there's actual content
    try:
        result["features"] = comp.features.count
    except Exception:
        pass

    return result
