#!/usr/bin/env python3
"""fusion_bridge_mcp.cli - install and run the fusion-bridge MCP adapter.

    fusion-bridge-mcp serve      # MCP server over stdio (what an MCP client launches)
    fusion-bridge-mcp status     # is the bridge / Fusion / the add-in actually there?
    fusion-bridge-mcp install    # fetch the runtime bundle and deploy the add-in
    fusion-bridge-mcp update     # same as install, for a machine that already has one
    fusion-bridge-mcp uninstall  # remove the add-in and the runtime

The split this CLI exists to implement
--------------------------------------
This wheel is the *adapter* only: `mcp_server.py` (MCP <-> HTTP) plus this package.
The bridge RUNTIME - server.py, describe.py, aps.py, handlers/, skills/, and the
AdomBridge add-in, ~3.6 MB - is NOT packaged here. It ships as a versioned tarball on
the GitHub Release and `install` fetches it:

    https://github.com/Paliddo86/fusion-bridge-mcp/releases/download/
        v<runtime>/fusion-bridge-runtime-<runtime>.tar.gz

Why not just put the runtime in the wheel? Because Fusion does not import the add-in
from site-packages: it scans its own add-in directories on disk
(`FusionAddins`, `Autodesk Fusion/API/AddIns`, ...) and loads whatever it finds there
at launch - so the add-in has to be DEPLOYED, and the rest of the runtime has to sit
somewhere the add-in's server can be started from. A fixed, per-user runtime
directory (`runtime_dir()`) gives us that, plus a version we can report and upgrade.

Only the stdlib is imported at module level. The `mcp` SDK is reached through
`mcp_server.build_server()`, imported lazily inside `serve`, so `install`/`status`/
`uninstall` keep working on a machine without it.
"""

import argparse
import asyncio
import importlib.util
import json
import os
import shutil
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from . import UPSTREAM_VERSION, __version__

RELEASE_BASE = "https://github.com/Paliddo86/fusion-bridge-mcp/releases"
BUNDLE_URL = RELEASE_BASE + "/download/v{version}/fusion-bridge-runtime-{version}.tar.gz"

DEFAULT_BRIDGE_PORT = 8773
ADDIN_PORT = 8774

RUNTIME_MANIFEST = "RUNTIME_MANIFEST.json"
ADDIN_RELATIVE = Path("addin") / "AdomBridge"
ADDIN_ENTRYPOINT = "AdomBridge.py"
MCP_SERVER_NAME = "fusion-bridge"

DOWNLOAD_CHUNK = 1 << 16
DOWNLOAD_TIMEOUT = 60.0
HTTP_TIMEOUT = 5.0
PROGRESS_STEP = 10  # report the download every 10%


class InstallError(Exception):
    """A step that cannot be completed, reported to the user as `error: <message>`."""


# --------------------------------------------------------------------------------------
# Where the runtime lives, and where Fusion looks for add-ins
# --------------------------------------------------------------------------------------


def runtime_dir() -> Path:
    """The per-user directory holding the bridge runtime.

    macOS   ~/Library/Application Support/fusion-bridge-mcp/
    Windows %LOCALAPPDATA%\\fusion-bridge-mcp\\
    Linux   $XDG_DATA_HOME/fusion-bridge-mcp/  (default ~/.local/share/...)

    The same helper is used to read and to write, so `status`, `install`, `update` and
    `uninstall` can never disagree about the location.
    """
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "fusion-bridge-mcp"
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or (Path.home() / "AppData" / "Local")
        return Path(base) / "fusion-bridge-mcp"
    base = os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local" / "share")
    return Path(base) / "fusion-bridge-mcp"


def runtime_path() -> Path:
    """The extracted runtime tree (`<runtime_dir>/runtime`): server.py, skills/, addin/."""
    return runtime_dir() / "runtime"


