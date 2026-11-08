"""Saving an electronics design the way Fusion means it to be saved (page issue #125).

Embedded by savedoc.rs and run on Fusion's main thread. A design is up to four documents with one
name: the design itself (EcadDesignProductType, the .fprj), its schematic (.fsch), its board (.fbrd)
and its 3D PCB (.f3d). What was learned live on DESKTOP-BAR6S0F, 2026-10-26:

- Save As on the BOARD document writes a new standalone .fbrd the design never links (the fork in
  the issue). Saving the DESIGN document saves .fprj, .fsch and .fbrd together, linked, and the 3D
  PCB beside them when it is open.
- Saving the design document in place moves each changed document to its next version in the
  same lineage (the board 1 -> 2), right after a first save as well as on a design opened from
  the cloud.
- doc.activate() can move the API's active document without moving Fusion's UI, and edits go to
  the UI's board: an edit then lands on another design and the one saved has nothing to save.
  Tabs are switched with UI.SwitchToTab (sd_activate), which moves both.
- Right after a first save, a DataFile's parentProject is not readable yet (InternalValidationError),
  so the reply names the project and folder the design was saved into.
"""
import time

SD_ECAD, SD_SCH, SD_BRD, SD_3D = 'EcadDesignProductType', 'SchematicProductType', 'BoardProductType', 'DesignProductType'


class SaveError(Exception):
    def __init__(self, code, message, **extra):
        super().__init__(message)
        self.code, self.extra = code, extra


def _tab_number(app, i):
    """The tab of the document at index i in app.documents. A document with no visible tab (opened by a script
    with visible=False) has no place in the tab bar, so every index after it was off by one (#141)."""
    return sum(1 for k in range(min(int(i), app.documents.count)) if app.documents.item(k).isVisible)


def sd_pt(d):
    try:
        return d.products.item(0).productType if d.products.count else ''
    except Exception:
        return ''


def sd_wait(cond, secs):
    import adsk
    end = time.time() + secs
    while time.time() < end:
        adsk.doEvents()
        try:
            if cond():
                return True
        except Exception:
            pass
        time.sleep(0.4)
    return False


def sd_docs(app):
    return [app.documents.item(i) for i in range(app.documents.count)]


def sd_family(app, anchor):
    """The design's documents by product type. A saved design's documents share their cloud name
    and folder; an unsaved one's are told apart only by order: Fusion opens a design's tabs right
    after its design document, so the family is the design document before the anchor and the
    unsaved, same-named documents that follow it."""
    docs = sd_docs(app)
    fam = {}
    if anchor.dataFile is not None:
        ref = anchor.dataFile
        folder = ref.parentFolder.id
        for d in docs:
            pt = sd_pt(d)
            if pt in (SD_ECAD, SD_SCH, SD_BRD) and d.dataFile is not None and d.dataFile.name == ref.name and d.dataFile.parentFolder.id == folder:
                fam.setdefault(pt, d)
        for d in docs:
            if sd_pt(d) == SD_3D and d.name == anchor.name and (d.dataFile is None or (d.dataFile.name == ref.name and d.dataFile.parentFolder.id == folder)):
                fam.setdefault(SD_3D, d)
        return fam
    i = next(k for k, d in enumerate(docs) if d == anchor)
    start = next((k for k in range(i, -1, -1) if sd_pt(docs[k]) == SD_ECAD and docs[k].dataFile is None and docs[k].name == anchor.name), None)
    if start is None:
        return {sd_pt(anchor): anchor}
    fam[SD_ECAD] = docs[start]
    for d in docs[start + 1:]:
        pt = sd_pt(d)
        if pt not in (SD_SCH, SD_BRD, SD_3D) or d.dataFile is not None or d.name != anchor.name or pt in fam:
            break
        fam[pt] = d
    return fam


def sd_activate(app, d):
    """Bring a document's tab to the front in the UI too (doc.activate() alone can leave the UI on
    another tab; see show_tab_of in routing_script.py)."""
    docs = sd_docs(app)
    i = next((k for k, x in enumerate(docs) if x == d), None)
    if i is None:
        d.activate()
    else:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))


def sd_anchor(app, name):
    if not name:
        if app.activeDocument is None:
            raise SaveError('no_document', 'No document is open')
        return app.activeDocument
    hits = [d for d in sd_docs(app) if d.name.lower() == name.lower() and sd_pt(d) in (SD_ECAD, SD_SCH, SD_BRD, SD_3D)]
    if not hits:
        raise SaveError('no_such_document', 'No open electronics document is named %s' % name)
    return next((d for d in hits if d == app.activeDocument), hits[0])


