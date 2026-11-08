"""Read-only configuration graph; incomplete scope never authorizes a write.

Table IDs are native stable identifiers. Controlled sources use fresh API equality,
with tokens used only to re-resolve wrappers invalidated by recompute. No method in
this module converts a design, activates a row, or writes a configuration cell.
"""
import math

import adsk.core
import adsk.fusion

from .core import MAX_ENTITIES, MechanicalError, identity, items, require, same

MAX_ROWS, MAX_COLUMNS, MAX_CELLS = 128, 128, 4096
_sources = []
CELL_TYPES = {
    "ConfigurationParameterColumn": "ConfigurationParameterCell",
    "ConfigurationSuppressColumn": "ConfigurationSuppressCell",
    "ConfigurationVisibilityColumn": "ConfigurationVisibilityCell",
    "ConfigurationPropertyColumn": "ConfigurationPropertyCell",
}


def concrete(entity):
    require(entity is not None and entity.isValid, "configuration_read_failed", "Configuration object is missing or invalid.")
    namespace = adsk.core if entity.objectType.startswith("adsk::core::") else adsk.fusion
    cls = getattr(namespace, entity.objectType.split("::")[-1], None)
    require(cls is not None and hasattr(cls, "cast"), "configuration_read_failed", "Concrete configuration wrapper is unavailable.", objectType=entity.objectType)
    result = cls.cast(entity)
    require(result is not None and result.isValid, "configuration_read_failed", "Concrete configuration cast failed.")
    return result


def bounded(collection, maximum):
    require(0 <= collection.count <= maximum, "configuration_budget", "Configuration inspection budget exceeded.")
    return [concrete(collection.item(index)) for index in range(collection.count)]


def native_id(entity):
    value = entity.id
    require(isinstance(value, str) and bool(value), "configuration_read_failed", "Native configuration ID is missing.")
    return value


def flag(value):
    require(type(value) is bool, "configuration_read_failed", "Native configuration flag is not a Boolean.")
    return value


def source_link(design, entity):
    entity = concrete(entity)
    context = getattr(entity, "assemblyContext", None)
    native = concrete(getattr(entity, "nativeObject", None) or entity)
    kind = native.objectType.split("::")[-1]
    owner_scope = "component"
    if kind.endswith("Parameter"):
        require(any(same(native, p) for p in items(design.allParameters)), "configuration_source_mismatch", "Controlled parameter is outside this design.")
        if kind == "UserParameter":
            require(same(native.design, design), "configuration_source_mismatch", "User parameter belongs to another design.")
            owner, owner_scope = design, "design"
        else:
            require(kind == "ModelParameter", "configuration_scope_unsupported", "Only user and model parameter ownership is supported.")
            owner = concrete(native.component)
            require(any(same(owner, c) for c in items(design.allComponents)), "configuration_source_mismatch", "Model parameter component is outside this design.")
    else:
        from .geometry import owner as component_owner
        owner = native if kind == "Component" else component_owner(native)
        if kind == "Occurrence":
            owner = native.component
        require(owner is not None and any(same(owner, c) for c in items(design.allComponents)),
                "configuration_source_mismatch", "Controlled source owner is outside this design.")
    key = None
    token = getattr(native, "entityToken", None)
    if token:
        for saved in _sources:
            if not saved["design"].isValid or not same(saved["design"], design) or saved["type"] != native.objectType:
                continue
            matches = design.findEntityByToken(saved["token"])
            require(len(matches) <= MAX_ENTITIES, "configuration_budget", "Configuration source lookup exceeds budget.")
            if any(candidate.isValid and same(candidate, native) for candidate in matches):
                require(all(candidate.isValid and candidate.objectType == native.objectType and same(candidate, native) for candidate in matches),
                        "configuration_source_mismatch", "Controlled source token resolves ambiguously.")
                key = saved["id"]
                break
    if key is None:
        key = identity(native)
        if token:
            require(len(_sources) < MAX_ENTITIES, "identity_budget", "Configuration source identity budget exhausted.")
            _sources.append({"design": design, "type": native.objectType, "token": token, "id": key})
    return {"entityId": key, "objectType": native.objectType, "ownerId": identity(owner), "ownerScope": owner_scope,
            "occurrenceId": identity(context) if context is not None else None}


def diagnostic_read(result, field, reader):
    """Retain stable table cells when a preview guard is unavailable.

    The missing guard remains an explicit read error, so strict preflight and
    recovery cannot infer that unavailable preview state is absent.
    """
    try:
        return reader()
    except Exception as error:
        result["readErrors"].append({"code": error.code if isinstance(error, MechanicalError) else "configuration_read_failed",
                                     "field": field, "message": str(error)})
        return {"unavailable": True}


