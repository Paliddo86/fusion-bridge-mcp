"""Outbound calls from this bridge BACK to Adom Bridge (ab) core.

Why this exists: as of **ab 1.9.84** a bridge's loopback callback
(`POST $ADOM_DIRECT_API_URL/command`) reaches the FULL ab verb set - not just
`desktop_*`. That means this bridge can, on its own:

  * `notify_user` the user directly (it used to be "Unknown desktop command", so
    the old code returned a `notifyUser` PAYLOAD and asked the driving AI to relay
    it - a workaround, now dropped);
  * enumerate peer ADs on the relay (`targets`) and route ANY verb to one via a
    cross-ab `target` - so a bridge running on a VM can reach the user on their
    LAPTOP ("notify them where they actually are").

Everything here is BEST-EFFORT and never raises: an older ab (pre-1.9.84), a
missing env var, or a closed port just yields None/False and the caller falls
back to its prior behavior. We CAPABILITY-PROBE (`GET /commands`) rather than
hardcode, per the SDK, so this keeps working as the verb set evolves.

Auth model (from disc #78): `X-Adom-Bridge-Token` = ATTRIBUTION only (ab binds
127.0.0.1 and trusts-by-install), so calls run ungated; a stale token 403s, so we
re-read it from the env on every call.
"""

import json
import os
import urllib.request
import urllib.error

_TIMEOUT = 8
# Cache the capability set for the process lifetime (the ab verb surface doesn't
# change under a running ab). None = not-yet-probed / unreachable.
_caps_cache = None
_self_name_cache = None


_discovered_base = None  # None = not tried, "" = tried+failed, str = discovered base URL


def _raw_get(url, timeout=4):
    """Low-level GET that does NOT go through _base_url (used by discovery so it can't recurse).
    Returns parsed JSON, or None."""
    try:
        req = urllib.request.Request(url, headers={"Accept": "application/json"}, method="GET")
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8", "replace"))
    except Exception:
        return None


def _looks_like_ad_api(data) -> bool:
    """Does a /commands response look like ab's verb catalog (so we don't bind a random service)?"""
    if isinstance(data, dict):
        if isinstance(data.get("commands"), list):
            return True
        # ab's /commands also carries a _hint mentioning POST /command
        if "command" in (data.get("_hint") or "").lower():
            return True
    return isinstance(data, list)


def _discover_direct_api():
    """Find ab's loopback direct-API base by port-probing, for the case where ab did NOT inject
    ADOM_DIRECT_API_URL into our spawn env (seen live on an ab 1.9.103 Hyper-V VM: the direct API
    WAS up on 127.0.0.1 but the env var was absent, so available() short-circuited to False and the
    bridge fell back to slow CLI relays - John caught it 2026-07-14). We enumerate the loopback TCP
    ports adom-desktop.exe is LISTENING on and probe each `/commands`; the one that returns ab's verb
    catalog is the direct API. Cached for the process lifetime. Windows-only + best-effort; never raises."""
    global _discovered_base
    if _discovered_base:                # cache SUCCESS only; a transient miss must not stick forever
        return _discovered_base
    try:
        import subprocess as _sp, re as _re
        cnw = getattr(_sp, "CREATE_NO_WINDOW", 0)
        # ab's image name changed with its rename; probe every name it has ever had.
        pids = set()
        for image in ("adom-bridge.exe", "Adom Bridge.exe", "adom-desktop.exe"):
            tl = _sp.run(["tasklist", "/FI", "IMAGENAME eq " + image, "/FO", "CSV", "/NH"],
                         capture_output=True, text=True, timeout=8, creationflags=cnw).stdout
            pids |= set(_re.findall(r'"' + _re.escape(image) + r'","(\d+)"', tl))
        if not pids:
            return None
        ns = _sp.run(["netstat", "-ano", "-p", "TCP"], capture_output=True, text=True,
                     timeout=8, creationflags=cnw).stdout
        seen = []
        for line in ns.splitlines():
            m = _re.search(r'\s(?:127\.0\.0\.1|\[::1\]):(\d+)\s+\S+\s+LISTENING\s+(\d+)', line)
            if m and m.group(2) in pids:
                p = int(m.group(1))
                if p not in seen:
                    seen.append(p)
        for p in seen:
            base = f"http://127.0.0.1:{p}"
            if _looks_like_ad_api(_raw_get(base + "/commands")):
                _discovered_base = base
                return base
    except Exception:
        pass
    return None


def _base_url():
    """The loopback direct-API base: the URL ab injects into our spawn env, else one we DISCOVER by
    probing adom-desktop's loopback ports (ab sometimes fails to inject the env var even though the
    API is up - see _discover_direct_api). None only if neither yields a reachable API."""
    env = (os.environ.get("ADOM_DIRECT_API_URL") or "").rstrip("/")
    if env:
        return env
    return _discover_direct_api()


def _token():
    # Re-read every call: after an ab restart the old token 403s.
    return os.environ.get("ADOM_BRIDGE_TOKEN") or ""


def _http(method, path, body=None, timeout=_TIMEOUT):
    """Best-effort JSON request to the direct API. Returns parsed dict/list or None."""
    base = _base_url()
    if not base:
        return None
    url = base + path
    data = None
    headers = {"Accept": "application/json"}
    tok = _token()
    if tok:
        headers["X-Adom-Bridge-Token"] = tok
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", "replace")
    except Exception:
        return None
    try:
        return json.loads(raw)
    except Exception:
        return None


