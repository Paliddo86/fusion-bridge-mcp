#!/usr/bin/env python3
"""Write the GitHub Release notes for a tag.

Called by .github/workflows/release.yml:

    python3 tools/release_notes.py --tag v1.0.0 --adapter 1.0.0 --runtime 1.13.84 \
        --repo owner/name --out release-notes.md

Why a script: the first version used a bare `cat > notes.md <<EOF` in the
workflow. An unquoted heredoc expands backticks and `$` inside the body, so every
markdown code fence and `$AD` in the notes was interpreted by the shell. Writing
the file from Python removes the whole class of problem.
"""

from __future__ import annotations

import argparse
from pathlib import Path

TEMPLATE = """\
MCP server for Autodesk Fusion 360 — a custom macOS fork of the Adom Bridge Fusion 360 bridge.

**Adapter** `{adapter}` · **bridge runtime** `{runtime}` (the upstream bridge version this fork carries)

## Install

```bash
pip install https://github.com/{repo}/releases/download/{tag}/fusion_bridge_mcp-{adapter}-py3-none-any.whl
fusion-bridge-mcp install
fusion-bridge-mcp status
```

Or straight from git, without the wheel:

```bash
uvx --from git+https://github.com/{repo} fusion-bridge-mcp install
```

## Assets

| asset | purpose |
|---|---|
| `fusion_bridge_mcp-{adapter}-py3-none-any.whl` | the MCP adapter + CLI (44 tools, 42 resources, 36 prompts) |
| `fusion-bridge-runtime-{runtime}.tar.gz` | the bridge runtime: server.py, the AdomBridge add-in, the 36 skills |
| `*.sha256`, `CHECKSUMS.md` | integrity |

The runtime is a separate asset on purpose. Fusion loads the add-in from a
directory it scans on disk, so the add-in must be **deployed**, not imported from
`site-packages`. `fusion-bridge-mcp install` downloads this bundle and deploys it.

## What this fork adds over upstream

- an **MCP entry point** (`mcp_server.py` / `fusion_bridge_mcp.adapter`): upstream has none
- the **36 skills travel over MCP** as resources, so any client sees them
- **macOS support**: Fusion detection, add-in deploy, start/stop, log paths
- **wheel + runtime bundle** distribution, no source checkout needed

See [NOTICE.md](https://github.com/{repo}/blob/main/NOTICE.md) for attribution. This is a
derivative of the Adom project, not the original, and is not affiliated with or endorsed by
Adom Industries, Inc. or Autodesk.
"""


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Write the GitHub Release notes for a tag.")
    ap.add_argument("--tag", required=True)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--runtime", required=True)
    ap.add_argument("--repo", required=True, help="owner/name")
    ap.add_argument("--out", default="release-notes.md")
    args = ap.parse_args(argv)

    body = TEMPLATE.format(
        tag=args.tag,
        adapter=args.adapter,
        runtime=args.runtime,
        repo=args.repo,
    )
    Path(args.out).write_text(body, encoding="utf-8")
    print(f"wrote {args.out} ({len(body)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