def child_ids(row):
    children = row.childRows
    require(len(children) <= MAX_ROWS, "configuration_budget", "Configuration hierarchy exceeds budget.")
    return [native_id(concrete(child)) for child in children]


def property_link(design, prop):
    prop = concrete(prop)
    group = concrete(prop.parent)
    require(group.objectType == "adsk::core::PropertyGroup", "configuration_scope_unsupported", "Only component property groups are supported.")
    owner = concrete(group.parent)
    require(owner.objectType == "adsk::fusion::Component", "configuration_scope_unsupported", "Property must belong to a component.")
    return {"id": native_id(prop), "name": prop.name, "objectType": prop.objectType,
            "groupId": native_id(group), "owner": source_link(design, owner), "readOnly": flag(prop.isReadOnly)}


def numeric_cell(design, cell, unit):
    value = cell.value
    require(isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value),
            "configuration_read_failed", "Configuration numeric value is not finite.")
    scales = {"mm": 10.0, "deg": 180.0 / math.pi, "kg": 1.0, "mm2": 100.0, "mm3": 1000.0, "": 1.0}
    # A bare number is valid in almost any supplied default unit; it does not
    # establish dimensionlessness. Fusion documents an empty native unit for it.
    canonical = "" if unit == "" else next((candidate for candidate in scales if candidate
                      and design.unitsManager.isValidExpression("1 " + candidate, unit)), None)
    require(canonical is not None, "configuration_scope_unsupported",
            "Configuration numeric units must be length, angle, mass, area, volume or unitless.", displayUnit=unit)
    result = {"internalValue": value, "internalUnitSystem": "Fusion database units (cm, rad, kg)", "displayUnit": unit}
    # convert returns -1 on ExpressionError, which is also a legitimate signed
    # cell value. Probe a positive basis against the known database scale, then
    # calculate signed values with that verified linear scale. Temperature and
    # other unsupported families never enter a speculative conversion fallback.
    scale = scales[canonical]
    basis = design.unitsManager.convert(1.0, "internalUnits", canonical)
    require(isinstance(basis, (int, float)) and not isinstance(basis, bool) and math.isfinite(basis)
            and basis > 0 and math.isclose(basis, scale, rel_tol=1e-12, abs_tol=1e-12),
            "configuration_read_failed", "Configuration unit conversion basis failed validation.", unit=canonical or "unitless")
    converted = value * scale
    require(math.isfinite(converted), "configuration_read_failed", "Converted configuration value is not finite.")
    result.update(value=converted, unit=canonical or "unitless")
    return result