def installed_runtime_version():
    """Version of the extracted runtime, or None when nothing is installed."""
    root = runtime_path()
    if not root.is_dir():
        return None
    manifest = root / RUNTIME_MANIFEST
    if manifest.is_file():
        try:
            data = json.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = None
        if isinstance(data, dict):
            version = str(data.get("runtimeVersion") or "").strip()
            if version:
                return version
    bridge_version = root / "BRIDGE_VERSION"
    if bridge_version.is_file():
        try:
            version = bridge_version.read_text(encoding="utf-8").strip()
        except OSError:
            version = ""
        if version:
            return version
    return "unknown"


def addin_target_dirs():
    """Every Fusion add-in directory Fusion has used, newest first.

    Fusion moved this directory across versions and silently ignores an add-in in the
    wrong one (issue #63: Fusion runs fine, port 8774 never comes up), so the add-in is
    written into all of them. Mirror of `install_addin.TARGET_CANDIDATES`, used when the
    runtime's own installer is unavailable.
    """
    if sys.platform == "darwin":
        autodesk = Path.home() / "Library" / "Application Support" / "Autodesk"
    elif os.name == "nt":
        roaming = os.environ.get("APPDATA") or (Path.home() / "AppData" / "Roaming")
        autodesk = Path(roaming) / "Autodesk"
    else:
        data = os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local" / "share")
        return [Path(data) / "Autodesk" / "FusionAddins"]
    return [
        autodesk / "FusionAddins",
        autodesk / "Autodesk Fusion" / "API" / "AddIns",
        autodesk / "Autodesk Fusion 360" / "API" / "AddIns",
    ]


def deployed_addin_dirs():
    """AdomBridge directories that exist under any candidate Fusion add-in location."""
    return [target for target in (base / "AdomBridge" for base in addin_target_dirs())
            if target.is_dir()]


def _addin_healthy(addin_dir: Path) -> bool:
    """True when the deployed add-in has its entry point (not a half-copied tree)."""
    return (addin_dir / ADDIN_ENTRYPOINT).is_file()


def _source_checkout():
    """The fusion-bridge checkout this CLI is running from, when there is one."""
    root = Path(__file__).resolve().parent.parent
    if (root / "server.py").is_file() and (root / ADDIN_RELATIVE).is_dir():
        return root
    return None


# --------------------------------------------------------------------------------------
# Bundle download and extraction
# --------------------------------------------------------------------------------------


def _download_bundle(version, dest: Path) -> Path:
    """Fetch the runtime tarball for `version` from the GitHub Release, reporting progress."""
    url = BUNDLE_URL.format(version=version)
    dest.parent.mkdir(parents=True, exist_ok=True)
    print("  downloading  %s" % url, flush=True)
    request = urllib.request.Request(
        url, headers={"User-Agent": "fusion-bridge-mcp/%s" % __version__})
    total = 0
    done = 0
    try:
        with urllib.request.urlopen(request, timeout=DOWNLOAD_TIMEOUT) as response:
            total = int(response.headers.get("Content-Length") or 0)
            mark = PROGRESS_STEP
            with open(dest, "wb") as handle:
                while True:
                    chunk = response.read(DOWNLOAD_CHUNK)
                    if not chunk:
                        break
                    handle.write(chunk)
                    done += len(chunk)
                    if not total:
                        continue
                    percent = done * 100 // total
                    while percent >= mark <= 100:
                        print("    %3d%%  %5.1f / %5.1f MB" % (mark, done / 1e6, total / 1e6),
                              flush=True)
                        mark += PROGRESS_STEP
    except urllib.error.HTTPError as exc:
        dest.unlink(missing_ok=True)
        if exc.code == 404:
            raise InstallError(
                "No runtime bundle is published for version %s.\n"
                "  asset: %s (HTTP 404)\n"
                "  The Release tag exists without the bundle, or neither exists yet. Check "
                "which versions do have one: %s\n"
                "  then retry with --runtime-version <version>." % (version, url, RELEASE_BASE))
        raise InstallError("Download failed: HTTP %s from %s" % (exc.code, url))
    except (urllib.error.URLError, OSError) as exc:
        dest.unlink(missing_ok=True)
        raise InstallError(
            "Download failed from %s: %s\n"
            "  Check the network/proxy, or install from a local bundle with "
            "--offline <tarball>." % (url, getattr(exc, "reason", exc)))
    if total and done != total:
        raise InstallError("Download truncated: %d of %d bytes from %s" % (done, total, url))
    print("    got %5.1f MB" % (done / 1e6), flush=True)
    return dest


