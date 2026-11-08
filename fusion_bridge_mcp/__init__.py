"""fusion-bridge-mcp — MCP server for Autodesk Fusion 360.

A macOS fork of the Adom Bridge Fusion 360 bridge. This package is the *adapter*
only: it speaks MCP to the client and HTTP to the bridge runtime.

Architecture:

    MCP client --stdio--> fusion-bridge-mcp --HTTP 127.0.0.1:8773--> bridge runtime
                      --> add-in (127.0.0.1:8774) --> Fusion

The bridge runtime (server.py, the AdomBridge add-in, the 36 skills) is not
importable from site-packages: Fusion loads the add-in from a directory it scans
on disk. `fusion-bridge-mcp install` fetches the runtime bundle and deploys it.
"""

__version__ = "1.0.1"

#: Upstream bridge version this fork is based on.
UPSTREAM_VERSION = "1.13.84"

__all__ = ["__version__", "UPSTREAM_VERSION"]
