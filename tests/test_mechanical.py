"""Offline safety regressions; live kernel behavior has a separate host gate."""
import importlib
from email.message import Message
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace as NS
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
adsk = ModuleType('adsk'); adsk.core = ModuleType('adsk.core'); adsk.fusion = ModuleType('adsk.fusion'); adsk.cam = ModuleType('adsk.cam')
adsk.core.Application = object
adsk.fusion.Design = NS(cast=lambda value: value)
adsk.fusion.CalculationAccuracy = NS(LowCalculationAccuracy=0, MediumCalculationAccuracy=1, HighCalculationAccuracy=2, VeryHighCalculationAccuracy=3)
adsk.fusion.FeatureHealthStates = NS(HealthyFeatureHealthState=0)
adsk.fusion.JointDirections = NS(XAxisJointDirection=0, YAxisJointDirection=1, ZAxisJointDirection=2, CustomJointDirection=3)
for module in (adsk, adsk.core, adsk.fusion, adsk.cam): sys.modules[module.__name__] = module
pkg = ModuleType('mechanical_test_commands'); pkg.__path__ = [str(ROOT/'addin/AdomBridge/commands')]; sys.modules[pkg.__name__] = pkg
m = importlib.import_module('mechanical_test_commands.mechanical')
c = importlib.import_module('mechanical_test_commands.mechanical.core')
x = importlib.import_module('mechanical_test_commands.mechanical.exporting')
ops = importlib.import_module('mechanical_test_commands.mechanical.operations')
journal_module = importlib.import_module('mechanical_test_commands.mechanical.journal')
handoff = importlib.import_module('mechanical_test_commands.mechanical.handoff')
sys.path.insert(0, str(Path(__file__).resolve().parent))
import mech_exe

class Collection:
    def __init__(self, values=()): self.values=list(values)
    @property
    def count(self): return len(self.values)
    def item(self,i): return self.values[i]
    def itemByName(self,name): return next((v for v in self.values if v.name==name),None)

class Entity:
    def __init__(self, **kwargs): self.__dict__.update(isValid=True, **kwargs)


class MeshCollection(Collection):
    isValid = True
    objectType = 'adsk::fusion::MeshBodies'


def component(name, volume=0):
    props=NS(volume=volume,area=volume*6,mass=volume*0.002,centerOfMass=NS(x=1,y=2,z=3))
    body=Entity(getPhysicalProperties=lambda a:props)
    return Entity(name=name,revisionId='r1',meshBodies=MeshCollection(),bRepBodies=Collection([body] if volume else []),sketches=Collection(),occurrences=Collection(),allOccurrences=Collection(),partNumber='',description='',material=None)

def isolated_history(testcase):
    """A private journal directory, the exe's stand-in writing it, and the add-in reading it."""
    folder = tempfile.TemporaryDirectory(); testcase.addCleanup(folder.cleanup)
    root = Path(folder.name) / 'history'
    for change in (patch.object(journal_module, 'storage_root', return_value=root),
                   patch.object(handoff, 'storage_root', return_value=root),
                   patch.object(m, 'observed_document', return_value={'documentId':'owned-offline-document'})):
        change.start(); testcase.addCleanup(change.stop)
    proofs = patch.object(m, '_history', m._Proofs()); proofs.start(); testcase.addCleanup(proofs.stop)
    testcase.exe = mech_exe.Exe(m, str(root))