def _validated_members(tar, dest: Path):
    """Members of `tar`, refusing anything that could escape `dest`.

    The bundle comes off the network, so its member names are untrusted input: an
    absolute path or a `..` component would write outside the runtime directory, a
    symlink/hardlink would let a later member write through it, and a device/fifo has no
    business in a runtime tree. Anything refused raises before a single file is written.
    """
    root = dest.resolve()
    members = []
    for member in tar.getmembers():
        name = member.name
        if not name:
            raise InstallError("Bundle contains a member with an empty name.")
        if Path(name).is_absolute() or (len(name) > 1 and name[1] == ":"):
            raise InstallError("Refusing to extract absolute path %r (path traversal "
                              "protection)." % name)
        parts = [part for part in name.replace("\\", "/").split("/") if part not in ("", ".")]
        if not parts:
            raise InstallError("Refusing to extract %r: no path inside the bundle "
                              "(path traversal protection)." % name)
        if ".." in parts:
            raise InstallError("Refusing to extract %r: path traversal protection."
                              % name)
        if member.issym() or member.islnk():
            raise InstallError("Refusing to extract link %r (path traversal protection)."
                              % name)
        if not (member.isreg() or member.isdir()):
            raise InstallError("Refusing to extract special file %r." % name)
        resolved = (root / Path(*parts)).resolve()
        if root not in resolved.parents:
            raise InstallError("Refusing to extract %r: it resolves outside %s (path "
                              "traversal protection)." % (name, root))
        members.append(member)
    return members


def extract_bundle(tarball: Path, dest: Path) -> Path:
    """Extract `tarball` into `dest`, refusing any member that would escape it."""
    dest.mkdir(parents=True, exist_ok=True)
    try:
        with tarfile.open(tarball, "r:*") as tar:
            members = _validated_members(tar, dest)
            if hasattr(tarfile, "data_filter"):  # 3.12+: second line of defence
                tar.extractall(dest, members=members, filter="data")
            else:
                tar.extractall(dest, members=members)
    except tarfile.TarError as exc:
        raise InstallError("%s is not a readable tar.gz bundle (%s)" % (tarball, exc))
    return dest


def replace_runtime(tarball: Path) -> Path:
    """Extract `tarball` over `<runtime_dir>/runtime`, replacing the previous runtime."""
    root = runtime_dir()
    root.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".staging-", dir=root))
    dest = runtime_path()
    try:
        extract_bundle(tarball, staging)
        if dest.exists():
            shutil.rmtree(dest)
        staging.rename(dest)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    return dest


# --------------------------------------------------------------------------------------
# Add-in deployment
# --------------------------------------------------------------------------------------


def _runtime_module(path: Path, name: str):
    """Load a module out of the runtime tree by path, or None when it is not there.

    The runtime is not a package on sys.path (it is a deployed tree), so its own
    install_addin.py / fusion_detect.py are reached with importlib. A module that fails
    to load is reported as absent: the caller always has a fallback.
    """
    if not path.is_file():
        return None
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        return None
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception:
        return None
    return module


def deploy_addin(runtime: Path):
    """Copy `<runtime>/addin/AdomBridge` into every Fusion add-in directory.

    Prefers the runtime's own install_addin.py: it already syncs per file, keeps
    Fusion's "run on startup" metadata, and refuses a payload whose relative imports do
    not resolve (issue #69 - half an add-in bricks Fusion on launch).
    """
    source = runtime / ADDIN_RELATIVE
    if not (source / ADDIN_ENTRYPOINT).is_file():
        raise InstallError("The runtime bundle has no add-in at %s - the bundle is "
                           "incomplete." % source)
    installer = _runtime_module(runtime / "install_addin.py", "fusion_bridge_install_addin")
    if installer is not None and hasattr(installer, "install"):
        installed = installer.install()
        if installed is None:
            raise InstallError("The runtime's install_addin.py refused the payload (unresolved "
                               "relative imports in the add-in); nothing was deployed.")
        return [Path(path) for path in installed]
    deployed = []
    for base in addin_target_dirs():
        target = base / "AdomBridge"
        try:
            base.mkdir(parents=True, exist_ok=True)
            shutil.copytree(source, target, dirs_exist_ok=True)
        except OSError as exc:
            print("  skipped %s: %s" % (target, exc))
            continue
        print("  deployed %s" % target)
        deployed.append(target)
    return deployed


