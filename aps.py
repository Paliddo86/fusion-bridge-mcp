"""Autodesk Platform Services (APS) cloud-file search for the Fusion bridge.

Gives the fusion bridge its OWN fast, server-indexed cloud search (the
Data Management API) instead of the slow folder-by-folder walk that
fusion_walk_cloud_tree / fusion_search_cloud_files do. Works even when
Fusion is closed — this is pure HTTPS against APS.

Auth: the bridge runs its OWN 3-legged PKCE OAuth (public client, no client
secret). Only the APS Client ID is stored. We can't reuse Fusion's in-session
token — Autodesk DPAPI-encrypts that store with app entropy on purpose — so the
bridge owns its own login. One-time: register a Desktop/PKCE app at
aps.autodesk.com (Data Management API enabled, callback below), put the Client
ID in config, then fusion_aps_signin once.

Verbs (CLI fusion_aps_*  ->  bridge command aps_*):
  fusion_aps_status                 - configured? signed in? token live?
  fusion_aps_set_client_id {clientId}
  fusion_aps_signin                 - opens browser; bg listener stores the token
  fusion_aps_search {query, hubId?, projectId?, limit?}
  fusion_aps_get {path}             - raw authenticated GET (dev/verify helper)

No third-party deps — stdlib only (the bridge must run on stock Python).
"""

import base64
import hashlib
import json
import os
import secrets
import subprocess
import threading
import time
import urllib.parse
import urllib.request
import urllib.error
import webbrowser
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

_usage_lock = threading.Lock()

# ── Constants ────────────────────────────────────────────────────────────────
APS_BASE = "https://developer.api.autodesk.com"
AUTH_URL = f"{APS_BASE}/authentication/v2/authorize"
TOKEN_URL = f"{APS_BASE}/authentication/v2/token"

# Fixed callback — MUST match the registered APS app exactly. 8910 is ab-native
# APS's port; the bridge uses its own so the two never collide.
# Fixed callback ports — tried in order at sign-in; the FIRST free one is bound.
# All of these (× localhost AND 127.0.0.1) are registered on the APS app, so a
# port collision on any user's machine just falls through to the next — no failed
# sign-ins. 8917 avoids ab's own APS on 8910; all verified free + outside Windows'
# reserved/excluded ranges (5357, 27339, 50000-50059) on a real box.
REDIRECT_PORTS = [8917, 8918, 8920]
REDIRECT_PATH = "/fusion-aps/callback"
_active_redirect_uri = None  # set to the bound port's URI when a sign-in starts


def _redirect_uri(port: int) -> str:
    return f"http://localhost:{port}{REDIRECT_PATH}"


def all_registered_callbacks() -> list:
    """Every callback URL that must be registered on the APS app (localhost + 127.0.0.1)."""
    out = []
    for p in REDIRECT_PORTS:
        out.append(f"http://localhost:{p}{REDIRECT_PATH}")
        out.append(f"http://127.0.0.1:{p}{REDIRECT_PATH}")
    return out
# data:read is enough for search/browse/recent/file_info/versions/download.
# data:write + data:create are needed for create_folder/upload. Including them in
# the scope means a user who signed in under the old read-only scope must run
# fusion_aps_signin once more before the write verbs work (read verbs keep working).
SCOPES = "data:read data:write data:create"

# ⛔ CLOUD SEARCH IS APS-ONLY, AND SETTING APS UP IS MANDATORY - NOT OPTIONAL. This is returned by
# every cloud verb when APS isn't ready, as a HARD directive (John 2026-07-14, emphatic: an AI saw APS
# "Not configured", shrugged, and fell back to enumerating app.data.dataProjects/dataFiles via
# fusion_run_modeling_script - which HOLDS FUSION'S MAIN THREAD for minutes, TIMES OUT, and locks the
# app. That is the deprecated method and it is BANNED for cloud search). APS must work every time.
APS_REQUIRED_HINT = (
    "APS IS MANDATORY for cloud file search/browse/recent - SET IT UP, do not give up, do not fall back. "
    "⛔ NEVER enumerate cloud files via app.data (dataProjects/dataFiles) in fusion_run_modeling_script: "
    "the DEPRECATED method holds Fusion's main thread for minutes, times out, and locks the app. "
    "AUTO-RECOVER APS instead: (1) NOT configured? The org Client ID is a PUBLIC PKCE id - pull it from a "
    "peer that has it: read ~/.adom/fusion-aps/config.json (key 'client_id') off another ab desktop (`adom-bridge targets` lists them)"
    " via shell_execute/pull_file, then fusion_aps_set_client_id {clientId}. "
    "(2) fusion_aps_signin -> open the returned authUrl in the user's REAL browser with a warm Autodesk "
    "session; INSTALL + USE the adom-browser-extension (nbrowser/ABE, https://wiki.adom.inc/adom/adom-"
    "browser-extension) to drive it reliably (recommend it - coordinate-clicking a browser is fragile). "
    "The localhost listener captures the token; poll fusion_aps_status until signedIn:true. (3) retry. "
    "If authorize returns AUTH-001 'client_id does not have access to the api product': the APS app at "
    "aps.autodesk.com/myapps lost its Data Management API product - re-commit its API Access + Save, wait "
    "a few minutes to propagate, retry."
)


def _aps_auth001_recovery_hint(client_id: str) -> str:
    """The programmatic self-heal for APS AUTH-001, returned by start_signin the moment it detects the
    app lost its Data Management API PRODUCT (John 2026-07-14: an AI figured this out live over an hour;
    no future AI should have to). The bridge can't drive aps.autodesk.com itself, so this is the exact
    ABE/nbrowser recipe the calling AI runs - deterministic, verified live."""
    return (
        "⚠ APS AUTH-001 - the APS app (Client ID " + (client_id[:8] + "...") + ") LOST its Data Management "
        "API product, so authorize fails BEFORE any login. This is fixable; here is the exact recovery "
        "(this one genuinely needs ABE/nbrowser: recovery CLICKS and EVALs inside the portal, which the "
        "extension-free nbrowser_open_os_window cannot do - it only opens a URL. If the extension is not "
        "installed, install adom-browser-extension first; ABE can do it automatically, roughly a 5-minute "
        "AI-driven process. Alternatively walk the USER through steps 1-3 by hand): "
        "(1) nbrowser_open_window {sessionId, thread, purpose, profile:<the warm Edge/Chrome profile>, "
        "url:'https://aps.autodesk.com/myapps/'} and open the app whose Client ID is this one. "
        "(2) On the app's App-settings page, API Access section: confirm 'Data Management API' is checked, "
        "then FORCE a re-commit - nbrowser_eval a click on the VISIBLE 'Save changes' button (filter "
        "buttons by /save changes/i AND offsetParent!==null; the bare text matches ~9 hidden duplicates so "
        "text-click is ambiguous). If Save is disabled, toggle any one unneeded API off first to dirty the "
        "form. (3) WAIT ~1-3 min for Autodesk to propagate. (4) Retry fusion_aps_signin; authorize now "
        "reaches the CONSENT screen ('<app> is requesting permission ... View/Manage/Write your data ... "
        "Allow') - nbrowser_navigate the session to the new authUrl, then nbrowser_click 'Allow'; the URL "
        "flips to localhost:<port>/fusion-aps/callback?code=... and the bridge captures the token. "
        "(5) Poll fusion_aps_status until signedIn:true. Verified live 2026-07-14. "
        "⚠ FIRST CHECK before driving anything: open myapps AS THIS USER - if it shows "
        "'It seems you don't have a hub yet', this Autodesk account is NOT in the APS "
        "developer hub that owns the app, the app is invisible to it, and NO portal "
        "driving from this account can fix AUTH-001 (dead-ended live 2026-08-21, "
        "adom-bridge#93). Route the fix to the app's owner (the developer-hub admin), "
        "and have them add this user to the developer hub so the next AUTH-001 is "
        "self-servable. Meanwhile the library mint works WITHOUT APS since 1.9.133 "
        "(the add-in saves the f3d straight to the cloud); APS stays needed only for "
        "cloud search and thumbnail verification."
    )


def _probe_authorize_auth001(auth_url: str) -> bool:
    """Server-side probe: does this app's /authorize return AUTH-001 (no api-product access)? The check
    happens BEFORE any user login, so the bridge can detect a broken app WITHOUT a browser and hand back
    the recovery recipe immediately (instead of the AI opening a browser only to hit the JSON error).
    A healthy app 302-redirects to login/consent (no AUTH-001 body); a broken one returns the AUTH-001
    JSON. Never raises; returns False on any ambiguity (don't false-block a real sign-in)."""
    class _NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None  # a redirect means the app is FINE - stop, don't chase the login page
    try:
        opener = urllib.request.build_opener(_NoRedirect)
        try:
            body = opener.open(urllib.request.Request(auth_url), timeout=8).read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            if e.code in (301, 302, 303, 307, 308):
                return False  # redirect to login = healthy app
            body = (e.read() or b"").decode("utf-8", "replace")
        except Exception:
            return False
        return ("AUTH-001" in body) or ("does not have access to the api product" in body.lower())
    except Exception:
        return False