class Safety(unittest.TestCase):
    def setUp(self):
        c._entities.clear(); c._documents.clear(); isolated_history(self)

    def test_document_creation_id_and_name_cannot_select_copy(self):
        a=Entity(name='Same',creationId='copied');b=Entity(name='Same',creationId='copied')
        app=NS(activeDocument=a,activeProduct=NS(parentDocument=a))
        first=c.active(app)[2];app.activeDocument=b;app.activeProduct=NS(parentDocument=b)
        self.assertNotEqual(c.active(app)[2], first)
        with self.assertRaises(c.MechanicalError) as error: c.target(app,{'target':{'documentId':first,'revision':'anything'}},True)
        self.assertEqual(error.exception.code,'wrong_document')

    def test_child_revision_changes_target_even_when_root_unchanged(self):
        root=component('root');child=component('child')
        root.allOccurrences=Collection()  # as in Fusion, the design's rootComponent IS the root listed in allComponents
        design=NS(isValid=True,parentDocument=Entity(isSaved=False,isModified=True),allComponents=Collection([root,child]),rootComponent=root,allParameters=Collection(),timeline=NS(count=0,markerPosition=0))
        root.parentDesign=child.parentDesign=design
        before=c.revision(design);child.revisionId='r2';self.assertNotEqual(c.revision(design),before)

    def test_occurrence_suppression_comes_from_timeline_not_occurrence(self):
        root=component('root');child=component('child')
        occurrence=Entity(component=child,transform2=NS(asArray=lambda:[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]),
                          timelineObject=NS(isSuppressed=False),isGrounded=False)
        root.allOccurrences=Collection([occurrence])
        design=NS(isValid=True,parentDocument=Entity(isSaved=False,isModified=True),allComponents=Collection([root,child]),rootComponent=root,
                  allParameters=Collection(),timeline=NS(count=0,markerPosition=0))
        root.parentDesign=child.parentDesign=design
        before=c.revision(design); occurrence.timelineObject.isSuppressed=True
        self.assertNotEqual(c.revision(design),before)
        occurrence.timelineObject=None; self.assertIsNone(c.occurrence_suppression(occurrence))

    def test_stale_and_ambiguous_tokens_refuse(self):
        t={'documentId':'d','revision':'r'};entity=Entity(objectType='adsk::fusion::BRepFace')
        design=NS(findEntityByToken=lambda token:[entity,entity])
        reference={**t,'token':'face','objectType':entity.objectType}
        with self.assertRaises(c.MechanicalError) as error:c.resolve(design,t,reference)
        self.assertEqual(error.exception.code,'ambiguous_reference')
        reference['revision']='old'
        with self.assertRaises(c.MechanicalError) as error:c.resolve(design,t,reference)
        self.assertEqual(error.exception.code,'stale_reference')

    def test_compute_false_and_warning_are_not_healthy(self):
        design=NS(computeAll=lambda:False,timeline=Collection())
        self.assertFalse(c.health(design,True)['healthy'])
        design.timeline=Collection([NS(healthState=1,name='Bad extrude',errorOrWarningMessage='lost face')])
        design.computeAll=lambda:True
        self.assertFalse(c.health(design,True)['healthy'])

    def test_repeat_id_returns_original_after_timeout_and_conflict_refuses(self):
        writes=[]
        request={'op':'parameter_set','sessionId':m.SESSION_ID,'operationId':'one','target':{},'name':'x','expression':'1 mm'}
        with patch.dict(m.OPERATIONS,parameter_set=lambda app,args:writes.append(args['expression']) or {'done':True}):
            first=self.exe.call(None,request)
            retry=self.exe.call(None,dict(request))
            conflict=self.exe.call(None,{**request,'expression':'2 mm'})
        self.assertTrue(first['success']);self.assertTrue(retry['replayed']);self.assertEqual(writes,['1 mm'])
        self.assertEqual(conflict['errorCode'],'operation_conflict')
        self.assertEqual(self.exe.status(request)['result'],first)

    def test_transport_metadata_is_excluded_from_payload_and_retry_fingerprint(self):
        writes = []
        request = {'op':'parameter_set','sessionId':m.SESSION_ID,'operationId':'transport',
                   'target':{},'name':'x','expression':'1 mm'}
        with patch.dict(m.OPERATIONS, parameter_set=lambda app,args:writes.append(args) or {'done':True}):
            first = self.exe.call(None, {**request,'caller':{'sessionId':'relay'},'_callerBudgetMs':60000})
            retry = self.exe.call(None, {**request,'caller':{'sessionId':'relay'},'_callerBudgetMs':5000})
            typo = self.exe.call(None, {**request,'unexpectedTransportField':True})
        self.assertTrue(first['success']); self.assertTrue(retry['replayed'])
        self.assertEqual(writes, [request]); self.assertEqual(typo['errorCode'],'invalid_input')

    def test_old_session_and_invalid_boolean_fail_before_handler(self):
        request={'op':'health','target':{},'recompute':'false'}
        self.assertEqual(self.exe.call(None,request)['errorCode'],'invalid_input')
        request={'op':'new_design','sessionId':'old','operationId':'one'}
        self.assertEqual(self.exe.call(None,request)['errorCode'],'session_mismatch')

    def test_failed_parameter_edit_restores_original_with_readback(self):
        p=Entity(name='depth',expression='10 mm',unit='mm')
        design=NS(allParameters=Collection([p]),unitsManager=NS(isValidExpression=lambda *a:True))
        def check(*args):
            if p.expression=='-1 mm':raise c.MechanicalError('unhealthy_model','bad feature')
            return {'healthy':True}
        compute=importlib.import_module('mechanical_test_commands.mechanical.compute');compute.reset()
        with patch.object(ops,'target',return_value=(None,design,{'documentId':'d','revision':'r'})),patch.object(compute,'health',side_effect=lambda d,recompute=False:{'healthy':p.expression!='-1 mm'}),patch.object(ops,'require_healthy',side_effect=check),patch.object(ops,'recovery_snapshot',side_effect=lambda d,health_state=None:{'expression':p.expression}),patch.object(ops,'current_target',return_value={}):
            with self.assertRaises(c.MechanicalError) as error:ops.parameter_set(None,{'name':'depth','expression':'-1 mm'})
        self.assertEqual(p.expression,'10 mm');self.assertTrue(error.exception.details['recovery']['verified'])

