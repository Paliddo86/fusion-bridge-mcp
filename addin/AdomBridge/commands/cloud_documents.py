import re
"""Cloud document management for Fusion 360.

Provides commands to save documents to Autodesk's cloud hub,
list cloud projects and files, and delete cloud files.

Uses the Fusion Data API (app.data) which requires the user
to be signed in to their Autodesk account.

Key concepts:
  - Hub: The team/organization account (e.g., "Adom")
  - Project: A folder-like container in the hub (e.g., "Personal", "Main")
  - DataFolder: Subfolder within a project
  - DataFile: A cloud-stored document with URN, version history, etc.
  - wip_urn: The version-specific URN used by Electronics libraries
             to reference 3D package models (e.g., urn:adsk.wipprod:fs.file:vf.xxx?version=1)
"""

import adsk.core
import adsk.fusion
import os
import threading
import time

# Walk/search progress state — updated by BFS loop on main thread,
# read by health endpoint on HTTP thread.
_walk_progress_lock = threading.Lock()
_walk_progress = None  # None when no walk/search is active


def get_walk_progress():
    """Return a snapshot of the current walk/search progress, or None."""
    with _walk_progress_lock:
        if _walk_progress is None:
            return None
        return dict(_walk_progress)


RESOLVE_PROJECT_TIMEOUT = 60  # seconds — overall wall-clock for hub/project enumeration


def _resolve_project(app, project_name=None):
    """Find a cloud project by name. Defaults to active project.

    Robustness:
      - Every per-hub / per-project access wrapped in try/except so a single
        bad hub or project can't crash the resolve.
      - adsk.doEvents() yields between hubs so the UI stays responsive.
      - Overall wall-clock cap (RESOLVE_PROJECT_TIMEOUT). On expired/trial
        subscriptions the cloud Data API can take 30s+ per hub; cap the
        total so a hung lookup returns an actionable error instead of
        hanging Fusion's main thread indefinitely.
    """
    data = app.data
    if not data:
        return None, "Fusion Data API not available. Are you signed in?"

    if not project_name:
        try:
            proj = data.activeProject
        except Exception as e:
            # Right after a launch Fusion's Data API answers InternalValidationError
            # (id.size()) until the Data panel has initialised; seen live 2026-09-03 on
            # the first open after fusion_restart_for_addin. Say so, and say what to do.
            return None, ("activeProject lookup failed: %s. Fusion's Data API is not ready yet "
                          "(normal for ~30-60s after launch): retry shortly, or pass projectName "
                          "so the hub is resolved by name instead of the active project." % e)
        if proj:
            return proj, None
        return None, "No active project"

    needle = project_name.lower()
    started = time.time()

    # Search all hubs for the named project. Each hub.dataProjects access
    # is a network round-trip on the cloud Data API; wrap in try/except so
    # one bad hub doesn't take down the whole search.
    try:
        hubs = data.dataHubs
        hub_count = hubs.count
    except Exception as e:
        return None, f"Failed to enumerate hubs: {e}"

    for i in range(hub_count):
        if time.time() - started > RESOLVE_PROJECT_TIMEOUT:
            try:
                adsk.core.Application.log(
                    f"AdomBridge: _resolve_project hit timeout ({RESOLVE_PROJECT_TIMEOUT}s) "
                    f"at hub {i}/{hub_count} searching for '{project_name}'"
                )
            except Exception:
                pass
            return None, (
                f"Project lookup timed out after {RESOLVE_PROJECT_TIMEOUT}s "
                f"(searched {i}/{hub_count} hubs). Cloud API may be slow or "
                f"unavailable."
            )
        try:
            hub = hubs.item(i)
        except Exception:
            continue
        try:
            projects = hub.dataProjects
            proj_count = projects.count
        except Exception:
            continue
        for j in range(proj_count):
            try:
                proj = projects.item(j)
                if proj.name.lower() == needle:
                    return proj, None
            except Exception:
                continue
        # Yield between hubs so the UI stays responsive even when the
        # team has many hubs / projects.
        try:
            adsk.doEvents()
        except Exception:
            pass

    return None, f"Project '{project_name}' not found"


def _resolve_folder(project, folder_path=None, create=False):
    """Navigate to a subfolder within a project. Returns root folder if no path given.
    create=True makes each missing folder on the path (save_to_cloud promises it).

    Robustness: each `dataFolders.item(i).name` access is wrapped in
    try/except so a single corrupted subfolder doesn't crash navigation.
    """
    try:
        folder = project.rootFolder
    except Exception as e:
        return None, f"rootFolder lookup failed: {e}"
    if not folder_path:
        return folder, None

    parts = [p.strip() for p in folder_path.replace("\\", "/").split("/") if p.strip()]
    for part in parts:
        needle = part.lower()
        found = False
        try:
            subfolders = folder.dataFolders
            sub_count = subfolders.count
        except Exception as e:
            return None, f"dataFolders lookup failed for '{folder.name}': {e}"
        for i in range(sub_count):
            try:
                sf = subfolders.item(i)
                if sf.name.lower() == needle:
                    folder = sf
                    found = True
                    break
            except Exception:
                continue
        if not found and create:
            try:
                made = subfolders.add(part)
            except Exception as e:
                return None, f"Could not create folder '{part}' in '{folder.name}': {e}"
            if made is None:
                return None, f"Could not create folder '{part}' in '{folder.name}'"
            folder, found = made, True
        if not found:
            return None, f"Folder '{part}' not found in '{folder.name}'"

    return folder, None


def handle_save_to_cloud(app: adsk.core.Application, args: dict) -> dict:
    """Save the active Fusion document to the cloud.

    Args:
        args: Dict with:
            - name: Name for the cloud document (required).
            - projectName: Target project name (optional, defaults to active project).
            - folderPath: Subfolder path within the project (optional, defaults to root).
            - description: Document description (optional).
    """
    name = args.get("name", "").strip()
    if not name:
        return {"success": False, "error": "No name specified. Provide a name for the cloud document."}

    doc = app.activeDocument
    if not doc:
        return {
            "success": False,
            "error": "No active document to save.",
            "_hint": "Call fusion_open_cloud_file or fusion_import_file first to have an active document, then retry.",
        }

    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")
    description = args.get("description", "")

    project, err = _resolve_project(app, project_name or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see available project names, then retry with a valid projectName.",
        }

    # missing folders on the path are created, as the verb's description says
    folder, err = _resolve_folder(project, folder_path or None, create=True)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Check the projectName with fusion_list_cloud_projects; the folders on folderPath are created when missing.",
        }

    try:
        result = doc.saveAs(name, folder, description, "")
    except Exception as e:
        return {
            "success": False,
            "error": f"saveAs failed: {e}",
            "data": {"hint": "The document may already exist with this name, or you may lack permissions."},
        }

    if not result:
        return {"success": False, "error": "saveAs returned False. The save may have failed."}

    # Collect saved file info
    response_data = {
        "name": name,
        "project": project.name,
        "folder": folder.name,
        "documentName": doc.name,
        "isSaved": doc.isSaved,
    }

    df = doc.dataFile
    if df:
        response_data["fileId"] = df.id
        try:
            response_data["versionId"] = df.versionId
        except Exception:
            pass
        try:
            response_data["versionNumber"] = df.versionNumber
        except Exception:
            pass
        # The wip_urn format used by Electronics libraries for 3D packages
        if df.id and "wipprod" in df.id:
            response_data["wipUrn"] = df.id

    return {
        "success": True,
        "output": f"Saved '{name}' to project '{project.name}'",
        "data": response_data,
    }