def remove_addin(runtime):
    """Delete the add-in from every Fusion add-in directory."""
    present = deployed_addin_dirs()
    installer = None
    if runtime is not None:
        installer = _runtime_module(runtime / "install_addin.py", "fusion_bridge_install_addin")
    if installer is not None and hasattr(installer, "uninstall"):
        installer.uninstall()
        return present
    for target in present:
        shutil.rmtree(target)
        print("  removed %s" % target)
    return present


def _local_fusion_facts():
    """Fusion installed / add-in deployed, read locally (no bridge available).

    Uses the runtime's fusion_detect.py when it is installed - it already knows every
    macOS and Windows install layout - and falls back to the add-in directories alone.
    """
    detect = _runtime_module(runtime_path() / "fusion_detect.py", "fusion_bridge_fusion_detect")
    if detect is not None and hasattr(detect, "detect_fusion"):
        try:
            return detect.detect_fusion()
        except Exception:
            pass
    return {"installed": None, "addin_installed": bool(deployed_addin_dirs())}


def _probe_addin(port=ADDIN_PORT, timeout=HTTP_TIMEOUT):
    """Ask the add-in's own HTTP server whether it is serving. Returns (ok, version-or-error)."""
    url = "http://127.0.0.1:%d/status" % port
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            raw = response.read().decode("utf-8", "replace")
    except (urllib.error.URLError, OSError) as exc:
        return False, str(getattr(exc, "reason", exc))
    try:
        payload = json.loads(raw)
        version = str(payload.get("version") or "unknown")
    except (ValueError, AttributeError):
        version = "unknown"
    return True, version


# --------------------------------------------------------------------------------------
# MCP client configuration
# --------------------------------------------------------------------------------------


def client_config_paths():
    """Config file of every MCP client this CLI knows how to configure."""
    home = Path.home()
    paths = {"pi": home / ".pi" / "agent" / "mcp.json"}
    if sys.platform == "darwin":
        paths["claude"] = (home / "Library" / "Application Support" / "Claude"
                           / "claude_desktop_config.json")
    elif os.name == "nt":
        roaming = os.environ.get("APPDATA") or (home / "AppData" / "Roaming")
        paths["claude"] = Path(roaming) / "Claude" / "claude_desktop_config.json"
    else:
        config = os.environ.get("XDG_CONFIG_HOME") or (home / ".config")
        paths["claude"] = Path(config) / "Claude" / "claude_desktop_config.json"
    paths["cursor"] = home / ".cursor" / "mcp.json"
    return paths


def mcp_config_block():
    """The `mcpServers` block every one of these clients understands."""
    return {"mcpServers": {MCP_SERVER_NAME: {"command": "fusion-bridge-mcp",
                                            "args": ["serve"]}}}


def write_client_config(client: str) -> Path:
    """Merge the block into a client's config file, keeping its other servers."""
    path = client_config_paths()[client]
    data = {}
    if path.is_file():
        try:
            data = json.loads(path.read_text(encoding="utf-8") or "{}")
        except ValueError as exc:
            raise InstallError("%s is not valid JSON (%s); fix it, or paste the block in by "
                               "hand." % (path, exc))
        if not isinstance(data, dict):
            raise InstallError("%s does not hold a JSON object; edit it by hand." % path)
    servers = data.setdefault("mcpServers", {})
    if not isinstance(servers, dict):
        raise InstallError("%s has a non-object \"mcpServers\"; edit it by hand." % path)
    servers[MCP_SERVER_NAME] = mcp_config_block()["mcpServers"][MCP_SERVER_NAME]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return path