class Exports(unittest.TestCase):
    def test_unlink_failure_after_hardlink_reports_published(self):
        with tempfile.TemporaryDirectory() as folder:
            destination = Path(folder) / 'out.step'; staged = Path(folder) / 'stage.step'
            staged.write_bytes(b'published model')
            with patch.object(Path, 'unlink', side_effect=OSError('staging file locked')):
                result = x.publish_staged(staged, destination, False)
            self.assertTrue(result['published']); self.assertIn('cleanupWarning', result)
            self.assertEqual(destination.read_bytes(), b'published model')

    def test_3mf_disables_printer_launch_and_multiple_outputs(self):
        with tempfile.TemporaryDirectory() as folder:
            destination = Path(folder) / 'out.3mf'
            options = NS(sendToPrintUtility=True, isOneFilePerBody=True)
            def factory(root, path): options.path=path; return options
            def execute(value):
                self.assertFalse(value.sendToPrintUtility); self.assertFalse(value.isOneFilePerBody)
                Path(value.path).write_bytes(b'3mf model'); return True
            design = NS(rootComponent=None, exportManager=NS(createC3MFExportOptions=factory, execute=execute))
            self.assertTrue(x.export_design(design, {'format':'3mf','outputPath':str(destination)})['published'])

    def test_missing_capability_preserves_existing_bytes(self):
        with tempfile.TemporaryDirectory() as folder:
            destination=Path(folder)/'out.fbx';destination.write_bytes(b'original')
            with self.assertRaises(c.MechanicalError):x.export_design(NS(exportManager=NS()),{'format':'fbx','outputPath':str(destination),'overwrite':True})
            self.assertEqual(destination.read_bytes(),b'original')

    def exercise(self,mode):
        with tempfile.TemporaryDirectory() as folder:
            destination=Path(folder)/'out.step';destination.write_bytes(b'original')
            def execute(path):
                if mode=='exception': raise RuntimeError('kernel failed')
                if mode!='empty':Path(path).write_bytes(b'new geometry')
                return mode!='false'
            manager=NS(createSTEPExportOptions=lambda path:path,execute=execute)
            if mode=='good':
                result=x.export_design(NS(exportManager=manager),{'format':'step','outputPath':str(destination),'overwrite':True})
                self.assertTrue(result['published']);self.assertEqual(destination.read_bytes(),b'new geometry')
            else:
                with self.assertRaises(Exception):x.export_design(NS(exportManager=manager),{'format':'step','outputPath':str(destination),'overwrite':True})
                self.assertEqual(destination.read_bytes(),b'original')
            self.assertEqual(list(Path(folder).iterdir()),[destination])
    def test_false_preserves(self):self.exercise('false')
    def test_exception_preserves(self):self.exercise('exception')
    def test_empty_preserves(self):self.exercise('empty')
    def test_success_publishes(self):self.exercise('good')
    def test_exclusive_publication_cannot_clobber_racing_writer(self):
        with tempfile.TemporaryDirectory() as folder:
            destination=Path(folder)/'out.step';staged=Path(folder)/'stage.step'
            destination.write_bytes(b'racing writer');staged.write_bytes(b'ours')
            with self.assertRaises(FileExistsError):x.publish_staged(staged,destination,False)
            self.assertEqual(destination.read_bytes(),b'racing writer')


