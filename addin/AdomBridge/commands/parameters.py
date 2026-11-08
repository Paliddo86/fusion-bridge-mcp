"""Parameter commands — get and set design parameters."""

import adsk.core
import adsk.fusion


def handle_get_parameters(app: adsk.core.Application, args: dict) -> dict:
    """Get all parameters (user + model) from the active design.

    Args:
        args: Optional dict with:
            - type: "user", "model", or "all" (default: "all").
    """
    product = app.activeProduct
    if not product or product.productType != "DesignProductType":
        return {
            "success": False,
            "error": "No active design open in Fusion 360",
            "_hint": "Call fusion_open_cloud_file to open a Design document, then retry.",
        }

    design = adsk.fusion.Design.cast(product)
    param_type = args.get("type", "all").lower()

    result = []

    def _safe(fn):
        # Parameter.value THROWS "Parameter is not numeric type" for string/text
        # parameters (caught live 2026-07-06: get_parameters blew up the whole call on
        # one non-numeric param). Read every field defensively so one odd parameter
        # never kills the enumeration.
        try:
            return fn()
        except Exception:
            return None

    if param_type in ("user", "all"):
        for i in range(design.userParameters.count):
            p = design.userParameters.item(i)
            result.append({
                "name": _safe(lambda: p.name),
                "type": "user",
                "expression": _safe(lambda: p.expression),
                "value": _safe(lambda: p.value),
                "unit": _safe(lambda: p.unit),
                "comment": _safe(lambda: p.comment),
            })

    if param_type in ("model", "all"):
        for i in range(design.allParameters.count):
            p = design.allParameters.item(i)
            # Skip user params if we already included them
            if param_type == "all" and _is_user_param(design, p.name):
                continue
            result.append({
                "name": _safe(lambda: p.name),
                "type": "model",
                "expression": _safe(lambda: p.expression),
                "value": _safe(lambda: p.value),
                "unit": _safe(lambda: p.unit),
            })

    return {
        "success": True,
        "data": result,
        "output": f"Found {len(result)} parameter(s).",
    }


def handle_set_parameter(app: adsk.core.Application, args: dict) -> dict:
    """Set a parameter's expression by name.

    Args:
        args: Dict with:
            - name: Parameter name (required).
            - expression: New expression string (required), e.g. "10 mm".
    """
    name = args.get("name", "")
    expression = args.get("expression", "")
    if not name:
        return {"success": False, "error": "No parameter name specified"}
    if not expression:
        return {"success": False, "error": "No expression specified"}

    product = app.activeProduct
    if not product or product.productType != "DesignProductType":
        return {
            "success": False,
            "error": "No active design open in Fusion 360",
            "_hint": "Call fusion_open_cloud_file to open a Design document, then retry.",
        }

    design = adsk.fusion.Design.cast(product)

    # Try user parameters first, then all parameters
    param = None
    for i in range(design.userParameters.count):
        p = design.userParameters.item(i)
        if p.name == name:
            param = p
            break

    if not param:
        for i in range(design.allParameters.count):
            p = design.allParameters.item(i)
            if p.name == name:
                param = p
                break

    if not param:
        return {
            "success": False,
            "error": f"Parameter not found: {name}",
            "_hint": "Call fusion_get_parameters to list all valid parameter names, then retry with a correct name.",
        }

    old_expr = param.expression
    try:
        param.expression = expression
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to set parameter: {e}",
            "_hint": "Verify the expression format (e.g., \"10 mm\", \"5 deg\") and that units match the parameter's unit type.",
        }

    return {
        "success": True,
        "output": f"Parameter '{name}' changed: {old_expr} -> {expression}",
        "data": {
            "name": param.name,
            "expression": param.expression,
            "value": param.value,
            "unit": param.unit,
        },
    }


def _is_user_param(design: adsk.fusion.Design, name: str) -> bool:
    """Check if a parameter name exists in the user parameters."""
    for i in range(design.userParameters.count):
        if design.userParameters.item(i).name == name:
            return True
    return False