APS_DIR = Path(os.path.expanduser("~")) / ".adom" / "fusion-aps"
CONFIG_FILE = APS_DIR / "config.json"
TOKENS_FILE = APS_DIR / "tokens.json"
USAGE_FILE = APS_DIR / "usage.json"
# Which browser+profile the user's Autodesk sign-in ACTUALLY succeeded in. Power users
# run many browsers and many profiles; the OS default is usually NOT the one holding the
# warm Autodesk/SSO session. So we REMEMBER the browser+profile that worked and reuse it,
# instead of blindly firing webbrowser.open (which hits the OS default and forces a fresh
# login in the wrong place). Set on a successful driven sign-in; reused on every later one.
SIGNIN_BROWSER_FILE = APS_DIR / "signin-browser.json"


def load_signin_browser() -> dict | None:
    """The remembered {name, exe, profileDir, newWindow} that last authed to Autodesk, or None."""
    try:
        return json.loads(SIGNIN_BROWSER_FILE.read_text(encoding="utf-8"))
    except Exception:
        return None


def save_signin_browser(pref: dict) -> bool:
    """Remember the browser+profile that just worked, so the next sign-in reuses it."""
    try:
        APS_DIR.mkdir(parents=True, exist_ok=True)
        keep = {k: pref.get(k) for k in ("name", "exe", "profileDir", "newWindow", "how")
                if pref.get(k) is not None}
        keep["updatedAt"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        SIGNIN_BROWSER_FILE.write_text(json.dumps(keep, indent=2), encoding="utf-8")
        return True
    except Exception:
        return False


def forget_signin_browser() -> bool:
    try:
        SIGNIN_BROWSER_FILE.unlink()
        return True
    except FileNotFoundError:
        return True
    except Exception:
        return False


def _launch_url_in_browser(pref: dict, url: str) -> bool:
    """Launch a SPECIFIC browser+profile (not the OS default) at url. Chromium browsers
    take --profile-directory + --new-window; a bare exe still opens its default profile.
    Returns True if the process spawned."""
    exe = pref.get("exe")
    if not exe or not os.path.exists(exe):
        return False
    cmd = [exe]
    prof = pref.get("profileDir")
    if prof:
        cmd.append(f"--profile-directory={prof}")
    if pref.get("newWindow", True):
        cmd.append("--new-window")
    cmd.append(url)
    try:
        # DELIBERATELY LOCAL, not ab's desktop_open_url: that verb takes only
        # {url, browser} and cannot pin --profile-directory, which is the whole point
        # here (the sign-in must land in the profile that owns the Autodesk session).
        # Revisit if desktop_open_url ever grows a profile arg.
        subprocess.Popen(cmd, close_fds=True,
                         creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        return True
    except Exception:
        return False

# ── NEVER-CHARGE GUARANTEE (hard rule) ───────────────────────────────────────
# A user must NEVER be charged money — a card on file is only ever for Autodesk's
# identity verification. Two independent guarantees enforce this:
#   1. ALLOWLIST — the bridge calls ONLY Autodesk Data Management endpoints, which
#      are in APS's always-FREE API set. The paid APIs (Model Derivative, Design
#      Automation, Reality Capture, etc.) are NEVER called. Any path outside the
#      allowlist is refused before the request goes out.
#   2. METER + HARD CAP — even free APIs have monthly quotas; we count calls per
#      UTC month and FAIL CLOSED at a conservative cap, so a runaway loop can never
#      push past the free tier into billable overage.
# Data Management (/project/v1/, /data/v1/) AND OSS storage (/oss/v2/) are all in
# APS's always-FREE set — transferring your OWN Fusion Team hub files (download/
# upload) is included with the subscription, never billed. The paid APIs
# (Model Derivative, Automation, Reality Capture) are never in this list.
# /123D-Circuits/ is api.library.io (Autodesk's managed electronics libraries): a free
# service reached with the same Autodesk identity, never metered like APS usage APIs.
FREE_PATH_PREFIXES = ("/project/v1/", "/data/v1/", "/oss/v2/", "/123D-Circuits/")
MONTHLY_CALL_CAP = 50000  # conservative; well under the DM free quota; fail-closed

_signin_lock = threading.Lock()
_signin_state = None  # {"verifier","state","server","thread","done","error","startedAt"}


# ── Extension-free browser opening (issue #12) ───────────────────────────────
# ABE gained OS-level, EXTENSION-FREE verbs (nbrowser_open_os_window / nbrowser_os_windows):
# they open a URL in a REAL browser PROFILE without the Adom extension being installed in that
# Chrome/Edge. That is exactly the OAuth-consent case, so APS sign-in no longer dead-ends on a
# machine with no extension (Caleb, issue #12). The extension is still WORTH having - with it we
# can drive/see/screenshot the window instead of asking the user to click - so we upsell it.
_ABE_UPSELL = ("OPTIONAL, offer it: installing the Adom browser extension lets me DRIVE this "
               "sign-in instead of asking the user to click (fill, read the result, screenshot, "
               "clean up). ABE can install it for them automatically - it is just slow, roughly a "
               "5-minute AI-driven process. See the 'installing-the-extension' flow. Everything "
               "here works WITHOUT it; the extension only buys automation.")


def _ad(verb: str, args: dict, timeout: int = 60) -> dict:
    """Call an ab verb from aps.py. Lazy-imports server (which imports us) so there is no
    circular import at module load. Never raises."""
    try:
        import server as _s
        return _s._ab_call(verb, args, timeout=timeout) or {}
    except Exception as e:
        return {"_adCallError": str(e)}


def _abe_state() -> dict:
    """Probe ABE readiness so the response can say WHY a path was chosen, and whether the
    (optional) extension is present. Degrades to {'known': False} on any failure."""
    r = _ad("nbrowser_readiness", {}, timeout=30)
    if not isinstance(r, dict) or r.get("_adCallError") or "ready" not in r:
        return {"known": False,
                "note": "could not probe ABE (verb missing or CLI too old); the extension-free "
                        "path does not need it"}
    return {"known": True,
            "ready": r.get("ready"),
            "extensionConnected": r.get("extensionConnected"),
            "profiles": r.get("profiles") or [],
            "chromeInstalled": r.get("chromeInstalled"),
            "extensionStale": r.get("extensionStale"),
            "staleProfiles": r.get("staleProfiles") or []}


def _open_os_window_extension_free(url: str, profile: str = None) -> dict:
    """Open `url` in the user's REAL browser profile with NO extension required.

    Returns {launched: bool, where: str, profile: str, error: str}. `launched` is intentionally
    generous: nbrowser_open_os_window returns ok:false when it cannot resolve the new window's
    hwnd within ~8s, but the browser usually DID open (the tab often groups into an existing
    window). For a consent flow, "the page is in front of the user" is what matters, so a fired
    launch counts.
    """
    if not profile:
        # reuse the Autodesk profile the user already picked for Fusion's own sign-in
        try:
            import server as _s
            profile = _s._remembered_signin_profile()
        except Exception:
            profile = None
    a = {"sessionId": "aps-signin-%d" % int(time.time()),
         "thread": "fusion-aps-signin",
         "purpose": "Autodesk APS consent",
         "url": url, "background": True}
    if profile:
        a["profile"] = profile
    # PREFER the extension window: nbrowser_open_window makes the tab AGENT-OWNED, and CDP only
    # drives tabs it owns (nbrowser_* refuses `not_owned` with no override). Owning it is what
    # lets us CLICK 'Allow' for the user instead of leaving a consent page sitting there - the
    # same pattern that finally completed the Fusion sign-in. Fall back to the extension-free
    # OS window when there is no extension (issue #12: it must still work with none installed).
    r = _ad("nbrowser_open_window", a, timeout=60)
    owned = isinstance(r, dict) and not r.get("_adCallError") and (r.get("ok") or r.get("sessionId"))
    if not owned:
        r = _ad("nbrowser_open_os_window", a, timeout=60)
    if not isinstance(r, dict) or r.get("_adCallError"):
        return {"launched": False, "profile": profile,
                "error": (r or {}).get("_adCallError") or "nbrowser_open_os_window unavailable"}
    launched = bool(r.get("ok") or r.get("hwnd") or r.get("sessionId")
                    or "Launch fired" in str(r.get("_hint", "")))
    if launched:
        # hand it to the bridge's janitor so a consent window is never left as desktop litter
        try:
            import server as _s
            _s._track_signin_window(a.get("sessionId"), a.get("profile"), kind="aps")
        except Exception:
            pass
    return {"launched": launched, "agentOwned": bool(owned),
            "where": r.get("displayName") or r.get("profile") or profile,
            "profile": r.get("profile") or profile,
            "hwnd": r.get("hwnd"),
            "extensionInstalled": r.get("extensionInstalled"),
            "error": None if launched else (str(r.get("error") or r.get("_hint") or "")[:200])}


def _signin_setup_chain(opened: bool, abe: dict) -> list:
    """A chain the agent can follow MECHANICALLY (Caleb's ask), instead of a bare command that
    may not exist on the box."""
    if opened:
        return [{"step": "user_approves",
                 "do": "Tell the user the exact profile it opened in and ask them to click Approve."},
                {"step": "poll", "verb": "fusion_aps_status",
                 "until": "signedIn:true"}]
    chain = [{"step": "open_consent", "verb": "nbrowser_open_os_window",
              "args": {"sessionId": "<task id>", "thread": "<your thread>",
                       "purpose": "Autodesk APS consent", "profile": "chrome:<their-autodesk-email>",
                       "url": "<authUrl from this response>"},
              "note": "EXTENSION-FREE - works with no Adom extension installed. If this verb is "
                      "unknown, the adom-bridge CLI on this box is too old: update it (or have "
                      "the user paste authUrl into the right profile by hand)."},
             {"step": "user_approves",
              "do": "Ask the user to approve in that profile; without the extension we cannot click for them."},
             {"step": "poll", "verb": "fusion_aps_status", "until": "signedIn:true"}]
    if abe.get("known") and not abe.get("extensionConnected"):
        chain.append({"step": "optional_extension", "flow": "installing-the-extension",
                      "why": "lets the assistant DRIVE the sign-in instead of asking for clicks",
                      "cost": "~5 minutes, ABE automates it"})
    return chain


# ── Config + token storage ───────────────────────────────────────────────────
def _read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}


def _write_json(path: Path, data: dict):
    APS_DIR.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2), encoding="utf-8")