class Readbacks(unittest.TestCase):
    """The 2026-10-27 certification gaps: base planes, plane re-reads, sketch frames, parameter
    owners and the material search."""
    def setUp(self):
        c._entities.clear()

    def root(self):
        comp = Entity(objectType='adsk::fusion::Component', name='root', entityToken='tok-root')
        design = NS(rootComponent=comp)
        comp.parentDesign = design
        for key, attr in c.ORIGIN_PLANES.items():
            setattr(comp, attr, Entity(objectType='adsk::fusion::ConstructionPlane', name=key.upper(), entityToken='tok-' + key, component=comp))
        return comp, design

    def test_a_base_plane_is_a_reference_that_resolves_through_its_component(self):
        comp, design = self.root()
        value = {'documentId': 'd', 'revision': 'r'}
        r = c.ref(comp.xZConstructionPlane, value)
        self.assertEqual(r['originPlane'], 'xz')
        self.assertEqual(r['component']['rootComponentId'], c.identity(comp))
        self.assertIs(c.resolve(design, value, r, ('BRepFace', 'ConstructionPlane')), comp.xZConstructionPlane)
        with self.assertRaises(c.MechanicalError) as e: c.resolve(design, value, r, ('BRepBody',))
        self.assertEqual(e.exception.code, 'wrong_entity_type')
        with self.assertRaises(c.MechanicalError) as e: c.resolve(design, {'documentId': 'd', 'revision': 'r2'}, r)
        self.assertEqual(e.exception.code, 'stale_reference')
        # an ordinary construction plane keeps its plain token reference
        offset = Entity(objectType='adsk::fusion::ConstructionPlane', name='Plane1', entityToken='tok-p1', component=comp)
        self.assertNotIn('originPlane', c.ref(offset, value))

    def test_a_parameter_names_its_owner(self):
        units = NS(isValidExpression=lambda expr, unit: expr == '1 mm', convert=lambda v, a, b: v * 10)
        design = NS(unitsManager=units)
        owned = Entity(objectType='adsk::fusion::ModelParameter', name='d3', expression='31.754 mm', unit='mm', value=3.1754,
                       createdBy=Entity(objectType='adsk::fusion::ExtrudeFeature', name='Tongue'), role='AlongDistance')
        row = c.parameter_data(design, owned)
        self.assertEqual((row['kind'], row['owner']['name'], row['owner']['role']), ('model', 'Tongue', 'AlongDistance'))
        user = Entity(objectType='adsk::fusion::UserParameter', name='width', expression='40 mm', unit='mm', value=4.0)
        self.assertEqual((c.parameter_data(design, user)['kind'], c.parameter_data(design, user)['owner']), ('user', None))

    def test_a_sketch_frame_maps_sketch_space_to_the_component(self):
        rb = importlib.import_module('mechanical_test_commands.mechanical.readback')
        class P:
            def __init__(s, x, y, z): s.x, s.y, s.z = x, y, z
            def copy(s): return P(s.x, s.y, s.z)
            def transformBy(s, m): s.x, s.y, s.z = s.x, 0.0, -s.y   # the xz plane: (sx, sy) -> (sx, 0, -sy)
        xz = NS(getAsCoordinateSystem=lambda: (P(0, 0, 0), P(1, 0, 0), P(0, 0, -1), P(0, 1, 0)))
        frame = rb.frame_data(xz)
        self.assertEqual((frame['xDirection'], frame['yDirection'], frame['normal']), ([1, 0, 0], [0, 0, -1], [0, 1, 0]))
        self.assertEqual(rb.to_component(NS(transform=xz), P(1.0, 2.0, 0.0)), [10.0, 0.0, -20.0])

    def test_the_material_search_reaches_the_libraries(self):
        purchased = importlib.import_module('mechanical_test_commands.mechanical.purchased')
        mats = lambda *names: Collection([Entity(id='id-' + n, name=n) for n in names])
        library = Entity(id='lib-fusion', name='Fusion Material Library', materials=mats('Copper', 'Steel'))
        app = NS(materialLibraries=Collection([library]))
        design = NS(materials=mats('Aluminum 6061'))
        with patch.object(purchased, 'target', return_value=(None, design, {'documentId': 'd', 'revision': 'r'})):
            found = purchased.material_list(app, {'query': 'copper'})
            self.assertEqual(found['materials'], [{'id': 'id-Copper', 'name': 'Copper', 'libraryId': 'lib-fusion'}])
            own = purchased.material_list(app, {'query': 'alum'})['materials']
            self.assertEqual(own, [{'id': 'id-Aluminum 6061', 'name': 'Aluminum 6061', 'libraryId': None}])
            scoped = purchased.material_list(app, {'query': 'steel', 'libraryId': 'lib-fusion'})['materials']
            self.assertEqual([r['name'] for r in scoped], ['Steel'])


