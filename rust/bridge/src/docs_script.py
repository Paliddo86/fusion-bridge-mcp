"""Document verbs in the EDA Bridge Standard's shape (document_info, close_all_documents), run on
Fusion's main thread through the script transport (docs.rs embeds this file)."""

KIND = {'EcadDesignProductType': 'project', 'SchematicProductType': 'schematic', 'BoardProductType': 'board',
        'DesignProductType': 'model', 'ElectronLibraryProductType': 'library', 'ElectronicsLibraryProductType': 'library'}


def _tab_number(app, i):
    """The tab of the document at index i in app.documents. A document with no visible tab (opened by a script
    with visible=False) has no place in the tab bar, so every index after it was off by one (#141)."""
    return sum(1 for k in range(min(int(i), app.documents.count)) if app.documents.item(k).isVisible)


def _pt(d):
    try:
        return d.products.item(0).productType if d.products.count else ''
    except Exception:
        return ''


def _location(df):
    """'Project/folder/sub/name.ext' for a saved file."""
    try:
        parts, f = [], df.parentFolder
        while f is not None and not f.isRoot:
            parts.append(f.name)
            f = f.parentFolder
        return '/'.join([df.parentProject.name] + list(reversed(parts)) + ['%s.%s' % (df.name, df.fileExtension)])
    except Exception:
        return None


def doc_info(app, args):
    d = app.activeDocument
    if d is None:
        return {'success': False, 'errorCode': 'no_document', 'error': 'No document is open'}
    df = d.dataFile
    pt = _pt(d)
    return {'success': True, 'name': d.name, 'kind': KIND.get(pt, 'other'), 'productType': pt,
            'saved': df is not None, 'modified': bool(d.isModified), 'path': _location(df) if df else None,
            'version': (df.versionNumber if df else None), 'project': (df.parentProject.name if df else None)}


def close_all(app, args):
    """Close every document without a save dialog; unsaved work is never lost by default."""
    save = args.get('save') is True
    discard = args.get('discardUnsaved') is True or args.get('force') is True
    scratch = args.get('includeScratch') is True or args.get('closeUntitled') is True
    closed, saved, kept = [], [], []
    for d in [app.documents.item(i) for i in range(app.documents.count)]:
        name = d.name
        try:
            df = d.dataFile
            modified = bool(d.isModified)
            if df is None:
                # never saved: a scratch document; a named one with changes would raise Fusion's Save box
                if not scratch:
                    kept.append({'name': name, 'why': 'scratch (never saved): pass includeScratch to close it'})
                    continue
                if modified and not discard:
                    kept.append({'name': name, 'why': 'scratch with unsaved changes: pass discardUnsaved to lose them'})
                    continue
                if modified and not name.lower().startswith('untitled'):
                    kept.append({'name': name, 'why': 'never saved and changed: Fusion would ask where to save it; save it with fusion_save_to_cloud first'})
                    continue
                d.close(False)
                closed.append(name)
                continue
            if modified:
                if save:
                    d.save('fusion-bridge: saved before close')
                    saved.append(name)
                elif not discard:
                    kept.append({'name': name, 'why': 'unsaved changes: pass save or discardUnsaved'})
                    continue
            d.close(False)
            closed.append(name)
        except Exception as e:
            kept.append({'name': name, 'why': 'Fusion refused: %s' % e})
    return {'success': True, 'closed': closed, 'saved': saved, 'keptOpen': kept, 'remaining': app.documents.count,
            '_hint': 'keptOpen lists what still needs a decision. Fusion keeps one blank design open when the last document closes.' if kept else
                     'Everything asked was closed. Fusion keeps one blank design open when the last document closes.'}