def get_client_id() -> str:
    return _read_json(CONFIG_FILE).get("client_id", "") or os.environ.get("FUSION_APS_CLIENT_ID", "")


def set_client_id(client_id: str):
    cfg = _read_json(CONFIG_FILE)
    cfg["client_id"] = client_id.strip()
    _write_json(CONFIG_FILE, cfg)


def _load_tokens() -> dict:
    return _read_json(TOKENS_FILE)


def _save_tokens(tok: dict):
    _write_json(TOKENS_FILE, tok)


# ── Usage meter (per UTC month; fail-closed cap) ─────────────────────────────
def _usage_month() -> str:
    return time.strftime("%Y-%m", time.gmtime())


def _read_usage() -> dict:
    u = _read_json(USAGE_FILE)
    if u.get("month") != _usage_month():
        u = {"month": _usage_month(), "calls": 0}  # auto-reset each month
    return u


def _bump_usage(n: int = 1) -> dict:
    with _usage_lock:  # parallel search calls share this
        u = _read_usage()
        u["calls"] = int(u.get("calls", 0)) + n
        _write_json(USAGE_FILE, u)
        return u


def _cap_remaining() -> int:
    return max(0, MONTHLY_CALL_CAP - int(_read_usage().get("calls", 0)))


# ── PKCE helpers ─────────────────────────────────────────────────────────────
def _pkce_pair():
    verifier = base64.urlsafe_b64encode(secrets.token_bytes(48)).rstrip(b"=").decode()
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()
    ).rstrip(b"=").decode()
    return verifier, challenge


# ── HTTP ─────────────────────────────────────────────────────────────────────
def _http(method: str, url: str, headers=None, data=None, timeout=30):
    """Return (status, json_or_text). Never raises on HTTP error — returns the body."""
    if isinstance(data, dict):
        data = urllib.parse.urlencode(data).encode()
    req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8", "replace")
            return resp.status, _maybe_json(body)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        return e.code, _maybe_json(body)
    except Exception as e:
        return 0, {"error": str(e)}


def _maybe_json(text: str):
    try:
        return json.loads(text)
    except Exception:
        return text


# ── Token lifecycle ──────────────────────────────────────────────────────────
def _exchange_code(code: str, verifier: str) -> dict:
    status, body = _http("POST", TOKEN_URL,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        data={
            "grant_type": "authorization_code",
            "code": code,
            "client_id": get_client_id(),
            "redirect_uri": _active_redirect_uri,  # must match the authorize request
            "code_verifier": verifier,
        })
    if status == 200 and isinstance(body, dict) and body.get("access_token"):
        body["obtained_at"] = int(time.time())
        body["expires_at"] = int(time.time()) + int(body.get("expires_in", 3600)) - 60
        _save_tokens(body)
        return {"ok": True}
    return {"ok": False, "status": status, "body": body}


def _refresh() -> bool:
    tok = _load_tokens()
    rt = tok.get("refresh_token")
    if not rt:
        return False
    # IMPORTANT: do NOT send scope on refresh. A refresh may only narrow scope, not
    # broaden it — so passing the (now wider) SCOPES would make Autodesk reject any
    # token originally granted read-only. Omitting scope preserves whatever was
    # granted, so existing read sessions keep refreshing; write features just need a
    # fresh fusion_aps_signin to consent to the wider scope.
    status, body = _http("POST", TOKEN_URL,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        data={
            "grant_type": "refresh_token",
            "refresh_token": rt,
            "client_id": get_client_id(),
        })
    if status == 200 and isinstance(body, dict) and body.get("access_token"):
        body["obtained_at"] = int(time.time())
        body["expires_at"] = int(time.time()) + int(body.get("expires_in", 3600)) - 60
        if "refresh_token" not in body:
            body["refresh_token"] = rt
        _save_tokens(body)
        return True
    return False


def _valid_access_token() -> str | None:
    tok = _load_tokens()
    if not tok.get("access_token"):
        return None
    if int(tok.get("expires_at", 0)) <= int(time.time()):
        if not _refresh():
            return None
        tok = _load_tokens()
    return tok.get("access_token")


def _aps_get(path: str, query: dict = None):
    """Authenticated GET against APS. `path` is absolute-from-host or a full URL.

    Enforces the never-charge guarantee: free-API allowlist + monthly hard cap.
    """
    at = _valid_access_token()
    if not at:
        return None, {"error": "not_signed_in"}

    # Guarantee 1: only ever hit free Data Management endpoints.
    only_path = path if not path.startswith("http") else urllib.parse.urlparse(path).path
    if not any(only_path.startswith(p) for p in FREE_PATH_PREFIXES):
        return 0, {"error": "blocked_non_free_endpoint", "path": only_path,
                   "_hint": "Refused — the bridge only calls free Data Management endpoints "
                            "(/project/v1/, /data/v1/) so you are never charged."}

    # Guarantee 2: fail closed at the monthly cap (never blow past the free tier).
    if _cap_remaining() <= 0:
        return 0, {"error": "usage_cap_reached", "cap": MONTHLY_CALL_CAP,
                   "_hint": f"Monthly APS call cap ({MONTHLY_CALL_CAP}) reached — refusing further "
                            "calls so you are never charged. Resets at the start of next UTC month."}

    url = path if path.startswith("http") else f"{APS_BASE}{path}"
    if query:
        url += ("&" if "?" in url else "?") + urllib.parse.urlencode(query)
    _bump_usage(1)
    return _http("GET", url, headers={"Authorization": f"Bearer {at}"})


# ── Sign-in (PKCE, browser + background callback listener) ────────────────────
class _CallbackHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != REDIRECT_PATH:
            self.send_response(404); self.end_headers(); return
        qs = urllib.parse.parse_qs(parsed.query)
        code = (qs.get("code") or [None])[0]
        state = (qs.get("state") or [None])[0]
        ok = False
        with _signin_lock:
            st = _signin_state
        if code and st and state == st["state"]:
            res = _exchange_code(code, st["verifier"])
            ok = res.get("ok", False)
            with _signin_lock:
                st["done"] = True
                st["error"] = None if ok else res
        html = (b"<html><body style='font-family:sans-serif;background:#111;color:#eee;"
                b"text-align:center;padding-top:80px'><h2>Fusion APS: "
                + (b"signed in &check;" if ok else b"sign-in failed")
                + b"</h2><p>You can close this tab and return to Adom.</p></body></html>")
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(html)

    def log_message(self, *a):
        pass