class ReferencedOccurrences(unittest.TestCase):
    def test_suppression_inside_a_referenced_design_is_decided_in_its_source(self):
        class Proxy(Entity):
            @property
            def timelineObject(self):
                raise RuntimeError('2 : InternalValidationError : dcInstance')
        ref_root = Entity(isReferencedComponent=True, assemblyContext=None)
        inner = Proxy(isReferencedComponent=False, assemblyContext=ref_root)
        self.assertIsNone(c.occurrence_suppression(inner))
        stray = Proxy(isReferencedComponent=False, assemblyContext=None)
        with self.assertRaises(RuntimeError):
            c.occurrence_suppression(stray)
        self.assertTrue(c.occurrence_suppression(Entity(timelineObject=NS(isSuppressed=True))))

    def test_a_component_patterns_copies_take_suppression_from_their_element(self):
        class Proxy(Entity):
            @property
            def timelineObject(self):
                raise RuntimeError('2 : InternalValidationError : dcInstance')
        copy = Proxy(isReferencedComponent=False, assemblyContext=None, fullPathName='Member:3')
        element = NS(isSuppressed=True, occurrences=[NS(fullPathName='Member:3')])
        pattern = NS(patternElements=Collection([NS(isSuppressed=False, occurrences=[NS(fullPathName='Member:1')]), element]))
        root = NS(features=NS(rectangularPatternFeatures=Collection([pattern]), circularPatternFeatures=Collection([])))
        copy.component = NS(parentDesign=NS(rootComponent=root))
        self.assertTrue(c.occurrence_suppression(copy))


class NativeProperties(unittest.TestCase):
    """partNumber and description are cloud reads (~0.6 s each per component on a cloud document)."""
    def setUp(self):
        c._entities.clear()
        self.astate = importlib.import_module('mechanical_test_commands.mechanical.assembly_state')
        self.astate._native_properties.clear()

    def component(self, reads):
        class Comp(Entity):
            @property
            def partNumber(self):
                reads.append('pn'); return 'ADM-1'
            @property
            def description(self):
                reads.append('desc'); return 'Bracket'
        return Comp(objectType='adsk::fusion::Component', name='Bracket', revisionId='r1', attributes=Collection(), material=None, entityToken='tok-bracket')

    def test_the_revision_witness_never_reads_them(self):
        reads = []
        comp = self.component(reads)
        row = self.astate.metadata(comp, native=False)
        self.assertEqual((row['partNumber'], row['description'], reads), (None, None, []))

    def test_they_are_read_once_per_component_revision(self):
        reads = []
        comp = self.component(reads)
        self.assertEqual(self.astate.metadata(comp)['partNumber'], 'ADM-1')
        self.astate.metadata(comp)
        self.assertEqual(reads, ['pn', 'desc'])
        comp.revisionId = 'r2'
        self.astate.metadata(comp)
        self.assertEqual(reads, ['pn', 'desc', 'pn', 'desc'])


