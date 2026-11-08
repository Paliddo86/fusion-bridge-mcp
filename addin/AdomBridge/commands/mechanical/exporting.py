"""Single-file exports: preflight, isolated staging, verified atomic publication."""
import hashlib
import os
from pathlib import Path
import tempfile

import adsk.fusion

from .core import MechanicalError, require

FORMATS = {
    "step": ("createSTEPExportOptions", (".step", ".stp"), "file"),
    "stl": ("createSTLExportOptions", (".stl",), "entity"),
    "3mf": ("createC3MFExportOptions", (".3mf",), "entity"),
    "f3d": ("createFusionArchiveExportOptions", (".f3d",), "file"),
    "iges": ("createIGESExportOptions", (".igs", ".iges"), "file"),
    "sat": ("createSATExportOptions", (".sat",), "file"),
    "smt": ("createSMTExportOptions", (".smt",), "file"),
    "usdz": ("createUSDExportOptions", (".usdz",), "file"),
    "fbx": ("createFBXExportOptions", (".fbx",), "file"),
    "skp": ("createSketchUpExportOptions", (".skp",), "file"),
}


def publish_staged(staged, destination, overwrite):
    require(staged.is_file() and staged.stat().st_size > 0, "empty_export", "Exporter produced no nonempty file.")
    size = staged.stat().st_size
    sha = hashlib.sha256(staged.read_bytes()).hexdigest()
    result = {"outputPath": str(destination), "fileSize": size, "sha256": sha,
              "published": True, "overwrite": overwrite}
    if overwrite:
        os.replace(str(staged), str(destination))
    else:
        # Atomic exclusive publication. Checking exists then replace has a race that
        # could overwrite another writer's file. A hardlink never replaces it.
        os.link(str(staged), str(destination))
        try:
            staged.unlink()
        except OSError as exc:
            # The hardlink is the publication commit point. A leftover staging
            # link is cleanup work, not a failed export or preserved destination.
            result["cleanupWarning"] = str(exc)
    return result


def export_design(design, args):
    fmt = args.get("format", "step").lower()
    require(fmt in FORMATS, "unsupported_format", "Single-file export format is unavailable. Sketch DXF requires an explicit sketch; OBJ sidecars need a bundle exporter.")
    method, suffixes, mode = FORMATS[fmt]
    manager = design.exportManager
    factory = getattr(manager, method, None)
    require(callable(factory), "unsupported_capability", method + " is unavailable in this Fusion build.")
    raw = args.get("outputPath")
    require(isinstance(raw, str) and raw, "invalid_path", "outputPath is required.")
    destination = Path(raw)
    require(destination.is_absolute(), "invalid_path", "outputPath must be absolute on the Fusion host.")
    require(destination.suffix.lower() in suffixes, "extension_mismatch", "File extension does not match requested format.")
    overwrite = args.get("overwrite", False)
    require(isinstance(overwrite, bool), "invalid_input", "overwrite must be a boolean.")
    require(not destination.exists() or overwrite, "destination_exists", "Destination exists; pass overwrite:true to atomically replace it.")
    require(not destination.is_symlink() and not destination.is_dir(), "invalid_path", "Destination must be a regular file path.")
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Entire unique directory prevents the exporter seeing an existing file and
    # raising an overwrite dialog. Same filesystem permits atomic publication.
    out = None
    try:
        with tempfile.TemporaryDirectory(prefix=".adom-export-", dir=str(destination.parent)) as folder:
            staged = Path(folder) / ("model" + destination.suffix)
            options = factory(design.rootComponent, str(staged)) if mode == "entity" else factory(str(staged))
            require(options is not None, "export_preflight_failed", "Exporter did not create options.")
            if hasattr(options, "isViewFit"):
                options.isViewFit = False
            if fmt in ("stl", "3mf"):
                options.sendToPrintUtility = False
                options.isOneFilePerBody = False
            if fmt == "stl":
                refinement = args.get("refinement", "medium")
                settings = {"low": "MeshRefinementLow", "medium": "MeshRefinementMedium", "high": "MeshRefinementHigh"}
                require(refinement in settings, "invalid_input", "refinement must be low, medium or high.")
                options.meshRefinement = getattr(adsk.fusion.MeshRefinementSettings, settings[refinement])
                # STL has no embedded unit declaration; request millimetres explicitly.
                require(hasattr(options, "unitType"), "unsupported_capability", "STL millimetre unit selection is unavailable.")
                options.unitType = adsk.fusion.DistanceUnits.MillimeterDistanceUnits
            require(bool(manager.execute(options)), "export_failed", "Fusion exporter returned false; destination was preserved.")
            out = publish_staged(staged, destination, overwrite)
            out.update({"format": fmt, "externalEffect": "file_published", "modelRollbackApplies": False})
            if fmt == "stl":
                out["lengthUnit"] = "mm"
    except Exception as exc:
        if out is None:
            raise
        out["cleanupWarning"] = str(exc)
    return out
