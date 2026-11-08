"""Command dispatch for the Adom Bridge Fusion 360 add-in."""

import adsk.core

from .app_state import handle_get_app_state
from .document_info import handle_document_info, handle_activate_document
from .export import (
    handle_export_step, handle_export_stl, handle_export_3mf,
    handle_export_f3d, handle_export_usdz, handle_export_fbx,
    handle_export_dxf, handle_export_dwg, handle_export_iges,
    handle_export_obj, handle_export_sat, handle_export_skp,
)
from .design_info import handle_get_design_info
from .electronics import (
    handle_electron_run,
    handle_execute_text_command,
    handle_export_source,
    handle_export_eagle_source,
    handle_open_electronics,
    handle_list_text_commands,
    handle_board_info,
)
from .electron_view import handle_electron_zoom, handle_electron_pan, handle_electron_select
from .open_lbr import handle_open_lbr, handle_export_lbr
from .import_file import handle_import_file
from .parameters import handle_get_parameters, handle_set_parameter
from .screenshot import handle_take_screenshot
from .close_document import handle_close_document, handle_close_all_documents
# The activity palette is a convenience, not the bridge. A copy of this add-in that lost
# activity_palette.py (issue #69 payload, or a stale sibling copy in a legacy AddIns dir that
# a later sync never reached: issue #75, Adonis) must still start, so the bridge can repair it.
try:
    from .activity_palette import handle_activity_banner
except ImportError as _e:  # pragma: no cover - only a broken install reaches this
    _ACTIVITY_IMPORT_ERROR = str(_e)

    def handle_activity_banner(app, args):
        return {"success": False, "errorCode": "addin_module_missing",
                "error": "activity_palette.py is missing from this add-in copy: " + _ACTIVITY_IMPORT_ERROR,
                "_hint": "The add-in on disk is incomplete. fusion_restart_for_addin {save:true} makes the bridge "
                         "re-sync every AddIns dir and restart Fusion."}
from .open_electronics_file import handle_open_schematic, handle_open_board, handle_show_3d_board, handle_show_2d_board, handle_show_schematic, handle_import_electronics
from .cloud_documents import (
    handle_save_to_cloud,
    handle_list_cloud_projects,
    handle_list_cloud_files,
    handle_delete_cloud_file,
    handle_create_cloud_folder,
    handle_open_cloud_file,
    handle_open_by_urn,
    handle_search_cloud_files,
    handle_export_cloud_file,
    handle_check_recovery,
    handle_walk_cloud_tree,
)
from .manufacturing import (
    handle_export_bom,
    handle_export_cpl,
    handle_export_gerbers,
    handle_set_design_rules,
    handle_export_board_image,
    handle_detect_layers,
)
from .silkscreen_capture import handle_take_silkscreen_screenshot
from .modeling import handle_run_modeling_script
from .assembly_bom import handle_assembly_bom
from .physical_properties import handle_physical_properties
from .mechanical import handle_mechanical

COMMAND_HANDLERS = {
    "mechanical": handle_mechanical,
    "activity_banner": handle_activity_banner,
    # App state
    "get_app_state": handle_get_app_state,
    "document_info": handle_document_info,
    "activate_document": handle_activate_document,
    # 3D CAD commands
    "import_file": handle_import_file,
    "export_step": handle_export_step,
    "export_stl": handle_export_stl,
    "export_3mf": handle_export_3mf,
    "export_f3d": handle_export_f3d,
    "export_usdz": handle_export_usdz,
    "export_fbx": handle_export_fbx,
    "export_dxf": handle_export_dxf,
    "export_dwg": handle_export_dwg,
    "export_iges": handle_export_iges,
    "export_obj": handle_export_obj,
    "export_sat": handle_export_sat,
    "export_skp": handle_export_skp,
    "get_design_info": handle_get_design_info,
    "assembly_bom": handle_assembly_bom,
    "physical_properties": handle_physical_properties,
    "get_parameters": handle_get_parameters,
    "set_parameter": handle_set_parameter,
    "take_screenshot": handle_take_screenshot,
    # Electronics commands (EAGLE via Electron.run)
    "electron_run": handle_electron_run,
    "electron_zoom": handle_electron_zoom,
    "electron_pan": handle_electron_pan,
    "electron_select": handle_electron_select,
    "execute_text_command": handle_execute_text_command,
    "open_electronics": handle_open_electronics,
    "list_text_commands": handle_list_text_commands,
    "board_info": handle_board_info,
    "export_source": handle_export_source,
    "export_eagle_source": handle_export_eagle_source,
    # Library file commands (Document.Open / EXPORT SCRIPT)
    "open_lbr": handle_open_lbr,
    "export_lbr": handle_export_lbr,
    # Document management
    "close_document": handle_close_document,
    "close_all_documents": handle_close_all_documents,
    # Electronics file opening
    "open_schematic": handle_open_schematic,
    "open_board": handle_open_board,
    "show_3d_board": handle_show_3d_board,
    "show_2d_board": handle_show_2d_board,
    "show_schematic": handle_show_schematic,
    "import_electronics": handle_import_electronics,
    # Cloud document management
    "save_to_cloud": handle_save_to_cloud,
    "list_cloud_projects": handle_list_cloud_projects,
    "list_cloud_files": handle_list_cloud_files,
    "delete_cloud_file": handle_delete_cloud_file,
    "create_cloud_folder": handle_create_cloud_folder,
    "open_cloud_file": handle_open_cloud_file,
    "open_by_urn": handle_open_by_urn,
    "check_recovery": handle_check_recovery,
    "search_cloud_files": handle_search_cloud_files,
    "walk_cloud_tree": handle_walk_cloud_tree,
    "export_cloud_file": handle_export_cloud_file,
    # Manufacturing exports (Gerbers, BOM, CPL, DRC, images)
    "export_bom": handle_export_bom,
    "export_cpl": handle_export_cpl,
    "export_gerbers": handle_export_gerbers,
    "set_design_rules": handle_set_design_rules,
    "export_board_image": handle_export_board_image,
    "detect_layers": handle_detect_layers,
    # Silkscreen layer capture (isolated top/bottom PNG)
    "take_silkscreen_screenshot": handle_take_silkscreen_screenshot,
    # In-app parametric modeling (free; runs adsk.fusion in the live session)
    "run_modeling_script": handle_run_modeling_script,
}


def dispatch_command(app: adsk.core.Application, command: str, args: dict) -> dict:
    """Dispatch a command to the appropriate handler.

    All handlers receive the Fusion Application object and an args dict.
    """
    handler = COMMAND_HANDLERS.get(command)
    if handler is None:
        return {
            "success": False,
            "error": f"Unknown add-in command: {command}",
            "_hint": f"Valid add-in commands: {', '.join(sorted(COMMAND_HANDLERS.keys()))}.",
        }
    result = handler(app, args)
    if command != "activity_banner":
        from .document_hygiene import attach_document_hygiene
        result = attach_document_hygiene(app, result)
    return result