class IdentityRegistry(unittest.TestCase):
    """2026-10-28: every lookup compared against every identity in the session, so the revision of a
    1,249-component assembly ran for many minutes. Lookups now compare a few candidates."""
    def setUp(self):
        c._entities.clear()

    def entity(self, name, token, owner="Owner"):
        return Entity(objectType='adsk::fusion::BRepBody', name=name, entityToken=token, parentComponent=NS(name=owner))

    def test_same_object_same_identity_and_same_names_stay_distinct(self):
        a, b = self.entity('Body1', 't-a'), self.entity('Body1', 't-b')
        self.assertEqual(c.identity(a), c.identity(a))
        self.assertNotEqual(c.identity(a), c.identity(b))

    def test_a_renamed_entity_is_found_by_its_token(self):
        a = self.entity('Body1', 't-a')
        key = c.identity(a)
        a.name = 'Renamed'
        self.assertEqual(c.identity(a), key)

    def test_a_reset_registry_forgets_its_indexes(self):
        a = self.entity('Body1', 't-a')
        key = c.identity(a)
        c._entities.clear()
        self.assertNotEqual(c.identity(a), key)

    def test_a_lookup_compares_a_few_candidates_not_the_session(self):
        for i in range(3000):
            c.identity(self.entity('Body%d' % i, 't-%d' % i, owner='C%d' % (i % 50)))
        probe = self.entity('Body1500', 't-new', owner='C0')
        calls = []
        real = c.same
        with patch.object(c, 'same', side_effect=lambda x, y: calls.append(1) or real(x, y)):
            c.identity(probe)
        self.assertLess(len(calls), 10)


