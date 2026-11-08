#!/usr/bin/env python3
"""Publish gate for an EAGLE .lbr: the checks `adom-lbr validate` does not do.
Usage: lbrgate.py <file.lbr> [...]    exit 0 = every file publishable."""
import sys, xml.etree.ElementTree as ET

def gate(path):
    bad = []
    r = ET.parse(path).getroot()
    syms = {s.get("name"): {p.get("name") for p in s.iter("pin")}
            for s in r.iter("symbol")}
    pkgs = {p.get("name"): {e.get("name") for e in list(p) if e.tag in ("smd", "pad")}
            for p in r.iter("package")}

    if not pkgs:                                            # 1. empty <packages>
        bad.append("no packages: the .lbr carries a symbol but no footprint")

    for ds in r.iter("deviceset"):
        dsn = ds.get("name")
        gates = {g.get("name"): g.get("symbol") for g in ds.iter("gate")}
        for dev in ds.iter("device"):
            pkg, cons = dev.get("package"), list(dev.iter("connect"))
            if not cons:                                    # 2. no connects
                bad.append("%s/%s: no connects" % (dsn, dev.get("name") or "''"))
                continue
            pads = pkgs.get(pkg, set())
            used = set()
            for c in cons:                                  # 3. dangling refs
                pin, pad = c.get("pin"), c.get("pad")
                spins = syms.get(gates.get(c.get("gate")), set())
                if spins and pin not in spins:
                    bad.append("%s: pin %r not in symbol %r" % (dsn, pin, gates.get(c.get("gate"))))
                if pad not in pads:
                    bad.append("%s: pin %r -> pad %r, no such pad in package %r (unused pads: %s)"
                               % (dsn, pin, pad, pkg, ", ".join(sorted(pads - used)) or "none"))
                used.add(pad)
            for g, sym in gates.items():                    # 5. pin coverage
                miss = syms.get(sym, set()) - {c.get("pin") for c in cons if c.get("gate") == g}
                if miss:
                    bad.append("%s: %d symbol pin(s) never connected: %s"
                               % (dsn, len(miss), ", ".join(sorted(miss))))

    for p3 in r.iter("package3d"):                          # 4. account-scoped 3D
        bad.append("package3d %r carries a wip_urn: account-scoped cloud state, do not publish"
                   % (p3.get("name") or p3.get("urn") or ""))
    return bad

rc = 0
for f in sys.argv[1:]:
    try:
        bad = gate(f)
    except Exception as e:
        bad = ["unparseable: %s" % e]
    print(("PASS  " if not bad else "FAIL  ") + f)
    for b in bad:
        print("        " + b)
    rc |= bool(bad)
sys.exit(rc)
