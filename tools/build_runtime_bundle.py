#!/usr/bin/env python3
"""Build the fusion-bridge RUNTIME bundle for a GitHub Release.

The wheel (`fusion_bridge_mcp`) ships only the MCP adapter. The bridge runtime
that actually talks to Fusion cannot live in site-packages: Fusion loads the
add-in from a directory it scans on disk, so the add-in must be DEPLOYED, not
imported. This script packs the runtime into a single tarball that
`fusion-bridge-mcp install` downloads and unpacks.

Usage:

    python3 tools/build_runtime_bundle.py                 # uses UPSTREAM_VERSION
    python3 tools/build_runtime_bundle.py --version 1.13.84
    python3 tools/build_runtime_bundle.py --out dist/

Output: dist/fusion-bridge-runtime-<version>.tar.gz

Reproducibility: entries are sorted, mtimes are normalised and ownership is
dropped, so the same tree always produces the same tarball layout. A SHA256 is
printed and written beside the tarball for the release notes.
"""

import argparse
import hashlib
import os
import sys
import tarfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Everything the bridge needs at runtime. Order is the extraction order, but the
# tarball is sorted anyway so it does not matter.
RUNTIME_FILES = [
    "server.py",
    "describe.py",
    "aps.py",
    "fusion_detect.py",
    "ab_client.py",
    "ad_client.py",
    "fusion_helper.py",
    "install_addin.py",
    "bridge.json",
    "BRIDGE_VERSION",
    "LICENSE",
]

RUNTIME_DIRS = [
    "handlers",
    "skills",
    "addin",
    "resources",
]

# Never ship these, even if they live inside a runtime dir.
EXCLUDE_DIRS = {"__pycache__", ".git", "target", "node_modules", ".pytest_cache"}
EXCLUDE_SUFFIXES = {".pyc", ".pyo", ".DS_Store", ".swp"}
EXCLUDE_NAMES = {".DS_Store"}


def read_version() -> str:
    f = REPO / "BRIDGE_VERSION"
    if not f.exists():
        sys.exit(f"error: {f} not found — cannot determine the runtime version")
    return f.read_text(encoding="utf-8").strip()


def collect(version: str) -> list[Path]:
    """Return the sorted list of repo-relative paths to pack."""
    picked: list[Path] = []
    missing: list[str] = []

    for name in RUNTIME_FILES:
        p = REPO / name
        if p.is_file():
            picked.append(p.relative_to(REPO))
        else:
            missing.append(name)

    for name in RUNTIME_DIRS:
        d = REPO / name
        if not d.is_dir():
            missing.append(name + "/")
            continue
        for root, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x not in EXCLUDE_DIRS]
            for f in files:
                if f in EXCLUDE_NAMES:
                    continue
                p = Path(root) / f
                if p.suffix in EXCLUDE_SUFFIXES:
                    continue
                picked.append(p.relative_to(REPO))

    if missing:
        print("warning: expected runtime paths not found:", ", ".join(missing), file=sys.stderr)

    # A bundle missing the two things that make it a bridge is useless: fail loud.
    for critical in ("server.py", "addin/AdomBridge/AdomBridge.py",
                     "addin/AdomBridge/AdomBridge.manifest",
                     "addin/AdomBridge/http_server.py"):
        if not (REPO / critical).exists():
            sys.exit(f"error: critical runtime file missing: {critical}")

    picked.sort(key=lambda p: p.as_posix())
    # de-duplicate while preserving order
    seen, out = set(), []
    for p in picked:
        if p not in seen:
            seen.add(p)
            out.append(p)
    return out


def build(version: str, outdir: Path) -> Path:
    files = collect(version)
    outdir.mkdir(parents=True, exist_ok=True)
    tar_path = outdir / f"fusion-bridge-runtime-{version}.tar.gz"

    # metadata/manifest stamped into the bundle so `install` can verify what it got
    manifest = {
        "runtimeVersion": version,
        "adapterUpstreamVersion": version,
        "fileCount": len(files),
        "entrypoint": "server.py",
        "addinPath": "addin/AdomBridge",
        "bridgeManifest": "bridge.json",
        "notes": (
            "Adom Bridge Fusion 360 bridge runtime (macOS fork). "
            "Deploy addin/AdomBridge into Fusion's add-in directory; run server.py for the bridge."
        ),
    }

    total = 0
    with tarfile.open(tar_path, "w:gz", compresslevel=9) as tar:
        for rel in files:
            src = REPO / rel
            info = tar.gettarinfo(str(src), arcname=rel.as_posix())
            # deterministic: no owner, no mtime drift
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            info.mode = 0o644
            with open(src, "rb") as fh:
                tar.addfile(info, fh)
            total += src.stat().st_size

        # stamp a small JSON manifest at the bundle root
        import io
        import json
        blob = json.dumps(manifest, indent=2).encode("utf-8")
        mi = tarfile.TarInfo("RUNTIME_MANIFEST.json")
        mi.size = len(blob)
        mi.mtime = 0
        mi.mode = 0o644
        tar.addfile(mi, io.BytesIO(blob))

    size = tar_path.stat().st_size
    print(f"packed {len(files)} files, {total / 1e6:.1f} MB raw -> {size / 1e6:.1f} MB compressed")
    print(f"wrote {tar_path}")

    sha = hashlib.sha256(tar_path.read_bytes()).hexdigest()
    (outdir / (tar_path.name + ".sha256")).write_text(f"{sha}  {tar_path.name}\n", encoding="utf-8")
    print(f"sha256 {sha}")
    return tar_path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", default=None, help="runtime version (default: BRIDGE_VERSION)")
    ap.add_argument("--out", default=str(REPO / "dist"), help="output directory (default: dist/)")
    ap.add_argument("--dry-run", action="store_true", help="list what would be packed")
    args = ap.parse_args()

    version = args.version or read_version()
    files = collect(version)

    if args.dry_run:
        print(f"would pack {len(files)} files as runtime {version}:")
        for p in files:
            print("  ", p.as_posix())
        return 0

    build(version, Path(args.out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
