"""In-app 'Adom is driving Fusion' banner - a docked Fusion palette.

John, 2026-08-28: "is there some mechanism fusion provides for in-app general status bar
or indicator... so a user knows they might not want to touch fusion during that
operation? cuz opening a multipart library is like a 10 to 15 minute operation."

Until now the ONLY signal was the Windows-taskbar progress bar, which vanishes the moment
Fusion is maximised in front of the user - precisely when they reach for the mouse. Fusion
exposes no writable status bar, and its ProgressDialog is modal and holds the main thread
(the exact dialog class this bridge spent a week eliminating). A PALETTE is the right
surface: Fusion's own docked HTML panel, persistent, non-modal, ours to render.

Design rules, each load-bearing:
- CREATED ONCE, lazily, on first show; later calls only set HTML. Palette creation is main-
  thread work and this bridge's whole discipline is not holding that thread; text updates
  are cheap, creation is not.
- NEVER raises into the caller. A banner is advisory. If Fusion refuses to build the palette
  (older API, no UI context), the import must proceed exactly as if the banner did not exist.
- It is a "please do not touch" ADVISORY, not a lock. Fusion has no read-only mode; a
  determined click still lands. The banner tells the human; it cannot stop them.
- Same text the wiki sees. The server passes the run's own stageLabel / percent / ETA, so a
  user at Fusion and a user at the wiki read one story.
"""

import adsk.core

PALETTE_ID = "AdomActivityPalette"

