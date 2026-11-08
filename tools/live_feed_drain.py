#!/usr/bin/env python3
"""Drain fusion_live_feed's box-side ring buffer into shotlog, from the container.

    python3 tools/live_feed_drain.py --target AdomLapper --channel fusion-live [--interval 1.0]

Publishes each frame with a kind tag so a viewer never confuses them:
  [main #n]   the Fusion window each tick (the ~1 fps video)
  [popup #n.m] each owned dialog/panel over it, tied to its tick
Proof frames are NOT here; they come from fusion_prove_result evidence[] (kind:proof).
"""
print = __import__("functools").partial(print, flush=True)
import argparse, json, subprocess, sys, time, os, base64

def ab(target, verb, args):
    p = subprocess.run(["adom-bridge", "--ai-thread", "fusion-live-feed", "--target", target, verb, json.dumps(args)],
                       capture_output=True, text=True, timeout=90)
    try:
        d = json.loads(p.stdout); x = d.get("data") or d
        if isinstance(x.get("output"), str):
            try: x = json.loads(x["output"])
            except Exception: pass
        return x
    except Exception:
        return {}

def main():
    a = argparse.ArgumentParser(); a.add_argument("--target", required=True); a.add_argument("--channel", default="fusion-live")
    a.add_argument("--interval", type=float, default=1.0); a.add_argument("--max-sec", type=float, default=600)
    o = a.parse_args()
    st = ab(o.target, "fusion_live_feed", {"action": "start", "intervalSec": o.interval, "maxSec": o.max_sec})
    print("feed:", "started" if st.get("started") else ("already running" if st.get("alreadyRunning") else st))
    since, t0, pushed, spins = 0, time.time(), 0, 0
    os.makedirs("/tmp/live_feed", exist_ok=True)
    while time.time() - t0 < o.max_sec:
        r = ab(o.target, "fusion_live_feed", {"action": "frames", "since": since})
        if r.get("error") or r.get("errorCode"):
            print("feed call failed:", r.get("error") or r.get("errorCode"), (r.get("_hint") or "")[:120])
        # HONESTY: the box tells us when it is spinning without capturing. Say it, do not hide it.
        if r.get("healthy") is False:
            spins += 1
            if spins in (1, 10) or spins % 60 == 0:
                print("FEED NOT LIVE (tick %s, captured %s): %s" % (r.get("tick"), r.get("captured"), r.get("_honesty")))
        for f in r.get("frames") or []:
            since = max(since, f["tick"])
            rf = ab(o.target, "read_file", {"path": f["path"], "encoding": "base64"})
            c = rf.get("content") or ""
            if not c: continue
            local = "/tmp/live_feed/%s_%d_%s.png" % (f["kind"], f["tick"], f.get("index", 0))
            try: open(local, "wb").write(base64.b64decode(c))
            except Exception: continue
            desc = ("[main #%d] %s | %d popup(s)" % (f["tick"], f.get("title", ""), f.get("popups", 0)) if f["kind"] == "main"
                    else "[popup #%d.%d] %s" % (f["tick"], f.get("index", 0), f.get("title", "")))
            subprocess.run(["adom-shotlog", "inject", "-c", o.channel, "-d", desc, "-s", "desktop_screenshot_window", "--native", local],
                           capture_output=True, text=True, timeout=30)
            pushed += 1
            if pushed in (1, 10) or pushed % 100 == 0:
                print("pushed %d frame(s); latest tick %d" % (pushed, since))
        if not r.get("running") and since: print("feed ended; pushed", pushed); break
        time.sleep(o.interval)
    ab(o.target, "fusion_live_feed", {"action": "stop"})

if __name__ == "__main__": main()