def start_signin(args: dict | None = None) -> dict:
    global _signin_state, _active_redirect_uri
    args = args or {}
    client_id = get_client_id()
    if not client_id:
        return {"success": False, "error": "No APS Client ID configured.",
                "errorCode": "aps_not_configured",
                "_hint": "Call fusion_aps_set_client_id {\"clientId\":\"...\"} first "
                         "(register a Desktop/PKCE app at https://aps.autodesk.com with "
                         "Data Management API + callbacks " + ", ".join(all_registered_callbacks()) + ")."}

    verifier, challenge = _pkce_pair()
    state = secrets.token_urlsafe(16)
    # Bind the FIRST free callback port from the registered set.
    server = None
    bound_port = None
    for port in REDIRECT_PORTS:
        try:
            server = HTTPServer(("127.0.0.1", port), _CallbackHandler)
            bound_port = port
            break
        except OSError:
            continue
    if server is None:
        return {"success": False,
                "error": f"All callback ports busy on this machine: {REDIRECT_PORTS}",
                "_hint": "Close whatever is holding these ports, or a sign-in is already in "
                         "progress. These are only bound for ~3 min during sign-in."}
    _active_redirect_uri = _redirect_uri(bound_port)

    with _signin_lock:
        _signin_state = {"verifier": verifier, "state": state, "server": server,
                         "done": False, "error": None, "startedAt": time.time()}

    def _serve():
        # Handle requests until the exchange completes or ~3 min elapses.
        server.timeout = 1
        deadline = time.time() + 180
        while time.time() < deadline:
            with _signin_lock:
                done = _signin_state and _signin_state["done"]
            if done:
                break
            server.handle_request()
        try:
            server.server_close()
        except Exception:
            pass

    threading.Thread(target=_serve, daemon=True).start()

    params = {
        "response_type": "code",
        "client_id": client_id,
        "redirect_uri": _active_redirect_uri,
        "scope": SCOPES,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
        "state": state,
        # No prompt=login — reuse the user's existing Autodesk browser session so
        # sign-in is one-click (or zero-click if already consented), not a forced
        # re-login every time.
    }
    auth_url = AUTH_URL + "?" + urllib.parse.urlencode(params)

    # ── PROGRAMMATIC AUTH-001 SELF-HEAL (John 2026-07-14) ────────────────────────────────────────
    # Detect a broken APS app (lost its Data Management API PRODUCT) SERVER-SIDE, before opening any
    # browser, and hand back the exact ABE-driven recovery recipe. Without this, the AI opens a browser,
    # hits a raw AUTH-001 JSON error page, and has to reverse-engineer the fix (it once took an hour).
    if _probe_authorize_auth001(auth_url):
        try:
            server.server_close()
        except Exception:
            pass
        with _signin_lock:
            _signin_state = None
        return {"success": False,
                "error": "APS AUTH-001: the app's Client ID does not have access to the Data Management API product.",
                "errorCode": "aps_app_missing_product",
                "authUrl": auth_url,
                "appManagementUrl": "https://aps.autodesk.com/myapps/",
                "clientId": client_id,
                "recoverable": True,
                "_hint": _aps_auth001_recovery_hint(client_id)}

    # ── SMART BROWSER CHOICE (John, 2026-07-08: "power users have tons of browsers with tons
    #    of profiles ... cache what browser you used when you successfully logged in ... don't
    #    be lazy") ─────────────────────────────────────────────────────────────────────────
    # Priority: (1) a browser+profile passed in THIS call, (2) the one we REMEMBERED from the
    # last successful sign-in, (3) if neither AND the caller can drive the extension, open
    # NOTHING and tell it to open in the user's REAL browser (new window) via nbrowser_* — that
    # window holds the warm Autodesk/SSO session. Only fall back to the OS default browser when
    # explicitly allowed, because on a multi-browser machine the default is usually the WRONG
    # one (no session / wrong profile) and forces a needless fresh login.
    passed = None
    if args.get("exe"):
        passed = {"name": args.get("browser") or args.get("name"), "exe": args.get("exe"),
                  "profileDir": args.get("profileDir"), "newWindow": args.get("newWindow", True)}
    pref = passed or load_signin_browser()
    opened_via = None
    opened_pref = None
    ext_free = None      # result of the EXTENSION-FREE open, if we take that path
    abe = _abe_state()   # probed once, reported either way (issue #12)
    if pref and _launch_url_in_browser(pref, auth_url):
        opened_via = "remembered_browser" if not passed else "specified_browser"
        opened_pref = {k: pref.get(k) for k in ("name", "exe", "profileDir")}
        # A browser passed explicitly is now the remembered one (it's what the caller chose to
        # drive). A previously-remembered one is refreshed to keep its timestamp warm.
        save_signin_browser(pref)
    elif args.get("allowDefaultBrowser") and not pref:
        # Legacy fallback — only when the caller opts in AND we have NOTHING remembered.
        #
        # Never when a browser IS remembered. The OS default is usually a different, personal
        # profile, so honouring the flag there silently dumps an Autodesk login into the wrong
        # browser. That happened live (2026-07-25): Edge was remembered in EXTENSION mode, which
        # carries no exe, so _launch_url_in_browser could not spawn it, control fell through to
        # this branch, and the consent opened in the user's personal Chrome. A remembered pref
        # that cannot be launched directly belongs on the extension path below, not here.
        # To genuinely prefer the OS default, call fusion_aps_forget_browser first.
        try:
            webbrowser.open(auth_url)
            opened_via = "os_default_browser"
        except Exception:
            try:
                os.startfile(auth_url)  # noqa: Windows
                opened_via = "os_default_browser"
            except Exception:
                opened_via = None
    else:
        # EXTENSION-FREE open (issue #12, Caleb): we used to stop here and hand back a hint
        # telling the AI to run nbrowser_open_window - which REQUIRES the ABE extension. On a
        # box without it (or on a CLI too old to know the verb) that hint was un-followable and
        # the whole sign-in dead-ended. ABE now exposes nbrowser_open_os_window, which opens a
        # URL in a REAL browser PROFILE with NO extension installed - exactly the OAuth-consent
        # case. Try that before giving up.
        ext_free = _open_os_window_extension_free(auth_url, args.get("profile"))
        if ext_free.get("launched"):
            opened_via = "os_window_extension_free"
        else:
            opened_via = "none_use_extension"

    if opened_via in ("remembered_browser", "specified_browser"):
        _hint = (f"Opened the Autodesk consent in the REMEMBERED browser ({opened_pref.get('name') or 'cached'}"
                 f"{'/' + opened_pref['profileDir'] if opened_pref.get('profileDir') else ''}) — the one that "
                 "authed last time. A background listener on port "
                 f"{bound_port} captures the token once you approve. Poll fusion_aps_status until signedIn:true.")
    elif opened_via == "os_default_browser":
        _hint = ("Opened in the OS DEFAULT browser (fallback). On a multi-browser machine this may be the "
                 "wrong profile; prefer driving authUrl in the user's real browser via nbrowser_open_window "
                 "(new window), then call fusion_aps_set_browser to remember it. Poll fusion_aps_status.")
    elif opened_via == "os_window_extension_free":
        _hint = ("Opened the Autodesk consent in %s using the EXTENSION-FREE path "
                 "(nbrowser_open_os_window) - no Adom browser extension required. TELL THE USER which "
                 "profile it opened in (never just 'your browser') and that THEY need to click Approve, "
                 "because without the extension I cannot see or drive that window. A listener on port "
                 "%s captures the token once they approve. Poll fusion_aps_status until signedIn:true. "
                 "%s" % (ext_free.get("where") or "the user's browser profile", bound_port,
                         _ABE_UPSELL))
    else:
        _hint = ("Could not open a browser for you. No REMEMBERED Autodesk browser, and the EXTENSION-FREE "
                 "open (nbrowser_open_os_window) did not fire%s. DO THIS: run the setupChain in this "
                 "response in order. Simplest manual fallback - have the USER paste authUrl into the "
                 "browser profile their Autodesk account lives in. A listener on port "
                 "%s captures the token once they approve; poll fusion_aps_status until signedIn:true. %s"
                 % ((": " + str(ext_free.get("error"))[:120]) if ext_free and ext_free.get("error") else "",
                    bound_port, _ABE_UPSELL))

    ok_open = bool(opened_via and opened_via != "none_use_extension")
    return {
        "success": True,
        "output": ("Autodesk sign-in started. " + (
            "Approve access in the opened browser, then call fusion_aps_status."
            if ok_open
            else "Open authUrl in the user's real browser (new window) to approve.")),
        "authUrl": auth_url,
        "redirectUri": _active_redirect_uri,
        "openedVia": opened_via,
        "openedBrowser": opened_pref,
        # Extension-free open no longer NEEDS the extension - keep the old flag truthful for
        # existing callers, but only assert it when we genuinely could not open anything.
        "needsExtensionOpen": opened_via == "none_use_extension",
        "extensionFree": ext_free,
        # Caleb's asks (issue #12): a structured code + a chain the agent can follow mechanically,
        # instead of a raw command that may not exist on this box.
        "errorCode": None if ok_open else "browser_open_failed",
        "abe": abe,
        "setupChain": _signin_setup_chain(ok_open, abe),
        "_hint": _hint,
    }


# ── Verb handlers ────────────────────────────────────────────────────────────
_USERINFO_CACHE = {}   # access_token[:24] -> {"email","name","at"}


def _aps_user_info() -> dict:
    """WHO the APS token belongs to - Autodesk's OIDC userinfo, cached per token.

    John, 2026-08-22: the dashboard cards must show WHO is signed in, because the
    release-readiness auth tests run multiple accounts (jlauer12@gmail.com,
    media@adom.inc, ...) and an anonymous green LED cannot distinguish them.
    Free endpoint (userinfo, no data charges); one call per token, then cached.
    """
    try:
        at = _valid_access_token()
    except Exception:
        at = None
    if not at:
        return {}
    # Key by a hash of the FULL token, never a prefix: every Autodesk token is a
    # JWT that begins with the identical base64 header ("eyJhbGciOi..."), so at[:24]
    # collides across ALL users and the cache returned the FIRST account's identity
    # for every later one (john shown while jlauer12 was actually signed in — caught
    # live 2026-08-22 during the multi-account auth tests). A full-token hash is unique
    # per token and never stores the secret itself.
    key = hashlib.sha256(at.encode()).hexdigest()
    hit = _USERINFO_CACHE.get(key)
    if hit:
        return hit
    try:
        import urllib.request as _u
        req = _u.Request("https://api.userprofile.autodesk.com/userinfo",
                         headers={"Authorization": "Bearer " + at})
        info = json.loads(_u.urlopen(req, timeout=10).read())
        out = {"email": info.get("email"), "name": info.get("name"),
               "userId": info.get("sub")}
        _USERINFO_CACHE.clear()
        _USERINFO_CACHE[key] = out
        return out
    except Exception:
        return {}