def sd_file(d, project=None, folder=None):
    df = d.dataFile
    if df is None:
        return None
    row = {'name': df.name, 'ext': df.fileExtension, 'fileId': df.id, 'version': df.versionNumber}
    for key, get, fallback in (('project', lambda: df.parentProject.name, project), ('folder', lambda: df.parentFolder.name, folder)):
        try:
            row[key] = get()
        except Exception:          # not readable yet right after a first save
            row[key] = fallback
    return row


def sd_latest(app, d):
    """The open document's own version: the cloud's latestVersionNumber lags a fresh save by
    seconds (it read 3 while the document already said 4, live 2026-10-26)."""
    return d.dataFile.versionNumber if d.dataFile is not None else None


def sd_project(app, name):
    hub = app.data.activeHub
    projects = [hub.dataProjects.item(i) for i in range(hub.dataProjects.count)]
    if name:
        hit = next((p for p in projects if p.name.lower() == name.lower()), None)
        if hit is None:
            raise SaveError('project_not_found', 'No project named %s in the %s hub' % (name, hub.name), projects=[p.name for p in projects])
        return hit
    try:
        if app.data.activeProject is not None:
            return app.data.activeProject
    except Exception:
        pass         # no project open in the data panel: Fusion answers InternalValidationError
    raise SaveError('project_required', 'No project is open in Fusion\'s data panel, so name one with projectName',
                    projects=[p.name for p in projects])


def sd_folder(project, path):
    """The folder at path under the project's root, creating the missing ones (never a project)."""
    folder, made = project.rootFolder, []
    for part in [p for p in (path or '').replace('\\', '/').split('/') if p.strip()]:
        sub = next((folder.dataFolders.item(i) for i in range(folder.dataFolders.count) if folder.dataFolders.item(i).name == part), None)
        if sub is None:
            sub = folder.dataFolders.add(part)
            made.append(part)
        folder = sub
    return folder, made


def sd_save_document(app, args):
    anchor = sd_anchor(app, args.get('documentName') or args.get('expectDocument'))
    fam = sd_family(app, anchor)
    ecad = fam.get(SD_ECAD)
    if ecad is None:
        raise SaveError('not_an_electronics_design', '%s is not part of an open electronics design' % anchor.name)
    if ecad.dataFile is None:
        raise SaveError('not_saved_yet', 'The design %s has never been saved' % ecad.name)
    members = {pt: d for pt, d in fam.items() if d.dataFile is not None or pt == SD_3D}
    before = {pt: (sd_latest(app, d) if d.dataFile is not None else None) for pt, d in members.items()}
    modified = {pt: d.isModified for pt, d in members.items()}
    if not any(modified.values()):
        return {'success': True, 'saved': False, 'design': sd_file(ecad), 'versions': before,
                '_hint': 'Fusion reports no unsaved change in this design, so there was nothing to save. If you edited it, check the edit landed on this design: fusion_routing_state names the board it read (expectDocument), and before 1.12.0 an edit could land on the board showing in Fusion instead of the one named.'}
    front = app.activeDocument
    ok = ecad.save(str(args.get('comment') or 'Saved by Adom'))
    sd_wait(lambda: not ecad.isModified and all(not d.isModified for d in members.values()), 90)
    # each changed board or schematic reports its new version once the upload is done
    sd_wait(lambda: all(sd_latest(app, d) != before.get(pt) for pt, d in sd_family(app, ecad).items()
                        if modified.get(pt) and pt in (SD_SCH, SD_BRD)), 60)
    fam = sd_family(app, ecad)
    try:
        if front is not None and front != app.activeDocument:
            sd_activate(app, front)
    except Exception:
        pass
    after = {pt: (sd_latest(app, d) if d.dataFile is not None else None) for pt, d in fam.items()}
    stuck = [pt for pt, m in modified.items() if m and pt in (SD_SCH, SD_BRD) and after.get(pt) == before.get(pt)]
    ok = bool(ok) and not stuck
    names = {SD_ECAD: 'design', SD_SCH: 'schematic', SD_BRD: 'board', SD_3D: '3dPcb'}
    return {'success': ok, 'saved': True, 'errorCode': None if ok else 'readback_mismatch',
            'error': None if ok else 'Fusion did not write a new version of the %s' % ', '.join(names[p] for p in stuck),
            'design': sd_file(ecad), 'files': {names[pt]: sd_file(d) for pt, d in fam.items()},
            'versions': {names[pt]: {'before': before.get(pt), 'after': after.get(pt)} for pt in fam},
            'linkedBoard': sd_file(fam[SD_BRD]) if SD_BRD in fam else None,
            '_hint': 'Saved in place: each changed document moved to its next version in the same lineage (versions.*.after), the design still links the board it was edited in (linkedBoard), and the 3D PCB is kept beside the design. Nothing was forked.'}