def read_table(design, result):
    table = concrete(design.configurationTopTable)
    rows, columns = bounded(table.rows, MAX_ROWS), bounded(table.columns, MAX_COLUMNS)
    require(len(rows) * len(columns) <= MAX_CELLS, "configuration_budget", "Configuration cell budget exceeded.")
    require(rows and len({native_id(row) for row in rows}) == len(rows), "configuration_read_failed", "Missing rows or duplicate configuration row IDs.")
    require(len({native_id(column) for column in columns}) == len(columns), "configuration_read_failed", "Duplicate configuration column IDs.")
    active = concrete(table.activeRow)
    require(any(same(active, row) for row in rows), "configuration_read_failed", "Active row is outside ordinary rows.")
    data = {"id": native_id(table), "name": table.name, "activeRowId": native_id(active),
            "activeRowName": active.name, "rows": [], "testRow": None, "columns": [], "cells": []}
    result["table"] = data
    custom = diagnostic_read(result, "top.hasCustomColumns", lambda: flag(table.hasCustomColumns))
    def read_test_row():
        row = table.testRow
        if row is None:
            return None
        row = concrete(row)
        require(all(not same(row, normal) and native_id(row) != native_id(normal) for normal in rows),
                "configuration_read_failed", "Test row overlaps an ordinary row.")
        return row
    test_row = diagnostic_read(result, "top.testRow", read_test_row)
    test_id = native_id(test_row) if test_row is not None and not isinstance(test_row, dict) else test_row
    result["preview"] = {"hasCustomColumns": custom, "testRowId": test_id}
    if custom is True:
        result["unsupported"].append("custom columns")
    all_rows = rows + ([test_row] if isinstance(test_id, str) else [])
    require(len(all_rows) <= MAX_ROWS and len(all_rows) * len(columns) <= MAX_CELLS,
            "configuration_budget", "Configuration table including its test row exceeds budget.")
    themes = bounded(table.customThemeTables, MAX_COLUMNS)
    for name in ("appearanceTable", "materialTable", "plasticRuleTable", "sheetMetalRuleTable"):
        theme = concrete(getattr(table, name))
        if theme.columns.count:
            themes.append(theme)
    result["themeTables"] = [{"id": native_id(t), "objectType": t.objectType,
                               "rowCount": t.rows.count, "columnCount": t.columns.count} for t in themes]
    if themes:
        result["unsupported"].append("theme-table contents")
    for row in all_rows:
        require(same(row.parentTable, table), "configuration_read_failed", "Configuration row belongs to another table.")
        def parent_id():
            parent = row.parentRow
            return native_id(concrete(parent)) if parent is not None else None
        hierarchy = {"parentId": diagnostic_read(result, "row." + row.id + ".parentRow", parent_id),
                     "childIds": diagnostic_read(result, "row." + row.id + ".childRows", lambda: child_ids(row)),
                     "isTestRow": diagnostic_read(result, "row." + row.id + ".isTestRow", lambda: flag(row.isTestRow))}
        reserved = isinstance(test_id, str) and same(row, test_row)
        if isinstance(hierarchy["parentId"], str) or (isinstance(hierarchy["childIds"], list) and hierarchy["childIds"]):
            result["unsupported"].append("hierarchical row: " + row.id)
        if type(hierarchy["isTestRow"]) is bool and hierarchy["isTestRow"] is not reserved:
            result["unsupported"].append("test-row flag disagrees with native table membership: " + row.id)
        row_data = {"id": row.id, "name": row.name, "index": row.index, "hierarchy": hierarchy,
                    "scope": "test" if reserved else "ordinary"}
        if reserved:
            data["testRow"] = row_data
        else:
            data["rows"].append(row_data)
    for column in columns:
        require(same(column.parentTable, table), "configuration_read_failed", "Configuration column belongs to another table.")
        kind = column.objectType.split("::")[-1]
        count_label = "column." + column.id + ".rowCount"
        def row_count():
            count = column.rowCount
            require(type(count) is int and 0 <= count <= MAX_ROWS, "configuration_read_failed", "Invalid column row count.")
            return count
        metadata = {"id": column.id, "index": column.index, "title": column.title, "type": kind,
                    "rowCount": diagnostic_read(result, count_label, row_count)}
        data["columns"].append(metadata)
        if type(metadata["rowCount"]) is int:
            diagnostic_read(result, count_label,
                lambda: require(metadata["rowCount"] == len(all_rows), "configuration_read_failed",
                                "Column row count differs from the ordinary plus reserved test-row inventory."))
        if kind not in CELL_TYPES:
            metadata["supported"] = False
            result["unsupported"].append("column type: " + kind)
            continue
        label = "column." + column.id
        if kind == "ConfigurationParameterColumn":
            metadata.update(source=diagnostic_read(result, label + ".source", lambda: source_link(design, column.parameter)),
                            displayUnit=diagnostic_read(result, label + ".displayUnit", lambda: column.parameter.unit))
        elif kind == "ConfigurationSuppressColumn":
            metadata["source"] = diagnostic_read(result, label + ".source", lambda: source_link(design, column.feature))
        elif kind == "ConfigurationVisibilityColumn":
            metadata["source"] = diagnostic_read(result, label + ".source", lambda: source_link(design, column.entity))
        else:
            metadata["property"] = diagnostic_read(result, label + ".property", lambda: property_link(design, column.parentProperty))
        for row in all_rows:
            label = "cell." + row.id + "." + column.id
            entry = {"rowId": row.id, "columnId": column.id, "type": CELL_TYPES[kind],
                     "scope": "test" if isinstance(test_id, str) and same(row, test_row) else "ordinary"}
            data["cells"].append(entry)
            def addressed_cell():
                cell = concrete(column.getCellByRowId(row.id))
                require(cell.objectType == "adsk::fusion::" + CELL_TYPES[kind], "configuration_read_failed", "Cell type disagrees with its column.")
                require(same(concrete(cell.parentRow), row) and same(concrete(cell.parentColumn), column),
                        "configuration_read_failed", "Cell address readback mismatch.")
                return cell
            cell = diagnostic_read(result, label + ".address", addressed_cell)
            if isinstance(cell, dict):
                entry["unavailable"] = True
                continue
            if kind == "ConfigurationParameterColumn":
                entry.update(expression=diagnostic_read(result, label + ".expression", lambda: cell.expression),
                             valueType=diagnostic_read(result, label + ".valueType", lambda: int(cell.valueType)))
                if entry["valueType"] == adsk.fusion.ParameterValueTypes.NumericParameterValueType:
                    measured = diagnostic_read(result, label + ".numeric", lambda: numeric_cell(design, cell, column.parameter.unit))
                    if measured.get("unavailable"):
                        entry["numeric"] = measured
                    else:
                        entry.update(measured)
                elif entry["valueType"] == adsk.fusion.ParameterValueTypes.TextParameterValueType:
                    entry["textValue"] = diagnostic_read(result, label + ".textValue", lambda: cell.textValue)
                elif not isinstance(entry["valueType"], dict):
                    diagnostic_read(result, label + ".valueType", lambda: require(False, "configuration_read_failed", "Unknown parameter-cell value type."))
            elif kind == "ConfigurationSuppressColumn":
                entry.update(isSuppressed=diagnostic_read(result, label + ".isSuppressed", lambda: flag(cell.isSuppressed)),
                             isEffectivelySuppressed=diagnostic_read(result, label + ".isEffectivelySuppressed", lambda: flag(cell.isEffectivelySuppressed)))
            elif kind == "ConfigurationVisibilityColumn":
                entry["isVisible"] = diagnostic_read(result, label + ".isVisible", lambda: flag(cell.isVisible))
            else:
                def property_value():
                    value = cell.value
                    require(isinstance(value, str), "configuration_scope_unsupported", "Only textual component property cells are supported.")
                    return value
                entry["value"] = diagnostic_read(result, label + ".value", property_value)