def handle_list_cloud_projects(app: adsk.core.Application, args: dict) -> dict:
    """List all cloud projects in the user's hub(s).

    Robustness (v1.0.2):
      - Per-hub / per-project try/except so a single bad item doesn't kill
        the whole listing.
      - adsk.doEvents() between hubs to keep the UI responsive.
      - Overall wall-clock cap of LIST_PROJECTS_TIMEOUT seconds.
      - Progress logged via Application.log so a slow listing is visible.

    Args:
        args: Dict (no required args).
    """
    data = app.data
    if not data:
        return {
            "success": False,
            "error": "Fusion Data API not available. Are you signed in?",
            "_hint": "Report to the user and ask them to sign into their Autodesk account in Fusion 360, then retry.",
        }

    LIST_PROJECTS_TIMEOUT = 90  # seconds — overall wall-clock cap
    started = time.time()
    list_timed_out = False

    hubs_data = []
    try:
        hubs = data.dataHubs
        hub_count = hubs.count
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to enumerate hubs: {e}",
            "_hint": "Cloud Data API may be unavailable. Verify sign-in status in Fusion (Help → About / sign-in icon).",
        }

    for i in range(hub_count):
        if time.time() - started > LIST_PROJECTS_TIMEOUT:
            list_timed_out = True
            try:
                adsk.core.Application.log(
                    f"AdomBridge: list_cloud_projects timed out at hub {i}/{hub_count}"
                )
            except Exception:
                pass
            break

        try:
            hub = hubs.item(i)
            hub_name = hub.name
            hub_id = hub.id
        except Exception:
            continue

        try:
            adsk.core.Application.log(
                f"AdomBridge: list_cloud_projects hub [{i+1}/{hub_count}] '{hub_name}'"
            )
        except Exception:
            pass

        projects_list = []
        try:
            projects = hub.dataProjects
            proj_count = projects.count
        except Exception:
            # Couldn't enumerate this hub's projects — still report the hub
            hubs_data.append({"name": hub_name, "id": hub_id, "projects": [], "_error": "dataProjects enumeration failed"})
            continue

        for j in range(proj_count):
            try:
                proj = projects.item(j)
                proj_info = {
                    "name": proj.name,
                    "id": proj.id,
                }
            except Exception:
                continue
            try:
                rf = proj.rootFolder
                if rf:
                    proj_info["rootFolderId"] = rf.id
            except Exception:
                pass
            projects_list.append(proj_info)

        hubs_data.append({
            "name": hub_name,
            "id": hub_id,
            "projects": projects_list,
        })

        # Yield between hubs
        try:
            adsk.doEvents()
        except Exception:
            pass

    # Also note the active project (safely)
    try:
        active_proj = data.activeProject
        active_name = active_proj.name if active_proj else "none"
    except Exception:
        active_name = "(lookup failed)"

    total = sum(len(h["projects"]) for h in hubs_data)
    elapsed = round(time.time() - started, 1)
    return {
        "success": True,
        "output": f"Found {total} projects across {len(hubs_data)} hub(s). Active: {active_name}"
                  f"{' (listing timed out, partial)' if list_timed_out else ''}",
        "data": {
            "activeProject": active_name,
            "hubs": hubs_data,
            "elapsedSeconds": elapsed,
            "listTimedOut": list_timed_out,
        },
        "_hint": "Use fusion_list_cloud_files with a projectName from this list to browse folders. "
                 "Use fusion_aps_search {query} for a search across folders (seconds, server-indexed).",
    }


LIST_FILES_TIMEOUT = 60  # seconds — overall wall-clock cap


def handle_list_cloud_files(app: adsk.core.Application, args: dict) -> dict:
    """List files in a cloud project/folder.

    Robustness (v1.0.2):
      - Per-file / per-subfolder try/except so a single corrupted item
        doesn't kill the whole listing.
      - adsk.doEvents() every 50 files to keep Fusion's UI responsive on
        folders containing thousands of files.
      - Overall wall-clock cap (LIST_FILES_TIMEOUT). Returns partial
        results with listTimedOut=true instead of hanging.

    Args:
        args: Dict with:
            - projectName: Project name (optional, defaults to active project).
            - folderPath: Subfolder path (optional, defaults to root folder).
    """
    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")

    project, err = _resolve_project(app, project_name or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see valid project names.",
        }

    folder, err = _resolve_folder(project, folder_path or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_files with this projectName to see valid folderPath values.",
        }

    started = time.time()
    list_timed_out = False
    files_skipped = 0
    files_list = []

    try:
        try:
            adsk.core.Application.log(
                f"AdomBridge: list_cloud_files '{folder_path or '(root)'}' in '{project.name}'"
            )
        except Exception:
            pass

        try:
            data_files = folder.dataFiles
            file_count = data_files.count
        except Exception as e:
            return {
                "success": False,
                "error": f"Failed to enumerate files: {e}",
                "_hint": "Call fusion_get_app_state to verify sign-in status; Autodesk cloud session may have expired. If so, the user must sign in via the Fusion UI.",
            }

        for i in range(file_count):
            # Yield every 50 files
            if i > 0 and i % 50 == 0:
                try:
                    adsk.doEvents()
                except Exception:
                    pass
            # Wall-clock cap
            if time.time() - started > LIST_FILES_TIMEOUT:
                list_timed_out = True
                try:
                    adsk.core.Application.log(
                        f"AdomBridge: list_cloud_files hit timeout at file {i}/{file_count}"
                    )
                except Exception:
                    pass
                break

            # Per-item try/except — a single corrupt/deleting file shouldn't
            # break the whole listing.
            try:
                df = data_files.item(i)
            except Exception:
                files_skipped += 1
                continue
            try:
                file_info = {"name": df.name}
            except Exception:
                files_skipped += 1
                continue
            try:
                file_info["id"] = df.id
            except Exception:
                pass
            try:
                file_info["versionNumber"] = df.versionNumber
            except Exception:
                pass
            try:
                file_info["versionId"] = df.versionId
            except Exception:
                pass
            try:
                file_info["fileExtension"] = df.fileExtension
            except Exception:
                pass
            try:
                file_info["dateModified"] = str(df.dateModified)
            except Exception:
                pass
            files_list.append(file_info)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to list files: {e}",
            "_hint": "Call fusion_get_app_state to verify sign-in status; Autodesk cloud session may have expired. If so, the user must sign in via the Fusion UI.",
        }

    # Also list subfolders (per-item resilient)
    subfolders = []
    folders_skipped = 0
    try:
        folders = folder.dataFolders
        for i in range(folders.count):
            if time.time() - started > LIST_FILES_TIMEOUT:
                list_timed_out = True
                break
            try:
                sf = folders.item(i)
                subfolders.append({"name": sf.name, "id": sf.id})
            except Exception:
                folders_skipped += 1
                continue
    except Exception:
        pass

    return {
        "success": True,
        "output": f"Found {len(files_list)} files and {len(subfolders)} folders in '{folder.name}' (project: {project.name})"
                  f"{' [listing timed out, partial]' if list_timed_out else ''}",
        "data": {
            "project": project.name,
            "folder": folder.name,
            "files": files_list,
            "subfolders": subfolders,
            "elapsedSeconds": round(time.time() - started, 1),
            "filesSkipped": files_skipped,
            "foldersSkipped": folders_skipped,
            "listTimedOut": list_timed_out,
        },
    }


