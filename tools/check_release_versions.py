#!/usr/bin/env python3
"""Resolve and cross-check the versions a release is built from.

Called by .github/workflows/release.yml, but runnable by hand:

    python3 tools/check_release_versions.py --tag v1.0.0

Why this is a script and not an inline `python -c`: the first version of the
workflow used a one-liner with a regex containing quote characters, which bash
mangled into "unexpected character after line continuation character". Shell
quoting around Python regexes is a trap; a file has no quoting problem.

Checks, all fatal:

  * the tag starts with `v`
  * tag minus `v` == `__version__` in fusion_bridge_mcp/__init__.py
  * BRIDGE_VERSION == bridge.json version

Prints the resolved values as `KEY=VALUE` on stdout when `--github-output` is
given (the workflow consumes them via $GITHUB_OUTPUT); otherwise prints a
human-readable report.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
INIT_PY = REPO / "fusion_bridge_mcp" / "__init__.py"
BRIDGE_VERSION_FILE = REPO / "BRIDGE_VERSION"
BRIDGE_JSON = REPO / "bridge.json"

# Matches:  __version__ = "1.0.0"   /  __version__ = '1.0.0'
_VERSION_RE = re.compile(r"""^__version__\s*=\s*["']([^"']+)["']""", re.MULTILINE)


def adapter_version() -> str:
    text = INIT_PY.read_text(encoding="utf-8")
    m = _VERSION_RE.search(text)
    if not m:
        raise SystemExit(f"error: no __version__ = \"...\" line found in {INIT_PY}")
    return m.group(1)


def runtime_version() -> str:
    if not BRIDGE_VERSION_FILE.exists():
        raise SystemExit(f"error: {BRIDGE_VERSION_FILE} not found")
    return BRIDGE_VERSION_FILE.read_text(encoding="utf-8").strip()


def json_version() -> str:
    data = json.loads(BRIDGE_JSON.read_text(encoding="utf-8"))
    v = data.get("version")
    if not v:
        raise SystemExit(f"error: {BRIDGE_JSON} has no 'version' field")
    return str(v).strip()


def check(tag: str) -> dict:
    problems: list[str] = []

    if not tag.startswith("v"):
        problems.append(f"tag {tag!r} must start with 'v' (e.g. v1.0.0)")
        tag_ver = tag
    else:
        tag_ver = tag[1:]

    ad = adapter_version()
    rt = runtime_version()
    jv = json_version()

    if tag_ver != ad:
        problems.append(
            f"tag ({tag_ver}) != adapter __version__ ({ad}) in "
            f"{INIT_PY.relative_to(REPO)} - bump it before tagging"
        )
    if rt != jv:
        problems.append(
            f"BRIDGE_VERSION ({rt}) != bridge.json version ({jv}) - bump them together"
        )
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.\-]+)?", tag_ver):
        problems.append(f"tag version {tag_ver!r} is not PEP 440-ish (X.Y.Z)")

    return {
        "tag": tag,
        "tag_ver": tag_ver,
        "adapter_ver": ad,
        "runtime_ver": rt,
        "json_ver": jv,
        "problems": problems,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Resolve and cross-check release versions.")
    ap.add_argument("--tag", required=True, help="the git tag, e.g. v1.0.0")
    ap.add_argument(
        "--github-output",
        action="store_true",
        help="emit KEY=VALUE lines for $GITHUB_OUTPUT instead of a report",
    )
    args = ap.parse_args(argv)

    r = check(args.tag)

    if args.github_output:
        for key in ("tag", "tag_ver", "adapter_ver", "runtime_ver"):
            print(f"{key}={r[key]}")

    print(f"tag             : {r['tag']}", file=sys.stderr if args.github_output else sys.stdout)
    print(
        f"adapter version : {r['adapter_ver']}",
        file=sys.stderr if args.github_output else sys.stdout,
    )
    print(
        f"runtime version : {r['runtime_ver']}",
        file=sys.stderr if args.github_output else sys.stdout,
    )
    print(
        f"bridge.json     : {r['json_ver']}",
        file=sys.stderr if args.github_output else sys.stdout,
    )

    if r["problems"]:
        for p in r["problems"]:
            print(f"::error::{p}")
            print(f"error: {p}", file=sys.stderr)
        return 1

    print("versions OK", file=sys.stderr if args.github_output else sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
