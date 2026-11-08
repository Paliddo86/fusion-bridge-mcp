import os
#!/usr/bin/env python3
"""Install the AdomBridge add-in into EVERY Fusion add-in location.

Fusion has MOVED its per-user add-in directory across versions, and an add-in
in the wrong one is silently ignored (Fusion loads fine, the add-in never runs,
port 8774 stays down - issue #63, root-caused live on a fresh 2026 install):

  NEW (2025+):  %APPDATA%/Autodesk/FusionAddins/AdomBridge/          <- current Fusion scans THIS
  legacy:       %APPDATA%/Autodesk/Autodesk Fusion/API/AddIns/       <- older builds
  legacy:       %APPDATA%/Autodesk/Autodesk Fusion 360/API/AddIns/   <- oldest naming

macOS has no %APPDATA%: the same three conventions live under
  ~/Library/Application Support/Autodesk/  (FusionAddins, "Autodesk Fusion"/API/AddIns,
  "Autodesk Fusion 360"/API/AddIns) - see TARGET_CANDIDATES.

Verified live (ADOMBASELINE, Fusion 2026.x): the add-in sat in the legacy
API/AddIns dir through a full launch and NEVER loaded; moved to FusionAddins it
came up in 20s with zero manual enabling (runOnStartup honored). So we install
to ALL locations - each Fusion version loads from the one it scans and ignores
the others (a duplicate instance would fail its 8774 bind and no-op). NEVER
assume a single path; scan all of TARGET_CANDIDATES.

Usage:
    python install_addin.py
    python install_addin.py --uninstall
"""

import shutil
import sys
from pathlib import Path

# Source: the add-in bundled with this plugin
SOURCE_DIR = Path(__file__).resolve().parent / "addin" / "AdomBridge"

# EVERY known Fusion add-in directory convention, newest first. Keep this list
# complete - see the module docstring for why (a single-path assumption caused
# the #63 silent-failure). FusionAddins is created even on a fresh profile.
if sys.platform == "darwin":
    # macOS: no %APPDATA%; the per-user root is ~/Library/Application Support/Autodesk.
    _mac_autodesk = Path.home() / "Library" / "Application Support" / "Autodesk"
    TARGET_CANDIDATES = [
        _mac_autodesk / "FusionAddins",
        _mac_autodesk / "Autodesk Fusion" / "API" / "AddIns",
        _mac_autodesk / "Autodesk Fusion 360" / "API" / "AddIns",
    ]
else:
    TARGET_CANDIDATES = [
        Path.home() / "AppData" / "Roaming" / "Autodesk" / "FusionAddins",
        Path.home() / "AppData" / "Roaming" / "Autodesk" / "Autodesk Fusion" / "API" / "AddIns",
        Path.home() / "AppData" / "Roaming" / "Autodesk" / "Autodesk Fusion 360" / "API" / "AddIns",
    ]


def _sync_directory(source: Path, target: Path):
    """Sync files from source to target, updating only changed files.

    - Copies new/modified files from source to target
    - Removes files in target that don't exist in source
    - Preserves the target directory itself (and any Fusion metadata)
    """
    import filecmp

    # Build set of relative paths in source
    source_files = set()
    for src_file in source.rglob("*"):
        rel = src_file.relative_to(source)
        source_files.add(rel)

        dst_file = target / rel
        if src_file.is_dir():
            dst_file.mkdir(parents=True, exist_ok=True)
        elif src_file.is_file():
            dst_file.parent.mkdir(parents=True, exist_ok=True)
            if not dst_file.exists() or not filecmp.cmp(str(src_file), str(dst_file), shallow=False):
                shutil.copy2(src_file, dst_file)
                print(f"  Updated: {rel}")

    # Remove files in target that aren't in source (but keep __pycache__ etc.)
    if target.exists():
        for dst_file in sorted(target.rglob("*"), reverse=True):
            rel = dst_file.relative_to(target)
            # Skip __pycache__ and .pyc files
            if "__pycache__" in str(rel):
                continue
            if rel not in source_files:
                if dst_file.is_file():
                    dst_file.unlink()
                    print(f"  Removed: {rel}")
                elif dst_file.is_dir() and not any(dst_file.iterdir()):
                    dst_file.rmdir()
                    print(f"  Removed dir: {rel}")


def find_addins_dir() -> Path | None:
    """Find an existing Fusion AddIns directory, or return the first candidate."""
    for candidate in TARGET_CANDIDATES:
        if candidate.exists():
            return candidate
    # Return the first one even if it doesn't exist — we'll create it
    return TARGET_CANDIDATES[0]