def handle_status(args: dict) -> dict:
    # NOTE: ab's relay only forwards output/data/success/error/_hint to the
    # cloud caller — every structured field MUST live under `data`.
    client_id = get_client_id()
    tok = _load_tokens()
    has_at = bool(tok.get("access_token"))
    live = has_at and int(tok.get("expires_at", 0)) > int(time.time())
    # signedIn MUST mean "I can actually call APS right now", not "a token file exists".
    # It used to be bool(has_at), so status happily reported signedIn:true while every search
    # died with APS token_expired - and fusion_readiness repeated that lie to the user
    # (caught live 2026-07-22). _valid_access_token() refreshes when possible, so this is the
    # honest signal: usable == live now, or refreshable now.
    usable = bool(live)
    if has_at and not live:
        try:
            usable = bool(_valid_access_token())
        except Exception:
            usable = False
    refresh_failed = bool(has_at and not usable)
    with _signin_lock:
        pending = bool(_signin_state and not _signin_state["done"]
                       and time.time() - _signin_state["startedAt"] < 180)
    usage = _read_usage()
    data = {
        "configured": bool(client_id),
        "signedIn": bool(usable),          # usable NOW (refreshed if needed), not merely present
        "signedInAs": (_aps_user_info() if usable else None),
        "hasStoredToken": bool(has_at),    # a token file exists (may be dead)
        "refreshFailed": refresh_failed,   # stored token is dead and could not be refreshed
        "tokenLive": bool(live),
        "tokenExpiresAt": tok.get("expires_at"),
        "scope": tok.get("scope", SCOPES),
        "signinPending": pending,
        "callbackUrls": all_registered_callbacks(),
        # Never-charge meter — the AI-driven setup watches this.
        "usageMonth": usage.get("month"),
        "callsThisMonth": int(usage.get("calls", 0)),
        "monthlyCap": MONTHLY_CALL_CAP,
        "capRemaining": _cap_remaining(),
        "freeEndpointsOnly": list(FREE_PATH_PREFIXES),
        "neverCharge": True,
    }
    if not client_id:
        hint = ("Not configured. Register a Desktop/PKCE app at https://aps.autodesk.com "
                "(enable Data Management API, set callbacks " + ", ".join(all_registered_callbacks())
                + "), then fusion_aps_set_client_id {\"clientId\":\"...\"} and fusion_aps_signin.")
        summary = "APS not configured"
    elif not has_at:
        hint = "Configured but not signed in. Call fusion_aps_signin (opens browser)."
        summary = "configured, not signed in"
    elif not live:
        hint = "Token expired and refresh failed. Call fusion_aps_signin again."
        summary = "signed in, token expired"
    else:
        hint = "Ready. Use fusion_aps_search {\"query\":\"...\"}."
        summary = "ready"
    return {"success": True, "output": summary, "data": data, "_hint": hint}


def handle_set_client_id(args: dict) -> dict:
    cid = (args.get("clientId") or args.get("client_id") or "").strip()
    if not cid:
        return {"success": False, "error": "Missing clientId."}
    set_client_id(cid)
    return {"success": True,
            "output": f"Saved APS Client ID ({cid[:6]}...). Now call fusion_aps_signin.",
            "data": {"configured": True}}


def handle_signin(args: dict) -> dict:
    res = start_signin(args or {})
    # Move custom fields under data so the relay forwards them.
    if res.get("success"):
        res["data"] = {"authUrl": res.pop("authUrl", None),
                       "redirectUri": res.pop("redirectUri", None),
                       "openedVia": res.pop("openedVia", None),
                       "openedBrowser": res.pop("openedBrowser", None),
                       "needsExtensionOpen": res.pop("needsExtensionOpen", False)}
        # SURFACE OPTIONS + TRADEOFFS (found live 2026-08-22; full matrix in the
        # fusion-auth-surfaces dev skill). The AI reads this hint, not the skill, so it
        # rides in the output. A browser you did not foreground does NOT paint, so a
        # background SCREENSHOT is stale - read the DOM (CDP eval), never diagnose from a
        # bg screenshot alone.
        res["data"]["_surfaceOptions"] = {
            "nb": "BEST if installed: nbrowser_eval (CDP) fills/clicks in the user's REAL "
                  "browser - their real Google/MS logins, Autodesk accepts it, no focus "
                  "steal, DOM is live so no stale-screenshot problem. Cost: the nbe install "
                  "is a heavy ask.",
            "pup": "No install, fully background. BUT (1) pup's profile only has accounts "
                   "YOU signed into it - it will NOT have an arbitrary account the user "
                   "'has'; (2) Autodesk gates pup at the PASSWORD page ('browser not "
                   "supported', survives a clean UA - it fingerprints the CDP launch), so "
                   "prefer the email-first PASSWORDLESS email-OTP path (fetch the code via "
                   "adom-google) and avoid the password page; (3) screenshots are stale "
                   "unless pup_configure {highFps:on}.",
            "foreground": "desktop_click on the real browser - real logins, vendor-accepted, "
                          "but STEALS FOCUS (rude). Last resort for one gated step; narrate it.",
            "_pathFacts": "Not every email has an Autodesk account (media@adom.inc did NOT - "
                          "email-first said 'couldn't find an account'); such users MUST use "
                          "SSO or Create Account. 'Continue with Google' only lists accounts "
                          "signed into THE BROWSER PROFILE you are driving."}
    return res


def handle_set_browser(args: dict) -> dict:
    """Remember the browser+profile that successfully authed to Autodesk, so every later
    fusion_aps_signin reuses it instead of the OS default. Call this right after a driven
    sign-in succeeds (with the browser you opened authUrl in). Fields: name, exe, profileDir,
    newWindow. `exe` is the browser binary on the host; profileDir is e.g. 'Default'/'Profile 2'."""
    exe = (args.get("exe") or "").strip()
    pref = {"name": args.get("name") or args.get("browser"), "exe": exe or None,
            "profileDir": args.get("profileDir"), "newWindow": args.get("newWindow", True),
            "how": args.get("how") or ("extension" if not exe else "launch")}
    if not exe and pref["how"] != "extension":
        return {"success": False, "error": "Provide exe (browser binary path) or how:'extension'.",
                "_hint": "e.g. {name:'Edge', exe:'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', profileDir:'Default'}"}
    ok = save_signin_browser(pref)
    return {"success": ok, "output": f"Remembered Autodesk sign-in browser: {pref.get('name') or exe or pref['how']}.",
            "data": {"saved": ok, "browser": load_signin_browser()}}


def handle_get_browser(args: dict) -> dict:
    """Show the remembered Autodesk sign-in browser (None if not set yet)."""
    pref = load_signin_browser()
    return {"success": True, "output": ("Remembered: " + (pref.get("name") or pref.get("exe") or pref.get("how"))
            if pref else "No Autodesk sign-in browser remembered yet."),
            "data": {"browser": pref}}


def handle_forget_browser(args: dict) -> dict:
    """Forget the remembered browser (next sign-in re-detects / uses the extension)."""
    ok = forget_signin_browser()
    return {"success": ok, "output": "Forgot the remembered Autodesk sign-in browser." if ok else "Could not clear it.",
            "data": {"cleared": ok}}


def handle_get(args: dict) -> dict:
    """Raw authenticated GET — dev/verify helper for nailing the APS grammar."""
    path = args.get("path")
    if not path:
        return {"success": False, "error": "Missing path."}
    method = str(args.get("method") or "GET").upper()
    if method == "GET":
        status, body = _aps_get(path, args.get("query"))
    else:
        # PATCH/POST/DELETE with a JSON:API body, e.g. hiding a stray item:
        # {method:"PATCH", path:"/data/v1/projects/<p>/items/<urn>",
        #  body:{jsonapi:{version:"1.0"}, data:{type:"items", id:<urn>, attributes:{hidden:true}}}}
        status, body = _aps_send(method, path, args.get("body") or {})
    return {"success": 200 <= (status or 0) < 300,
            "output": f"HTTP {status}",
            "data": {"httpStatus": status, "body": body}}


def _list_hubs():
    status, body = _aps_get("/project/v1/hubs")
    if status != 200 or not isinstance(body, dict):
        return [], (status, body)
    return body.get("data", []), None


def _list_projects(hub_id):
    status, body = _aps_get(f"/project/v1/hubs/{hub_id}/projects")
    if status != 200 or not isinstance(body, dict):
        return []
    return body.get("data", [])


def _top_folders(hub_id, project_id):
    status, body = _aps_get(f"/project/v1/hubs/{hub_id}/projects/{project_id}/topFolders")
    if status != 200 or not isinstance(body, dict):
        return []
    return body.get("data", [])