def handle_delete_cloud_file(app: adsk.core.Application, args: dict) -> dict:
    """Delete a file from the cloud by name.

    Searches the specified project/folder for the file and deletes it.

    Args:
        args: Dict with:
            - fileName: Name of the file to delete (required).
            - projectName: Project name (optional, defaults to active project).
            - folderPath: Subfolder path (optional, defaults to root folder).
    """
    file_name = args.get("fileName", "").strip()
    file_id = (args.get("fileId") or args.get("lineageUrn") or "").strip()  # PRECISE delete by id
    if not file_name and not file_id:
        return {"success": False, "error": "Provide fileName or fileId (lineage urn for a precise delete)."}

    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")

    project, err = _resolve_project(app, project_name or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see valid project names.",
        }

    folder, err = _resolve_folder(project, folder_path or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_files with this projectName to see valid folderPath values.",
        }

    # Find the file (per-item resilient, with yield + timeout)
    target = None
    started = time.time()
    needle = file_name.lower()
    id_needle = file_id  # exact lineage-urn match when provided (safe in shared folders)
    try:
        try:
            data_files = folder.dataFiles
            file_count = data_files.count
        except Exception as e:
            return {"success": False, "error": f"Failed to enumerate files in '{folder.name}': {e}"}
        for i in range(file_count):
            if i > 0 and i % 50 == 0:
                try:
                    adsk.doEvents()
                except Exception:
                    pass
            if time.time() - started > FIND_FILE_TIMEOUT:
                return {
                    "success": False,
                    "error": f"File lookup timed out after {FIND_FILE_TIMEOUT}s "
                             f"(scanned {i}/{file_count} files in '{folder.name}')",
                    "_hint": "Folder may be too large; provide a more specific folderPath, "
                             "or use fusion_aps_search {query}.",
                }
            try:
                df = data_files.item(i)
                if id_needle:
                    if df.id == id_needle:
                        target = df
                        break
                elif df.name.lower() == needle:
                    target = df
                    break
            except Exception:
                continue
    except Exception as e:
        return {"success": False, "error": f"Failed to search files: {e}"}

    if not target:
        return {
            "success": False,
            "error": f"File '{file_name}' not found in '{folder.name}' (project: {project.name})",
            "_hint": "Call fusion_aps_search {query} to locate the file, or fusion_list_cloud_files to list what's in the folder.",
        }

    file_id = target.id
    try:
        target.deleteMe()
    except Exception as e:
        return {
            "success": False,
            "error": f"Delete failed: {e}",
            "data": {"hint": "You may lack permissions to delete this file."},
        }

    return {
        "success": True,
        "output": f"Deleted '{file_name}' from '{folder.name}' (project: {project.name})",
        "data": {
            "fileName": file_name,
            "fileId": file_id,
            "project": project.name,
            "folder": folder.name,
        },
    }


def handle_create_cloud_folder(app: adsk.core.Application, args: dict) -> dict:
    """Create a new folder in a cloud project.

    Args:
        args: Dict with:
            - folderName: Name of the folder to create (required).
            - projectName: Target project (optional, defaults to active project).
            - parentPath: Parent folder path (optional, defaults to project root).
    """
    folder_name = args.get("folderName", "").strip()
    if not folder_name:
        return {"success": False, "error": "No folderName specified."}

    project_name = args.get("projectName", "")
    parent_path = args.get("parentPath", "")

    project, err = _resolve_project(app, project_name or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see available projects, or fusion_list_cloud_files with a known project name to browse folders.",
        }

    parent, err = _resolve_folder(project, parent_path or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see available projects, or fusion_list_cloud_files with a known project name to browse folders.",
        }

    # Check if folder already exists (per-item resilient, with yield + timeout)
    needle = folder_name.lower()
    started = time.time()
    try:
        existing = parent.dataFolders
        existing_count = existing.count
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to enumerate subfolders of '{parent.name}': {e}",
            "_hint": "Cloud Data API may be unavailable. Verify sign-in status in Fusion.",
        }
    for i in range(existing_count):
        if i > 0 and i % 50 == 0:
            try:
                adsk.doEvents()
            except Exception:
                pass
        if time.time() - started > 30:
            # Don't hang the main thread enumerating a huge subfolder list.
            # If we didn't find a match in 30s, fall through to create — Fusion
            # itself will detect a collision on add().
            break
        try:
            sf = existing.item(i)
            if sf.name.lower() == needle:
                return {
                    "success": True,
                    "output": f"Folder '{folder_name}' already exists in '{parent.name}'",
                    "data": {
                        "folderName": sf.name,
                        "folderId": sf.id,
                        "project": project.name,
                        "parentFolder": parent.name,
                        "alreadyExisted": True,
                    },
                }
        except Exception:
            continue

    try:
        new_folder = parent.dataFolders.add(folder_name)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to create folder: {e}",
            "_hint": "Call fusion_get_app_state to verify sign-in status; Autodesk cloud session may have expired. If so, the user must sign in via the Fusion UI.",
        }

    return {
        "success": True,
        "output": f"Created folder '{folder_name}' in '{parent.name}' (project: {project.name})",
        "data": {
            "folderName": new_folder.name,
            "folderId": new_folder.id,
            "project": project.name,
            "parentFolder": parent.name,
        },
    }


FIND_FILE_TIMEOUT = 60  # seconds — overall wall-clock cap for find


def _find_cloud_file(app, args):
    """Find a cloud file by name in a project/folder. Returns (dataFile, project, folder, error).

    Robustness (v1.0.2): per-file try/except + doEvents every 50 files + wall-clock cap.
    """
    file_name = args.get("fileName", "").strip()
    if not file_name:
        return None, None, None, "No fileName specified."

    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")

    project, err = _resolve_project(app, project_name or None)
    if err:
        return None, None, None, err

    folder, err = _resolve_folder(project, folder_path or None)
    if err:
        return None, None, None, err

    file_ext = args.get("fileExtension", "").strip().lower().lstrip(".")
    needle = file_name.lower()
    started = time.time()

    target = None
    try:
        try:
            data_files = folder.dataFiles
            file_count = data_files.count
        except Exception as e:
            return None, None, None, f"Failed to enumerate files in '{folder.name}': {e}"

        for i in range(file_count):
            # Yield every 50 files so Fusion UI stays responsive
            if i > 0 and i % 50 == 0:
                try:
                    adsk.doEvents()
                except Exception:
                    pass
            # Wall-clock cap
            if time.time() - started > FIND_FILE_TIMEOUT:
                return None, None, None, (
                    f"File lookup timed out after {FIND_FILE_TIMEOUT}s "
                    f"(scanned {i}/{file_count} files in '{folder.name}'). "
                    f"Folder may be too large; try a more specific folderPath."
                )

            # Per-item try/except
            try:
                df = data_files.item(i)
                df_name = df.name
            except Exception:
                continue

            if df_name.lower() == needle:
                # If extension filter specified, match it
                if file_ext:
                    try:
                        if df.fileExtension.lower().lstrip(".") != file_ext:
                            continue
                    except Exception:
                        continue
                target = df
                break
    except Exception as e:
        return None, None, None, f"Failed to search files: {e}"

    if not target:
        ext_hint = f" with extension '.{file_ext}'" if file_ext else ""
        return None, None, None, f"File '{file_name}'{ext_hint} not found in '{folder.name}' (project: {project.name})"

    return target, project, folder, None


def handle_check_recovery(app: adsk.core.Application, args: dict) -> dict:
    """Check if a cloud file has a recovery document from a previous crash.

    NOTE: The Fusion Python API does not reliably expose recovery document
    status. This command attempts to check, but may return false negatives.
    The most reliable detection is at the bridge level — if open_cloud_file
    times out, it's likely because a recovery dialog appeared.

    Args:
        fileName: Name of the file to check (required).
        projectName: Project to search in (optional, defaults to active project).
        folderPath: Subfolder path (optional, defaults to root).
    """
    target, project, folder, err = _find_cloud_file(app, args)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see available projects, or fusion_list_cloud_files with a known project name to browse folders.",
        }

    # Try to check for recovery doc — but these API methods may not exist
    has_recovery = False
    try:
        if hasattr(target, 'hasRecoveryDocument'):
            has_recovery = target.hasRecoveryDocument
    except Exception:
        pass

    return {
        "success": True,
        "output": (
            f"Recovery document found for '{target.name}' (API check)."
            if has_recovery
            else f"No recovery document detected for '{target.name}' via API. "
                 "Note: this check is not 100% reliable — Fusion may still show "
                 "a recovery dialog when opening. The bridge will detect this "
                 "via timeout if it happens."
        ),
        "data": {
            "fileName": target.name,
            "fileId": target.id,
            "project": project.name,
            "hasRecovery": has_recovery,
            "apiReliable": hasattr(target, 'hasRecoveryDocument'),
        },
    }