def _check_relative_imports(src_root):
    """Refuse a payload whose relative imports do not all resolve INSIDE the payload.

    Issue #69 (Colby, 2026-08-28): the sync delivered a new commands/__init__.py that imports
    .activity_palette, without activity_palette.py, and Fusion's add-in bricked with a
    ModuleNotFoundError dialog storm. The half that landed referenced the half that did not.
    This is the transactional guarantee made cheap: check the payload before touching the
    installed tree, and if it is incomplete leave the installed add-in exactly as it was -
    which also means the mirror step can never delete a hand-restored file to re-brick a box.
    Returns a list of "file: imports X" problems; empty means clean.
    """
    import os, re
    problems = []
    py = []
    for d, _, fs in os.walk(src_root):
        for f in fs:
            if f.endswith(".py"):
                py.append(os.path.join(d, f))
    have = set(os.path.normpath(p) for p in py)
    have_dirs = set(os.path.normpath(os.path.dirname(p)) for p in py)
    pat = re.compile(r"^\s*from\s+(\.+)([\w.]*)\s+import", re.M)
    for f in py:
        try:
            txt = open(f, encoding="utf-8", errors="ignore").read()
        except Exception:
            continue
        for m in pat.finditer(txt):
            dots, mod = m.group(1), m.group(2)
            base = os.path.dirname(f)
            for _ in range(len(dots) - 1):
                base = os.path.dirname(base)
            if not mod:
                continue
            cand = os.path.normpath(os.path.join(base, *mod.split(".")))
            if not (cand + ".py" in have or os.path.join(cand, "__init__.py") in have or cand in have_dirs):
                problems.append("%s imports %s%s" % (os.path.relpath(f, src_root), dots, mod))
    return problems


def install():
    _src = os.path.join(os.path.dirname(os.path.abspath(__file__)), "addin", "AdomBridge")
    _bad = _check_relative_imports(_src)
    if _bad:
        print("[install_addin] REFUSING: payload has unresolved relative imports - installing it "
              "would brick the add-in (issue #69): " + "; ".join(_bad))
        return None
    """Install the AdomBridge add-in into EVERY known Fusion add-in location.

    ALL of TARGET_CANDIDATES get the add-in (see module docstring - Fusion moved
    the scan dir across versions and ignores the others, so single-path installs
    silently fail on the "wrong" version). Smart update per target: copies
    individual changed files rather than deleting the folder, preserving Fusion's
    "Run on Startup" metadata.

    NOTE for the AI running this: after install, YOU restart Fusion yourself
    (fusion_stop then fusion_start) so it rescans - NEVER ask the user to
    restart Fusion; doing things for the user is the entire point of the bridge.
    """
    if not SOURCE_DIR.exists():
        print(f"ERROR: Source add-in not found at {SOURCE_DIR}")
        sys.exit(1)

    print(f"Source:  {SOURCE_DIR}")
    installed_to = []
    for addins_dir in TARGET_CANDIDATES:
        target = addins_dir / "AdomBridge"
        try:
            addins_dir.mkdir(parents=True, exist_ok=True)
            if target.exists():
                _sync_directory(SOURCE_DIR, target)
                print(f"Updated: {target}")
            else:
                shutil.copytree(SOURCE_DIR, target)
                print(f"Installed: {target}")
            # VERIFY the copy on disk, not the intent (issue #75, Adonis: a stale copy in the
            # legacy API/AddIns dir imported activity_palette without the file and bricked
            # every Fusion launch). A copy whose relative imports do not resolve is replaced
            # wholesale; a sync that silently left it broken is exactly what we cannot afford.
            bad_on_disk = _check_relative_imports(str(target))
            if bad_on_disk:
                print(f"  REPAIRING {target}: copy on disk is incomplete ({'; '.join(bad_on_disk)[:200]})")
                shutil.rmtree(target, ignore_errors=True)
                shutil.copytree(SOURCE_DIR, target)
                bad_on_disk = _check_relative_imports(str(target))
                if bad_on_disk:
                    print(f"  STILL BROKEN after repair: {target}: {bad_on_disk}")
                    continue
            installed_to.append(str(target))
        except OSError as e:
            print(f"  skip {target}: {e}")

    print()
    print(f"Add-in present in {len(installed_to)} location(s); Fusion loads it from the one "
          "its version scans (runOnStartup:true) and ignores the rest.")
    print("Restart Fusion via the BRIDGE (fusion_stop then fusion_start) so it rescans - "
          "never ask the user to do it. The add-in serves http://127.0.0.1:8774.")
    return installed_to


def uninstall():
    """Uninstall the AdomBridge add-in from EVERY known location."""
    removed = 0
    for candidate in TARGET_CANDIDATES:
        target = candidate / "AdomBridge"
        if target.exists():
            shutil.rmtree(target)
            print(f"Removed: {target}")
            removed += 1
    if not removed:
        print("AdomBridge add-in is not installed.")


def main():
    if "--uninstall" in sys.argv:
        uninstall()
    else:
        install()


if __name__ == "__main__":
    main()