def _folder_search(project_id, folder_id, query, limit):
    """Recursive name search within a folder.

    APS's `:search` endpoint is the server-side recursive INDEX (returns all
    descendant items in a few paged calls — vastly faster than the add-in walk).
    Its server-side `filter[attributes.displayName]` matching proved unreliable
    (returned 0 for known files), so we page through the index and filter
    displayName CLIENT-SIDE (case-insensitive substring) — robust + still fast.
    """
    ql = query.lower()
    matches, seen = [], set()

    def _take(it):
        iid = it.get("id")
        if iid in seen:
            return False
        name = (it.get("attributes", {}) or {}).get("displayName", "") or ""
        if ql in name.lower():
            seen.add(iid)
            matches.append(it)
        return len(matches) >= limit

    # Pass 1 — the folder's DIRECT contents. :search only recurses subfolders, so
    # items sitting in the top folder itself (e.g. a design straight in "Main")
    # would otherwise be missed.
    status, body = _aps_get(f"/data/v1/projects/{project_id}/folders/{folder_id}/contents",
                            {"page[limit]": 200})
    if status == 200 and isinstance(body, dict):
        for it in body.get("data", []):
            if it.get("type") == "items" and _take(it):
                return matches, None

    # Pass 2 — the recursive server index (fast, free), paged. Filter client-side
    # (the server-side displayName filter is unreliable — returns 0 for known files).
    url, next_q = f"/data/v1/projects/{project_id}/folders/{folder_id}/search", {"page[limit]": 200}
    for _ in range(25):
        status, body = _aps_get(url, next_q)
        if status != 200 or not isinstance(body, dict):
            return matches, (None if matches else (status, body))
        for it in body.get("data", []):
            if _take(it):
                return matches, None
        nxt = (body.get("links", {}) or {}).get("next", {})
        nxt_href = nxt.get("href") if isinstance(nxt, dict) else nxt
        if not nxt_href:
            break
        url, next_q = nxt_href, None
    return matches, None


def handle_search(args: dict) -> dict:
    query = (args.get("query") or "").strip()
    if not query:
        return {"success": False, "error": "Missing query."}
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.",
                "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    limit = int(args.get("limit", 50))
    hub_filter = args.get("hubId")
    project_filter = args.get("projectId")

    hubs, err = _list_hubs()
    if err:
        return {"success": False, "error": "APS hubs request failed.",
                "data": {"httpStatus": err[0], "body": err[1]},
                "_hint": "Token may lack data:read scope or the app isn't Data-Management-enabled."}

    # Build the (hub, project, folder) task list, then search projects IN PARALLEL.
    scanned = {"hubs": 0, "projects": 0, "folders": 0}
    tasks = []
    for hub in hubs:
        hub_id = hub.get("id")
        if hub_filter and hub_id != hub_filter:
            continue
        scanned["hubs"] += 1
        projects = _list_projects(hub_id)
        if project_filter:
            projects = [p for p in projects if p.get("id") == project_filter] or \
                       [{"id": project_filter, "attributes": {"name": project_filter}}]
        for proj in projects:
            pid = proj.get("id")
            pname = (proj.get("attributes", {}) or {}).get("name", "")
            scanned["projects"] += 1
            for folder in _top_folders(hub_id, pid):
                scanned["folders"] += 1
                tasks.append((hub_id, pid, pname, folder.get("id")))

    def _scan(task):
        hub_id, pid, pname, fid = task
        hits, _ = _folder_search(pid, fid, query, limit)
        out = []
        for h in hits:
            a = h.get("attributes", {}) or {}
            out.append({"name": a.get("displayName"), "type": h.get("type"), "id": h.get("id"),
                        "hubId": hub_id, "projectId": pid, "projectName": pname,
                        "lastModified": a.get("lastModifiedTime")})
        return out

    results, seen = [], set()
    with ThreadPoolExecutor(max_workers=12) as pool:
        for out in pool.map(_scan, tasks):
            for r in out:
                if r["id"] in seen:
                    continue
                seen.add(r["id"])
                results.append(r)

    results.sort(key=lambda r: (r.get("lastModified") or ""), reverse=True)
    return {
        "success": True,
        "output": f"{len(results)} match(es) for '{query}'",
        "data": {
            "query": query,
            "count": len(results),
            "results": results[:limit],
            "scanned": scanned,
        },
        "_hint": f"Found {len(results)} match(es) via the APS server index across "
                 f"{scanned['projects']} project(s), newest first. Pass projectId to scope to "
                 "one project (faster). Use fusion_aps_open to open a hit in Fusion.",
    }


def find_one(query: str) -> dict | None:
    """Best single match for a query (newest) — used by fusion_aps_open."""
    res = handle_search({"query": query, "limit": 5})
    rs = res.get("data", {}).get("results", [])
    return rs[0] if rs else None


def handle_browse(args: dict) -> dict:
    """Browse the cloud without launching Fusion: hubs+projects → top folders → contents."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    hub_id = args.get("hubId")
    project_id = args.get("projectId")
    folder_id = args.get("folderId")

    if project_id and folder_id:
        s, b = _aps_get(f"/data/v1/projects/{project_id}/folders/{folder_id}/contents", {"page[limit]": 200})
        items = [{"name": (x.get("attributes", {}) or {}).get("displayName") or (x.get("attributes", {}) or {}).get("name"),
                  "type": x.get("type"), "id": x.get("id")} for x in (b.get("data", []) if isinstance(b, dict) else [])]
        return {"success": s == 200, "output": f"{len(items)} item(s)",
                "data": {"level": "folder", "projectId": project_id, "folderId": folder_id, "items": items}}

    if project_id:
        if not hub_id:
            hubs, _ = _list_hubs()
            hub_id = hubs[0].get("id") if hubs else None
        folders = _top_folders(hub_id, project_id)
        items = [{"name": (f.get("attributes", {}) or {}).get("name"), "type": "folder", "id": f.get("id")}
                 for f in folders]
        return {"success": True, "output": f"{len(items)} top folder(s)",
                "data": {"level": "project", "hubId": hub_id, "projectId": project_id, "items": items}}

    hubs, err = _list_hubs()
    if err:
        return {"success": False, "error": "APS hubs request failed.", "data": {"body": err[1]}}
    out = []
    for h in hubs:
        hid = h.get("id")
        if hub_id and hid != hub_id:
            continue
        projs = _list_projects(hid)
        out.append({"hub": (h.get("attributes", {}) or {}).get("name"), "hubId": hid,
                    "projects": [{"name": (p.get("attributes", {}) or {}).get("name"), "id": p.get("id")} for p in projs]})
    nprej = sum(len(x["projects"]) for x in out)
    return {"success": True, "output": f"{len(out)} hub(s), {nprej} project(s)",
            "data": {"level": "hubs", "hubs": out}}


def handle_recent(args: dict) -> dict:
    """Most-recently-modified files across the team hub (or one project). Reuses the
    parallel index scan with a match-all filter, then sorts by lastModifiedTime."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    limit = int(args.get("limit", 25))
    hub_filter = args.get("hubId")
    project_filter = args.get("projectId")
    hubs, err = _list_hubs()
    if err:
        return {"success": False, "error": "APS hubs request failed.", "data": {"body": err[1]}}
    tasks = []
    for hub in hubs:
        hid = hub.get("id")
        if hub_filter and hid != hub_filter:
            continue
        projects = _list_projects(hid)
        if project_filter:
            projects = [p for p in projects if p.get("id") == project_filter] or \
                       [{"id": project_filter, "attributes": {"name": project_filter}}]
        for proj in projects:
            pid = proj.get("id")
            pname = (proj.get("attributes", {}) or {}).get("name", "")
            for f in _top_folders(hid, pid):
                tasks.append((pid, pname, f.get("id")))

    def _scan(t):
        pid, pname, fid = t
        hits, _ = _folder_search(pid, fid, "", 200)  # "" matches everything
        out = []
        for h in hits:
            a = h.get("attributes", {}) or {}
            out.append({"name": a.get("displayName"), "type": h.get("type"), "id": h.get("id"),
                        "projectId": pid, "projectName": pname, "lastModified": a.get("lastModifiedTime")})
        return out

    items, seen = [], set()
    with ThreadPoolExecutor(max_workers=12) as pool:
        for out in pool.map(_scan, tasks):
            for r in out:
                if r["id"] in seen:
                    continue
                seen.add(r["id"])
                items.append(r)
    items = [i for i in items if i.get("lastModified")]
    items.sort(key=lambda r: r["lastModified"], reverse=True)
    items = items[:limit]
    return {"success": True, "output": f"{len(items)} recent file(s)",
            "data": {"count": len(items), "results": items},
            "_hint": "Most-recently-modified designs across the team hub, newest first."}