def _print_client_config(written=None):
    """Print the paste-ready MCP block, and where each detected client wants it."""
    print("\nMCP client configuration - paste into a client's config file:")
    print(json.dumps(mcp_config_block(), indent=2))
    paths = client_config_paths()
    present = [(name, path) for name, path in paths.items() if path.is_file()]
    if written:
        print("\n  written: %s (%s)" % (paths[written], written))
    if not present:
        print("\n  No MCP client config found. Create one of these and paste the block above:")
        for name, path in paths.items():
            print("    %-7s %s" % (name, path))
        return
    print("\n  detected:")
    for name, path in present:
        state = "updated" if name == written else "paste the block above"
        print("    %-7s %-6s %s" % (name, path, state))


# --------------------------------------------------------------------------------------
# Output helpers
# --------------------------------------------------------------------------------------


def _ok(message):
    print("  \u2705 %s" % message)


def _fail(message):
    print("  \u274c %s" % message)


def _warn(message):
    print("  \u26a0\ufe0f  %s" % message)


def _info(message):
    print("  \u2022  %s" % message)


# --------------------------------------------------------------------------------------
# The adapter (mcp_server.py) - imported lazily
# --------------------------------------------------------------------------------------


def _load_adapter():
    """Import the MCP adapter, which carries the bridge transport and build_server().

    Lazy, because only `serve` and `status` need it, and because in a source checkout the
    adapter sits beside the package instead of inside it.
    """
    try:
        import mcp_server
        return mcp_server
    except ImportError:
        pass
    source_root = Path(__file__).resolve().parent.parent
    if str(source_root) not in sys.path:
        sys.path.insert(0, str(source_root))
    try:
        import mcp_server
    except ImportError as exc:
        raise InstallError(
            "The MCP adapter (mcp_server.py) could not be imported (%s). It ships beside the "
            "fusion_bridge_mcp package, not inside it: run this command from a fusion-bridge "
            "checkout, or reinstall fusion-bridge-mcp." % exc)
    return mcp_server


def _configure_bridge(adapter, bridge_port=None, bridge_url=None):
    """Point the adapter at the bridge: either `--bridge-url` or `--bridge-port`."""
    if bridge_url:
        parsed = urllib.parse.urlsplit(
            bridge_url if "://" in bridge_url else "http://" + bridge_url)
        if parsed.scheme not in ("http", ""):
            raise InstallError("--bridge-url only supports http:// (the bridge speaks plain "
                               "HTTP on localhost), got %r." % bridge_url)
        if parsed.hostname:
            adapter.BRIDGE_HOST = parsed.hostname
        adapter.set_bridge_port(parsed.port or DEFAULT_BRIDGE_PORT)
    elif bridge_port:
        adapter.set_bridge_port(bridge_port)


def _skills_dir(adapter):
    """The runtime's skills when installed (they are not in the wheel), else the adapter's."""
    runtime_skills = runtime_path() / "skills"
    return runtime_skills if runtime_skills.is_dir() else adapter.SKILLS_DIR


# --------------------------------------------------------------------------------------
# Commands
# --------------------------------------------------------------------------------------


def cmd_serve(args) -> int:
    """Run the MCP server on stdio: what an MCP client launches."""
    adapter = _load_adapter()
    _configure_bridge(adapter, args.bridge_port, args.bridge_url)
    skills_dir = _skills_dir(adapter)
    try:
        server = adapter.build_server(skills_dir=skills_dir)
    except ImportError as exc:
        raise InstallError("The MCP SDK is not installed (%s). Install it with: "
                           "pip install 'mcp>=2'" % exc)
    print("fusion-bridge-mcp %s serving on stdio -> http://%s:%d, skills from %s"
          % (__version__, adapter.BRIDGE_HOST, adapter._bridge_port, skills_dir),
          file=sys.stderr, flush=True)
    asyncio.run(server.run_stdio_async())
    return 0