def _design_3d(app, args):
    """The 3D PCB of the design in front (or named by expectDocument): its DesignProductType tab of
    the same name (and the same cloud folder when saved)."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    anchor = app.activeDocument
    want = args.get('expectDocument')
    if want:
        named = [d for d in docs if d.name == want]
        if not named:
            return None, None, 'No open document is named %s' % want
        anchor = named[0]
    if anchor is None:
        return None, None, 'No document is open'
    if _pt(anchor) == 'DesignProductType':
        return docs.index(anchor), anchor, None
    for i, d in enumerate(docs):
        if _pt(d) == 'DesignProductType' and d.name == anchor.name:
            if anchor.dataFile is None or d.dataFile is None or d.dataFile.parentFolder.id == anchor.dataFile.parentFolder.id:
                return i, d, None
    return None, None, 'The design %s has no 3D PCB open: show it once with fusion_show_3d_board' % anchor.name


SIDES = {'top': 'TopViewOrientation', 'bottom': 'BottomViewOrientation', 'left': 'LeftViewOrientation', 'right': 'RightViewOrientation',
         'front': 'FrontViewOrientation', 'back': 'BackViewOrientation', 'iso': 'IsoTopRightViewOrientation'}


def tempfile_dir():
    import tempfile
    return tempfile.gettempdir()


def _render_process():
    """render_process.py, embedded by docs.rs as RENDER_PROCESS_SOURCE, run as its own module each call (never
    cached in sys.modules, so a bridge update reaches it without a Fusion restart)."""
    import types
    mod = types.ModuleType('adom_render_process')
    exec(RENDER_PROCESS_SOURCE, mod.__dict__)
    return mod


def render_board_3d(app, args):
    """A 3D picture of the board from one side, through Fusion's own viewport, with the tab the
    user had put back afterwards."""
    import adsk.core, adsk.fusion
    side = (args.get('side') or 'top').lower()
    if side not in SIDES:
        return {'success': False, 'errorCode': 'bad_args', 'error': 'side is top, bottom, left, right, front, back or iso'}
    high = (args.get('quality') or 'basic') == 'high'
    rotate = args.get('rotate')
    if isinstance(rotate, str):
        try:
            rotate = [float(v) for v in rotate.split(',')]
        except ValueError:
            return {'success': False, 'errorCode': 'bad_args', 'error': "rotate is 'x,y,z' degrees, e.g. '-45,0,45'"}
    if rotate is not None and len(rotate) != 3:
        return {'success': False, 'errorCode': 'bad_args', 'error': "rotate is 'x,y,z' degrees"}
    refs = args.get('refs') or []
    if isinstance(refs, str):
        refs = [refs]
    fmt = (args.get('format') or 'png').lower()
    out = args.get('outputPath')
    if not out:
        import tempfile, os
        out = os.path.join(tempfile.gettempdir(), 'adom-render', 'board-%s.%s' % (side, fmt)).replace('\\', '/')
    import os
    os.makedirs(os.path.dirname(out) or '.', exist_ok=True)
    i, doc, err = _design_3d(app, args)
    if err:
        return {'success': False, 'errorCode': 'no_3d_board', 'error': err}
    docs = [app.documents.item(k) for k in range(app.documents.count)]
    front = docs.index(app.activeDocument) if app.activeDocument in docs else None
    try:
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, i))
        vp = app.activeViewport
        cam = vp.camera
        cam.viewOrientation = getattr(adsk.core.ViewOrientations, SIDES[side])
        cam.cameraType = adsk.core.CameraTypes.PerspectiveCameraType if args.get('perspective') else adsk.core.CameraTypes.OrthographicCameraType
        cam.isFitView = True
        cam.isSmoothTransition = False
        vp.camera = cam
        framed = None
        if refs:
            # frame the named parts: the union of their 3D boxes (Packages' child occurrences are
            # PACKAGE:REF), the camera kept on its side and moved onto their centre
            d = adsk.fusion.Design.cast(app.activeProduct)
            want = {str(r).upper() for r in refs}
            boxes, found = [], set()
            for o in d.rootComponent.occurrences:
                if o.component.name != 'Packages':
                    continue
                for c in o.childOccurrences:
                    r = c.name.rsplit(':', 1)[-1].upper()
                    # a part with no 3D body has an empty box at the origin: it cannot be framed
                    has_body = c.bRepBodies.count > 0 or any(x.bRepBodies.count for x in c.component.allOccurrences)
                    if r in want and has_body and c.boundingBox:
                        boxes.append(c.boundingBox)
                        found.add(r)
            missing = sorted(want - found)
            if not boxes:
                return {'success': False, 'errorCode': 'unknown_parts', 'error': 'No 3D body for %s on the board (a part with no 3D package cannot be framed: fusion_bodies lists them under noBody)' % ', '.join(missing)}
            lo = [min(b.minPoint.x for b in boxes), min(b.minPoint.y for b in boxes), min(b.minPoint.z for b in boxes)]
            hi = [max(b.maxPoint.x for b in boxes), max(b.maxPoint.y for b in boxes), max(b.maxPoint.z for b in boxes)]
            margin = float(args.get('marginMm', 2.0)) / 10.0
            centre = adsk.core.Point3D.create(*[(a + b) / 2 for a, b in zip(lo, hi)])
            cam = vp.camera
            off = cam.target.vectorTo(cam.eye)
            eye = centre.copy(); eye.translateBy(off)
            cam.target, cam.eye = centre, eye
            cam.isFitView = False
            span = max(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]) + 2 * margin
            cam.viewExtents = span
            cam.isSmoothTransition = False
            vp.camera = cam
            framed = {'refs': sorted(found), 'missing': missing, 'pivot': [round(v * 10, 3) for v in (centre.x, centre.y, centre.z)], 'zoom': round(span * 10, 3)}
        if rotate:
            # turn the eye (and the up vector) about the target: x, then y, then z, in degrees
            import math
            cam = vp.camera
            off = cam.target.vectorTo(cam.eye); up = cam.upVector
            for axis, deg in zip(((1, 0, 0), (0, 1, 0), (0, 0, 1)), rotate):
                if deg:
                    m = adsk.core.Matrix3D.create()
                    m.setToRotation(math.radians(deg), adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0, 0, 0))
                    off.transformBy(m); up.transformBy(m)
            eye = cam.target.copy(); eye.translateBy(off)
            cam.eye, cam.upVector = eye, up
            cam.isSmoothTransition = False
            vp.camera = cam
        zoom = float(args.get('zoom', 1.0))
        if zoom and abs(zoom - 1.0) > 1e-6:
            cam = vp.camera
            cam.viewExtents = cam.viewExtents / (zoom * zoom)
            cam.isSmoothTransition = False
            vp.camera = cam
        w, h = int(args.get('width', 1600)), int(args.get('height', 900))
        transparent = (args.get('background') == 'transparent') and fmt == 'png'
        if high:
            # Fusion's local raytracer from this camera (the mechanical layer's render_image does
            # the same from a named view); every render setting it touches is put back
            import time
            d = adsk.fusion.Design.cast(app.activeProduct)
            rendering = d.renderManager.rendering
            keep = (rendering.aspectRatio, rendering.resolutionWidth, rendering.resolutionHeight, rendering.renderQuality, rendering.isBackgroundTransparent)
            # Fusion writes into a staging folder of our own and the image is moved to `out` only on success,
            # so a render that outlives renderSeconds can never write `out` later (Drew Owens, PR #40 note)
            import uuid, shutil
            stage = os.path.join(tempfile_dir(), 'adom-fusion-render', uuid.uuid4().hex)
            os.makedirs(stage, exist_ok=True)
            staged = os.path.join(stage, os.path.basename(out))
            cancel = None
            try:
                rendering.aspectRatio = adsk.fusion.RenderAspectRatios.CustomRenderAspectRatio
                rendering.resolutionWidth, rendering.resolutionHeight = w, h
                rendering.renderQuality = max(25, int(args.get('renderQuality', 25)))
                rendering.isBackgroundTransparent = transparent
                started = time.time()
                future = rendering.startLocalRender(staged)
                deadline = time.time() + float(args.get('renderSeconds', 45))
                while future is not None and future.renderState != adsk.fusion.LocalRenderStates.FinishedLocalRenderState and time.time() < deadline:
                    adsk.doEvents(); time.sleep(0.25)
                done = future is not None and future.renderState == adsk.fusion.LocalRenderStates.FinishedLocalRenderState
                if not done and future is not None:
                    # RenderFuture has no cancel(): end this render's own RenderProcess.exe, as render_image does
                    cancel = _render_process().cancel(future, started, adsk.fusion.LocalRenderStates, pump=adsk.doEvents)
            finally:
                rendering.aspectRatio, rendering.resolutionWidth, rendering.resolutionHeight, rendering.renderQuality, rendering.isBackgroundTransparent = keep
            if not done:
                if cancel and cancel.get('cancelled'):
                    shutil.rmtree(stage, ignore_errors=True)
                    why = 'it was cancelled and wrote nothing'
                else:
                    why = ('it could not be cancelled (%s); if it finishes later its image stays in the staging folder %s, never at outputPath'
                           % ((cancel or {}).get('reason', 'no render job'), stage))
                return {'success': False, 'errorCode': 'render_timeout', 'cancel': cancel,
                        'error': 'the ray-traced render did not finish in renderSeconds; %s (a smaller width/height or a larger renderSeconds helps)' % why}
            ok = os.path.exists(staged)
            if ok:
                try:
                    os.replace(staged, out)
                except OSError:
                    shutil.move(staged, out)  # the staging folder is on another volume
            shutil.rmtree(stage, ignore_errors=True)
        else:
            opts = adsk.core.SaveImageFileOptions.create(out)
            opts.width, opts.height = w, h
            opts.isAntiAliased = True
            opts.isBackgroundTransparent = transparent
            ok = vp.saveAsImageFileWithOptions(opts)
    finally:
        if front is not None and front != i:
            app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, front))
    if not ok or not os.path.exists(out):
        return {'success': False, 'errorCode': 'render_failed', 'error': 'Fusion did not write %s' % out}
    return {'success': True, 'path': out, 'side': side, 'width': w, 'height': h, 'format': fmt, 'design': doc.name,
            'background': 'transparent' if transparent else 'default', 'quality': 'high' if high else 'basic', 'framed': framed, 'rotate': rotate,
            '_hint': 'Fusion\'s own 3D view of the design\'s 3D PCB, fitted to the board from that side; the tab you had is back in front. A 3D PCB out of date with the board shows the old placement: fusion_show_3d_board updates it.'}


def project_files(app, args):
    """The design's documents: the ones open in Fusion and, for a saved design, its cloud files of
    the same name in the same folder (project, schematic, board, 3D PCB), with version and date."""
    docs = [app.documents.item(i) for i in range(app.documents.count)]
    want = args.get('expectDocument') or args.get('project') or (app.activeDocument.name if app.activeDocument else None)
    mine = [d for d in docs if d.name == want]
    if not mine:
        return {'success': False, 'errorCode': 'not_open', 'error': 'No open design is named %s' % want,
                'open': sorted({d.name for d in docs})}
    files, folder = [], None
    for d in mine:
        df = d.dataFile
        row = {'kind': KIND.get(_pt(d), 'other'), 'name': d.name, 'open': True, 'saved': df is not None, 'modified': bool(d.isModified), 'tab': docs.index(d)}
        if df is not None:
            folder = folder or df.parentFolder
            row.update({'path': _location(df), 'version': df.versionNumber})
        files.append(row)
    if folder is not None:
        seen = {f.get('path') for f in files}
        for i in range(folder.dataFiles.count):
            df = folder.dataFiles.item(i)
            if df.name != want:
                continue
            path = _location(df)
            row = next((f for f in files if f.get('path') == path), None)
            if row is None:
                row = {'kind': {'fprj': 'project', 'fsch': 'schematic', 'fbrd': 'board', 'f3d': 'model'}.get(df.fileExtension, 'other'), 'name': df.name, 'open': False, 'saved': True, 'path': path}
                files.append(row)
            row['version'] = df.latestVersionNumber
            for attr, key in (('dateModified', 'modified'), ('dateCreated', 'created')):
                try:
                    v = getattr(df, attr)
                    row[key + 'At'] = int(v) if v else None
                except Exception:
                    pass
    modified = max([f.get('modifiedAt') or 0 for f in files] or [0]) or None
    return {'success': True, 'name': want, 'files': files, 'modifiedAt': modified, 'open': True,
            'project': mine[0].dataFile.parentProject.name if mine[0].dataFile is not None else None}


def handle(app, args, action):
    if action == 'front_tab':
        docs = [app.documents.item(i) for i in range(app.documents.count)]
        return {'success': True, 'index': docs.index(app.activeDocument) if app.activeDocument in docs else None}
    if action == 'switch_tab':
        app.executeTextCommand('UI.SwitchToTab %d' % _tab_number(app, int(args['index'])))
        return {'success': True}
    if action == 'render_board_3d':
        return render_board_3d(app, args)
    if action == 'document_info':
        return doc_info(app, args)
    if action == 'close_all_documents':
        return close_all(app, args)
    if action == 'project_files':
        return project_files(app, args)
    return {'success': False, 'errorCode': 'unknown_action', 'error': action}