def handle_file_info(args: dict) -> dict:
    """Version history + metadata for a file: who last changed it, when, how many versions."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    project_id = args.get("projectId")
    urn = args.get("urn")
    name = args.get("name")
    query = (args.get("query") or "").strip()
    if query and not (project_id and urn):
        m = find_one(query)
        if not m:
            return {"success": False, "error": f"No cloud file matching '{query}'."}
        project_id, urn, name = m.get("projectId"), m.get("id"), m.get("name")
    if not (project_id and urn):
        return {"success": False, "error": "Provide query, or projectId + urn."}

    # Derive the item (lineage) urn from a version urn (vf.<base>?version=N).
    raw = urn.split("?")[0]
    item_urn = urn
    if "fs.file:vf." in raw:
        item_urn = "urn:adsk.wipprod:dm.lineage:" + raw.split("fs.file:vf.")[-1]
    enc = urllib.parse.quote(item_urn, safe="")
    s, b = _aps_get(f"/data/v1/projects/{project_id}/items/{enc}/versions")
    if s != 200 or not isinstance(b, dict):
        return {"success": False, "error": "Could not fetch versions.",
                "data": {"httpStatus": s, "body": b, "itemUrn": item_urn}}
    versions = []
    for v in b.get("data", []):
        a = v.get("attributes", {}) or {}
        versions.append({
            "versionNumber": a.get("versionNumber"),
            "lastModifiedTime": a.get("lastModifiedTime"),
            "lastModifiedUserName": a.get("lastModifiedUserName"),
            "createTime": a.get("createTime"),
            "createUserName": a.get("createUserName"),
            "storageSize": a.get("storageSize"),
        })
    versions.sort(key=lambda x: (x.get("versionNumber") or 0), reverse=True)
    tip = versions[0] if versions else {}
    return {
        "success": True,
        "output": f"{name or item_urn}: {len(versions)} version(s), last by {tip.get('lastModifiedUserName')}",
        "data": {"name": name, "projectId": project_id, "itemUrn": item_urn,
                 "versionCount": len(versions), "tip": tip, "versions": versions[:20]},
        "_hint": "Version history newest-first; tip = current version (who/when/size).",
    }


# ── Free Data Management writes + file transfer ──────────────────────────────
# Everything below stays within the user's OWN Fusion Team hub. Data Management
# writes (folders/items/versions) and OSS storage transfer (download/upload) are
# in APS's always-FREE set, so the never-charge guarantee still holds (the gating
# in _aps_send mirrors _aps_get). Writes need data:write/data:create scope, so a
# user signed in under the old read-only scope must fusion_aps_signin once more.
JSONAPI = "application/vnd.api+json"


def _aps_send(method: str, path: str, json_body=None, content_type=JSONAPI, timeout=120):
    """Authenticated POST/PATCH/PUT with the SAME never-charge gating as _aps_get."""
    at = _valid_access_token()
    if not at:
        return None, {"error": "not_signed_in"}
    only_path = path if not path.startswith("http") else urllib.parse.urlparse(path).path
    if not any(only_path.startswith(p) for p in FREE_PATH_PREFIXES):
        return 0, {"error": "blocked_non_free_endpoint", "path": only_path,
                   "_hint": "Refused — only free Data Management / OSS storage endpoints are "
                            "allowed, so you are never charged."}
    if _cap_remaining() <= 0:
        return 0, {"error": "usage_cap_reached", "cap": MONTHLY_CALL_CAP}
    url = path if path.startswith("http") else f"{APS_BASE}{path}"
    headers = {"Authorization": f"Bearer {at}"}
    data = None
    if json_body is not None:
        data = json.dumps(json_body).encode()
        headers["Content-Type"] = content_type
    _bump_usage(1)
    return _http(method, url, headers=headers, data=data, timeout=timeout)


def _http_bytes(method: str, url: str, data=None, headers=None, timeout=300):
    """Raw binary HTTP (presigned S3 up/download) — _http decodes text, which would
    corrupt binary, so transfers use this. Returns (status, bytes)."""
    req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        return 0, str(e).encode()


def _lineage_urn(urn: str) -> str:
    raw = urn.split("?")[0]
    if "fs.file:vf." in raw:
        return "urn:adsk.wipprod:dm.lineage:" + raw.split("fs.file:vf.")[-1]
    return urn


def _tip_version_urn(project_id: str, urn: str):
    """Given a version OR lineage urn, return a concrete version urn (tip if lineage)."""
    if "?version=" in urn or "fs.file:vf." in urn.split("?")[0]:
        return urn, None
    enc = urllib.parse.quote(_lineage_urn(urn), safe="")
    s, b = _aps_get(f"/data/v1/projects/{project_id}/items/{enc}")
    if s != 200 or not isinstance(b, dict):
        return None, (s, b)
    tip = (((b.get("data") or {}).get("relationships") or {}).get("tip") or {}).get("data") or {}
    return tip.get("id"), None


def _storage_of_version(project_id: str, version_urn: str):
    enc = urllib.parse.quote(version_urn, safe="")
    s, b = _aps_get(f"/data/v1/projects/{project_id}/versions/{enc}")
    if s != 200 or not isinstance(b, dict):
        return None, None, (s, b)
    d = b.get("data") or {}
    storage = (((d.get("relationships") or {}).get("storage") or {}).get("data") or {}).get("id")
    name = (d.get("attributes") or {}).get("name")
    return storage, name, None


def _parse_oss(storage_urn: str):
    # urn:adsk.objects:os.object:<bucketKey>/<objectKey>
    tail = storage_urn.split("os.object:", 1)[-1]
    bucket, _, obj = tail.partition("/")
    return bucket, obj


def handle_versions(args: dict) -> dict:
    """Full version history of a file (every version, uncapped): who/when/size per version."""
    info = handle_file_info(args)
    if not info.get("success"):
        return info
    d = info["data"]
    project_id, item_urn = d["projectId"], d["itemUrn"]
    enc = urllib.parse.quote(item_urn, safe="")
    s, b = _aps_get(f"/data/v1/projects/{project_id}/items/{enc}/versions", {"page[limit]": 200})
    versions = []
    for v in (b.get("data", []) if isinstance(b, dict) else []):
        a = v.get("attributes", {}) or {}
        versions.append({"versionNumber": a.get("versionNumber"), "id": v.get("id"),
                         "lastModifiedTime": a.get("lastModifiedTime"),
                         "lastModifiedUserName": a.get("lastModifiedUserName"),
                         "createTime": a.get("createTime"), "createUserName": a.get("createUserName"),
                         "storageSize": a.get("storageSize")})
    versions.sort(key=lambda x: (x.get("versionNumber") or 0), reverse=True)
    return {"success": True, "output": f"{d.get('name') or item_urn}: {len(versions)} version(s)",
            "data": {"name": d.get("name"), "projectId": project_id, "itemUrn": item_urn,
                     "versionCount": len(versions), "versions": versions},
            "_hint": "Complete version history, newest-first."}


def handle_download(args: dict) -> dict:
    """Download a cloud file to the local machine (no Fusion needed).
    args: query | (projectId + urn); optional saveDir, fileName."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    project_id, urn, name = args.get("projectId"), args.get("urn"), args.get("name")
    query = (args.get("query") or "").strip()
    if query and not (project_id and urn):
        m = find_one(query)
        if not m:
            return {"success": False, "error": f"No cloud file matching '{query}'."}
        project_id, urn, name = m.get("projectId"), m.get("id"), m.get("name")
    if not (project_id and urn):
        return {"success": False, "error": "Provide query, or projectId + urn."}

    version_urn, err = _tip_version_urn(project_id, urn)
    if not version_urn:
        return {"success": False, "error": "Could not resolve a version.", "data": {"detail": err}}
    storage_urn, vname, err = _storage_of_version(project_id, version_urn)
    if not storage_urn:
        return {"success": False, "error": "Could not find storage for this version.",
                "data": {"detail": err}}
    bucket, obj = _parse_oss(storage_urn)
    s, b = _aps_get(f"/oss/v2/buckets/{bucket}/objects/{urllib.parse.quote(obj, safe='')}/signeds3download")
    if s != 200 or not isinstance(b, dict) or not b.get("url"):
        return {"success": False, "error": "Could not get a download URL.",
                "data": {"httpStatus": s, "body": b}}

    save_dir = args.get("saveDir") or str(Path(os.path.expanduser("~")) / "Downloads")
    fname = args.get("fileName") or vname or name or "download.bin"
    Path(save_dir).mkdir(parents=True, exist_ok=True)
    out_path = str(Path(save_dir) / fname)
    st, data = _http_bytes("GET", b["url"])
    if st != 200:
        return {"success": False, "error": f"Download failed (HTTP {st}).",
                "data": {"bytes": len(data)}}
    with open(out_path, "wb") as fh:
        fh.write(data)
    return {"success": True, "output": f"Downloaded {fname} ({len(data):,} bytes) to {out_path}",
            "data": {"path": out_path, "bytes": len(data), "name": fname, "version": version_urn},
            "_hint": "Saved to the local machine; opening in Fusion is a separate step (fusion_aps_open)."}


