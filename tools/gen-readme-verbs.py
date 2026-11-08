#!/usr/bin/env python3
"""Rewrite the "Full verb catalog" section of README.md from contracts/describe.json.

Usage: python3 tools/gen-readme-verbs.py [--check]

The section runs from the line "## Full verb catalog" up to, not including, the next line
that starts with "## ". The intro line is kept, the count is real, and the verbs are grouped
into families by keyword rules on the verb name (first matching rule wins). Running it twice
changes nothing. The second count in the README (the "N verbs - `fusion_describe" line in the
Install section) is kept in step too. --check exits 1 when the README is out of date and writes nothing.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
README = os.path.join(ROOT, "README.md")
CONTRACT = os.path.join(ROOT, "contracts", "describe.json")
HEADING = "## Full verb catalog"
DEFAULT_INTRO = (
    "All verbs return a rich `_hint`. `fusion_describe '{}'` is the machine-readable source "
    "of truth; this table is generated from it."
)
MAX_LEN = 160
MIN_LEN = 40   # a shorter first sentence ("START HERE for a schematic.") takes the next one too

# (family, keywords). A keyword that starts with "=" must equal the verb name without its
# "fusion_" prefix; any other keyword is a substring test. Order matters: first match wins.
RULES = [
    ("Demo and narration", ["demo", "narrate"]),
    ("Sign-in and seat", ["signin", "seat", "_browser", "client_id"]),
    ("Design rules and manufacturing",
     ["design_rules", "detect_layers", "stackup", "=export_gerbers", "=export_bom", "=export_cpl"]),
    ("Exports", ["export_", "optimized_glb"]),
    ("Dialogs and recovery",
     ["dialog", "close_", "recovery", "restart", "alerts", "action_ledger",
      "notify_owner", "reset_workspace", "cancel_run"]),
    ("Cloud files and APS", ["aps_", "cloud", "by_urn"]),
    ("Libraries and 3D packages",
     ["lbr", "library", "package", "symbol", "footprint", "attributes", "prove_result"]),
    ("Schematic, board and routing",
     ["schematic", "board", "rout", "electron", "eagle", "test_project", "showcase",
      "show_project"]),
    ("View, state and capture",
     ["screenshot", "=state", "=view", "live_feed", "interactive", "document", "driving_badge",
      "window_sizing"]),
    ("Lifecycle and readiness",
     ["=start", "=stop", "=kill", "readiness", "addin_status", "install", "update",
      "describe", "verb_times", "app_state"]),
    ("Modeling and parameters",
     ["modeling", "parameter", "bodies", "design_info", "physical_properties", "assembly_bom",
      "import_step", "open_design"]),
]
FAMILY_ORDER = [
    "Lifecycle and readiness", "Sign-in and seat", "Cloud files and APS",
    "Libraries and 3D packages", "Schematic, board and routing",
    "Design rules and manufacturing", "Exports", "View, state and capture",
    "Dialogs and recovery", "Modeling and parameters", "Demo and narration", "Other",
]


def family_of(name):
    short = name[len("fusion_"):] if name.startswith("fusion_") else name
    for family, keys in RULES:
        for key in keys:
            if key.startswith("="):
                if short == key[1:]:
                    return family
            elif key in short:
                return family
    return "Other"


# Non-ASCII characters are written as code points so this file stays plain ASCII.
EM_DASH, EN_DASH, ARROW, ELLIPSIS = chr(0x2014), chr(0x2013), chr(0x2192), chr(0x2026)
QUOTES = {chr(0x2018): "'", chr(0x2019): "'", chr(0x201C): '"', chr(0x201D): '"'}


def clean(text):
    text = " ".join((text or "").split())
    text = re.sub(r"\s*" + EM_DASH + r"\s*", ", ", text)
    text = text.replace(EN_DASH, "-").replace(ARROW, "->").replace(ELLIPSIS, "...")
    for fancy, plain in QUOTES.items():
        text = text.replace(fancy, plain)
    text = "".join(ch for ch in text if ord(ch) < 0x2000)   # symbols and emoji
    return " ".join(text.split())


def first_sentence(text):
    text = clean(text)
    for m in re.finditer(r"[.!?](?=\s)", text):
        head = text[:m.start()]
        if re.search(r"(e\.g|i\.e|vs|etc|\s[A-Za-z])$", head):
            continue
        if m.end() < MIN_LEN:
            continue
        text = text[:m.end()]
        break
    if len(text) > MAX_LEN:
        cut = text[:MAX_LEN].rsplit(" ", 1)[0].rstrip(",;:( ")
        text = cut + "..."
    return text


def build_section(verbs, intro):
    groups = {}
    for verb in verbs:
        groups.setdefault(family_of(verb["name"]), []).append(verb)
    out = [HEADING, "", intro,
           "_%d verbs._ Regenerate with `python3 tools/gen-readme-verbs.py`." % len(verbs), ""]
    for family in FAMILY_ORDER:
        rows = sorted(groups.get(family, []), key=lambda v: v["name"])
        if not rows:
            continue
        out.append("**%s**" % family)
        out.append("")
        for verb in rows:
            out.append("- `%s` - %s" % (verb["name"], first_sentence(verb.get("summary"))))
        out.append("")
    return out


def main():
    check = "--check" in sys.argv[1:]
    with open(CONTRACT, encoding="utf-8") as fh:
        verbs = json.load(fh)["verbs"]
    with open(README, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    try:
        start = next(i for i, line in enumerate(lines) if line.strip() == HEADING)
    except StopIteration:
        sys.exit("README.md has no '%s' section" % HEADING)
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
               len(lines))
    intro = next((l for l in lines[start + 1:end] if l.strip()), DEFAULT_INTRO)
    if intro.startswith(("_", "*", "-")):
        intro = DEFAULT_INTRO
    tail = re.sub(r"\b\d+ verbs(?= -\s+`fusion_describe)", "%d verbs" % len(verbs),
                  "\n".join(lines[end:])).split("\n")
    new_lines = lines[:start] + build_section(verbs, intro) + tail
    changed = new_lines != lines
    if changed and not check:
        with open(README, "w", encoding="utf-8") as fh:
            fh.write("\n".join(new_lines))
    print("%d verbs, README.md %s" % (
        len(verbs), ("out of date" if check else "updated") if changed else "unchanged"))
    if check and changed:
        sys.exit(1)


if __name__ == "__main__":
    main()