def _ecad_open_reprimand(app, intent=None):
    """Detect when an AI opened an electronics CHILD file instead of the PROJECT.

    Electronics files are a parent/child chain: PROJECT (EcadDesignProductType) ->
    schematic (SchematicProductType) -> board/.brd (BoardProductType) -> 3D (the leaf,
    generated from the .brd). Only the PROJECT should be opened directly; its schematic and
    board open from it. Returns a reprimand string for a child file, a confirmation for the
    project, or '' if it cannot tell.
    """
    try:
        pt = app.activeProduct.productType
    except Exception:
        return ''
    if pt == 'EcadDesignProductType':
        return ('OK: opened the electronics PROJECT file (EcadDesignProductType) - correct. '
                'Its schematic, board and 3D are children; switch to them from here.')
    nav = (' Electronics files are a parent/child chain: PROJECT (EcadDesignProductType) ->'
           ' schematic (SchematicProductType) -> board/.brd (BoardProductType) -> 3D (the leaf,'
           ' generated from the .brd). ALWAYS open the PROJECT file; its schematic and board open'
           ' from it. NEVER open a schematic, .brd, or 3D file directly to get the board.')
    if pt == 'DesignProductType':
        # A DesignProductType is every mechanical design in Fusion; it is only the wrong file
        # when it is the 3D leaf of an electronics chain (issue #86: a legitimate mechanical
        # open was told "STOP - WRONG FILE"). Judge by evidence, or by the caller's stated intent.
        if intent == 'mechanical':
            return 'OK: opened a mechanical design (DesignProductType), as requested.'
        name = ''
        try:
            name = (app.activeDocument.name or '') if app.activeDocument else ''
        except Exception:
            pass
        looks_ecad = bool(re.search(r'(?i)(\b3d\b|_3d\b|\bpcb\b|\bboard\b)', name))
        if intent == 'electronics' or looks_ecad:
            return ('STOP - WRONG FILE: you opened a 3D model file (DesignProductType) that looks like '
                    'the 3D leaf of an electronics design, not the project.' + nav)
        return ('OK: opened a mechanical design (DesignProductType). If you actually wanted an '
                'electronics board, its 3D is a child of the PROJECT (EcadDesignProductType): open '
                'the project and switch views there; pass intent:"electronics" to make this a refusal.')
    label = {'SchematicProductType': 'the SCHEMATIC',
             'BoardProductType': 'the BOARD (.brd) / its 3D PCB'}.get(pt)
    if label:
        return ('STOP - WRONG FILE: you opened ' + label + ' (' + pt + ') directly, not the project.'
                + nav)
    return ''


def handle_open_cloud_file(app: adsk.core.Application, args: dict) -> dict:
    """Open a cloud file in Fusion 360 by name.

    PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

    NOTE: If the file has a recovery document from a previous crash, Fusion
    will show a blocking modal dialog. The bridge layer (server.py) detects
    this via timeout and reports it to the AI. The AI should then screenshot
    and decide how to handle the dialog.

    Args:
        fileName: Name of the file to open (required).
        projectName: Project to search in (optional, defaults to active project).
        folderPath: Subfolder path (optional, defaults to root).
    """
    target, project, folder, err = _find_cloud_file(app, args)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see available projects, or fusion_list_cloud_files with a known project name to browse folders.",
        }

    # Open the file — if a recovery dialog appears, this call will block
    # and the bridge will detect the timeout.
    # Retry on InternalValidationError: Fusion's cloud hub frequently
    # throws this on the first try after a recent close, but the second
    # attempt (with a brief delay) usually succeeds.
    import time
    doc = None
    last_err = None
    attempts = []
    for attempt in range(3):
        try:
            doc = app.documents.open(target)
            if doc:
                break
        except Exception as e:
            last_err = str(e)
            attempts.append(f"attempt {attempt+1}: {last_err}")
            if "InternalValidationError" in last_err and attempt < 2:
                time.sleep(2 + attempt * 2)  # 2s, 4s
                continue
            break

    if not doc:
        return {
            "success": False,
            "error": f"Failed to open '{target.name}' after {len(attempts)} attempts: {last_err}",
            "_hint": "Call fusion_screenshot to check for a recovery dialog or other modal, then fusion_dismiss_blocking_dialogs to answer it, then retry.",
            "data": {"fileId": target.id, "attempts": attempts},
        }

    # Explicitly activate the freshly-opened doc. Fusion's cloud hub can
    # auto-reopen recent tabs in the background, stealing focus from the
    # doc we just opened. Force it back.
    activate_note = None
    try:
        doc.activate()
        activate_note = "activated"
    except Exception as e:
        activate_note = f"activate-failed: {e}"

    # Enumerate ALL currently open docs so the caller can see any ghost tabs
    # that Fusion auto-reopened alongside the target.
    open_docs = []
    try:
        for i in range(app.documents.count):
            d = app.documents.item(i)
            df_name = None
            try:
                df_name = d.dataFile.name if d.dataFile else None
            except Exception:
                df_name = None
            open_docs.append({
                "name": d.name,
                "dataFileName": df_name,
                "isActive": (d == app.activeDocument),
            })
    except Exception:
        pass

    # Check whether the now-active doc actually matches the target
    active_matches = False
    try:
        active_df_name = app.activeDocument.dataFile.name if app.activeDocument and app.activeDocument.dataFile else None
        active_matches = (active_df_name == target.name)
    except Exception:
        pass

    response_data = {
        "fileName": target.name,
        "fileId": target.id,
        "project": project.name,
        "documentName": doc.name,
        "activateResult": activate_note,
        "activeMatchesTarget": active_matches,
        "openDocuments": open_docs,
        "ghostTabCount": max(0, len(open_docs) - 1),
    }

    try:
        response_data["versionNumber"] = target.versionNumber
    except Exception:
        pass
    try:
        response_data["fileExtension"] = target.fileExtension
    except Exception:
        pass
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
        response_data["activeWorkspace"] = ws
    except Exception:
        pass

    _rep = _ecad_open_reprimand(app, (args or {}).get('intent'))
    return {
        "success": True,
        "output": ((_rep + " || ") if _rep.startswith("STOP") else "") + f"Opened '{target.name}' from project '{project.name}'",
        "_hint": _rep,
        "message": f"Opened '{target.name}'. IMPORTANT: Fusion may show a blocking dialog "
                   f"('What to design?', 'PCB out of date', etc.) that is NOT visible to the API. "
                   f"You MUST screenshot the Fusion window now to verify no dialog is blocking.",
        "data": response_data,
        "postOpenCheck": {
            "required": True,
            "steps": [
                "Run desktop_list_windows to find the Fusion HWND",
                "Run desktop_screenshot_window with that HWND",
                "If a dialog is visible, answer it with fusion_dismiss_blocking_dialogs",
                "Screenshot again to confirm the dialog is gone",
            ],
            "commonDialogs": [
                {"name": "What to design?", "dismiss": "fusion_dismiss_blocking_dialogs"},
                {"name": "PCB out of date", "dismiss": "Click Update or X to dismiss"},
                {"name": "Recovered Documents", "dismiss": "fusion_dismiss_recovery"},
                {"name": "Save changes?", "dismiss": "fusion_dismiss_blocking_dialogs"},
            ],
        },
    }