# THE ADOM WORDMARK, inline (John, 2026-08-28: "why is there no branding here? our logo is
# needed so this looks trusted"). A banner that says "do not touch your app" has to be
# recognisably ours, not a teal dot. Embedded so it renders with no network and no file.
ADOM_LOGO_SVG = '<svg id="Layer_1" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1072.9 236"><defs><style>.cls-1{fill:#00b6af;stroke-width:0px;}</style></defs><path class="cls-1" d="M212.94,23.05c-25.84-25.84-62.87-30.69-82.73-10.84-8.06,8.06-12.04,18.94-12.21,30.76h0s0,.02,0,.02c0,.04,0,.08,0,.12v73.29c0,.89.72,1.61,1.61,1.61h72.5l-.02-.02c12.17.03,23.42-3.94,31.69-12.2,19.85-19.85,15-56.89-10.84-82.73Z"/><path class="cls-1" d="M23.05,212.95c25.84,25.84,62.87,30.69,82.73,10.84,8.06-8.06,12.04-18.94,12.21-30.76h0s0-.02,0-.02c0-.04,0-.08,0-.12v-73.29c0-.89-.72-1.61-1.61-1.61H43.89l.02.02c-12.17-.03-23.42,3.94-31.69,12.2-19.85,19.85-15,56.89,10.84,82.73Z"/><path class="cls-1" d="M1065.5,25.22h-58.49c-2.98,0-5.68,1.87-6.73,4.65l-44.4,117.54c-.06.17-.31.17-.37,0l-45.44-117.59c-1.07-2.75-3.76-4.6-6.72-4.6h-58.79c-4.09,0-7.4,3.31-7.4,7.4v35.06l-.03-.12c-1.24,4.99-5.89,6-5.89,6,0,0-4.51,1.02-7.61-2.53v.03c-17.82-29.81-50.31-48.93-89.04-48.93-41.5,0-75.86,21.85-92.76,55.23v-.07c-2.86,5.19-8.74,5.13-8.74,5.13,0,0-5.63-.01-8.41-4.61-16.49-32.68-50.56-52.59-94.79-52.59h-73.46c-4.09,0-7.4,3.31-7.4,7.4v121.27s0,.03,0,.04c-.95,5.15-4.75,5.94-4.75,5.94,0,0-3.88.85-6.77-3.32l.05.23-46.05-126.83c-1.03-2.84-3.75-4.74-6.77-4.74h-57.14c-3.01,0-5.73,1.9-6.76,4.73l-68.67,188.38c-1.05,2.88-.18,6.21,2.4,8.19,1.31,1.01,2.98,1.47,4.64,1.47h34.96s6.43,0,6.43,0c2.5,0,4.83-1.39,5.9-3.65,8.49-17.86,26.5-29.64,48.37-29.64s39.71,11.73,48.19,29.53c1.1,2.32,3.49,3.76,6.06,3.76h120.78c31.22,0,57.85-9.95,77.02-28.77,7.38-7.25,13.43-15.72,18.05-25.1v.09c2.74-5.19,8.37-5.13,8.37-5.13,0,0,5.43,0,8.08,4.64,4.88,9.88,11.26,18.78,18.91,26.42,18.6,18.6,44.61,29.78,74.26,29.78,38.74,0,71.23-19.18,89.05-49.07l-.02.07c3.1-3.55,7.61-2.53,7.61-2.53,0,0,4.65,1,5.89,6l.03-.12v36.32c0,4.09,3.31,7.4,7.4,7.4h45.79c4.09,0,7.4-3.31,7.4-7.4v-86.26c0-.23.32-.28.39-.06l30.37,88.86c1,2.91,3.74,4.87,6.81,4.87h39.69c3.08,0,5.82-1.96,6.81-4.87l30.1-88.02c.07-.21.39-.16.39.06v85.43c0,4.09,3.31,7.4,7.4,7.4h45.79c4.09,0,7.4-3.31,7.4-7.4V32.62c0-4.09-3.31-7.4-7.4-7.4ZM372.31,147.57c-.15.21-.5.57-1.1.57h-30.33c-.61,0-.95-.35-1.1-.56-.15-.21-.38-.65-.19-1.23l15.06-45.79c.27-.81.92-.94,1.29-.94h0c.37,0,1.02.12,1.29.93l15.26,45.8c.19.58-.04,1.01-.19,1.23ZM525.72,173.84h-14.52c-.86,0-1.56-.7-1.56-1.56v-92.16c0-.86.7-1.56,1.56-1.56h14.52c30.81,0,48.48,17.47,48.48,47.92s-17.67,47.36-48.48,47.36ZM734.6,173.26c-26.51,0-42.33-17.71-42.34-47.36,0-14.59,3.88-26.68,11.21-34.98,7.33-8.29,18.09-12.67,31.13-12.67,12.11,0,22.28,3.84,29.58,11.13.5.5.98,1.01,1.45,1.54,7.4,8.33,11.31,20.42,11.31,34.97,0,29.21-16.22,47.36-42.34,47.36Z"/></svg>'
_palette = None
_last_reload_ts = 0.0
_user_closed = [False]   # the user clicked the palette's X: stay hidden until the next run
_handlers = []           # keep event handler references alive (Fusion drops unreferenced ones)


class _PaletteClosedHandler(adsk.core.UserInterfaceGeneralEventHandler):
    """The user closed the palette with its X (John, 2026-08-30: "it has no close button so i
    can't get rid of it when its in the way"). Remember that so the run's periodic updates do
    not pop it straight back; the next run (action 'show') brings it back."""
    def notify(self, args):
        try:
            _user_closed[0] = True
        except Exception:
            pass