def cmd_status(args) -> int:
    """Report adapter, runtime, bridge, Fusion and add-in state. 0 when all green."""
    adapter = _load_adapter()
    _configure_bridge(adapter, args.bridge_port, args.bridge_url)
    address = "%s:%d" % (adapter.BRIDGE_HOST, adapter._bridge_port)
    print("fusion-bridge-mcp %s (runtime upstream %s)" % (__version__, UPSTREAM_VERSION))
    ok = True

    version = installed_runtime_version()
    if version:
        _ok("runtime %s installed at %s" % (version, runtime_path()))
    else:
        checkout = _source_checkout()
        if checkout is not None:
            _ok("runtime: not CLI-installed; running from the source checkout %s "
                "(run `fusion-bridge-mcp install` for a managed copy)" % checkout)
        else:
            _warn("runtime not installed in %s - run `fusion-bridge-mcp install`" % runtime_dir())

    payload = adapter._bridge_get("/status", timeout=HTTP_TIMEOUT * 2)
    reachable = isinstance(payload, dict) and not payload.get("errorCode")
    if reachable:
        _ok("bridge reachable on %s (bridge %s, led %s)"
            % (address, payload.get("bridgeVersion") or "unknown",
               payload.get("led") or "unknown"))
    else:
        ok = False
        detail = payload.get("error") if isinstance(payload, dict) else "no answer"
        _fail("bridge NOT reachable on %s: %s" % (address, detail))
        _info("start it with: python3 %s --port %d"
              % (runtime_path() / "server.py", adapter._bridge_port))

    facts = payload.get("fusion") if reachable and isinstance(payload.get("fusion"), dict) \
        else _local_fusion_facts()
    installed = facts.get("installed")
    if installed is True:
        _ok("Fusion installed (%s)" % (facts.get("exe_path") or "found"))
    elif installed is False:
        ok = False
        _fail("Fusion is not installed")
    else:
        _warn("Fusion installation unknown (bridge down, no local detector)")

    healthy = [path for path in deployed_addin_dirs() if _addin_healthy(path)]
    if healthy:
        _ok("add-in deployed (%s)" % healthy[0])
    elif facts.get("addin_installed") is True:
        _ok("add-in deployed (%s)" % (facts.get("addins_dir") or "reported by the bridge"))
    else:
        ok = False
        _fail("add-in not deployed in any of: %s"
              % ", ".join(str(path) for path in addin_target_dirs()))

    serving, detail = _probe_addin()
    if serving:
        _ok("add-in answering on 127.0.0.1:%d (version %s)" % (ADDIN_PORT, detail))
    else:
        ok = False
        _fail("add-in not answering on 127.0.0.1:%d (%s)" % (ADDIN_PORT, detail))
        _info("Fusion loads add-ins only at launch: restart Fusion, then retry")
    return 0 if ok else 1


def cmd_install(args) -> int:
    """Download the runtime bundle, extract it and deploy the add-in."""
    return _run_install(runtime_version=args.runtime_version, force=args.force,
                        offline=args.offline, write_config=args.write_config)


def cmd_update(args) -> int:
    """Install over an existing runtime, reporting an upgrade when the version moves."""
    previous = installed_runtime_version()
    print("Current runtime: %s" % (previous or "none installed"))
    code = _run_install(runtime_version=args.runtime_version, force=False,
                        offline=args.offline, write_config=args.write_config)
    current = installed_runtime_version()
    if code == 0 and previous and current and previous != current:
        print("\nUpgraded runtime %s -> %s. Restart Fusion so it reloads the add-in."
              % (previous, current))
    elif code == 0 and current:
        print("\nRuntime %s is already current; nothing to upgrade (use `install --force` to "
              "re-download it)." % current)
    return code