def handle_search_cloud_files(app: adsk.core.Application, args: dict) -> dict:
    """Search for files in a cloud project by name.

    [WARNING - DO NOT USE FOR SEARCH] This goes through Fusion's rate-limited
    add-in Data API. It is SLOW (a real project can take 30+ MINUTES) and CAN
    CRASH the live Fusion session: the add-in connection gets forcibly closed
    (WinError 10054) and Fusion goes down. Use fusion_aps_search instead (APS
    Data Management server-side index - returns in SECONDS). Read the
    'fusion-aps-search' skill before touching this verb. Only fall back here as
    a last resort when APS is unconfigured AND you accept the crash risk.

    Case-insensitive substring match on file names. Searches the root folder
    by default (fast, safe). Set recursive=true to search subfolders — but
    this makes one cloud API call per folder, so use maxDepth and maxFolders
    to stay safe.

    SAFETY (v1.0.1 — May 2026): The previous implementation had thin
    defenses and could freeze Fusion's main thread for long enough that
    Windows marked the window "Not Responding" — users perceived this as
    a crash. The fixes:

      - Per-folder wall-clock timeout (FOLDER_TIMEOUT = 30s). If a single
        folder's cloud calls take >30s we skip the rest of that folder
        and move on, instead of blocking the main thread indefinitely.
      - Overall search wall-clock cap (SEARCH_TIMEOUT = 120s). The whole
        search aborts cleanly with partial results if exceeded.
      - Per-file try/except wrapping `df.name` and `df.fileExtension`
        access. A single corrupt file no longer kills the folder.
      - adsk.doEvents() yields every 50 files (not just per-folder), so
        a folder with thousands of files doesn't starve the UI.
      - adsk.core.Application.log() per-folder progress so post-mortem
        forensics show exactly where the search was at any point.
      - Iterative BFS queue (no recursion). Prevents Python stack growth
        on deep trees.

    Args:
        args: Dict with:
            - query: Search string (required). Matches file names.
            - projectName: Limit search to this project (optional, defaults to active).
            - folderPath: Start search in this subfolder (optional, defaults to root).
            - recursive: Search subfolders (default: false — root only).
            - maxDepth: Max recursion depth (default: 2, no upper cap).
            - maxFolders: Max folders to visit (default: 10, no upper cap).
                          Default is intentionally low — search_cloud_files is
                          tuned for quick targeted lookups. For exhaustive walks
                          across hundreds of folders, prefer fusion_walk_cloud_tree
                          which has a 600s envelope vs search's 120s.
            - maxResults: Max results to return (default: 20, no upper cap).
            - folderTimeout: Per-folder cloud-API timeout in seconds (default: 30).
            - searchTimeout: Overall wall-clock timeout in seconds (default: 120).
                             If you raise this above 180, also pass `timeout:<N>`
                             at the bridge level so the HTTP waiter envelope
                             doesn't give up first.

    Returns data:
        files, totalFound, foldersSearched, foldersSkipped, project, truncated,
        folderLimitReached, searchTimedOut, elapsedSeconds, hint.
    """
    query = args.get("query", "").strip().lower()
    if not query:
        return {"success": False, "error": "No query specified."}

    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")
    recursive = bool(args.get("recursive", False))
    try:
        # No hard upper caps in v1.0.2+ — the per-folder timeout, doEvents
        # yield, per-file try/except, overall search timeout, and stale-lock
        # watchdog all make hard caps unnecessary. You can ask for an
        # exhaustive search of the entire cloud (e.g. maxFolders=10000); the
        # overall timeout governs when partial results are returned.
        max_depth = max(0, int(args.get("maxDepth", 2)))
        max_folders = max(1, int(args.get("maxFolders", 10)))
        max_results = max(1, int(args.get("maxResults", 20)))
    except (TypeError, ValueError):
        return {
            "success": False,
            "error": "maxDepth / maxFolders / maxResults must be integers.",
        }

    # Bounds on cloud-API time. The per-folder timeout protects against a
    # single slow folder. The overall search timeout is the escape hatch.
    # Both are tunable per-call via folderTimeout / searchTimeout args.
    try:
        FOLDER_TIMEOUT = max(5, int(args.get("folderTimeout", 30)))
        SEARCH_TIMEOUT = max(10, int(args.get("searchTimeout", 120)))
    except (TypeError, ValueError):
        FOLDER_TIMEOUT = 30
        SEARCH_TIMEOUT = 120

    # NOTE: the HTTP-server per-command timeout (180s by default for
    # search_cloud_files in http_server.PER_COMMAND_TIMEOUT) is the OUTER
    # envelope. If you set searchTimeout > 180, also pass `timeout`:<N>
    # at the bridge level so the HTTP waiter doesn't give up first. For
    # truly exhaustive walks of huge trees (10+ minutes), prefer
    # fusion_walk_cloud_tree (600s envelope, default maxFolders=500).

    data = app.data
    if not data:
        return {"success": False, "error": "Fusion Data API not available."}

    # Resolve starting point
    if project_name:
        project, err = _resolve_project(app, project_name)
        if err:
            return {
                "success": False,
                "error": err,
                "_hint": "Call fusion_list_cloud_projects to see valid project names.",
            }
    else:
        try:
            project = data.activeProject
        except Exception as e:
            return {"success": False, "errorCode": "data_api_not_ready",
                    "error": f"activeProject lookup failed: {e}",
                    "_hint": "Fusion's Data API is not ready yet (normal for ~30-60s after launch, "
                             "seen after fusion_restart_for_addin): retry shortly, or pass "
                             "projectName so the project is resolved by name."}
        if not project:
            return {"success": False, "error": "No active project. Specify projectName."}

    start_folder, err = _resolve_folder(project, folder_path or None)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_files with this projectName to see valid folderPath values.",
        }

    results = []
    folders_visited = 0
    folders_skipped = 0
    files_skipped = 0
    search_timed_out = False

    # Set progress state for external observers
    global _walk_progress
    search_start = time.time()
    with _walk_progress_lock:
        _walk_progress = {
            "command": "search_cloud_files",
            "startedAt": search_start,
            "project": project.name,
            "query": query,
            "foldersVisited": 0,
            "currentFolder": folder_path or "(root)",
            "filesFound": 0,
            "elapsedSeconds": 0,
        }

    # Iterative BFS queue: list of (folder, path, depth). No recursion =
    # bounded Python stack depth + easy to short-circuit on timeout.
    start_path = folder_path or ""
    queue = [(start_folder, start_path, 0)]

    try:
        while queue:
            # Overall search timeout — return partial results cleanly.
            if time.time() - search_start > SEARCH_TIMEOUT:
                search_timed_out = True
                try:
                    adsk.core.Application.log(
                        f"AdomBridge: search hit overall timeout ({SEARCH_TIMEOUT}s) "
                        f"after {folders_visited} folder(s), {len(results)} result(s)"
                    )
                except Exception:
                    pass
                break

            # Folder / result caps
            if folders_visited >= max_folders or len(results) >= max_results:
                break

            folder, path, depth = queue.pop(0)
            folders_visited += 1
            folder_start = time.time()

            try:
                adsk.core.Application.log(
                    f"AdomBridge: search [{folders_visited}/{folders_visited + len(queue)}] '{path or '(root)'}' (depth={depth})"
                )
            except Exception:
                pass

            # ── Search files in this folder ──
            try:
                data_files = folder.dataFiles
                # If even getting the dataFiles handle took >FOLDER_TIMEOUT,
                # skip iteration entirely.
                if time.time() - folder_start > FOLDER_TIMEOUT:
                    folders_skipped += 1
                    try:
                        adsk.core.Application.log(
                            f"AdomBridge: search skipping slow folder '{path}' (>{FOLDER_TIMEOUT}s on dataFiles)"
                        )
                    except Exception:
                        pass
                else:
                    file_count = data_files.count
                    for i in range(file_count):
                        # Yield every 50 files to keep Fusion responsive.
                        # Without this, a folder with 1000 files would
                        # block the main thread for the whole iteration.
                        if i > 0 and i % 50 == 0:
                            try:
                                adsk.doEvents()
                            except Exception:
                                pass
                        # Per-file timeout check (cheap).
                        if time.time() - folder_start > FOLDER_TIMEOUT:
                            folders_skipped += 1
                            try:
                                adsk.core.Application.log(
                                    f"AdomBridge: search folder '{path}' hit per-folder timeout at file {i}"
                                )
                            except Exception:
                                pass
                            break
                        if len(results) >= max_results:
                            break

                        # Per-file try/except — a single corrupt or
                        # in-flight-deletion file shouldn't kill the
                        # whole folder. Just skip it and keep going.
                        # We COUNT every skip so the caller knows "0 results"
                        # could be a false negative if files_skipped > 0.
                        try:
                            df = data_files.item(i)
                        except Exception:
                            files_skipped += 1
                            continue
                        try:
                            df_name = df.name
                        except Exception:
                            files_skipped += 1
                            continue
                        if query in df_name.lower():
                            info = {
                                "name": df_name,
                                "project": project.name,
                                "folderPath": path or "/",
                            }
                            try:
                                info["id"] = df.id
                            except Exception:
                                pass
                            try:
                                info["versionNumber"] = df.versionNumber
                            except Exception:
                                pass
                            try:
                                info["fileExtension"] = df.fileExtension
                            except Exception:
                                pass
                            results.append(info)
            except Exception as e:
                folders_skipped += 1
                try:
                    adsk.core.Application.log(
                        f"AdomBridge: search dataFiles error in '{path}': {e}"
                    )
                except Exception:
                    pass

            # Update progress for external observers (health endpoint, bridge)
            with _walk_progress_lock:
                if _walk_progress is not None:
                    _walk_progress["foldersVisited"] = folders_visited
                    _walk_progress["currentFolder"] = path
                    _walk_progress["filesFound"] = len(results)
                    _walk_progress["elapsedSeconds"] = round(time.time() - search_start, 1)
                    _walk_progress["queueSize"] = len(queue)

            # Yield to Fusion's event loop between folders.
            try:
                adsk.doEvents()
            except Exception:
                pass

            # ── Enqueue subfolders ──
            if recursive and depth < max_depth and (time.time() - folder_start <= FOLDER_TIMEOUT):
                try:
                    subfolders = folder.dataFolders
                    sub_count = subfolders.count
                    for i in range(sub_count):
                        if len(results) >= max_results:
                            break
                        try:
                            sf = subfolders.item(i)
                            sub_path = f"{path}/{sf.name}" if path else sf.name
                            queue.append((sf, sub_path, depth + 1))
                        except Exception:
                            continue
                except Exception as e:
                    folders_skipped += 1
                    try:
                        adsk.core.Application.log(
                            f"AdomBridge: search dataFolders error in '{path}': {e}"
                        )
                    except Exception:
                        pass
    finally:
        with _walk_progress_lock:
            _walk_progress = None

    hit_folder_limit = folders_visited >= max_folders
    hit_result_limit = len(results) >= max_results
    elapsed_total = time.time() - search_start

    # ── searchComplete: TRUE only if we exhausted the search space ──
    # Any cap hit OR any skip means we might have missed matches.
    # The AI should treat "0 results + searchComplete=false" as
    # "result unknown", never as "confirmed not present".
    search_complete = (
        not hit_folder_limit
        and not hit_result_limit
        and not search_timed_out
        and folders_skipped == 0
        and files_skipped == 0
    )

    # Cost analysis — useful for both reporting back and informing
    # whether to ask the user for narrower scope next time.
    folders_per_sec = (folders_visited / elapsed_total) if elapsed_total > 0 else 0.0
    cost_analysis = {
        "elapsedSeconds": round(elapsed_total, 1),
        "foldersPerSecond": round(folders_per_sec, 2),
        # Rough estimate based on observed throughput. If the user wanted
        # an exhaustive sweep of the entire project, this is how long it
        # might take given THIS search's average folder latency. (Can't
        # know the project's actual folder count without walking it.)
        "estimatedSecondsPer100Folders": round(100 / folders_per_sec) if folders_per_sec > 0 else None,
    }

    # ── Build a hint that's EXPLICIT about what kind of "0" we got ──
    hint_parts = []

    if search_complete:
        if len(results) == 0:
            hint_parts.append(
                f"COMPLETE SEARCH: searched {folders_visited} folder(s) in "
                f"{round(elapsed_total)}s and found NO files matching '{query}'. "
                f"This is a confirmed negative — the file does not exist in "
                f"'{folder_path or project.name}'."
            )
        else:
            hint_parts.append(
                f"COMPLETE SEARCH: all {len(results)} matches returned."
            )
    else:
        # Search was incomplete — be explicit about WHY and what it implies
        reasons = []
        if hit_folder_limit:
            reasons.append(f"hit maxFolders={max_folders}")
        if hit_result_limit:
            reasons.append(f"hit maxResults={max_results}")
        if search_timed_out:
            reasons.append(f"hit searchTimeout={SEARCH_TIMEOUT}s")
        if folders_skipped > 0:
            reasons.append(f"{folders_skipped} folder(s) errored/timed out")
        if files_skipped > 0:
            reasons.append(f"{files_skipped} file(s) errored on .name access")

        hint_parts.append(
            f"INCOMPLETE SEARCH ({'; '.join(reasons)}). "
            f"Found {len(results)} match(es) in {folders_visited} folder(s) "
            f"over {round(elapsed_total)}s — but this is a LOWER BOUND, not exhaustive."
        )

        if len(results) == 0:
            hint_parts.append(
                "CRITICAL: 0 results does NOT mean the file is absent. The search "
                "stopped before exhausting the scope. Do NOT report 'file not found' "
                "to the user — re-run with broader caps OR narrower folderPath first."
            )

    # ── Proactive scope-narrowing nudge for broad searches ──
    # If this was a wide search and took non-trivial time, the AI should
    # (a) tell the user why this is slow (Autodesk API design, NOT adom-desktop)
    # (b) suggest the simple fix (folderPath narrowing)
    # (c) mention the long-term community fix (paid APS, fork + PR)
    if recursive and not folder_path and elapsed_total > 30:
        cost_str = (
            f" (at the observed {folders_per_sec:.1f} folders/sec, "
            f"a 10-folder subtree would take ~{round(10 / folders_per_sec)}s "
            f"vs this {round(elapsed_total)}s scan)"
            if folders_per_sec > 0 else ""
        )
        hint_parts.append(
            f"BROAD-SCOPE NOTE: searched '{project.name}' from root in "
            f"{round(elapsed_total)}s. WHY SLOW: Autodesk's free Fusion 360 "
            f"Python API has no indexed file-search endpoint, so we must walk "
            f"the cloud Data API one folder at a time — each folder is a "
            f"separate HTTP round-trip to Autodesk. THIS IS AN AUTODESK API "
            f"LIMITATION, NOT AN ADOM-DESKTOP LIMITATION — when you tell the "
            f"user it's slow, attribute it to Autodesk / Fusion 360, not to "
            f"adom-desktop's bridge. Autodesk's PAID Autodesk Platform Services "
            f"(APS, formerly Forge) Data Management API DOES expose indexed "
            f"search that would make this ~instant; if your team has APS "
            f"credentials, you (or anyone in the community) can fork the "
            f"Adom Bridge Fusion bridge to add an aps_search verb and PR it "
            f"back to https://wiki.adom.inc/adom/fusion-bridge "
            f"— the whole Adom community would benefit. FOR THIS SEARCH "
            f"RIGHT NOW: just ask the user 'do you know which subfolder "
            f"<query> is typically stored in?'{cost_str} Naming a folder "
            f"like 'Molecules' cuts search 10-50x with zero new API."
        )
    elif not recursive and len(results) == 0:
        hint_parts.append(
            "Search was limited to root folder (recursive=false). If the file "
            "could be in a subfolder, set recursive=true and ideally folderPath "
            "to the most likely parent (ask the user if unsure)."
        )

    # If we hit the result cap, more matches may exist
    if hit_result_limit:
        hint_parts.append(
            f"RESULT CAP HIT: you got the first {max_results} matches but more "
            f"may exist. Raise maxResults to see all."
        )

    full_hint = " | ".join(hint_parts) if hint_parts else None

    return {
        "success": True,
        "output": f"Found {len(results)} file(s) matching '{query}' "
                  f"(searched {folders_visited} folder(s) in '{project.name}'"
                  f"{', search timed out' if search_timed_out else ''}"
                  f"{', INCOMPLETE' if not search_complete else ''})",
        "data": {
            "query": query,
            "files": results,
            "totalFound": len(results),
            "foldersSearched": folders_visited,
            "foldersSkipped": folders_skipped,
            "filesSkipped": files_skipped,
            "project": project.name,
            "scopeFolderPath": folder_path or "(project root)",
            "recursive": recursive,
            # ── The big flag the AI must check before reporting "no match" ──
            "searchComplete": search_complete,
            "truncated": hit_result_limit,
            "folderLimitReached": hit_folder_limit,
            "searchTimedOut": search_timed_out,
            "elapsedSeconds": round(elapsed_total, 1),
            "costAnalysis": cost_analysis,
            "hint": full_hint,
        },
        "_hint": full_hint,  # also at top level for AI scaffolding that checks _hint
    }