def _html(title, detail, percent, eta_sec, tone):
    pct = max(0, min(100, int(percent or 0)))
    eta = ""
    if eta_sec is not None:
        m, s = divmod(max(0, int(eta_sec)), 60)
        eta = ("about %dm %02ds left" % (m, s)) if m else ("about %ds left" % s)
    bar_color = {"working": "#00b8b1", "done": "#3fb950", "failed": "#f85149"}.get(tone, "#00b8b1")
    return """<!doctype html><html><head><meta charset="utf-8"><style>
      html,body{margin:0;background:#0d1117;color:#e6edf3;font:13px/1.45 'Segoe UI',system-ui,sans-serif}
      .wrap{padding:12px 14px}
      .brand{display:flex;align-items:center;gap:8px;font-weight:600;font-size:12px;letter-spacing:.02em;color:#00b8b1;text-transform:uppercase}
      .mark svg{height:16px;width:auto;display:block}
      .brandtxt{color:#8b949e;text-transform:none;letter-spacing:0;font-weight:500}
      .dot{width:8px;height:8px;border-radius:50%%;background:%(bar)s;box-shadow:0 0 0 3px rgba(0,184,177,.18)}
      h1{font-size:15px;font-weight:600;margin:8px 0 2px}
      .detail{color:#8b949e;font-size:12px;min-height:1.4em}
      .track{height:6px;background:#21262d;border-radius:3px;margin:10px 0 6px;overflow:hidden}
      .fill{height:100%%;width:%(pct)d%%;background:%(bar)s;transition:width .4s ease}
      .meta{display:flex;justify-content:space-between;font-size:11px;color:#8b949e;font-variant-numeric:tabular-nums}
      .warn{margin-top:10px;padding:8px 10px;border-left:3px solid %(bar)s;background:#161b22;border-radius:6px;font-size:12px}
    </style></head><body><div class="wrap">
      <div class="brand"><span class="mark">%(logo)s</span><span class="dot"></span><span class="brandtxt">is driving Fusion</span></div>
      <h1>%(title)s</h1>
      <div class="detail">%(detail)s</div>
      <div class="track"><div class="fill"></div></div>
      <div class="meta"><span>%(pct)d%%</span><span>%(eta)s</span></div>
      %(warn)s
    </div></body></html>""" % {
        "bar": bar_color, "pct": pct, "title": _esc(title), "detail": _esc(detail), "eta": eta,
        "logo": ADOM_LOGO_SVG,
        "warn": ('<div class="warn">Please do not click, type or close documents in Fusion '
                 'until this finishes. Your other apps are unaffected.</div>'
                 if tone == "working" else ""),
    }


