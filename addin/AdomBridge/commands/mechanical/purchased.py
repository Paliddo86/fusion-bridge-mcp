"""Local supplier CAD ingestion and Fusion's native McMaster command."""
import hashlib
from pathlib import Path
from urllib.parse import urlparse

import adsk.core

from . import transactions
from .assemblies import apply_metadata, created_occurrence_result, hybrid, parent_component, place_created_occurrence, transform
from .assembly_state import metadata
from .core import MechanicalError, active, current_target, items, ref, require, resolve, target

MCM_COMMAND = "InsertMcMasterCarrComponentCommand"


def file_hash(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def preflight_file(args):
    path = Path(args["filename"])
    require(path.is_absolute() and path.is_file() and not path.is_symlink(), "invalid_source", "Supplier CAD must be an absolute regular local file.")
    require(path.suffix.lower() in (".step", ".stp", ".sat"), "unsupported_format", "Local supplier ingestion supports STEP and SAT.")
    require(0 < path.stat().st_size <= 512 * 1024 * 1024, "invalid_source", "Supplier CAD must be nonempty and no larger than 512 MiB.")
    provenance = args["provenance"]
    url = urlparse(provenance["sourceUrl"])
    require(url.scheme == "https" and url.hostname and not url.username and not url.password, "invalid_provenance", "sourceUrl must be an HTTPS supplier URL without credentials.")
    if provenance["supplier"].lower() in ("mcmaster", "mcmaster-carr"):
        require(url.hostname.lower() == "mcmaster.com" or url.hostname.lower().endswith(".mcmaster.com"), "invalid_provenance", "McMaster provenance must use the supplier's domain.")
    sha = file_hash(path)
    if "sha256" in args:
        require(args["sha256"].lower() == sha, "source_hash_mismatch", "Downloaded CAD does not match the expected SHA-256.")
    return path, sha


def cad_import(app, args):
    # Source preflight runs before any transaction/model mutation. The importer
    # only reads this file; no cloud upload, browser download or file rewrite.
    target(app, args, True)
    path, sha = preflight_file(args)
    document_count = app.documents.count
    manager = app.importManager
    factory = manager.createSATImportOptions if path.suffix.lower() == ".sat" else manager.createSTEPImportOptions
    options = factory(str(path))
    require(options is not None, "import_unavailable", "Fusion refused import options.")
    options.isViewFit = False

    def mutate(design, value, request):
        hybrid(design)
        parent = parent_component(design, value, request)
        expected = transform(request.get("transform"))
        occurrence = parent.occurrences.addNewComponent(expected)
        require(occurrence is not None, "import_failed", "Could not create import wrapper component.")
        comp = occurrence.component
        comp.name = request.get("name", path.stem)
        imported = manager.importToTarget2(options, comp)
        require(imported is not None and imported.count > 0, "import_failed", "Fusion returned no imported objects.")
        require(app.documents.count == document_count, "import_document_side_effect", "Importer changed the document set; inspect documents before continuing.")
        components = [comp] + [o.component for o in items(comp.allOccurrences)]
        require(any(c.bRepBodies.count for c in components), "import_empty", "Imported CAD contains no BRep bodies.")
        require(file_hash(path) == sha, "source_changed", "Supplier CAD changed while import was running.")
        provenance = {**request["provenance"], "sourceFile": str(path), "sha256": sha,
                      "asPurchased": request.get("asPurchased", True), "partNumber": request["provenance"]["supplierPartNumber"]}
        apply_metadata(comp, provenance)
        captured = place_created_occurrence(design, parent, occurrence, expected)
        return lambda fresh: {**created_occurrence_result(design, parent, occurrence, fresh, expected), "positionCaptured": captured,
                              "source": {"filename": str(path), "sha256": sha},
                              "importedObjectCount": imported.count, "externalEffects": "read-only source file; no cloud or document creation requested"}
    try:
        return transactions.execute(app, args, mutate)
    except MechanicalError as exc:
        exc.details["sourceFileModified"] = False
        exc.details["documentSetUnchanged"] = app.documents.count == document_count
        raise


def mcmaster_discover(app, args):
    _, design, _ = active(app)
    command = app.userInterface.commandDefinitions.itemById(MCM_COMMAND)
    return {"target": current_target(app), "commandId": MCM_COMMAND, "available": command is not None,
            "name": command.name if command else None, "designIntent": str(getattr(design, "designIntent", "unavailable")),
            "requirements": ["Hybrid design", "online supplier access", "interactive selection and download"],
            "downloadedCadAlternative": "cad_import with local STEP/SAT and supplier provenance"}


def mcmaster_open(app, args):
    _, design, value = target(app, args, True)
    hybrid(design)
    command = app.userInterface.commandDefinitions.itemById(MCM_COMMAND)
    require(command is not None, "mcmaster_unavailable", "Fusion's native McMaster command is not installed.")
    require(command.execute(), "mcmaster_launch_failed", "Fusion refused the native McMaster command.")
    return {"commandId": MCM_COMMAND, "launchRequested": True, "targetAtLaunch": value,
            "completion": "interactive; inspect after inserting or cancelling", "refreshRequired": True,
            "transactional": False, "recovery": "native command controls its own user interaction"}


def material_list(app, args):
    _, design, value = target(app, args)
    libraries = [{"id": library.id, "name": library.name, "count": library.materials.count} for library in items(app.materialLibraries)]
    query = args.get("query", "").lower()
    if args.get("libraryId"):
        matches = [lib for lib in items(app.materialLibraries) if lib.id == args["libraryId"]]
        require(len(matches) == 1, "material_library_missing", "Material library ID must identify one loaded library.")
        sources = [(matches[0].id, matches[0].materials)]
    else:
        # the document's own materials first, then every loaded library: a new design holds only
        # the materials it uses, so a search for "copper" there used to find nothing. Each row
        # names the libraryId that material_assign needs (null: the document's own)
        sources = [(None, design.materials)] + [(lib.id, lib.materials) for lib in items(app.materialLibraries)]
    rows = [{"id": material.id, "name": material.name, "libraryId": library_id}
            for library_id, source in sources for material in items(source) if query in material.name.lower()]
    limit = args.get("limit", 100)
    return {"target": value, "libraries": libraries, "materials": rows[:limit], "complete": len(rows) <= limit,
            "matchingCount": len(rows), "libraryId": args.get("libraryId")}


def material_assign(app, args):
    def mutate(design, value, request):
        entity = resolve(design, value, request["entity"], ("Component", "BRepBody"))
        require(getattr(entity, "assemblyContext", None) is None, "definition_reference_required", "Material assignment affects a native component or body definition.")
        source = design.materials
        if request.get("libraryId"):
            matches = [lib for lib in items(app.materialLibraries) if lib.id == request["libraryId"]]
            require(len(matches) == 1, "material_library_missing", "Material library ID must identify one loaded library.")
            source = matches[0].materials
        material = source.itemById(request["materialId"])
        require(material is not None, "material_missing", "Material ID was not found in the selected source.")
        entity.material = material
        def result(fresh):
            require(entity.material is not None and entity.material.id == material.id, "material_failed", "Assigned material ID did not read back; same display names are not material identity.")
            return {"entity": ref(entity, fresh), "material": {"id": entity.material.id, "name": entity.material.name}}
        return result
    return transactions.execute(app, args, mutate)


OPERATIONS = {"cad_import": cad_import, "mcmaster_open": mcmaster_open, "material_assign": material_assign,
              "mcmaster_discover": mcmaster_discover, "material_list": material_list}
WRITES = {"cad_import", "mcmaster_open", "material_assign"}