def handle_export_cloud_file(app: adsk.core.Application, args: dict) -> dict:
    """Export the active Fusion document to a local file for transfer.

    Exports the currently open document as STEP, STL, F3D, or other format
    to a local file path. The file can then be pulled back to Docker via pull_file.

    Args:
        args: Dict with:
            - outputPath: Local file path to export to (required).
            - format: Export format (optional, default "step").
                     Supported: "step", "stl", "f3d", "iges", "sat", "smt".
    """
    output_path = args.get("outputPath", "").strip()
    if not output_path:
        return {"success": False, "error": "No outputPath specified."}

    output_path = output_path.replace("\\", "/")
    fmt = args.get("format", "step").lower().strip()

    doc = app.activeDocument
    if not doc:
        return {
            "success": False,
            "error": "No active document to export.",
            "_hint": "Call fusion_open_cloud_file to open a file, then retry.",
        }

    # Get the design product
    design = None
    try:
        design = adsk.fusion.Design.cast(app.activeProduct)
    except Exception:
        pass

    if not design:
        return {
            "success": False,
            "error": "Active document is not a Fusion Design. Cannot export.",
            "_hint": "Call fusion_activate_workspace with target='design' or open a Fusion Design document first, then retry.",
            "data": {"hint": "Open a design document first, or switch to the Design workspace."},
        }

    # Ensure output directory exists
    out_dir = os.path.dirname(output_path)
    if out_dir and not os.path.exists(out_dir):
        os.makedirs(out_dir, exist_ok=True)

    root = design.rootComponent
    export_mgr = design.exportManager

    try:
        if fmt in ("step", "stp"):
            options = export_mgr.createSTEPExportOptions(output_path, root)
            export_mgr.execute(options)
        elif fmt == "stl":
            options = export_mgr.createSTLExportOptions(root, output_path)
            export_mgr.execute(options)
        elif fmt in ("iges", "igs"):
            options = export_mgr.createIGESExportOptions(output_path, root)
            export_mgr.execute(options)
        elif fmt == "sat":
            options = export_mgr.createSATExportOptions(output_path, root)
            export_mgr.execute(options)
        elif fmt == "smt":
            options = export_mgr.createSMTExportOptions(output_path, root)
            export_mgr.execute(options)
        elif fmt == "f3d":
            options = export_mgr.createFusionArchiveExportOptions(output_path)
            export_mgr.execute(options)
        else:
            return {
                "success": False,
                "error": f"Unsupported format: {fmt}",
                "data": {"supportedFormats": ["step", "stl", "f3d", "iges", "sat", "smt"]},
            }
    except Exception as e:
        return {"success": False, "error": f"Export failed: {e}"}

    if not os.path.exists(output_path):
        return {"success": False, "error": f"Export file was not created at {output_path}"}

    size = os.path.getsize(output_path)

    return {
        "success": True,
        "output": f"Exported '{doc.name}' as {fmt.upper()} to {output_path} ({size} bytes)",
        "data": {
            "outputPath": output_path,
            "format": fmt,
            "fileSize": size,
            "documentName": doc.name,
        },
    }