def _shell_html(initial_payload=None, event_stream_url=None):
    """The banner page, written ONCE. Every later update arrives through
    palette.sendInfoToHTML("update", json) and is applied by the page's own script.

    THE THREAD LEAK (John, 2026-08-30: Fusion at 21,900 OS threads after 30 hours of
    imports, add-in answering in seconds, restart prompts, runs crawling). Measured on a
    fresh Fusion with add-in 1.0.7: 85 HTTP requests to the add-in added zero threads, five
    library open/close cycles added zero after the first, but 20 banner updates added 58 OS
    threads, about three per update. The old update path re-set htmlFileURL every time,
    which made Fusion's embedded browser reload the page (and leak its threads) on every
    frame, every part and every 10 s watchdog beat: thousands of reloads per run. Now the
    page loads once and the bridge only sends it data."""
    return """<!doctype html><html><head><meta charset="utf-8"><style>
      html,body{margin:0;background:#0d1117;color:#e6edf3;font:13px/1.45 'Segoe UI',system-ui,sans-serif}
      .wrap{padding:12px 14px}
      .brand{display:flex;align-items:center;gap:8px;font-weight:600;font-size:12px;letter-spacing:.02em;color:#00b8b1;text-transform:uppercase}
      .mark svg{height:16px;width:auto;display:block}
      .brandtxt{color:#8b949e;text-transform:none;letter-spacing:0;font-weight:500}
      .dot{width:8px;height:8px;border-radius:50%%;background:#00b8b1;box-shadow:0 0 0 3px rgba(0,184,177,.18)}
      h1{font-size:15px;font-weight:600;margin:8px 0 2px}
      .detail{color:#8b949e;font-size:12px;min-height:1.4em}
      .track{height:6px;background:#21262d;border-radius:3px;margin:10px 0 6px;overflow:hidden}
      .fill{height:100%%;width:0%%;background:#00b8b1;transition:width .4s ease}
      .track.sweep .fill{width:30%%;transition:none;animation:sweep 1.4s ease-in-out infinite}
      @keyframes sweep{0%%{transform:translateX(-100%%)}100%%{transform:translateX(340%%)}}
      @media (prefers-reduced-motion:reduce){.track.sweep .fill{animation:none;width:100%%;opacity:.35}}
      .meta{display:flex;justify-content:space-between;font-size:11px;color:#8b949e;font-variant-numeric:tabular-nums}
      .warn{margin-top:10px;padding:8px 10px;border-left:3px solid #00b8b1;background:#161b22;border-radius:6px;font-size:12px}
      .warn[hidden]{display:none}
      .routing[hidden]{display:none}
      .routing{border-top:1px solid #30363d;margin-top:12px;padding-top:4px}
      .routing h2{font-size:12px;color:#00b8b1;margin:10px 0 3px}
      .routing p{font-size:13px;white-space:pre-line;margin:0;color:#c9d1d9}
      .connection{color:#8b949e;font-size:11px;margin-top:8px}
    </style></head><body><div class="wrap">
      <div class="brand"><span class="mark">%(logo)s</span><span class="dot" id="dot"></span><span class="brandtxt">is driving Fusion</span></div>
      <h1 id="title">Working</h1>
      <div class="detail" id="detail"></div>
      <div class="track" id="track"><div class="fill" id="fill"></div></div>
      <div class="meta"><span id="pct">0%%</span><span id="eta"></span></div>
      <div class="routing" id="routing" hidden><h2>Selected connection</h2><p id="routeConnection"></p><h2>Calculated route</h2><p id="routePlan"></p><h2>Measured checks</h2><p id="routeChecks"></p><h2>Recent operations</h2><p id="routeEvents"></p><div class="connection" id="streamStatus"></div></div>
      <div class="warn" id="warn">Please do not click, type or close documents in Fusion until this finishes. Your other apps are unaffected.</div>
    </div>
    <script>
      var COLORS={working:'#00b8b1',done:'#3fb950',failed:'#f85149'};
      function apply(d){
        var c=COLORS[d.tone]||COLORS.working;
        document.getElementById('title').textContent=d.title||'Working';
        document.getElementById('detail').textContent=d.detail||'';
        var route=d.routing||{};
        document.getElementById('routing').hidden=!d.routing;
        ['Connection','Plan','Checks','Events'].forEach(function(k){document.getElementById('route'+k).textContent=route[k.toLowerCase()]||'';});
        document.getElementById('fill').style.background=c;
        document.getElementById('dot').style.background=c;
        // a new step (new title) restarts the clock the time-based bar runs on
        if(d.title!==cur.title){ cur.t0=Date.now(); }
        cur.title=d.title; cur.tone=d.tone||'working';
        cur.pct=(d.percent===null||d.percent===undefined)?null:Math.max(0,Math.min(100,parseInt(d.percent,10)||0));
        cur.eta=(d.etaSec===null||d.etaSec===undefined)?null:Math.max(0,+d.etaSec);
        cur.etaText=d.eta||'';
        paint();
        var w=document.getElementById('warn'); w.hidden=(d.tone&&d.tone!=='working'); w.style.borderLeftColor=c;
      }
      var cur={title:null,t0:Date.now(),pct:null,eta:null,etaText:'',tone:'working'};
      function paint(){
        var fill=document.getElementById('fill'), track=document.getElementById('track');
        var pctEl=document.getElementById('pct'), etaEl=document.getElementById('eta');
        if(cur.tone!=='working'){ track.classList.remove('sweep'); fill.style.width='100%%'; pctEl.textContent=cur.tone==='done'?'Done':'Stopped'; etaEl.textContent=''; return; }
        if(cur.pct!==null){ track.classList.remove('sweep'); fill.style.width=cur.pct+'%%'; pctEl.textContent=cur.pct+'%%'; etaEl.textContent=cur.etaText; return; }
        var el=(Date.now()-cur.t0)/1000;
        if(cur.eta){
          // elapsed against the usual time for this step, never past 95%% until it really ends
          track.classList.remove('sweep');
          var p=Math.min(95,Math.round(100*el/cur.eta)); fill.style.width=p+'%%'; pctEl.textContent=p+'%%';
          var left=Math.round(cur.eta-el);
          etaEl.textContent=left>0?(left>=60?('about '+Math.floor(left/60)+'m '+('0'+left%%60).slice(-2)+'s left'):('about '+left+'s left')):'taking longer than usual';
        } else {
          track.classList.add('sweep'); fill.style.width=''; pctEl.textContent='Working';
          etaEl.textContent=Math.round(el)+'s';
        }
      }
      setInterval(paint,1000);
      window.fusionJavaScriptHandler={handle:function(action,data){
        try{ if(action==='update'){ apply(JSON.parse(data||'{}')); } }catch(e){ return 'ERR '+e; }
        return 'OK';
      }};
      try{ apply(%(initial)s); }catch(e){}
      var streamUrl=%(stream_url)s;
      if(streamUrl){
        var stream=new EventSource(streamUrl);
        stream.onmessage=function(e){try{var d=JSON.parse(e.data);apply(d);document.getElementById('streamStatus').textContent='Live from Adom container / event '+d.sequence;fetch('ack',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sequence:d.sequence})});}catch(err){document.getElementById('streamStatus').textContent='Display error: '+err;}};
        stream.onerror=function(){document.getElementById('streamStatus').textContent='Connection interrupted; reconnecting. Results may be stale.';};
      }
    </script></body></html>""" % {"logo": ADOM_LOGO_SVG, "initial": (initial_payload or "{}").replace("</", "<\\/"), "stream_url": __import__("json").dumps(event_stream_url)}


