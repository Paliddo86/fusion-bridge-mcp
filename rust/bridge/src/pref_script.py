
import json
gp = app.preferences.generalPreferences
T  = adsk.core.UserInterfaceThemes
PZ = adsk.core.PanZoomOrbitShortcuts
MO = adsk.core.DefaultModelingOrientations
DU = adsk.fusion.DistanceUnits

THEME = {'light':[T.LightGrayUserInterfaceTheme],'lightgray':[T.LightGrayUserInterfaceTheme],
         'dark':[T.DarkGrayUserInterfaceTheme,T.DarkBlueUserInterfaceTheme],
         'darkgray':[T.DarkGrayUserInterfaceTheme],'darkblue':[T.DarkBlueUserInterfaceTheme],
         'classic':[T.ClassicUserInterfaceTheme],'device':[T.DeviceUserInterfaceTheme],
         'auto':[T.DeviceUserInterfaceTheme]}
THEME_NAME  = {0:'classic',1:'lightgray',2:'darkblue',3:'darkgray',4:'device'}
ORBIT = {'fusion360':PZ.Fusion360PanZoomOrbitShortcut,'fusion':PZ.Fusion360PanZoomOrbitShortcut,
         'alias':PZ.AliasPanZoomOrbitShortcut,'inventor':PZ.InventorPanZoomOrbitShortcut,
         'solidworks':PZ.SolidWorksPanZoomOrbitShortcut,'tinkercad':PZ.TinkercadPanZoomOrbitShortcut,
         'powermill':PZ.PowerMillPanZoomOrbitShortcut}
ORBIT_NAME  = {0:'fusion360',1:'alias',2:'inventor',3:'solidworks',4:'tinkercad',5:'powermill'}
ORIENT = {'yup':MO.YUpModelingOrientation,'zup':MO.ZUpModelingOrientation}
ORIENT_NAME = {0:'yup',1:'zup'}
UNITS = {'mm':DU.MillimeterDistanceUnits,'cm':DU.CentimeterDistanceUnits,'m':DU.MeterDistanceUnits,
         'in':DU.InchDistanceUnits,'inch':DU.InchDistanceUnits,'ft':DU.FootDistanceUnits}
UNITS_NAME  = {0:'mm',1:'cm',2:'m',3:'in',4:'ft'}

def read_current():
    cur = {
      'theme': THEME_NAME.get(gp.userInterfaceTheme, gp.userInterfaceTheme),
      'activeTheme': THEME_NAME.get(gp.activeUserInterfaceTheme, gp.activeUserInterfaceTheme),
      'invertScrollZoom': bool(gp.isZoomDirectionReversed),
      'orbitScheme': ORBIT_NAME.get(gp.panZoomOrbitShortcuts, gp.panZoomOrbitShortcuts),
      'modelingOrientation': ORIENT_NAME.get(gp.defaultModelingOrientation, gp.defaultModelingOrientation),
      'gestureNav': bool(gp.isGestureBasedViewNavigationUsed),
      'cameraPivot': bool(gp.isCameraPivotEnabled),
    }
    try:
        cur['lengthUnit'] = UNITS_NAME.get(design.fusionUnitsManager.distanceDisplayUnits,
                                           design.fusionUnitsManager.distanceDisplayUnits)
    except Exception:
        pass
    return cur

def try_set(obj, attr, cands, want):
    err = None
    for c in cands:
        if c is None: continue
        try:
            setattr(obj, attr, c)
            return {'requested': want, 'ok': True}
        except Exception as e:
            err = str(e)
    return {'requested': want, 'ok': False, 'error': err or 'no valid value'}

reqs = json.loads(__REQS__)
applied = {}
for k, v in reqs.items():
    kv = v if isinstance(v, bool) else str(v).strip().lower()
    if k == 'theme':
        c = THEME.get(kv);  applied[k] = try_set(gp,'userInterfaceTheme', c or [], v) if c else {'requested':v,'ok':False,'error':'unknown theme (use light/darkblue/darkgray/classic/device/dark)'}
    elif k in ('invertScrollZoom','reverseZoom'):
        applied['invertScrollZoom'] = try_set(gp,'isZoomDirectionReversed',[bool(v)],v)
    elif k == 'orbitScheme':
        c = ORBIT.get(kv);  applied[k] = try_set(gp,'panZoomOrbitShortcuts',[c],v) if c is not None else {'requested':v,'ok':False,'error':'unknown orbitScheme'}
    elif k == 'modelingOrientation':
        c = ORIENT.get(kv); applied[k] = try_set(gp,'defaultModelingOrientation',[c],v) if c is not None else {'requested':v,'ok':False,'error':'unknown modelingOrientation (yup/zup)'}
    elif k == 'gestureNav':
        applied[k] = try_set(gp,'isGestureBasedViewNavigationUsed',[bool(v)],v)
    elif k == 'cameraPivot':
        applied[k] = try_set(gp,'isCameraPivotEnabled',[bool(v)],v)
    elif k == 'lengthUnit':
        c = UNITS.get(kv)
        if c is None: applied[k] = {'requested':v,'ok':False,'error':'unknown unit (mm/cm/m/in/ft)'}
        else:
            try: applied[k] = try_set(design.fusionUnitsManager,'distanceDisplayUnits',[c],v)
            except Exception as e: applied[k] = {'requested':v,'ok':False,'error':str(e)}
    else:
        applied[k] = {'requested': v, 'ok': False, 'error': 'unknown preference key'}

result = {'applied': applied, 'current': read_current()}
print(json.dumps(result))