def snapshot(design, strict=False):
    result = {"configured": None, "isConfiguration": None, "configurationRowId": None,
              "complete": False, "table": None, "configuredOccurrences": [],
              "unsupported": [], "readErrors": [], "configurationWritesEnabled": False,
              "scope": "loaded Design ordinary and reserved test rows; no activation or recompute"}
    try:
        result["configured"] = flag(design.isConfiguredDesign)
        result["isConfiguration"] = flag(design.isConfiguration)
        row_id = design.configurationRowId
        require(isinstance(row_id, str), "configuration_read_failed", "Native design configuration row ID is not a string.")
        result["configurationRowId"] = row_id
        require(bool(row_id) == result["isConfiguration"], "configuration_read_failed", "Native design configuration flag and row ID disagree.")
        if result["isConfiguration"]:
            result["unsupported"].append("generated single-configuration design: source-table identity and durable lifecycle are unsupported")
        if result["configured"]:
            read_table(design, result)
        # A standard design can still contain externally configured occurrences.
        for occurrence in items(design.rootComponent.allOccurrences):
            if flag(occurrence.isConfiguration):
                row = concrete(occurrence.configurationRow)
                data_file = occurrence.configuredDataFile
                require(data_file is not None and data_file.isValid, "configuration_read_failed", "Configured occurrence source file is unavailable.")
                result["configuredOccurrences"].append({"occurrenceId": identity(occurrence), "path": occurrence.fullPathName,
                    "tableId": native_id(concrete(row.parentTable)), "rowId": native_id(row), "rowName": row.name,
                    "sourceFileId": data_file.id, "sourceVersion": data_file.versionNumber})
        if result["configuredOccurrences"]:
            result["unsupported"].append("configured inserted occurrences: source-table witness and cloud lifecycle are deferred")
    except Exception as error:
        result["readErrors"].append({"code": error.code if isinstance(error, MechanicalError) else "configuration_read_failed", "message": str(error)})
    result["complete"] = not result["unsupported"] and not result["readErrors"]
    if strict:
        require(result["complete"], "configuration_scope_unsupported", "A complete supported configuration witness is required before model mutation or complete BOM export.", configuration=result)
    return result


def provenance(graph):
    table = graph["table"]
    return {"configured": graph["configured"], "isConfiguration": graph["isConfiguration"],
            "tableId": table["id"] if table else None,
            "rowId": graph["configurationRowId"] if graph["isConfiguration"] else table["activeRowId"] if table else None, "rowName": table["activeRowName"] if table else None,
            "scope": "current active configuration only", "graphComplete": graph["complete"]}


def inspect(app, args):
    from .core import target
    _, design, value = target(app, args)
    return {"target": value, "configuration": snapshot(design)}


READS = {"configuration_inspect": inspect}
