"""Measured timing history + the `progress` contract block (page issue #32).

John, 2026-08-19: a slow verb must never answer "can take a couple of minutes".
That string is a shrug - not measured, never narrowing, impossible to draw a bar
from. This module is the other half of the fix: it keeps a ROLLING MEASURED
HISTORY per (phase, startKind) on THIS machine, so an estimate is the p50 of what
actually happened here rather than a literal someone typed once.

Shape returned by `progress_block()` is exactly the contract in issue #32:

    {"phase","stepLabel","percent","elapsedSec","estimatedSec","etaSec",
     "confidence","startKind"}

Rules the callers rely on:
  - `percent` is MONOTONIC (enforced here via `floor`), never walks backwards.
  - `estimatedSec` is REVISED as the run proceeds: once elapsed passes the
    estimate, the estimate grows rather than pinning eta at 0 and lying.
  - `confidence` is "measured" ONLY when this machine has real samples for that
    phase; "typical" when we are running on a seeded literal; "unknown" when we
    genuinely cannot say.

Storage: ~/.adom/fusion-bridge/timings.json, last _KEEP samples per key, written
atomically. A corrupt or unwritable store degrades to in-memory - timing history
must never be able to break a launch.
"""

import json
import os
import pathlib
import threading
import time

_KEEP = 10                      # rolling samples per (phase, startKind)
_LOCK = threading.Lock()
_MEM = {}                       # fallback when the file cannot be read/written


def _path():
    d = pathlib.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge"
    try:
        d.mkdir(parents=True, exist_ok=True)
    except Exception:
        pass
    return d / "timings.json"


def _load():
    try:
        with open(_path(), "r", encoding="utf-8") as f:
            d = json.load(f)
        return d if isinstance(d, dict) else {}
    except Exception:
        return dict(_MEM)


def _save(d):
    global _MEM
    _MEM = dict(d)
    try:
        p = _path()
        tmp = str(p) + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(d, f, indent=1)
        os.replace(tmp, str(p))
    except Exception:
        pass                    # in-memory copy still serves this process


def _key(phase, start_kind=None):
    return "%s|%s" % (phase, start_kind or "any")


def record(phase, seconds, start_kind=None):
    """Add one MEASURED sample. Call it on success AND on a clean failure that
    still consumed real time - both are truth about how long this box takes."""
    try:
        seconds = float(seconds)
    except Exception:
        return
    if seconds < 0 or seconds > 7200:
        return                  # a clock jump is not a sample
    with _LOCK:
        d = _load()
        k = _key(phase, start_kind)
        row = d.get(k) or {}
        samples = [s for s in (row.get("samples") or []) if isinstance(s, (int, float))]
        samples.append(round(seconds, 1))
        row["samples"] = samples[-_KEEP:]
        row["lastAt"] = int(time.time())
        d[k] = row
        _save(d)


def _pct(samples, q):
    if not samples:
        return None
    s = sorted(samples)
    i = min(len(s) - 1, max(0, int(round(q * (len(s) - 1)))))
    return s[i]


def stats(phase, start_kind=None):
    """{'p50','p90','count'} of real samples on THIS machine, or count 0."""
    row = _load().get(_key(phase, start_kind)) or {}
    s = [x for x in (row.get("samples") or []) if isinstance(x, (int, float))]
    return {"p50": _pct(s, 0.5), "p90": _pct(s, 0.9), "count": len(s)}


def estimate(phase, fallback, start_kind=None):
    """Seconds this phase should take here: measured p50 when we have samples,
    else the caller's seeded literal. Returns (seconds, confidence)."""
    st = stats(phase, start_kind)
    if st["count"] >= 2 and st["p50"]:
        return float(st["p50"]), "measured"
    if st["count"] == 1 and st["p50"]:
        # one sample beats a literal but is not yet a distribution
        return float(st["p50"]), "typical"
    try:
        return float(fallback), "typical"
    except Exception:
        return 0.0, "unknown"


def progress_block(phase, started, step_label, percent=None, start_kind=None,
                   fallback=None, floor=None):
    """Build the issue-#32 progress block. `started` is a time.time() stamp.

    percent=None derives a percentage from elapsed/estimate - honest for a phase
    with no internal checkpoints, and it still moves, which is the whole point.
    `floor` keeps it monotonic across polls (pass the last percent you reported).
    """
    elapsed = max(0.0, time.time() - float(started))
    est0, conf = estimate(phase, fallback if fallback is not None else 0, start_kind)
    est = est0
    overdue = bool(est0 and elapsed > est0)
    # REVISE rather than lie: a run that outlives its estimate gets a bigger estimate (and
    # a non-zero eta), never a bar pinned at 100% or an eta of 0.
    #
    # ⚠️ BUT A REVISED ESTIMATE MUST NOT FREEZE THE BAR (caught live 2026-08-19): the first
    # cut recomputed est = elapsed * 1.25, which makes percent EXACTLY 80 forever while the
    # eta grows without bound - a spinner with no end wearing a progress bar's clothes,
    # which is the very thing issue #32 exists to kill. So past the estimate the bar CREEPS
    # asymptotically toward 95 (never reaching it, because we genuinely do not know), and
    # once we are past twice what this machine has ever taken we stop claiming to know at
    # all: confidence drops to "unknown" and `overdue` tells the caller to say so out loud.
    if overdue:
        est = elapsed * 1.25
        if conf == "measured":
            conf = "typical"    # we are past what this machine has ever measured
        if elapsed > 2 * est0:
            conf = "unknown"
    if percent is None:
        if not est0:
            percent = 0
        elif not overdue:
            # Runs 0 -> 80 across the estimate. It deliberately does NOT reach 100: the
            # estimate is a p50, so half of all runs are still going at this point.
            percent = int(80 * elapsed / est0)
        else:
            # Continues 80 -> 95 from exactly where the branch above ended. The two
            # formulas MUST meet at elapsed == est0, or percent walks BACKWARDS at the
            # crossover - which it did (91% then 84% on a live poll, 2026-08-19), breaking
            # the contract's monotonic rule with the very code meant to honour it.
            over = elapsed - est0
            percent = int(80 + 15 * (over / (over + est0)))     # 80 -> 95, never 100
    percent = max(0, min(99 if est else 0, int(percent)))
    if floor is not None:
        percent = max(int(floor), percent)
    eta = max(0, int(round(est - elapsed))) if est else None
    return {
        "phase": phase,
        "stepLabel": step_label,
        "percent": percent,
        "elapsedSec": int(round(elapsed)),
        "estimatedSec": int(round(est)) if est else None,
        "etaSec": eta,
        "confidence": conf if est else "unknown",
        "startKind": start_kind,
        "overdue": overdue,
    }