def handle_walk_cloud_tree(app: adsk.core.Application, args: dict) -> dict:
    """Walk a cloud folder tree iteratively and return a flat list of all files and folders.

    [WARNING - DO NOT USE FOR SEARCH] Goes through Fusion's rate-limited add-in
    Data API: SLOW (30+ MINUTES on a real project) and CAN CRASH the live Fusion
    session (add-in connection forcibly closed, WinError 10054 - Fusion goes
    down). Use fusion_aps_search instead (APS server-side index, seconds). See
    the 'fusion-aps-search' skill. Last-resort only, when APS is unconfigured
    and you accept the crash risk.

    Purpose: replace the broken recursive search_cloud_files. No sleeps, no tiny
    hardcoded caps. Single add-in call does the whole BFS inside the main thread
    — one request to the bridge, one response back.

    Args:
        args: Dict with:
            - projectName: Project name (optional, defaults to active project).
            - folderPath: Starting folder path (optional, defaults to root).
            - maxDepth: Max recursion depth (optional, default 10).
            - maxFolders: Hard cap on number of folders visited (optional, default 500).
            - extensions: List of file extensions to include, e.g. ["f3d", "fprj"]
                          (optional, default: include all files).
            - nameContains: Substring filter for file/folder names, case-insensitive
                          (optional, default: no filter).
            - includeFiles: If False, return only the folder tree (default True).

    Returns data:
        - project: project name
        - rootFolder: starting folder name
        - folders: [{path, name, id, depth}]
        - files:   [{path, folder, name, id, fileExtension, versionNumber}]
        - stats: {foldersVisited, foldersSkipped, filesFound, maxDepthReached, truncated}
    """
    project_name = args.get("projectName", "") or None
    folder_path = args.get("folderPath", "") or None
    max_depth = int(args.get("maxDepth", 10))
    max_folders = int(args.get("maxFolders", 500))
    extensions = args.get("extensions") or []
    if isinstance(extensions, str):
        extensions = [extensions]
    extensions = {e.lower().lstrip(".") for e in extensions}
    name_contains = (args.get("nameContains") or "").lower()
    include_files = args.get("includeFiles", True)

    project, err = _resolve_project(app, project_name)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_projects to see valid project names.",
        }

    root, err = _resolve_folder(project, folder_path)
    if err:
        return {
            "success": False,
            "error": err,
            "_hint": "Call fusion_list_cloud_files with this projectName to see valid folderPath values.",
        }

    folders_out = []
    files_out = []
    folders_visited = 0
    folders_skipped = 0
    files_found = 0
    max_depth_reached = 0
    truncated = False

    # Iterative BFS queue: list of (folder, path, depth)
    start_path = root.name if folder_path else ""
    queue = [(root, start_path, 0)]

    # Set progress state so external callers (health endpoint, bridge busy
    # gate) know a walk is running and can report progress.
    global _walk_progress
    walk_start = time.time()
    with _walk_progress_lock:
        _walk_progress = {
            "command": "walk_cloud_tree",
            "startedAt": walk_start,
            "project": project.name,
            "rootFolder": root.name,
            "foldersVisited": 0,
            "queueSize": len(queue),
            "currentFolder": start_path or "(root)",
            "filesFound": 0,
            "elapsedSeconds": 0,
        }

    try:
        while queue:
            if folders_visited >= max_folders:
                truncated = True
                break

            folder, path, depth = queue.pop(0)
            folders_visited += 1
            if depth > max_depth_reached:
                max_depth_reached = depth

            # Per-folder wall-clock timeout — if a single folder's cloud API
            # calls take >30s, skip remaining work and move on. We can't
            # interrupt folder.dataFiles mid-call, but we can skip subfolder
            # enumeration and file iteration if the first call was too slow.
            FOLDER_TIMEOUT = 30  # seconds
            folder_start = time.time()

            adsk.core.Application.log(
                f"AdomBridge: walk [{folders_visited}/{folders_visited + len(queue)}] '{path}'"
            )

            folders_out.append({
                "path": path,
                "name": folder.name,
                "id": folder.id,
                "depth": depth,
            })

            # List files in this folder
            if include_files:
                try:
                    data_files = folder.dataFiles
                    if time.time() - folder_start > FOLDER_TIMEOUT:
                        adsk.core.Application.log(
                            f"AdomBridge: walk skipping slow folder '{path}' (>{FOLDER_TIMEOUT}s on dataFiles)"
                        )
                        folders_skipped += 1
                    else:
                        for i in range(data_files.count):
                            # Yield every 50 files to keep Fusion responsive
                            if i > 0 and i % 50 == 0:
                                try:
                                    adsk.doEvents()
                                except Exception:
                                    pass
                            if time.time() - folder_start > FOLDER_TIMEOUT:
                                adsk.core.Application.log(
                                    f"AdomBridge: walk folder '{path}' hit timeout during file iteration at file {i}"
                                )
                                folders_skipped += 1
                                break
                            try:
                                df = data_files.item(i)
                                name = df.name
                                ext = ""
                                try:
                                    ext = (df.fileExtension or "").lower()
                                except Exception:
                                    pass
                                if extensions and ext not in extensions:
                                    continue
                                if name_contains and name_contains not in name.lower():
                                    continue
                                entry = {
                                    "path": (path + "/" + name) if path else name,
                                    "folder": path,
                                    "name": name,
                                    "id": df.id,
                                    "fileExtension": ext,
                                }
                                try:
                                    entry["versionNumber"] = df.versionNumber
                                except Exception:
                                    pass
                                files_out.append(entry)
                                files_found += 1
                            except Exception:
                                continue
                except Exception:
                    folders_skipped += 1

            # Enqueue subfolders (unless at depth limit or folder timed out)
            if depth < max_depth and (time.time() - folder_start <= FOLDER_TIMEOUT):
                try:
                    subfolders = folder.dataFolders
                    for i in range(subfolders.count):
                        try:
                            sf = subfolders.item(i)
                            sub_path = (path + "/" + sf.name) if path else sf.name
                            queue.append((sf, sub_path, depth + 1))
                        except Exception:
                            continue
                except Exception:
                    folders_skipped += 1
            elif depth < max_depth:
                adsk.core.Application.log(
                    f"AdomBridge: walk skipping subfolders of '{path}' (folder timed out)"
                )

            # Update progress for external observers (health endpoint, bridge)
            with _walk_progress_lock:
                if _walk_progress is not None:
                    _walk_progress["foldersVisited"] = folders_visited
                    _walk_progress["queueSize"] = len(queue)
                    _walk_progress["currentFolder"] = path
                    _walk_progress["filesFound"] = files_found
                    _walk_progress["elapsedSeconds"] = round(time.time() - walk_start, 1)

            # Yield to Fusion's event loop to prevent "Not Responding" state.
            try:
                adsk.doEvents()
            except Exception:
                pass

    finally:
        with _walk_progress_lock:
            _walk_progress = None

    summary = (
        f"Walked {folders_visited} folders under '{root.name}' in project '{project.name}' "
        f"(max depth {max_depth_reached}). Found {files_found} files."
    )
    if truncated:
        summary += f" TRUNCATED at maxFolders={max_folders} — increase limit or narrow folderPath."

    return {
        "success": True,
        "output": summary,
        "data": {
            "project": project.name,
            "rootFolder": root.name,
            "folders": folders_out,
            "files": files_out,
            "stats": {
                "foldersVisited": folders_visited,
                "foldersSkipped": folders_skipped,
                "filesFound": files_found,
                "maxDepthReached": max_depth_reached,
                "truncated": truncated,
            },
        },
    }