def _payload(title, detail, percent, eta_sec, tone):
    import json as _json
    eta = ""
    if eta_sec is not None:
        m, s = divmod(max(0, int(eta_sec)), 60)
        eta = ("about %dm %02ds left" % (m, s)) if m else ("about %ds left" % s)
    # percent None means the caller does not know it (most verbs: a document opening, a view
    # switching). The page then fills the bar by elapsed time against etaSec, or sweeps it when
    # there is no ETA either. Sending 0 painted "0%" for the whole step (issue #121).
    pct = None if percent is None else max(0, min(100, int(percent)))
    return _json.dumps({"title": str(title or "Working"), "detail": str(detail or ""),
                        "percent": pct, "eta": eta,
                        "etaSec": None if eta_sec is None else max(0, int(eta_sec)),
                        "tone": tone or "working"})


def _write_html(html):
    """Write the banner HTML to a stable temp file; return its path for the palette."""
    import os, tempfile
    d = os.path.join(tempfile.gettempdir(), "adom-fusion-bridge")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, "activity_banner.html")
    with open(path, "w", encoding="utf-8") as f:
        f.write(html)
    # A proper file:/// URL. Verified on hardware 2026-08-28: this renders; and the earlier
    # "invisible palette" was never Fusion's fault - palettes are OWNED POPUP windows that a
    # parent-window capture omits by design. Verify a palette via the capture's
    # screenshots[] popup array, never the main frame.
    return "file:///" + path.replace("\\", "/")


