"""Bounded scene observation and detached camera preparation; never renders.

All native access occurs on the main thread. Missing fields remain explicit;
this is not a complete scene witness or evidence of native queue ownership.
"""
import inspect
import json
import math
import threading

from .core import SESSION_ID, MechanicalError, require, target

MAX_ENVIRONMENTS = 32
MAX_OUTPUT_BYTES = 128 * 1024


def scene_observation(app, document, design, *, max_environments=16):
    import adsk.core
    import adsk.fusion

    def need(value, message):
        if not value: raise RuntimeError(message)

    need(threading.current_thread() is threading.main_thread(), 'Native render reads require the main thread')
    need(type(max_environments) is int and 0 <= max_environments <= MAX_ENVIRONMENTS, 'Invalid environment probe budget')
    need(document is not None and document.isValid and design is not None and design.isValid, 'Owned native design is invalid')
    need(app.activeDocument == document and design.parentDocument == document, 'Current native document/design identity differs')
    document_count = app.documents.count
    need(type(document_count) is int and 0 <= document_count <= 128, 'Open document inventory exceeds probe budget or has invalid count')
    need(sum(app.documents.item(i) == document for i in range(document_count)) == 1, 'Exact owned document is not uniquely open')
    result = {'schemaVersion':1, 'status':'OBSERVED', 'renderStarted':False, 'settingsWritten':False,
              'sceneWitnessComplete':False, 'renderQualified':False, 'nativeQueueIdle':'UNVERIFIED', 'exclusiveSettingsOwnership':'UNVERIFIED',
              'errors':[], 'classes':{}, 'camera':{}, 'rendering':{}, 'scene':{}, 'environments':{},
              'gaps':['No render execution or RenderFuture instance was created.',
                      'No global renderer queue/coexistence proof.',
                      'Appearance, material, texture, light and occurrence-visibility graphs are outside this bounded probe.',
                      'Environment IDs and names do not prove custom environment content identity.',
                      'No admitted revision-to-pixels or full source snapshot claim.']}

    def text(value):
        need(isinstance(value,str) and len(value) <= 256, 'Native string exceeds probe budget or has wrong type')
        return value

    def number(value):
        need(type(value) in (int,float) and math.isfinite(value), 'Native scalar is not a finite number')
        return value

    def boolean(value):
        need(type(value) is bool, 'Native scalar is not Boolean')
        return value

    def enum(value):
        need(not isinstance(value,(bool,str)), 'Native enum has invalid type')
        result = int(value)
        need(result == value, 'Native enum is not an exact integer')
        return result

    def point(value, scale=10):
        return [number(number(getattr(value,axis)) * scale) for axis in ('x','y','z')]

    def read(path, callback, unit=None, native_unit=None):
        try:
            data = callback()
            row = {'available':True,'value':data}
            if unit is not None: row['unit'] = unit
            if native_unit is not None: row['nativeUnit'] = native_unit
            return row
        except Exception as error:
            reason = str(error)[:512]
            result['errors'].append({'field':path,'error':reason})
            return {'available':False,'error':reason}

    def skipped(reason): return {'available':None,'notRead':reason}

    def object_read(path, callback):
        try:
            value = callback()
            need(value is not None and value.isValid, 'Native object absent or invalid')
            text(value.objectType)
            return value, {'available':True,'objectType':value.objectType}
        except Exception as error:
            reason = str(error)[:512]
            result['errors'].append({'field':path,'error':reason})
            return None, {'available':False,'error':reason}

    def fields(destination, prefix, owner, specs):
        for name, converter, unit, native_unit in specs:
            destination[name] = read(prefix + '.' + name, lambda name=name,converter=converter:converter(getattr(owner,name)), unit, native_unit)

    result['documentBefore'] = {'modified':read('document.modified',lambda:boolean(document.isModified)),
                                'saved':read('document.saved',lambda:boolean(document.isSaved))}
    # Static class inspection does not invoke descriptors, constructors, start,
    # activation, cancel or any native future polling.
    class_members = {'RenderManager':('rendering','sceneSettings','renderEnvironments','lights','isRenderWorkspaceActive'),
        'Rendering':('startLocalRender','aspectRatio','resolution','resolutionWidth','resolutionHeight','renderQuality','isBackgroundTransparent'),
        'RenderFuture':('filename','imageWidth','imageHeight','progress','renderState','isValid'),
        'SceneSettings':('backgroundType','backgroundEnvironment','backgroundSolidColor','isDepthOfFieldEnabled'),
        'RenderEnvironment':('id','name','isCustomEnvironment')}
    for name,members in class_members.items():
        try:
            cls = getattr(adsk.fusion,name)
            availability = {}
            for member in members:
                try: inspect.getattr_static(cls,member); availability[member] = True
                except AttributeError: availability[member] = False
            result['classes'][name] = {'available':True,'members':availability,'methodInvocation':False}
        except Exception as error:
            result['classes'][name] = {'available':False,'error':str(error)[:512]}

    camera_enums = {name:read('CameraTypes.'+name,lambda name=name:enum(getattr(adsk.core.CameraTypes,name)))
                    for name in ('OrthographicCameraType','PerspectiveCameraType','PerspectiveWithOrthoFacesCameraType')}
    background_enums = {name:read('RenderSceneBackgroundTypes.'+name,
                       lambda name=name:enum(getattr(adsk.fusion.RenderSceneBackgroundTypes,name)))
                       for name in ('SolidColorRenderSceneBackgroundType','EnvironmentRenderSceneBackgroundType')}
    result['enums'] = {'camera':camera_enums,'background':background_enums}

    viewport, result['viewport'] = object_read('activeViewport',lambda:app.activeViewport)
    camera = None
    if viewport is not None:
        camera, result['camera']['object'] = object_read('camera',lambda:viewport.camera)
    if camera is not None:
        fields(result['camera'],'camera',camera,[('cameraType',enum,'native CameraTypes enum',None),
             ('eye',point,'mm','cm'),('target',point,'mm','cm'),('upVector',lambda p:point(p,1),'unitless',None),
             ('isFitView',boolean,None,None),('isSmoothTransition',boolean,None,None)])
        result['camera']['space'] = 'world/root'
        kind = result['camera']['cameraType'].get('value')
        if kind is not None and kind == camera_enums['OrthographicCameraType'].get('value'):
            def extents():
                okay,width,height = camera.getExtents()
                need(okay is True, 'Orthographic camera extent read refused')
                return [number(number(width)*10),number(number(height)*10)]
            result['camera']['extents'] = read('camera.getExtents',extents,'mm','cm')
            result['camera']['perspectiveAngle'] = skipped('Orthographic camera')
        elif kind is not None and kind in (camera_enums['PerspectiveCameraType'].get('value'),camera_enums['PerspectiveWithOrthoFacesCameraType'].get('value')):
            result['camera']['perspectiveAngle'] = read('camera.perspectiveAngle',lambda:number(math.degrees(number(camera.perspectiveAngle))), 'deg', 'rad')
            result['camera']['extents'] = skipped('Perspective camera')
        else:
            result['errors'].append({'field':'camera.cameraType','error':'Unknown/unreadable camera mode; conditional lens getters not read'})
            result['camera']['extents'] = result['camera']['perspectiveAngle'] = skipped('Unknown camera mode')

    manager, result['renderManager'] = object_read('design.renderManager',lambda:design.renderManager)
    if manager is not None:
        matched = read('renderManager.parentDesign',lambda:boolean(manager.parentDesign == design))
        result['renderManager']['ownerMatches'] = matched
        if matched.get('value') is not True:
            result['errors'].append({'field':'renderManager.parentDesign','error':'Foreign/unreadable renderer owner; remaining renderer objects not read'})
            manager = None
    if manager is not None:
        result['renderManager']['workspaceActive'] = read('renderManager.isRenderWorkspaceActive',lambda:boolean(manager.isRenderWorkspaceActive))
        rendering, result['rendering']['object'] = object_read('renderManager.rendering',lambda:manager.rendering)
        if rendering is not None:
            fields(result['rendering'],'rendering',rendering,[('aspectRatio',enum,'native RenderAspectRatios enum',None),
                ('resolution',enum,'native RenderResolutions enum',None),('resolutionWidth',number,'px',None),
                ('resolutionHeight',number,'px',None),('renderQuality',number,'native quality (25..100)',None),
                ('isBackgroundTransparent',boolean,None,None)])
        scene, result['scene']['object'] = object_read('renderManager.sceneSettings',lambda:manager.sceneSettings)
        if scene is not None:
            specs = [('aspectRatio',enum,'native RenderAspectRatios enum',None),('aspectRatioWidth',number,'ratio',None),
                ('aspectRatioHeight',number,'ratio',None),('backgroundType',enum,'native RenderSceneBackgroundTypes enum',None),
                ('brightness',number,'lux',None),('cameraExposure',number,'EV',None),('cameraFocalLength',number,'mm','mm'),
                ('cameraType',enum,'native CameraTypes enum',None),('groundOffset',lambda n:number(number(n)*10),'mm','cm'),
                ('lightAngle',lambda n:number(math.degrees(number(n))),'deg','rad')]
            specs += [(name,boolean,None,None) for name in ('isDepthOfFieldEnabled','isGroundDisplayed','isGroundFlattened','isGroundReflections')]
            fields(result['scene'],'scene',scene,specs)
            for condition,names in (('isDepthOfFieldEnabled',('centerOfFocus','depthOfFieldBlur')),
                                    ('isGroundFlattened',('groundPosition',)),('isGroundReflections',('groundRoughness',))):
                enabled = result['scene'][condition].get('value')
                for name in names:
                    if enabled is True:
                        vector = name in ('centerOfFocus','groundPosition')
                        fields(result['scene'],'scene',scene,[(name,point if vector else number,'mm' if vector else 'unitless','cm' if vector else None)])
                    else: result['scene'][name] = skipped(condition + ' is false or unreadable')
            background = result['scene']['backgroundType'].get('value')
            if background is not None and background == background_enums['SolidColorRenderSceneBackgroundType'].get('value'):
                def color():
                    value = scene.backgroundSolidColor
                    return {name:number(getattr(value,name)) for name in ('red','green','blue','opacity')}
                result['scene']['backgroundSolidColor'] = read('scene.backgroundSolidColor',color,'RGBA 0..255; opacity ignored by scene')
                result['scene']['backgroundEnvironment'] = skipped('Solid background')
            elif background is not None and background == background_enums['EnvironmentRenderSceneBackgroundType'].get('value'):
                env, descriptor = object_read('scene.backgroundEnvironment',lambda:scene.backgroundEnvironment)
                if env is not None:
                    fields(descriptor,'scene.backgroundEnvironment',env,[('id',text,None,None),('name',text,None,None),('isCustomEnvironment',boolean,None,None)])
                result['scene']['backgroundEnvironment'] = descriptor
                result['scene']['backgroundSolidColor'] = skipped('Environment background')
            else:
                result['errors'].append({'field':'scene.backgroundType','error':'Unknown/unreadable background mode; conditional getters not read'})
                result['scene']['backgroundEnvironment'] = result['scene']['backgroundSolidColor'] = skipped('Unknown background mode')
        environments, result['environments']['object'] = object_read('renderManager.renderEnvironments',lambda:manager.renderEnvironments)
        if environments is not None:
            count = read('environments.count',lambda:number(environments.count))
            result['environments']['count'] = count
            n = count.get('value');result['environments']['items'] = []
            if type(n) is int and n >= 0:
                result['environments']['truncated'] = n > max_environments
                for index in range(min(n,max_environments)):
                    env, descriptor = object_read('environment.' + str(index),lambda index=index:environments.item(index))
                    if env is not None:
                        fields(descriptor,'environment.'+str(index),env,[('id',text,None,None),('name',text,None,None),('isCustomEnvironment',boolean,None,None)])
                    result['environments']['items'].append(descriptor)
            else: result['errors'].append({'field':'environments.count','error':'Environment count is not a nonnegative integer'})
    result['documentAfter'] = {'modified':read('document.modified.after',lambda:boolean(document.isModified)),
                              'saved':read('document.saved.after',lambda:boolean(document.isSaved)),
                              'stillActive':read('document.active.after',lambda:boolean(app.activeDocument == document))}
    result['documentFlagsUnchanged'] = (result['documentBefore']['modified'].get('available') is True
        and result['documentBefore']['saved'].get('available') is True
        and result['documentBefore']['modified'] == result['documentAfter']['modified']
        and result['documentBefore']['saved'] == result['documentAfter']['saved']
        and result['documentAfter']['stillActive'].get('value') is True)
    classes_complete = all(row.get('available') is True and all(row.get('members',{}).values()) for row in result['classes'].values())
    result['observationComplete'] = not result['errors'] and classes_complete and not result['environments'].get('truncated',False)
    result['status'] = 'OBSERVED' if result['observationComplete'] else 'PARTIAL'
    need(len(json.dumps(result,allow_nan=False).encode()) <= MAX_OUTPUT_BYTES,'Render probe output exceeds byte budget')
    return result