def available() -> bool:
    """True if the direct API is reachable at all (env var present + port open)."""
    if not _base_url():
        return False
    return capabilities() is not None


def _names_from_list(items):
    """Collect verb names from a list of strings and/or {name|command|verb} dicts."""
    out = set()
    for it in items or []:
        if isinstance(it, str):
            out.add(it)
        elif isinstance(it, dict):
            n = it.get("name") or it.get("command") or it.get("verb")
            if n:
                out.add(n)
    return out


def _parse_command_names(data):
    """Extract the full verb-name set from a /commands response, tolerant of BOTH the legacy
    `{"commands":[...]}` shape AND the current ab (1.9.x) shape, where verbs are split across:
      topLevel: ["notify_user", "targets", ...]            (flat name list)
      cliRequired: ["pull_file", "shell_execute", ...]     (flat name list)
      bridges: [{name, verbs:[...]}, ...]                  (per-bridge verb lists)
      desktop: {commands:["screenshot_window", ...]}       (SHORT names; real verb = "desktop_"+name)
    Without this, capabilities() returned None on every current ab, so available() was always False
    and the bridge silently fell back to slow CLI relays (root-caused live, John 2026-07-14)."""
    names = set()
    if isinstance(data, list):
        return _names_from_list(data)
    if not isinstance(data, dict):
        return names
    if isinstance(data.get("commands"), list):          # legacy shape
        names |= _names_from_list(data["commands"])
    for k in ("topLevel", "cliRequired"):               # current: flat name lists
        if isinstance(data.get(k), list):
            names |= _names_from_list(data[k])
    for b in (data.get("bridges") or []):               # current: per-bridge verb lists
        if isinstance(b, dict):
            names |= _names_from_list(b.get("verbs"))
    dk = data.get("desktop")                            # current: desktop short names -> desktop_*
    if isinstance(dk, dict):
        for c in (dk.get("commands") or []):
            if isinstance(c, str):
                names.add(c)
                names.add("desktop_" + c)
    return names


def capabilities():
    """Set of ab command names from `GET /commands` (cached). None if unreachable/unparseable."""
    global _caps_cache
    if _caps_cache is not None:
        return _caps_cache
    data = _http("GET", "/commands")
    if data is None:
        return None
    names = _parse_command_names(data)
    if not names:
        return None
    _caps_cache = names
    return names


def has(command: str):
    """True/False if `command` is in the probed capability set; None if unknown
    (probe failed). Callers should try when the answer isn't an explicit False."""
    caps = capabilities()
    if caps is None:
        return None
    return command in caps


def call(command: str, args=None, target=None, timeout=_TIMEOUT):
    """Dispatch ANY ab verb back through the direct API. Returns the parsed JSON
    response dict, or None on any failure (never raises).

    target: a peer ab clientName (from `peers()`), or "all" to fan out. Omit to
    run on THIS ab (the one hosting the bridge)."""
    payload = {"command": command}
    if args:
        payload["args"] = args
    if target:
        payload["target"] = target
    return _http("POST", "/command", body=payload, timeout=timeout)


def self_name():
    """This ab's own clientName (to exclude it from peer routing). Best-effort."""
    global _self_name_cache
    if _self_name_cache is not None:
        return _self_name_cache or None
    st = _http("GET", "/status") or {}
    name = st.get("clientName") or st.get("name") or st.get("hostname")
    _self_name_cache = name or ""
    return name or None


def peers():
    """clientNames of the OTHER ADs on the relay (excludes self). [] if none/unknown."""
    res = call("targets") or {}
    # tolerate shape drift: the peer list is under `clients` on current ab, `targets` on older.
    tlist = None
    if isinstance(res, dict):
        tlist = res.get("targets") or res.get("clients")
    if not isinstance(tlist, list):
        return []
    me = self_name()
    out = []
    for t in tlist:
        n = t if isinstance(t, str) else ((t or {}).get("name") or (t or {}).get("hostname") or (t or {}).get("clientName"))
        if n and n != me:
            out.append(n)
    return out


def notify(title, body, level="info", buttons=None, target=None, reach_user=False):
    """Fire an ab `notify_user` toast. Returns the response dict, or None if the
    direct API is unreachable or ab is too old to have notify_user (caller falls
    back to returning a notifyUser payload for the driving AI to relay).

    reach_user: when this bridge may be running UNATTENDED (e.g. on a VM) while the
    user is on another machine, set True to also reach peer ADs. Since ab can't yet
    resolve which peer is "attended", we fan out to "all" when peers exist so the
    toast lands wherever the user actually is. `target` (explicit) overrides this.
    """
    if has("notify_user") is False:
        return None
    args = {"title": title, "body": body, "level": level}
    if buttons:
        args["buttons"] = buttons
    tgt = target
    if tgt is None and reach_user:
        # Fan out to EVERY connected desktop so the toast lands wherever the user actually is. Do NOT
        # gate on peers() (John 2026-07-14): the `targets` response shape drifted to {clients:[...]}
        # so peers() wrongly returns [] and the toast would hit only THIS (unattended VM). The relay
        # honors target="all" directly - verified reaching the user's laptop.
        tgt = "all"
    res = call("notify_user", args, target=tgt)
    return res
