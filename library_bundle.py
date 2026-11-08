"""Cloud Library bundle -> parts[] for the fusion_install_library_bundle composite (#68).

Pure helpers, no Fusion, no bridge state: unpack a staged zip safely, find the .lbr and its
STEP models, place them under a stable library folder, and pair every deviceset with its
model by name. server.py wires the result into the existing create_library chain, so the
bundle lane runs the identical convert -> bind -> prove code path as every other import.

Bundle shapes accepted (a wrapping top-level folder is fine):
  <Lib>.lbr + <Part>.step ...              (beside it, or under 3dmodels/ or models/)
  a bare .lbr staged directly (lbrPath)     with its STEPs as siblings
"""
import os
import re
import shutil
import zipfile

MAX_ZIP_BYTES = 256 * 1024 * 1024
MAX_UNPACKED_BYTES = 512 * 1024 * 1024
_NAME_OK = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}$")
_MODEL_EXT = (".step", ".stp")


def safe_extract(zip_path: str, dest: str) -> int:
    """Extract with a zip-slip guard and an unpacked-size cap. Returns files extracted."""
    total = count = 0
    dest = os.path.realpath(dest)
    with zipfile.ZipFile(zip_path) as zf:
        for info in zf.infolist():
            name = info.filename
            if not name or name.endswith("/"):
                continue
            norm = name.replace("\\", "/")
            if norm.startswith("/") or re.match(r"^[A-Za-z]:", norm) or ".." in norm.split("/"):
                raise ValueError("unsafe zip entry: %s" % name)
            total += info.file_size
            if total > MAX_UNPACKED_BYTES:
                raise ValueError("bundle unpacks past the %d MB cap" % (MAX_UNPACKED_BYTES // 2 ** 20))
            target = os.path.realpath(os.path.join(dest, norm))
            if not target.startswith(dest):
                raise ValueError("unsafe zip entry: %s" % name)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with zf.open(info) as src, open(target, "wb") as out:
                shutil.copyfileobj(src, out)
            count += 1
    return count


def find_bundle_files(root: str):
    """The shallowest .lbr wins; models are every .step/.stp anywhere in the bundle."""
    lbrs, models = [], []
    for d, _dirs, files in os.walk(root):
        for f in files:
            p = os.path.join(d, f)
            if f.lower().endswith(".lbr"):
                lbrs.append(p)
            elif f.lower().endswith(_MODEL_EXT):
                models.append(p)
    lbrs.sort(key=lambda p: (p.count(os.sep), p))
    return (lbrs[0] if lbrs else None), sorted(models), lbrs


def _stem(p: str) -> str:
    return os.path.splitext(os.path.basename(p))[0]


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]", "", (s or "").lower())


def pair_parts(lbr_path: str, models: list, metadata: dict) -> dict:
    """Pair every deviceset in the .lbr with a STEP by name.

    Match order per deviceset: exact deviceset name, exact package name, then a normalised
    (alnum-only, case-folded) match on either, then a normalised prefix match. When the
    library has exactly one deviceset and the bundle exactly one model, they pair regardless
    of name (a one-part library is N=1, not a special case). Devicesets that share an already
    paired PACKAGE are not unbound: they are listed in shared[] (the package3d is per package and
    the binding pass covers every device on it). Returns {parts:[{package, lbrPath, modelPath,
    matchedBy}], unbound:[{package, why}], shared:[{deviceset, package, via}], models}.
    """
    # ONE PART PER (deviceset, PACKAGE), not per deviceset (John, 2026-08-30): a deviceset
    # may carry package VARIANTS (LAN7800 "", "-HV", "-SB" on QFN-48_7x7mm, -HV, -SB), each
    # bound to its own STEP so Change Package swaps footprint and 3D together. The 3D package
    # is per PACKAGE, so every distinct package is its own mint; devicesets sharing a package
    # still share the mint (shared[]).
    units = []          # (deviceset, deviceName, package)
    devicesets = []
    for d in metadata.get("devicesets") or []:
        ds = d.get("name") or ""
        devicesets.append(ds)
        seen = set()
        for v in d.get("variants") or []:
            pk = v.get("package") or ""
            if not pk or pk in seen:
                continue
            seen.add(pk)
            units.append((ds, str(v.get("device") or ""), pk))
        if not seen:
            units.append((ds, "", ""))
    packages = {ds: next((pk for (d2, _n, pk) in units if d2 == ds and pk), "") for ds in devicesets}
    by_stem = {_stem(m): m for m in models}
    by_norm = {_norm(_stem(m)): m for m in models}
    parts, unbound, used = [], [], set()

    def _take(m, how, ds, pk="", dev=""):
        used.add(m)
        parts.append({"deviceset": ds, "device": dev, "package": pk or packages.get(ds) or ds,
                      "lbrPath": lbr_path, "modelPath": m, "matchedBy": how})

    shared = []
    bound_pkgs = {}
    for (ds, dev, pk) in units:
        if pk and pk in bound_pkgs:
            # Several devicesets on ONE package (symbol variants, value clones): the 3D package
            # is per PACKAGE, so the mint done for the first deviceset serves all of them and the
            # binding pass writes the urn into every device that uses the package.
            shared.append({"deviceset": ds, "package": pk, "via": bound_pkgs[pk]})
            continue
        # Match order: package name (the variant's own identity), deviceset+device name
        # (LAN7800-HV), then the bare deviceset (baseline device only).
        cands = []
        if pk:
            cands.append((pk, "package"))
        if dev:
            cands.append((ds + dev, "deviceset+device"))
        else:
            cands.append((ds, "deviceset"))
        hit = None
        for name, how in cands:
            if name in by_stem and by_stem[name] not in used:
                hit = (by_stem[name], how); break
        if not hit:
            for name, how in cands:
                n = _norm(name)
                if n and n in by_norm and by_norm[n] not in used:
                    hit = (by_norm[n], how + "-normalised"); break
        if not hit:
            for name, how in cands:
                n = _norm(name)
                for k, m in by_norm.items():
                    if n and m not in used and (k.startswith(n) or n.startswith(k)) and len(n) >= 4:
                        hit = (m, how + "-prefix"); break
                if hit:
                    break
        if hit:
            _take(hit[0], hit[1], ds, pk, dev)
            if pk:
                bound_pkgs[pk] = ds
        else:
            unbound.append({"package": pk or ds, "deviceset": ds, "device": dev,
                            "why": "no STEP in the bundle named like the package %r" % pk
                                   + (" or the device %r" % (ds + dev))})
    if not parts and len(units) == 1 and len(models) == 1:
        unbound.clear()
        _take(models[0], "only-one-of-each", units[0][0], units[0][2], units[0][1])
    return {"parts": parts, "unbound": unbound, "shared": shared, "models": models,
            "modelsUnused": [m for m in models if m not in used]}


def library_name_ok(name: str) -> bool:
    return bool(_NAME_OK.match(name or ""))


def place_library(lbr_path: str, models: list, dest_dir: str) -> dict:
    """Copy the .lbr and its models to the stable install folder; return the new paths."""
    os.makedirs(dest_dir, exist_ok=True)
    new_lbr = os.path.join(dest_dir, os.path.basename(lbr_path))
    shutil.copyfile(lbr_path, new_lbr)
    new_models = {}
    for m in models:
        t = os.path.join(dest_dir, os.path.basename(m))
        if os.path.realpath(m) != os.path.realpath(t):
            shutil.copyfile(m, t)
        new_models[m] = t
    return {"lbrPath": new_lbr.replace("\\", "/"),
            "models": {k: v.replace("\\", "/") for k, v in new_models.items()}}


def split_per_part(lbr_path: str, out_dir: str) -> dict:
    """One library per deviceset, so Create3DPackage loads THAT part's footprint.

    Fusion's Package3D generator loads the FIRST package of the open library whatever path the
    command names, so minting three parts against one merged library put every chip on the
    first part's footprint (John saw it, 2026-08-29). Each per-part file keeps the merged file's
    header and layers and carries exactly: the packages that deviceset's devices use (its
    primary package first), the symbols its gates use, and the deviceset itself.
    Returns {deviceset: {"lbrPath", "package", "symbols"}}.
    """
    import copy
    import xml.etree.ElementTree as ET
    os.makedirs(out_dir, exist_ok=True)
    tree = ET.parse(lbr_path)
    root = tree.getroot()
    lib = root.find(".//library")
    if lib is None:
        raise ValueError("no <library> in %s" % lbr_path)
    pk_all = {p.get("name"): p for p in lib.findall("./packages/package")}
    sy_all = {sy.get("name"): sy for sy in lib.findall("./symbols/symbol")}
    out = {}
    for ds in lib.findall("./devicesets/deviceset"):
        name = ds.get("name") or ""
        pkgs = []
        for dev in ds.findall("./devices/device"):
            pk = dev.get("package")
            if pk and pk not in pkgs:
                pkgs.append(pk)
        syms = []
        for g in ds.findall("./gates/gate"):
            sy = g.get("symbol")
            if sy and sy not in syms:
                syms.append(sy)
        # ONE FILE PER (deviceset, PACKAGE) (John, 2026-08-30): a deviceset with package
        # VARIANTS (LAN7800 "", "-HV", "-SB") needs each variant minted on a library whose
        # FIRST (and only) package is THAT variant's footprint, with only the devices that
        # use it. out[deviceset] keeps pointing at the primary package's file for callers
        # that think per deviceset; out[deviceset + "::" + package] is the per-variant key.
        for which, only_pk in [(None, None)] + [(pk, pk) for pk in pkgs]:
            new_root = copy.deepcopy(root)
            new_lib = new_root.find(".//library")
            for tag in ("packages", "symbols", "devicesets", "packages3d"):
                el = new_lib.find(tag)
                if el is not None:
                    new_lib.remove(el)
            use_pkgs = [only_pk] if only_pk else pkgs
            pe = ET.SubElement(new_lib, "packages")
            for pk in use_pkgs:
                if pk in pk_all:
                    pe.append(copy.deepcopy(pk_all[pk]))
            se = ET.SubElement(new_lib, "symbols")
            for sy in syms:
                if sy in sy_all:
                    se.append(copy.deepcopy(sy_all[sy]))
            de = ET.SubElement(new_lib, "devicesets")
            ds2 = copy.deepcopy(ds)
            devs = ds2.find("./devices")
            for dev in list(ds2.findall("./devices/device")):
                for inst in dev.findall("package3dinstances"):
                    dev.remove(inst)
                if only_pk and dev.get("package") != only_pk and devs is not None:
                    devs.remove(dev)
            de.append(ds2)
            # EAGLE order inside <library>: description?, packages, symbols, devicesets
            children = list(new_lib)
            head = [c for c in children if c.tag not in ("packages", "symbols", "devicesets")]
            for c in children:
                new_lib.remove(c)
            for c in head + [pe, se, de]:
                new_lib.append(c)
            safe = re.sub(r"[^A-Za-z0-9._-]+", "_", name) or "part"
            if only_pk:
                safe += "__" + (re.sub(r"[^A-Za-z0-9._-]+", "_", only_pk) or "pkg")
            fp = os.path.join(out_dir, safe + ".lbr").replace("\\", "/")
            ET.ElementTree(new_root).write(fp, encoding="utf-8", xml_declaration=True)
            entry = {"lbrPath": fp, "package": (only_pk or (pkgs[0] if pkgs else "")),
                     "symbols": syms, "packages": use_pkgs}
            if only_pk:
                out[name + "::" + only_pk] = entry
            else:
                out[name] = entry
    return out