def _run_install(runtime_version=None, force=False, offline=None, write_config=None) -> int:
    """The install pipeline shared by `install` and `update`."""
    target_version = runtime_version or UPSTREAM_VERSION
    print("fusion-bridge-mcp %s - installing bridge runtime %s" % (__version__, target_version))
    print("  runtime dir: %s" % runtime_dir())

    tarball = None
    downloaded = None
    try:
        if offline:
            tarball = Path(offline).expanduser()
            if not tarball.is_file():
                raise InstallError("--offline bundle not found: %s" % tarball)
            print("  bundle:      %s (offline)" % tarball)
        elif not force and installed_runtime_version() == target_version:
            print("  runtime %s already installed (pass --force to download it again)."
                  % target_version)
        else:
            tarball = downloaded = _download_bundle(
                target_version, runtime_dir() / (".download-%s.tar.gz" % target_version))

        if tarball is not None:
            runtime = replace_runtime(tarball)
            print("  extracted:   %s" % runtime)
        else:
            runtime = runtime_path()

        version = installed_runtime_version() or "unknown"
        if tarball is not None and version not in (target_version, "unknown"):
            _warn("bundle reports runtime %s, expected %s" % (version, target_version))

        deployed = deploy_addin(runtime)
    finally:
        if downloaded is not None:
            downloaded.unlink(missing_ok=True)

    ok = _print_install_summary(runtime, version, deployed)
    if write_config:
        path = write_client_config(write_config)
        print("\n  wrote %s" % path)
    _print_client_config(written=write_config)
    return 0 if ok else 1


def _print_install_summary(runtime: Path, version, deployed) -> bool:
    """Report runtime, add-in, Fusion and bridge state after an install."""
    print("\nSummary")
    ok = True
    if (runtime / "server.py").is_file():
        _ok("runtime %s at %s" % (version, runtime))
    else:
        ok = False
        _fail("runtime %s has no server.py at %s" % (version, runtime))

    healthy = [path for path in deployed_addin_dirs() if _addin_healthy(path)]
    if healthy:
        _ok("add-in deployed: %s" % ", ".join(str(path) for path in healthy))
    else:
        ok = False
        _fail("add-in NOT deployed into any Fusion add-in directory")

    facts = _local_fusion_facts()
    installed = facts.get("installed")
    if installed is True:
        _ok("Fusion installed (%s)" % (facts.get("exe_path") or "found"))
    elif installed is False:
        _fail("Fusion is not installed - the add-in cannot load until it is")
    else:
        _warn("Fusion installation could not be determined")

    serving, detail = _probe_addin()
    if serving:
        _ok("add-in answering on 127.0.0.1:%d (version %s)" % (ADDIN_PORT, detail))
    else:
        _info("add-in not answering on 127.0.0.1:%d yet - Fusion loads add-ins only at "
              "launch, so restart Fusion (or start it) to load this one" % ADDIN_PORT)
    print("  \u2022  start the bridge server with: python3 %s --port %d"
          % (runtime / "server.py", DEFAULT_BRIDGE_PORT))
    return ok


def cmd_uninstall(args) -> int:
    """Remove the add-in from Fusion's directories and delete the runtime tree."""
    present = deployed_addin_dirs()
    root = runtime_dir()
    if not present and not root.exists():
        print("Nothing to uninstall: no AdomBridge add-in and no runtime in %s." % root)
        return 0

    print("This removes:")
    for target in present:
        print("  add-in:  %s" % target)
    if root.exists():
        print("  runtime: %s" % root)
    if not args.yes:
        try:
            answer = input("Type 'yes' to continue: ").strip().lower()
        except EOFError:
            raise InstallError("No answer on stdin; nothing was removed (pass --yes to skip "
                               "the confirmation).")
        if answer not in ("y", "yes"):
            print("Aborted; nothing was removed.")
            return 1

    remove_addin(root if root.exists() else None)
    if root.exists():
        shutil.rmtree(root)
        print("  removed %s" % root)

    print("\nMCP client config files were NOT touched. Delete the \"%s\" entry from:"
          % MCP_SERVER_NAME)
    paths = client_config_paths()
    existing = [(name, path) for name, path in paths.items() if path.is_file()]
    if existing:
        for name, path in existing:
            print("    %-7s %s" % (name, path))
    else:
        print("    (none found)")
    return 0