def camera_arguments(value):
    """Validate plain input before constructing any native temporary object."""
    require(isinstance(value, dict), 'invalid_input', 'camera must be an object.')
    common = {'type', 'space', 'unit', 'eye', 'target', 'upVector'}
    kind = value.get('type')
    lens = {'extents'} if kind == 'orthographic' else {'perspectiveAngle', 'angleUnit'}
    require(kind in ('orthographic', 'perspective') and set(value) == common | lens,
            'invalid_input', 'Supply one supported camera type with its exact geometry and lens fields.')
    require(value['space'] == 'world' and value['unit'] == 'mm', 'invalid_input', 'Camera positions use world-space mm.')

    def scalar(x):
        require(type(x) in (int, float) and math.isfinite(x) and abs(x) <= 1e9,
                'invalid_input', 'Camera geometry must be finite and bounded to 1e9 mm.')
        return x

    def vector(name):
        row = value[name]
        require(isinstance(row, list) and len(row) == 3, 'invalid_input', name + ' requires three numbers.')
        return [scalar(x) for x in row]

    eye, look, up = (vector(name) for name in ('eye', 'target', 'upVector'))
    direction = [b-a for a,b in zip(eye,look)]
    distance, up_length = math.hypot(*direction), math.hypot(*up)
    require(distance > 1e-9, 'invalid_input', 'Camera eye and target must differ.')
    require(abs(up_length-1) <= 1e-6 and abs(sum(a*b for a,b in zip(direction,up))/distance) <= 1e-6,
            'invalid_input', 'upVector must be a unit vector perpendicular to the view direction.')
    if kind == 'orthographic':
        extents = value['extents']
        require(isinstance(extents,list) and len(extents)==2, 'invalid_input', 'Orthographic extents require width and height in mm.')
        require(all(scalar(x) >= 1e-6 for x in extents), 'invalid_input', 'Orthographic extents must be positive.')
    else:
        require(value['angleUnit']=='deg' and 0 < scalar(value['perspectiveAngle']) < 180,
                'invalid_input', 'Perspective angle must be strictly between 0 and 180 degrees.')
    return value