def handle_open_by_urn(app: adsk.core.Application, args: dict) -> dict:
    """Open a cloud file directly by its APS/Fusion URN.

    PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

    Works for ANY file regardless of folder nesting (unlike open_cloud_file,
    which searches by name only in a project's root folder). APS hands the
    bridge the exact file URN; we resolve it to a Fusion DataFile via
    Data.findFileById and open it.
    """
    urn = (args.get("urn") or args.get("fileUrn") or "").strip()
    if not urn:
        return {"success": False, "error": "No urn specified."}

    # File-based breadcrumb (Application.log only reaches the in-app Text Commands
    # palette, which is unreadable headless). Opening a heavy assembly or an
    # electronics/PCB design can hard-crash the host mid-open with no other trace;
    # this leaves the last step on disk so the crash point is provable afterward.
    import os as _os, tempfile as _tf
    _crumb_path = _os.path.join(_tf.gettempdir(), "adom_open_by_urn.log")
    def _crumb(msg):
        try:
            with open(_crumb_path, "a", encoding="utf-8") as _fh:
                _fh.write(msg + "\n")
                _fh.flush()
        except Exception:
            pass
        try:
            adsk.core.Application.log("[open_by_urn] " + msg)
        except Exception:
            pass

    data = app.data
    # APS returns a version urn like urn:adsk.wipprod:fs.file:vf.<base>?version=N.
    # findFileById wants the file/lineage id — try several derivations.
    raw = urn.split("?")[0]
    candidates = [urn, raw]
    if "fs.file:vf." in raw:
        base = raw.split("fs.file:vf.")[-1]
        candidates.append("urn:adsk.wipprod:dm.lineage:" + base)
        candidates.append(base)

    _crumb("resolving urn: " + urn[:80])

    df = None
    tried = []
    for cid in candidates:
        try:
            f = data.findFileById(cid)
            if f:
                df = f
                tried.append(cid[:55] + " -> FOUND")
                break
            tried.append(cid[:55] + " -> none")
        except Exception as e:
            tried.append(cid[:55] + " -> " + str(e)[:40])
    if not df:
        _crumb("FAILED to resolve urn to a DataFile. tried=" + " | ".join(tried))
        return {"success": False,
                "error": "Could not resolve the file URN to a Fusion DataFile.",
                "_hint": "URN resolution failed — an open-path issue, NOT the Fusion license. A "
                         "read-only/expired/trial Fusion still opens+views files (only save/export "
                         "are blocked). Check the urn form / file access.",
                "data": {"tried": tried, "urn": urn}}

    _crumb("resolved '%s' (id=%s) — about to call documents.open "
           "(PCB/electronics designs spin up the Electronics editor here; a heavy "
           "assembly downloads all referenced parts here — the wedge/crash point)"
           % (df.name, df.id))

    doc = None
    last_err = None
    for attempt in range(3):
        try:
            doc = app.documents.open(df)
            if doc:
                _crumb("documents.open returned a doc for '%s'" % df.name)
                break
        except Exception as e:
            last_err = str(e)
            if "InternalValidationError" in last_err and attempt < 2:
                time.sleep(2 + attempt * 2)
                continue
            break
    if not doc:
        return {"success": False, "error": "Open failed for '%s': %s" % (df.name, last_err),
                "_hint": "documents.open failed — an open-path issue, NOT the Fusion license. A "
                         "read-only/expired/trial Fusion still opens+views files (only save/export "
                         "are blocked). Likely a slow/large assembly, a blocking Electronics picker, "
                         "or a transient cloud error — retry / check fusion_state {screenshot:false} for a modal.",
                "data": {"fileName": df.name, "fileId": df.id}}
    try:
        doc.activate()
    except Exception:
        pass
    _rep = _ecad_open_reprimand(app, (args or {}).get('intent'))
    return {"success": True, "output": ((_rep + " || ") if _rep.startswith("STOP") else "") + ("Opened '%s'." % df.name),
            "_hint": _rep,
            "data": {"fileName": df.name, "fileId": df.id, "resolved": tried}}