# --------------------------------------------------------------------------------------
# Argument parsing
# --------------------------------------------------------------------------------------


def _add_bridge_options(parser):
    parser.add_argument(
        "--bridge-port", type=int, default=None,
        help="Bridge HTTP port on 127.0.0.1 (default %d, or $FUSION_BRIDGE_PORT)."
             % DEFAULT_BRIDGE_PORT)
    parser.add_argument(
        "--bridge-url", default=None,
        help="Full bridge URL instead of a port, e.g. http://127.0.0.1:8773 "
             "(http:// only - the bridge is plain HTTP).")


def _add_install_options(parser, force_allowed=True):
    parser.add_argument(
        "--runtime-version", default=None,
        help="Runtime version to fetch (default %s, the upstream this adapter targets)."
             % UPSTREAM_VERSION)
    parser.add_argument(
        "--offline", default=None, metavar="TARBALL",
        help="Use a local fusion-bridge-runtime-<version>.tar.gz instead of downloading "
             "(skips the network entirely).")
    parser.add_argument(
        "--write-config", choices=sorted(client_config_paths()), metavar="CLIENT",
        help="Also write the MCP entry into that client's config file (otherwise the "
             "block is only printed).")
    if force_allowed:
        parser.add_argument(
            "--force", action="store_true",
            help="Re-download and re-extract even when that runtime version is already "
                 "installed.")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="fusion-bridge-mcp",
        description="MCP server for Autodesk Fusion 360 - adapter installer and launcher.",
        epilog="Typical first run:  fusion-bridge-mcp install   (fetches the bridge runtime "
               "and deploys the AdomBridge add-in), then point your MCP client at "
               "`fusion-bridge-mcp serve` and start the bridge server. "
               "`fusion-bridge-mcp status` tells you what is missing.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    subparsers = parser.add_subparsers(dest="command", metavar="<command>", required=True)

    serve = subparsers.add_parser(
        "serve", help="run the MCP server on stdio (what an MCP client launches)",
        description="Run the MCP server over stdio and proxy tools to the bridge on "
                    "--bridge-port. Needs the `mcp` SDK (pip install 'mcp>=2').")
    _add_bridge_options(serve)
    serve.set_defaults(handler=cmd_serve)

    status = subparsers.add_parser(
        "status", help="report adapter, runtime, bridge, Fusion and add-in state",
        description="Print one line per check: adapter version, installed runtime, bridge "
                    "reachability, Fusion installed, add-in deployed, add-in answering on "
                    "8774. Exits 0 when every check passes, 1 otherwise.")
    _add_bridge_options(status)
    status.set_defaults(handler=cmd_status)

    install = subparsers.add_parser(
        "install", help="download the runtime bundle and deploy the add-in into Fusion",
        description="Fetch fusion-bridge-runtime-<version>.tar.gz from the GitHub Release, "
                    "extract it into the per-user runtime directory, deploy the AdomBridge "
                    "add-in into every Fusion add-in directory, and print the MCP client "
                    "configuration. Verifies the result at the end.")
    _add_install_options(install)
    install.set_defaults(handler=cmd_install)

    update = subparsers.add_parser(
        "update", help="install over an existing runtime, reporting version changes",
        description="Same pipeline as install, without --force: an already-current runtime "
                    "is not re-downloaded. Use `install --force` to force a re-download.")
    _add_install_options(update, force_allowed=False)
    update.set_defaults(handler=cmd_update)

    uninstall = subparsers.add_parser(
        "uninstall", help="remove the add-in from Fusion and delete the runtime",
        description="Delete the AdomBridge add-in from every Fusion add-in directory and "
                    "the runtime directory. Asks for confirmation unless --yes is passed. "
                    "MCP client config files are left alone and reported instead.")
    uninstall.add_argument("--yes", "-y", action="store_true",
                           help="Skip the confirmation prompt.")
    uninstall.set_defaults(handler=cmd_uninstall)
    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    try:
        return args.handler(args)
    except InstallError as exc:
        print("error: %s" % exc, file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