def detached_camera(value):
    import adsk.core
    value = camera_arguments(value)
    try:
        camera = adsk.core.Camera.create()
        require(camera is not None, 'camera_unavailable', 'Camera.create returned no detached camera.')
        # Autodesk FUS-263280: transient Camera.isValid can be false even when
        # the exact Camera wrapper retains its geometry. Keep that flag as
        # evidence; validate type and every requested value independently.
        validity_before = camera.isValid
        require(type(validity_before) is bool and isinstance(camera, adsk.core.Camera)
                and camera.objectType == adsk.core.Camera.classType(),
                'camera_unavailable', 'Camera.create returned an unexpected wrapper/type or validity field.')
        camera.cameraType = getattr(adsk.core.CameraTypes, 'OrthographicCameraType' if value['type']=='orthographic' else 'PerspectiveCameraType')
        camera.eye = adsk.core.Point3D.create(*(x/10 for x in value['eye']))
        camera.target = adsk.core.Point3D.create(*(x/10 for x in value['target']))
        camera.upVector = adsk.core.Vector3D.create(*value['upVector'])
        camera.isFitView = False
        camera.isSmoothTransition = False
        if value['type'] == 'orthographic':
            require(camera.setExtents(*(x/10 for x in value['extents'])) is True,
                    'camera_readback_mismatch', 'Detached camera extent setter refused.')
        else:
            camera.perspectiveAngle = math.radians(value['perspectiveAngle'])
        def finite_native(number):
            require(type(number) in (int,float) and math.isfinite(number),
                    'camera_readback_mismatch', 'Detached camera returned a nonnumeric or nonfinite value.')
            return number
        result = {'type':value['type'], 'space':'world', 'unit':'mm',
                  'eye':[finite_native(getattr(camera.eye,k))*10 for k in ('x','y','z')],
                  'target':[finite_native(getattr(camera.target,k))*10 for k in ('x','y','z')],
                  'upVector':[finite_native(getattr(camera.upVector,k)) for k in ('x','y','z')]}
        expected_kind = getattr(adsk.core.CameraTypes, 'OrthographicCameraType' if value['type']=='orthographic' else 'PerspectiveCameraType')
        require(not isinstance(camera.cameraType,bool) and camera.cameraType == expected_kind and camera.isFitView is False and camera.isSmoothTransition is False,
                'camera_readback_mismatch', 'Detached camera mode or behavior differs.')
        if value['type'] == 'orthographic':
            okay,width,height = camera.getExtents()
            require(okay is True, 'camera_readback_mismatch', 'Detached camera extent read refused.')
            result['extents'] = [finite_native(width)*10,finite_native(height)*10]
        else:
            result.update(perspectiveAngle=math.degrees(finite_native(camera.perspectiveAngle)), angleUnit='deg')
        # Tolerance must never turn invalid native geometry into a valid camera.
        try:
            camera_arguments(result)
        except MechanicalError as error:
            raise MechanicalError('camera_readback_mismatch', 'Detached camera returned invalid geometry or lens: ' + str(error))
        def equal(left,right):
            if isinstance(right,list):
                return isinstance(left,list) and len(left)==len(right) and all(equal(a,b) for a,b in zip(left,right))
            if type(right) in (int,float):
                return type(left) in (int,float) and math.isfinite(left) and math.isclose(left,right,rel_tol=1e-10,abs_tol=1e-8)
            return type(left) is type(right) and left==right
        require(all(equal(result.get(k),v) for k,v in value.items()), 'camera_readback_mismatch', 'Detached camera did not retain the requested geometry or lens.')
        validity_after = camera.isValid
        require(type(validity_after) is bool, 'camera_readback_mismatch', 'Camera validity readback is not Boolean.')
        return {'available':True, 'readbackVerified':True, 'assignedToViewport':False,
                'nativeValidity':{'before':validity_before, 'after':validity_after,
                    'knownIssueReference':'FUS-263280', 'renderEligibility':'NOT_ESTABLISHED'},
                'scope':'independent temporary camera; current scene is unchanged', 'descriptor':result}
    except MechanicalError:
        raise
    except Exception as error:
        raise MechanicalError('camera_unavailable', 'Detached camera construction/readback failed: ' + str(error)[:512])