class Patterns(unittest.TestCase):
    """Rectangular and circular patterns repeat bodies, features, faces or occurrences. A feature or
    face pattern beats a sketch pattern by orders of magnitude on big grids, and a component pattern
    keeps one definition, so both are what an efficient design is built from."""
    OPTIONS = NS(AdjustPatternCompute=0, IdenticalPatternCompute=1, OptimizedPatternCompute=2)

    def setUp(self):
        self.f = importlib.import_module('mechanical_test_commands.mechanical.features')
        self.entities = {}
        def resolve(design, value, r, allowed=()):
            entity = self.entities[r['token']]
            if allowed and entity.objectType.split('::')[-1] not in allowed:
                raise c.MechanicalError('wrong_entity_type', 'Operation does not accept this entity type.')
            return entity
        for target, name, replacement in (
                (adsk.fusion, 'PatternComputeOptions', self.OPTIONS),
                (adsk.fusion, 'PatternDistanceType', NS(SpacingPatternDistanceType='spacing')),
                (self.f, 'resolve', resolve), (self.f, 'collection', list),
                (self.f, 'quantity', lambda design, value, unit='mm', positive=False: value),
                (self.f, 'axis', lambda design, value, comp, selector: ('axis', comp.name, selector)),
                (self.f, 'feature_data', lambda feature, fresh: {'name': feature.name}),
                (self.f, 'topology_data', lambda body, fresh: {})):
            change = patch.object(target, name, replacement, create=True)
            change.start(); self.addCleanup(change.stop)
        self.root = self.component('root')
        self.design = NS(rootComponent=self.root)
        self.value = {'documentId': 'd', 'revision': 'r'}

    def component(self, name):
        comp = Entity(objectType='adsk::fusion::Component', name=name, occurrences=Collection())
        made = []
        class Group:
            def createInput(group, entities, axis, *rest):
                inp = Entity(entities=entities, axis=axis, rest=rest, patternComputeOption=None, setDirectionTwo=lambda *a: True)
                made.append(inp)
                return inp
            def add(group, inp):
                count = inp.rest[0] if inp.rest else inp.quantity
                return Entity(name='Pattern', bodies=Collection(), patternElements=Collection(range(count)),
                              patternComputeOption=inp.patternComputeOption)
        comp.features = NS(rectangularPatternFeatures=Group(), circularPatternFeatures=Group())
        comp.made = made
        return comp

    def add(self, token, **kwargs):
        self.entities[token] = Entity(entityToken=token, **kwargs)
        return {'token': token, 'documentId': 'd', 'revision': 'r', 'objectType': kwargs.get('objectType')}

    def create(self, **args):
        return self.f.feature_create(self.design, self.value, args)(self.value)

    def test_a_feature_pattern_computes_identical_by_default(self):
        hole = self.add('hole', objectType='adsk::fusion::HoleFeature', timelineObject=NS(), parentComponent=self.root, assemblyContext=None)
        result = self.create(type='rectangular_pattern', features=[hole], axis='x', count=4, spacing=20)
        self.assertEqual(result['pattern'], {'patterned': 'features', 'computeOption': 'identical', 'instanceCount': 4})
        inp = self.root.made[-1]
        self.assertEqual((inp.entities, inp.patternComputeOption), ([self.entities['hole']], self.OPTIONS.IdenticalPatternCompute))

    def test_each_compute_option_reaches_fusion(self):
        body = self.add('body', objectType='adsk::fusion::BRepBody', parentComponent=self.root, assemblyContext=None)
        for name, value in (('identical', 1), ('optimized', 2), ('adjust', 0)):
            result = self.create(type='circular_pattern', bodies=[body], axis='z', count=6, computeOption=name)
            self.assertEqual((self.root.made[-1].patternComputeOption, result['pattern']['computeOption']), (value, name))

    def test_an_occurrence_pattern_is_built_in_their_parent(self):
        frame = self.component('Frame')
        frame_occurrence = Entity(objectType='adsk::fusion::Occurrence', component=frame)
        natives = [Entity(objectType='adsk::fusion::Occurrence', name='T-nut:%d' % i) for i in (1, 2)]
        refs = [self.add('tnut%d' % i, objectType='adsk::fusion::Occurrence', assemblyContext=frame_occurrence, nativeObject=natives[i])
                for i in (0, 1)]
        result = self.create(type='rectangular_pattern', occurrences=refs, axis='x', count=5, spacing=40)
        self.assertEqual((len(frame.made), len(self.root.made)), (1, 0))
        inp = frame.made[0]
        self.assertEqual((inp.entities, inp.axis, inp.patternComputeOption), (natives, ('axis', 'Frame', 'x'), None))
        self.assertEqual(result['pattern'], {'patterned': 'occurrences', 'computeOption': None, 'instanceCount': 5})

    def test_a_root_occurrence_is_patterned_in_the_root(self):
        bracket = Entity(objectType='adsk::fusion::Occurrence', name='Bracket:1', assemblyContext=None)
        self.root.occurrences = Collection([bracket])
        self.entities['bracket'] = bracket
        self.create(type='circular_pattern', occurrences=[{'token': 'bracket'}], axis='z', count=4)
        self.assertEqual(self.root.made[-1].entities, [bracket])

    def test_mixed_or_missing_inputs_and_mixed_parents_are_refused(self):
        body = self.add('body', objectType='adsk::fusion::BRepBody', parentComponent=self.root, assemblyContext=None)
        hole = self.add('hole', objectType='adsk::fusion::HoleFeature', timelineObject=NS(), parentComponent=self.root, assemblyContext=None)
        sketch = self.add('sketch', objectType='adsk::fusion::Sketch', parentComponent=self.root, assemblyContext=None)
        context = lambda name: Entity(objectType='adsk::fusion::Occurrence', component=self.component(name))
        a = self.add('a', objectType='adsk::fusion::Occurrence', assemblyContext=context('Left'), nativeObject=Entity())
        b = self.add('b', objectType='adsk::fusion::Occurrence', assemblyContext=context('Right'), nativeObject=Entity())
        for args, code in ((dict(bodies=[body], features=[hole]), 'mixed_pattern_inputs'), (dict(), 'invalid_selection'),
                           (dict(occurrences=[a, b]), 'mixed_parents'), (dict(features=[sketch]), 'wrong_entity_type')):
            with self.assertRaises(c.MechanicalError) as e:
                self.create(type='rectangular_pattern', axis='x', count=3, spacing=10, **args)
            self.assertEqual(e.exception.code, code, args)

    def test_the_schema_takes_the_new_inputs_and_checks_the_option(self):
        schema = json.loads((ROOT / 'addin/AdomBridge/commands/mechanical/schema.json').read_text(encoding='utf-8'))['feature_create']
        ref = {'documentId': 'd', 'revision': 'r', 'objectType': 'adsk::fusion::HoleFeature'}
        base = {'op': 'feature_create', 'sessionId': 's', 'operationId': 'o', 'target': {'documentId': 'd', 'revision': 'r'},
                'type': 'rectangular_pattern', 'axis': 'x', 'count': 3, 'spacing': 10}
        for key in ('features', 'faces', 'occurrences', 'bodies'):
            mech_exe.validate({**base, key: [ref], 'computeOption': 'optimized'}, schema)
        with self.assertRaises(mech_exe.MechanicalError):
            mech_exe.validate({**base, 'features': [ref], 'computeOption': 'fastest'}, schema)
        # the exe refuses any key the operation's top-level properties do not list
        self.assertLessEqual({'features', 'faces', 'occurrences', 'computeOption'}, set(schema['properties']))

if __name__=='__main__':unittest.main()