def sd_save_new(app, args):
    anchor = app.activeDocument
    if anchor is None:
        raise SaveError('no_document', 'No document is open')
    fam = sd_family(app, anchor)
    ecad = fam.get(SD_ECAD)
    if ecad is None:
        return {'notElectronics': True}
    name = str(args.get('name') or '').strip()
    if not name:
        raise SaveError('bad_args', 'name is required for a first save')
    if ecad.dataFile is not None and args.get('asCopy') is not True:
        raise SaveError('already_saved_use_save_document',
                        'This electronics design is already saved as %s in %s/%s: a Save As by name would fork it' % (ecad.dataFile.name, ecad.dataFile.parentProject.name, ecad.dataFile.parentFolder.name),
                        design=sd_file(ecad))
    project = sd_project(app, args.get('projectName'))
    folder, made = sd_folder(project, args.get('folderPath'))
    clash = [folder.dataFiles.item(i).fileExtension for i in range(folder.dataFiles.count) if folder.dataFiles.item(i).name == name]
    if clash and args.get('overwrite') is not True:
        raise SaveError('name_taken', '%s/%s already holds %s named %s' % (project.name, folder.name, ', '.join(sorted(set(clash))), name), existing=clash)
    front_pt = sd_pt(anchor)
    ok = ecad.saveAs(name, folder, str(args.get('description') or ''), '')
    # the first save REPLACES the design's document object (a new creationId; the old handle throws
    # InternalValidationError), so it is found again by the file it was just saved as
    def saved_design():
        for d in sd_docs(app):
            try:
                if sd_pt(d) == SD_ECAD and d.dataFile is not None and d.dataFile.name == name and d.dataFile.parentFolder.id == folder.id:
                    return d
            except Exception:
                pass
        return None
    sd_wait(lambda: saved_design() is not None, 90)
    ecad = saved_design()
    if ecad is None:
        raise SaveError('save_not_confirmed', 'Fusion did not report the design saved as %s in %s/%s within 90 s' % (name, project.name, folder.name))
    sd_wait(lambda: all(d.dataFile is not None for pt, d in sd_family(app, ecad).items() if pt != SD_3D), 60)
    fam = sd_family(app, ecad)
    names = {SD_ECAD: 'design', SD_SCH: 'schematic', SD_BRD: 'board', SD_3D: '3dPcb'}
    saved = {names[pt]: sd_file(d, project.name, folder.name) for pt, d in fam.items()}
    if front_pt in fam:
        sd_activate(app, fam[front_pt])     # the same kind of tab as before, in the UI too
    return {'success': bool(ok) and saved.get('design') is not None, 'saved': True, 'files': saved, 'project': project.name,
            'folder': folder.name, 'foldersCreated': made,
            '_hint': 'The DESIGN was saved: .fprj with its .fsch and .fbrd, linked (and the 3D PCB beside them when it is open). Save later changes in place with fusion_save_document; fusion_save_to_cloud refuses this design from now on unless asCopy:true.'}


def sd_handle(app, args, action):
    try:
        if action == 'save_document':
            return sd_save_document(app, args)
        if action == 'save_to_cloud':
            return sd_save_new(app, args)
        return {'success': False, 'errorCode': 'unknown_action', 'error': action}
    except SaveError as e:
        hints = {'not_saved_yet': 'Save it the first time with fusion_save_to_cloud {name, projectName, folderPath}; it saves the whole design, linked.',
                 'already_saved_use_save_document': 'Save the changes in place with fusion_save_document {comment}. Pass asCopy:true only when you really want a second, separate copy of the design.',
                 'project_required': 'Pass projectName (one of projects); folderPath is created if missing.',
                 'project_not_found': 'Pass one of projects as projectName.',
                 'name_taken': 'Pick another name, or open the existing design and save into it with fusion_save_document.',
                 'not_an_electronics_design': 'Bring a design, schematic or board to the front (fusion_show_2d_board {documentName}).'}
        return {'success': False, 'errorCode': e.code, 'error': str(e), **e.extra, '_hint': hints.get(e.code, '')}