def render_scene_inspect(app, args):
    # This guard must precede target() and every native getter, including ownership.
    require(threading.current_thread() is threading.main_thread(), 'wrong_thread', 'Native render reads require the main thread.')
    require(args.get('sessionId') == SESSION_ID, 'session_mismatch', 'Scene inspection requires the current add-in session.')
    wanted = args.get('target')
    require(isinstance(wanted,dict) and isinstance(wanted.get('documentId'),str) and isinstance(wanted.get('revision'),str),
            'invalid_input', 'Scene inspection requires exact target documentId and revision.')
    budget = args.get('maxEnvironments',16)
    require(type(budget) is int and 0 <= budget <= MAX_ENVIRONMENTS, 'invalid_input', 'maxEnvironments must be an integer from 0 to 32.')
    if 'camera' in args: camera_arguments(args['camera'])
    document, design, value = target(app,args)
    first = scene_observation(app,document,design,max_environments=budget)
    prepared = detached_camera(args['camera']) if 'camera' in args else {'available':None, 'notRequested':True}
    second = scene_observation(app,document,design,max_environments=budget)
    _, _, current = target(app,args)
    require(value == current and first == second, 'scene_changed_during_inspection',
            'The model target or bounded scene observations changed during inspection; no settings were restored.')
    require(first['documentFlagsUnchanged'], 'scene_changed_during_inspection',
            'Unchanged saved/modified document flags could not be verified.')
    first.update(target=value, detachedCamera=prepared, observedStateUnchanged=True,
                 unchangedScope='available fields only; unreadable fields and omitted scene graphs remain unverified',
                 observationPasses=2, maxEnvironments=budget)
    require(len(json.dumps(first,allow_nan=False).encode('utf-8')) <= MAX_OUTPUT_BYTES,
            'output_too_large', 'Scene inspection exceeds its 128 KiB result budget.')
    return first


READS = {'render_scene_inspect': render_scene_inspect}