def handle_create_folder(args: dict) -> dict:
    """Create a subfolder in a project. args: projectId, parentFolderId, name."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    project_id = args.get("projectId")
    parent = args.get("parentFolderId") or args.get("folderId")
    fname = (args.get("name") or "").strip()
    if not (project_id and parent and fname):
        return {"success": False, "error": "Provide projectId, parentFolderId, name."}
    body = {"jsonapi": {"version": "1.0"},
            "data": {"type": "folders",
                     "attributes": {"name": fname,
                                    "extension": {"type": "folders:autodesk.core:Folder", "version": "1.0"}},
                     "relationships": {"parent": {"data": {"type": "folders", "id": parent}}}}}
    s, b = _aps_send("POST", f"/data/v1/projects/{project_id}/folders", body)
    if s not in (200, 201) or not isinstance(b, dict):
        return {"success": False, "error": "Create folder failed.",
                "data": {"httpStatus": s, "body": b},
                "_hint": "HTTP 403 = sign-in lacks data:write/data:create (fusion_aps_signin again). "
                         "409 = a folder with that name already exists here."}
    fid = (b.get("data") or {}).get("id")
    return {"success": True, "output": f"Created folder '{fname}'",
            "data": {"folderId": fid, "name": fname, "projectId": project_id}}


def handle_upload(args: dict) -> dict:
    """Upload a local file as a NEW cloud file. args: projectId, folderId, localPath; opt fileName."""
    if not _valid_access_token():
        return {"success": False, "error": "Not signed in to APS.", "errorCode": "aps_not_signed_in",
                "_hint": APS_REQUIRED_HINT}
    project_id, folder_id, local = args.get("projectId"), args.get("folderId"), args.get("localPath")
    if not (project_id and folder_id and local):
        return {"success": False, "error": "Provide projectId, folderId, localPath."}
    # FOLDER HYGIENE: never dump loose files into a shared project ROOT (see fusion-cloud-hygiene).
    _KNOWN_ROOT_FOLDERS = {"urn:adsk.wipprod:fs.folder:co.jyO4vxQXR6S9zFpTQWnZAg"}
    if folder_id in _KNOWN_ROOT_FOLDERS:
        return {"success": False, "errorCode": "refused_root_upload",
                "error": "REFUSED: that folderId is a shared project ROOT - the bridge never writes "
                         "loose files there (it clutters the team's cloud).",
                "_hint": "Create/choose a real subfolder first (fusion_aps_create_folder under an "
                         "'Adom AI Workspace' folder), or just call fusion_make_3d_package / "
                         "fusion_build_library_3d, which now default to a non-root workspace folder. "
                         "See the fusion-cloud-hygiene skill."}
    if not os.path.isfile(local):
        return {"success": False, "error": f"Local file not found: {local}"}
    fname = args.get("fileName") or os.path.basename(local)
    data = open(local, "rb").read()

    # 1. Reserve a storage object in the target folder.
    sbody = {"jsonapi": {"version": "1.0"},
             "data": {"type": "objects", "attributes": {"name": fname},
                      "relationships": {"target": {"data": {"type": "folders", "id": folder_id}}}}}
    s, b = _aps_send("POST", f"/data/v1/projects/{project_id}/storage", sbody)
    if s not in (200, 201) or not isinstance(b, dict):
        return {"success": False, "error": "Storage reservation failed.",
                "data": {"httpStatus": s, "body": b},
                "_hint": "HTTP 403 = sign-in lacks data:write/data:create — fusion_aps_signin again."}
    storage_urn = (b.get("data") or {}).get("id")
    bucket, obj = _parse_oss(storage_urn)
    objenc = urllib.parse.quote(obj, safe="")

    # 2. Get a signed S3 upload URL, 3. PUT the bytes, 4. complete the upload.
    s, b = _aps_get(f"/oss/v2/buckets/{bucket}/objects/{objenc}/signeds3upload")
    if s != 200 or not isinstance(b, dict) or not b.get("urls"):
        return {"success": False, "error": "Could not get an upload URL.", "data": {"httpStatus": s, "body": b}}
    upload_key, put_url = b.get("uploadKey"), b["urls"][0]
    st, _ = _http_bytes("PUT", put_url, data=data, headers={"Content-Type": "application/octet-stream"})
    if st not in (200, 201):
        return {"success": False, "error": f"S3 upload PUT failed (HTTP {st})."}
    s, b = _aps_send("POST", f"/oss/v2/buckets/{bucket}/objects/{objenc}/signeds3upload",
                     {"uploadKey": upload_key}, content_type="application/json")
    if s not in (200, 201, 202):
        return {"success": False, "error": "Upload completion failed.", "data": {"httpStatus": s, "body": b}}

    # 5. Create the item (new file, version 1) pointing at the uploaded object.
    ibody = {"jsonapi": {"version": "1.0"},
             "data": {"type": "items",
                      "attributes": {"displayName": fname,
                                     "extension": {"type": "items:autodesk.core:File", "version": "1.0"}},
                      "relationships": {"tip": {"data": {"type": "versions", "id": "1"}},
                                        "parent": {"data": {"type": "folders", "id": folder_id}}}},
             "included": [{"type": "versions", "id": "1",
                           "attributes": {"name": fname,
                                          "extension": {"type": "versions:autodesk.core:File", "version": "1.0"}},
                           "relationships": {"storage": {"data": {"type": "objects", "id": storage_urn}}}}]}
    s, b = _aps_send("POST", f"/data/v1/projects/{project_id}/items", ibody)
    if s not in (200, 201) or not isinstance(b, dict):
        return {"success": False,
                "error": "Create item failed (this verb uploads NEW files; the name may already exist).",
                "data": {"httpStatus": s, "body": b}}
    item_id = (b.get("data") or {}).get("id")
    return {"success": True, "output": f"Uploaded '{fname}' ({len(data):,} bytes) as a new cloud file.",
            "data": {"itemUrn": item_id, "name": fname, "bytes": len(data),
                     "projectId": project_id, "folderId": folder_id}}


# Command-name -> handler (invoked as fusion_aps_* from the CLI).
HANDLERS = {
    "aps_status": handle_status,
    "aps_set_client_id": handle_set_client_id,
    "aps_signin": handle_signin,
    "aps_search": handle_search,
    "aps_browse": handle_browse,
    "aps_recent": handle_recent,
    "aps_file_info": handle_file_info,
    "aps_versions": handle_versions,
    "aps_download": handle_download,
    "aps_create_folder": handle_create_folder,
    "aps_upload": handle_upload,
    "aps_get": handle_get,
}


# ── 3D thumbnail state: deterministic, free-endpoint, no pixels ─────────────────────────
# John, 2026-08-21: "you must ensure [the 3D thumbnail] is there ... is there a programmatic
# way to test for a thumbnail working correctly so you don't have to do a screenshot
# analysis?" There is: the Library Manager thumbnail is rendered by Autodesk's CLOUD against
# the bound f3d, and Data Management exposes it as a RELATIONSHIP on the file's version -
# present once rendered, absent while Fusion "takes its good old time". Reading that
# relationship is a free /data/v1 call: an HTTP/JSON state check, never image analysis.


def thumbnail_state(project_id: str, folder_id: str, wip_urn: str = "", file_name: str = "") -> dict:
    """State of the cloud thumbnail for a file in a folder we uploaded to.

    Match by wip_urn (the .lbr package3d urn, minus ?version=) or by displayName.
    Returns {state: ready|pending|not_found|not_signed_in|error, item, thumbnailId}.
    """
    if not project_id or not folder_id:
        return {"state": "error", "why": "projectId and folderId required"}
    want_urn = (wip_urn or "").split("?", 1)[0].strip()
    status, data = _aps_get(
        "/data/v1/projects/%s/folders/%s/contents" % (project_id, folder_id),
        {"include": "versions"})
    if status is None:
        return {"state": "not_signed_in"}
    if status != 200 or not isinstance(data, dict):
        return {"state": "error", "http": status,
                "why": (data or {}).get("error") if isinstance(data, dict) else None}
    items = data.get("data") or []
    included = data.get("included") or []
    # join tip versions to items; match by version urn or display name
    hit_item = None
    hit_version = None
    def _vf_token(u):
        # namespaces differ between what the mint writes (urn:adsk.wipprod:fs.file:vf.X)
        # and what the listing returns - the vf.<id> token is the stable join key
        u = str(u or "").split("?", 1)[0]
        return u.rsplit("vf.", 1)[-1] if "vf." in u else u
    want_tok = _vf_token(want_urn)
    def _norm(s):
        # Part names round-trip through KiCad/EAGLE/upload with commas, dots, dashes and
        # underscores mangled differently at each hop ("TJA1051T_3,118" vs
        # "tja1051t-3-118" - issue #47 failure mode 4). Compare alphanumerics only.
        return "".join(c for c in str(s or "").lower() if c.isalnum())
    fn = _norm(file_name)
    for v in included:
        vid_tok = _vf_token(v.get("id"))
        vname = _norm((v.get("attributes") or {}).get("displayName"))
        # names match EITHER direction: the f3d is uploaded as ADOM-<part>-3D, so the
        # part name is a substring of the file, not a prefix (caught live: not_found on a
        # file that was sitting right there)
        name_hit = fn and (vname.startswith(fn) or fn in vname)
        if (want_tok and vid_tok == want_tok) or name_hit:
            hit_version = v
            break
    if hit_version is None and file_name:
        fn2 = _norm(file_name)
        for it in items:
            nm = _norm((it.get("attributes") or {}).get("displayName"))
            if nm.startswith(fn2) or fn2 in nm:
                hit_item = it
                break
    src = hit_version or hit_item
    if src is None:
        return {"state": "not_found", "searched": {"urn": want_urn, "name": file_name},
                "itemCount": len(items)}
    rel = ((src.get("relationships") or {}).get("thumbnails") or {})
    tid = ((rel.get("data") or {}) or {}).get("id")
    if tid:
        return {"state": "ready", "thumbnailId": tid,
                "item": str((src.get("attributes") or {}).get("displayName") or "")}
    return {"state": "pending",
            "item": str((src.get("attributes") or {}).get("displayName") or ""),
            "why": "version exists but Fusion's cloud has not rendered a thumbnail yet"}