def _esc(s):
    return (str(s or "").replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


def _delete_all_palettes(ui) -> int:
    """Remove every palette that is ours (by id or by the 'Adom' name). Never raises."""
    n = 0
    try:
        for i in range(ui.palettes.count - 1, -1, -1):
            try:
                pal = ui.palettes.item(i)
                if pal.id == PALETTE_ID or str(pal.name or "").strip().lower() == "adom":
                    pal.deleteMe(); n += 1
            except Exception:
                pass
    except Exception:
        pass
    return n


def handle_activity_banner(app: adsk.core.Application, args: dict) -> dict:
    """Show / update / hide the banner. args: {action: show|update|hide, title, detail,
    percent, etaSec, tone: working|done|failed}. Never raises."""
    global _palette
    action = str(args.get("action") or "update").lower()
    try:
        ui = app.userInterface
        if action == "hide":
            _user_closed[0] = False
            # DELETE, do not just hide (John, 2026-08-30, two ADOM palettes stacked in one
            # Fusion): a hidden palette was not found by itemById on the next run, so a
            # second one was created under it. Gone means gone; the next run makes a fresh one.
            _removed = _delete_all_palettes(ui)
            _palette = None
            return {"success": True, "visible": False, "removed": _removed}
        if action == "show":
            _user_closed[0] = False   # a new run starts: the palette is wanted again

        payload = _payload(args.get("title") or "Working", args.get("detail") or "",
                           args.get("percent"), args.get("etaSec"), args.get("tone") or "working")
        # ALWAYS RE-RESOLVE BY ID. A cached handle outlived a deleted palette on hardware
        # (2026-08-28) and every later call operated on a ghost: add() reported success,
        # Fusion's registry said nothing existed, and the retry failed with
        # "InternalValidationError: pArea". Fusion is the source of truth for whether the
        # palette exists; the module variable is only a hint.
        _palette = ui.palettes.itemById(PALETTE_ID)
        if _palette is None:
            # never stack: any leftover palette with our name goes before a new one is made
            _delete_all_palettes(ui)
        created = False
        if _palette is None:
            # The first paint carries the real state (John, 2026-08-30: a palette reading
            # "Working / 0%" for its first seconds is worse than no palette).
            path = _write_html(_shell_html(payload))
            created = True
            _palette = ui.palettes.add(
                PALETTE_ID, "Adom", path,
                True,    # isVisible
                True,    # showCloseButton - the user must be able to get it out of the way
                         # (John, 2026-08-30); a close sticks until the next run's 'show'
                True,    # isResizable
                360, 190)
            try:
                _h = _PaletteClosedHandler()
                _palette.closed.add(_h)
                _handlers.append(_h)
            except Exception:
                pass
            # Dock AFTER add, and verified on hardware: add() then dockingState=Right
            # reads back as 4 (docked right). Setting it any earlier is the pArea error.
            try:
                _palette.dockingState = adsk.core.PaletteDockingStates.PaletteDockStateRight
            except Exception:
                pass
            # Fusion's electronics editors leave it FLOATING (dockingState reads 0 after the
            # set, 2026-09-05) and drop it over the canvas, where it hid the symbol's right pins
            # in the tour and, on a 2026-10-08 recording, the silkscreen being drawn. Park it in
            # the bottom-right corner of Fusion's own window, off the drawing.
            _park_floating(_palette)
        else:
            if _user_closed[0]:
                # The user closed it during this run: honour that, keep the content current
                # (so reopening shows the truth) but never pop it back in their way.
                pass
            elif not _palette.isVisible:
                _palette.isVisible = True
            if action == "show":
                _park_floating(_palette)   # a new run: back to the corner, even if dragged last run
        # DATA, NOT A RELOAD: the page applies the payload in place. Right after creation
        # the page may not have run its script yet; retry briefly, and only if the message
        # never lands fall back to one reload of the shell (never per update).
        delivered = False
        last_err = ""
        import time as _t
        for _try in range(12 if created else 3):
            try:
                r = _palette.sendInfoToHTML("update", payload)
                if str(r or "").startswith("OK"):
                    delivered = True
                    break
                last_err = "handler returned %r" % (str(r)[:60],)
            except Exception as _e:
                last_err = str(_e)[:100]
            _t.sleep(0.25)
        # FAIL-SAFE (John, 2026-08-30, "still seeing this awful palette window that gives me
        # zero information"): if the in-place message does not land, re-render the page WITH
        # the state baked in, but never more than once per 5 s (each re-render costs Fusion's
        # embedded browser a few threads; per-update re-rendering was the 21,900-thread leak).
        # Wrong-but-cheap is not an option; stale is not an option either.
        global _last_reload_ts
        if not delivered and not created:
            now = _t.time()
            if now - _last_reload_ts >= 5.0:
                try:
                    _palette.htmlFileURL = _write_html(_shell_html(payload))
                    _last_reload_ts = now
                    delivered = True
                    last_err = "re-rendered: " + last_err
                except Exception as _e:
                    last_err = "re-render failed: %s" % str(_e)[:80]
        # Report what FUSION says, not what we intended.
        q = ui.palettes.itemById(PALETTE_ID)
        return {"success": q is not None, "visible": bool(q.isVisible) if q else False,
                "dockingState": int(q.dockingState) if q else None,
                "delivered": delivered, "created": created, "userClosed": _user_closed[0],
                "deliveryError": (last_err or None)}
    except Exception as e:
        # Advisory only: a banner that cannot be drawn must never fail the import.
        return {"success": False, "error": "activity banner unavailable: %s" % str(e)[:160],
                "advisory": True}


def _park_floating(palette):
    """Move a FLOATING palette to the bottom-right corner of the Fusion window that owns it.
    Never raises.

    Measured on hardware (arav-rog, 1.11.40, 2026-10-17), the two API calls the old code relied
    on do not work for a floating electronics palette:
    - setSize is ignored, and so are setMinimumSize/setMaximumSize: the window stays 380x410
      logical whatever is asked, and a Win32 resize snaps back at once. So the size is a given
      and only the position is ours.
    - setPosition throws InternalValidationError (setAreaPosition) for a negative X, which is
      every position on a monitor left of the primary one. The old math also aimed at the
      PRIMARY screen (GetSystemMetrics), not the screen Fusion is on.
    So: find the palette's own top-level window (titled ADOM, in this process), read its owner's
    rect, and SetWindowPos it there without resizing. Both rects come from the same Win32 calls
    in the same process, so they share one coordinate space whatever the DPI setup."""
    try:
        if int(palette.dockingState) != 0:
            return
        palette.setSize(260, 120)   # harmless where ignored; honoured by builds that allow it
        import ctypes
        from ctypes import wintypes
        u = ctypes.windll.user32
        pid = ctypes.windll.kernel32.GetCurrentProcessId()
        found = []

        @ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
        def _cb(h, _l):
            p = wintypes.DWORD()
            u.GetWindowThreadProcessId(h, ctypes.byref(p))
            if p.value == pid and u.IsWindowVisible(h):
                b = ctypes.create_unicode_buffer(64)
                u.GetWindowTextW(h, b, 64)
                if b.value.strip().lower() == "adom":
                    found.append(h)
            return True
        u.EnumWindows(_cb, 0)
        if not found:
            return
        h = found[0]
        owner = u.GetWindow(h, 4)   # GW_OWNER: the Fusion main window
        if not owner:
            return

        def _rect(w):
            r = wintypes.RECT()
            u.GetWindowRect(w, ctypes.byref(r))
            return r.left, r.top, r.right, r.bottom
        pl, pt, pr, pb = _rect(h)
        ol, ot, orr, ob = _rect(owner)
        w, hh = pr - pl, pb - pt
        x = max(ol, orr - w - 16)
        y = max(ot, ob - hh - 48)   # clear of Fusion's bottom toolbar
        u.SetWindowPos(h, 0, x, y, 0, 0, 0x0001 | 0x0004 | 0x0010)   # NOSIZE | NOZORDER | NOACTIVATE
    except Exception:
        pass


def teardown():
    """Called from the add-in's stop(): remove the palette so a reload does not stack them."""
    global _palette
    try:
        if _palette:
            _palette.deleteMe()
    except Exception:
        pass
    _palette = None
