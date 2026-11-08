#!/usr/bin/env python3
"""Package a built mechanical development checkpoint for a pinned test host."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    version = (root / "BRIDGE_VERSION").read_text().strip()
    if "dev" not in version:
        raise SystemExit("This helper packages development versions only")
    binary = root / "rust/target/x86_64-pc-windows-gnu/release/adom-fusion-bridge.exe"
    # Cargo's shared target cache can retain an artifact from another copied
    # checkout. The native shell embeds this exact catalog with include_str!;
    # file existence and a successful cached build do not establish identity.
    if (root / "contracts/describe.json").read_bytes() not in binary.read_bytes():
        raise SystemExit("Native executable embeds a stale/missing verb catalog; rebuild in an isolated target directory")
    sources = {"adom-fusion-bridge.exe": binary}
    for name in ("BRIDGE_VERSION", "bridge.json", "package.json", "describe.py", "server.py", "contracts/describe.json"):
        sources[name] = root / name
    for source in (root / "addin").rglob("*"):
        if source.is_file() and "__pycache__" not in source.parts and source.suffix != ".pyc":
            sources[source.relative_to(root).as_posix()] = source
    records = []
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.output, "x", zipfile.ZIP_DEFLATED) as archive:
        for name, source in sorted(sources.items()):
            data = source.read_bytes()
            records.append({"path": name, "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
            archive.writestr(name, data)
        manifest = {"version": version, "files": records}
        archive.writestr("mechanical-dev-manifest.json", json.dumps(manifest, indent=2))
    args.output.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"version": version, "path": str(args.output), "fileCount": len(records), "bytes": args.output.stat().st_size, "sha256": hashlib.sha256(args.output.read_bytes()).hexdigest()}))


if __name__ == "__main__":
    main()