def terminal_block(phase, started, step_label, start_kind=None, failed=False):
    """The completion frame: percent 100 (or the phase it died in) + the ACTUALS,
    so a caller can log truth and a bar can turn red in the right place."""
    elapsed = max(0.0, time.time() - float(started))
    return {
        "phase": phase,
        "stepLabel": step_label,
        "percent": 100 if not failed else None,
        "elapsedSec": int(round(elapsed)),
        "estimatedSec": int(round(elapsed)),
        "etaSec": 0 if not failed else None,
        "confidence": "measured",
        "startKind": start_kind,
        "failed": bool(failed),
        "measured": {"phase": phase, "seconds": round(elapsed, 1), "startKind": start_kind},
    }


def summary():
    """Per-machine history for describe/status: what a caller can tell the user
    BEFORE they click ("typically 45s cold, 3s warm on THIS machine")."""
    d = _load()
    out = {}
    for k, row in d.items():
        phase, _, kind = k.partition("|")
        s = [x for x in (row.get("samples") or []) if isinstance(x, (int, float))]
        if not s:
            continue
        out.setdefault(phase, {})[kind] = {
            "p50": _pct(s, 0.5), "p90": _pct(s, 0.9), "count": len(s),
            "lastAt": row.get("lastAt"),
        }
    return out


def sentence():
    """One human line for a UI/hint, built only from what we actually measured."""
    s = summary().get("launch") or {}
    bits = []
    for kind in ("cold", "warm"):
        if s.get(kind, {}).get("p50") is not None:
            bits.append("%s %ss" % (kind, int(s[kind]["p50"])))
    if not bits:
        return None
    return "on this machine Fusion starts in " + ", ".join(bits) + " (measured)"


# ── Per-VERB run times ─────────────────────────────────────────────────────────────────
# John, 2026-08-21: "you need to start tracking the length of time it takes to run verbs
# and possibly ... store times in the log and analyze the log later to calc run times".
#
# Two stores on purpose, because they answer different questions:
#   - the ROLLING window above (last _KEEP samples) drives the live estimate: recent, cheap,
#     and it tracks a machine that got slower or faster.
#   - the APPEND-ONLY LOG below is the historical record: every run with its outcome, so a
#     question nobody thought to ask up front ("did the 3D package regress after 1.9.9x?",
#     "which verb owns the 546s?") is answerable later instead of lost.
# A rolling window can only answer what it still remembers; a log can be re-analysed.

_RUNLOG_MAX_BYTES = 4 * 1024 * 1024


def _runlog_path():
    return _path().parent / "verb-times.jsonl"


def log_run(verb: str, seconds: float, ok: bool = True, note: str = "") -> None:
    """Append one verb run to the historical log AND the rolling window. Never raises."""
    try:
        record("verb:" + verb, seconds)
        p = _runlog_path()
        try:
            if p.exists() and p.stat().st_size > _RUNLOG_MAX_BYTES:
                p.replace(str(p) + ".1")        # one generation back, never unbounded
        except Exception:
            pass
        with open(p, "a", encoding="utf-8") as f:
            f.write(json.dumps({"at": int(time.time()), "verb": verb,
                                "seconds": round(float(seconds), 2),
                                "ok": bool(ok), "note": note[:120]}) + "\n")
    except Exception:
        pass


def verb_estimate(verb: str):
    """(seconds, confidence) for a verb from its rolling window. (0,'unknown') if new."""
    st = stats("verb:" + verb)
    if st["count"] >= 3 and st["p50"]:
        return float(st["p50"]), "measured"
    if st["count"] >= 1 and st["p50"]:
        return float(st["p50"]), "typical"
    return 0.0, "unknown"


def verb_report(limit: int = 40) -> dict:
    """Aggregate the historical log: per verb count / p50 / p90 / max / failures.

    This is the 'analyse the log later' half. Cheap enough to expose from a verb, so the
    numbers are readable from the Dashboard instead of requiring someone to SSH in.
    """
    runs = {}
    try:
        with open(_runlog_path(), "r", encoding="utf-8") as f:
            for line in f:
                try:
                    r = json.loads(line)
                except Exception:
                    continue
                d = runs.setdefault(r.get("verb", "?"), {"secs": [], "fail": 0})
                d["secs"].append(float(r.get("seconds") or 0))
                if not r.get("ok"):
                    d["fail"] += 1
    except Exception:
        return {}
    out = {}
    for verb, d in runs.items():
        s = d["secs"]
        out[verb] = {"count": len(s), "p50": _pct(s, 0.5), "p90": _pct(s, 0.9),
                     "max": max(s) if s else None, "failures": d["fail"]}
    return dict(sorted(out.items(), key=lambda kv: -(kv[1]["p90"] or 0))[:limit])
