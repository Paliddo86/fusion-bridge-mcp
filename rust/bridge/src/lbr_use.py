

# ── loading a .lbr for ADD / REPLACE (issues #135, #136) ────────────────────────────────────────
# Fusion 2706's ADD reads the library file as text. It raises "PlacePartWorker.cpp, Line 23.
# Condition: assetAdded" on every ADD from a file that has (each bisected live on 2706.1.14,
# 2026-10-31 and 2026-11-01, on a library that adds cleanly otherwise):
#   - elements inside <library> with no line break between them (one is enough);
#   - a UTF-8 byte-order mark;
#   - attributes in single quotes;
#   - a <package3d> whose <packageinstance> names a footprint the file does not have (#136: adom-lbr
#     assemble merges parts with identical footprints, and a 3D entry written per original part was left
#     pointing at the merged-away name). One is enough, whichever part is being added.
# Fine as they are: indentation, tabs, CRLF, no XML declaration, a DOCTYPE, comments, non-ASCII
# text, apostrophes and escaped HTML in text, the EAGLE version, an unresolvable 3D package URN.
# So such a file is rewritten in place before USE, keeping its element tree exactly, apart from those
# dead 3D entries, which no device uses and which are named in the reply. In place,
# because a copy at another path with the same library name makes Fusion ask which one to take
# (its Select Library dialog) once both have been USEd.

def _lbr_shape(root):
    return [(e.tag if isinstance(e.tag, str) else 'comment', sorted(e.attrib.items()), (e.text or '').strip(), (e.tail or '').strip())
            for e in root.iter()]


def _orphan_3d(root):
    """The <package3d> entries Fusion 2706's ADD cannot take and nothing uses: every footprint they name
    is missing from the file, and no device points at them. (package3d element, its parent) pairs."""
    lib = root.find('.//library')
    if lib is None: return []
    packages = {p.get('name') for p in lib.iter('package')}
    used = {i.get('package3d_urn') for i in lib.iter('package3dinstance')}
    holder = lib.find('packages3d')
    out = []
    for q in (list(holder) if holder is not None else []):
        names = [i.get('name') for i in q.iter('packageinstance')]
        if names and not any(n in packages for n in names) and not ({q.get('wip_urn'), q.get('urn')} & used):
            out.append((q, holder))
    return out


def lbr_needs_rewrite(data):
    """True when the file (bytes or text) has anything Fusion 2706's ADD cannot read: a byte-order
    mark, single-quoted attributes, or two elements inside <library> with no line break between
    them. A compact header outside <library> and a single-quoted XML declaration are fine: Fusion's
    own exports have them."""
    import re as _re
    if isinstance(data, str): data = data.encode('utf-8')
    if data[:3] == b'\xef\xbb\xbf' or data[:2] in (b'\xff\xfe', b'\xfe\xff'): return True
    body = _re.sub(rb'^\s*<\?xml[^>]*\?>', b'', data, count=1)
    if _re.search(rb"""<[A-Za-z][^<>"']*\s[\w:.-]+\s*=\s*'""", body): return True
    i = data.find(b'<library')
    if i >= 0 and _re.search(rb'>[ \t]*<', data[i:]) is not None: return True
    if b'<package3d ' not in data: return False
    import xml.etree.ElementTree as _ET
    try: return bool(_orphan_3d(_ET.fromstring(data)))
    except _ET.ParseError: return False


def normalize_lbr(path, removed=None):
    """Rewrite a .lbr Fusion 2706 cannot ADD from in place: no byte-order mark, double-quoted
    attributes, a line break between elements, no dead 3D entries (their names go on `removed`). Returns
    True when it rewrote the file. Apart from those entries only the file's formatting changes: the
    element tree (tags, attributes, text, comments) is compared before and after, and nothing is written
    when it differs."""
    import os as _os
    import re as _re
    import xml.etree.ElementTree as _ET
    from pathlib import Path as _P
    p = _P(path)
    raw = p.read_bytes()
    if not lbr_needs_rewrite(raw):
        return False
    # parsed from bytes, so the file's own declared encoding is honoured; written back as UTF-8
    root = _ET.fromstring(raw, parser=_ET.XMLParser(target=_ET.TreeBuilder(insert_comments=True)))
    for q, holder in _orphan_3d(root):
        holder.remove(q)
        if removed is not None: removed.append(q.get('name'))
    before = _lbr_shape(root)
    _ET.indent(root, space='')
    body = _ET.tostring(root, encoding='unicode')
    again = _ET.fromstring(body, parser=_ET.XMLParser(target=_ET.TreeBuilder(insert_comments=True)))
    if _lbr_shape(again) != before:
        raise RuntimeError('rewriting %s for Fusion would change more than its formatting' % p.name)
    m = _re.search(r'<!DOCTYPE[^>]*>', raw[:2000].decode('latin-1'))
    out = '<?xml version="1.0" encoding="utf-8"?>\n' + (m.group(0) + '\n' if m else '') + body + '\n'
    tmp = p.with_name(p.name + '.adom-tmp')
    tmp.write_text(out, encoding='utf-8')
    _os.replace(str(tmp), str(p))
    return True


def use_library(app, path, removed=None):
    """USE a .lbr on this machine so ADD/REPLACE can take its devices, rewriting it first when
    Fusion 2706 could not read it (see above). Returns True when the file was rewritten; the 3D entries
    it dropped go on `removed`."""
    from pathlib import Path as _P
    import sys as _sys
    # two in-use libraries with one name at different paths make the next ADD open Fusion's Select Library
    # picker, a modal that blocks it, and a library can't be taken out of use from a command. So the second
    # is refused. The record lives in this Fusion process and sees only what the bridge loaded since it
    # started; Fusion's own list survives a restart, so a conflict from an earlier session still gets the picker.
    used = _sys.__dict__.setdefault('_adom_lbr_used', {})
    full, name = _P(path).as_posix(), _P(path).stem.lower()
    if used.get(name, full).lower() != full.lower():
        raise RuntimeError('library_name_in_use: a library named %s is already in use from %s, and loading %s too would make Fusion ask which one to take (a dialog that blocks it). Use that path or give this file another name. To clear it, take the other file away and restart Fusion: a restart alone does not, as its in-use list survives one' % (_P(path).stem, used[name], full))
    try:
        changed = normalize_lbr(path, removed)
    except PermissionError:
        raise RuntimeError('library_not_writable: %s is formatted in a way Fusion 2706 cannot ADD from (PlacePartWorker assetAdded: '
                           'no line breaks between elements, a byte-order mark or single-quoted attributes), and this machine will not let '
                           'the bridge rewrite it. Save it somewhere writable and pass that path' % path)
    app.executeTextCommand("Electron.run USE '%s';" % _P(path).as_posix())
    used[name] = full
    return changed
