"""Offline native-adapter fault tests; no Fusion, graphics display or host access."""
import copy
import importlib
import threading
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch
from test_mechanical import c, Entity, Collection, MeshCollection
from test_mechanical_exploded_view import fixture as assembly_fixture, pose

p = importlib.import_module('mechanical_test_commands.mechanical.exploded_preview_prototype')
plan = importlib.import_module('mechanical_test_commands.mechanical.exploded_view')
real_revision = c.revision


def enrich_blank(document, design, root):
    """Native-like blank data for the actual core aggregate and presentation witness."""
    document.attributes = Collection()
    document.name = 'Untitled'
    document.creationId = 'native-presentation'
    design.attributes = Collection()
    design.designType = design.designIntent = 0
    design.isConfiguredDesign = design.isConfiguration = False
    design.configurationRowId = ''
    design.timeline.markerPosition = 0
    design.snapshots = Collection()
    design.snapshots.hasPendingSnapshot = False
    for name in ('allOccurrences', 'joints', 'asBuiltJoints', 'jointOrigins', 'rigidGroups', 'motionLinks',
                 'constructionAxes', 'constructionPlanes', 'constructionPoints', 'modelParameters', 'canvases', 'decals'):
        if not hasattr(root, name):
            setattr(root, name, Collection())
    for name in ('isBodiesFolderLightBulbOn', 'isConstructionFolderLightBulbOn', 'isOriginFolderLightBulbOn',
                 'isSketchFolderLightBulbOn', 'isJointsFolderLightBulbOn', 'isJointOriginsFolderLightBulbOn',
                 'isCanvasFolderLightBulbOn', 'isDecalFolderLightBulbOn'):
        setattr(root, name, True)
    root.attributes = Collection()
    root.name = 'Presentation'
    root.partNumber = root.description = ''
    root.material = None
    for field, kind, vector in (
        ('xConstructionAxis', 'ConstructionAxis', (1., 0., 0.)),
        ('yConstructionAxis', 'ConstructionAxis', (0., 1., 0.)),
        ('zConstructionAxis', 'ConstructionAxis', (0., 0., 1.)),
        ('xYConstructionPlane', 'ConstructionPlane', (0., 0., 1.)),
        ('xZConstructionPlane', 'ConstructionPlane', (0., 1., 0.)),
        ('yZConstructionPlane', 'ConstructionPlane', (1., 0., 0.)),
        ('originConstructionPoint', 'ConstructionPoint', None)):
        xyz = NS(x=0., y=0., z=0.)
        geometry = xyz if vector is None else NS(origin=xyz, direction=NS(x=vector[0], y=vector[1], z=vector[2]), normal=NS(x=vector[0], y=vector[1], z=vector[2]))
        setattr(root, field, Entity(objectType='adsk::fusion::' + kind, name=field, component=root, entityToken='origin-' + field,
            nativeObject=None, isDeletable=False, isLightBulbOn=False, isVisible=False,
            geometry=geometry, attributes=Collection()))


class PresentationDesign(Entity):
    @property
    def rev(self):
        return self.revision_override if self.revision_override is not None else real_revision(self)
    @rev.setter
    def rev(self, value):
        self.revision_override = None if value == 'blank' else value


class Matrix:
    def __init__(self, values=None): self.values = list(values or p.IDENTITY)
    def setWithArray(self, values): self.values = list(values); return True
    def asArray(self): return list(self.values)


def body(component=None, transient=False):
    points = [(0.,0.,0.), (1.,0.,0.), (0.,1.,0.), (0.,0.,1.)]
    result = Entity(objectType='adsk::fusion::BRepBody', isTransient=transient, isSolid=True,
                    parentComponent=component, nativeObject=None, isVisible=True, volume=1/6,
                    boundingBox=NS(minPoint=NS(x=0.,y=0.,z=0.), maxPoint=NS(x=1.,y=1.,z=1.)),
                    faces=Collection([object()]*4), edges=Collection([object()]*6),
                    vertices=Collection([Entity(objectType="adsk::fusion::BRepVertex",geometry=NS(x=x,y=y,z=z)) for x,y,z in points]))
    def proxy(occ):
        return Entity(objectType='adsk::fusion::BRepBody', nativeObject=result, assemblyContext=occ,
                      isVisible=getattr(occ,'bodyVisible',True))
    result.createForAssemblyContext=proxy
    return result


class Graphics(Collection):
    def __init__(self, parent=None, root_collection=None):
        super().__init__(); self.parent=parent; self.root_collection=root_collection
        self.isValid=True; self.id=''; self.isVisible=True; self.isSelectable=True; self.isChildrenSelectable=True
        self.transform=Matrix(); self.objectType='adsk::fusion::CustomGraphicsGroup'
        self.fail_transform=False; self.delete_ack=True; self.ignore_delete=False; self.null_body=False
        self.ignore_transform=False; self._transform=Matrix()
    @property
    def transform(self): return self._transform
    @transform.setter
    def transform(self, value):
        if getattr(self,'fail_transform',False):raise RuntimeError('native transform refused')
        if not getattr(self,'ignore_transform',False):self._transform=Matrix(value.asArray())
        self.effect()
    def effect(self):
        if self.root_collection is not None:
            if self.root_collection.on_effect is not None:self.root_collection.on_effect()
        elif isinstance(self.parent, Graphics):self.parent.effect()
    def addGroup(self):
        group=Graphics(self);self.values.append(group);self.effect();return group
    def addBRepBody(self,native):
        if self.null_body:return None
        copied=copy.deepcopy(native);copied.isTransient=True;copied.parentComponent=None
        graphic=Entity(objectType='adsk::fusion::CustomGraphicsBRepBody',parent=self,isVisible=True,
                       transform=Matrix(),bRepBody=copied)
        self.values.append(graphic);self.effect();return graphic
    def deleteMe(self):
        if self.delete_ack and not self.ignore_delete:
            self.root_collection.values.remove(self);self.isValid=False;self.effect()
        return self.delete_ack


class Roots(Collection):
    def __init__(self,component):super().__init__();self.component=component;self.fail=False;self.on_effect=None
    def add(self):
        if self.fail:return None
        value=Graphics(self.component,self);self.values.append(value);value.effect();return value


class PreviewTests(unittest.TestCase):
    def setUp(self):
        c._documents.clear();c._entities.clear();self.f=assembly_fixture()
        for comp in (self.f.root,self.f.shared,self.f.pin):
            comp.meshBodies=Collection();comp.bRepBodies=Collection([body(comp)])
        for occ in self.f.root.allOccurrences.values:
            occ.isVisible=True;occ.timelineObject=NS(isSuppressed=False)
        self.f.design.parentDocument=self.f.doc
        self.f.design.rev='source-rev'
        self.f.doc.doc_id='source';self.f.doc.objectType='adsk::core::Document'
        self.f.value={'documentId':'source','revision':'source-rev'}
        presentation_doc=Entity(objectType='adsk::core::Document',isSaved=False,isModified=False,doc_id='presentation')
        root=Entity(objectType='adsk::fusion::Component',bRepBodies=Collection(),meshBodies=MeshCollection(),
                    occurrences=Collection(),sketches=Collection(),features=Collection(),revisionId='blank')
        root.customGraphicsGroups=Roots(root)
        design=PresentationDesign(parentDocument=presentation_doc,rootComponent=root,allComponents=Collection([root]),
                      allParameters=Collection(),timeline=NS(count=0),revision_override=None)
        root.parentDesign=design
        enrich_blank(presentation_doc, design, root)
        self.doc,self.design,self.root=presentation_doc,design,root
        self.app=NS(documents=Collection([self.f.doc,self.doc]),activeDocument=self.f.doc)
        def activate(doc):self.app.activeDocument=doc;return True
        self.f.doc.activate=lambda:activate(self.f.doc);self.doc.activate=lambda:activate(self.doc)
        patches=[patch.object(p,'document_identity',side_effect=lambda d:d.doc_id),
                 patch.object(p,'revision',side_effect=lambda d:d.rev),
                 patch.object(p,'target',return_value=(self.f.doc,self.f.design,self.f.value)),
                 patch.object(plan,'target',return_value=(self.f.doc,self.f.design,self.f.value)),
                 patch.object(plan,'configuration_snapshot',return_value={'configured':False,'isConfiguration':False,'configurationRowId':'','table':None,'complete':True}),
                 patch.object(p.adsk.core,'Matrix3D',NS(create=Matrix),create=True)]
        for item in patches:item.start();self.addCleanup(item.stop)
        self.args={'op':'exploded_view_plan','sessionId':p.SESSION_ID,'target':self.f.value,'unit':'mm',
            'sampleTimes':[.5], 'steps':[{'durationSeconds':1,'moves':[
                {'occurrence':c.ref(self.f.a,self.f.value),'direction':[1,0,0],'distance':10,'frame':'local'},
                {'occurrence':c.ref(self.f.ap,self.f.value),'direction':[1,0,0],'distance':5,'frame':'parent'}]}]}
        self.generated=plan.exploded_view_plan(self.app,self.args)
        self.receipt={'success':True,'ownerId':'native-owned-run','sessionId':p.SESSION_ID,'documentId':'presentation'}
    def prepared(self):return p.prepare(self.app,self.args,self.generated['planId'])
    def lease(self):return p.OwnedPresentationLease(self.app,self.doc,self.design,'native-owned-run',lambda:copy.deepcopy(self.receipt))
    def preview(self):return p.Preview(self.prepared(),self.lease())

    def enable_revision_effects(self):
        effects = []
        def changed():
            effects.append(1)
            self.root.revisionId = 'owned-graphics-' + str(len(effects))
        self.root.customGraphicsGroups.on_effect = changed
        return effects

    def test_owned_effects_advance_only_private_presentation_revision(self):
        effects = self.enable_revision_effects()
        preview = self.preview()
        initial = copy.deepcopy(preview.lease.verified_state)
        preview.create()
        for sequence, index in enumerate((0, 2, 0), 2):
            before = preview.lease.model_revision
            preview.sample(index)
            self.assertNotEqual(preview.lease.model_revision, before)
            self.assertEqual(preview.lease.model_revision, real_revision(self.design))
            self.assertEqual(preview.lease.sequence, sequence)
            self.assertEqual(preview.lease.verified_state['witnessDigest'], initial['witnessDigest'])
        before_show = preview.lease.model_revision
        preview.activate()
        self.assertEqual(preview.lease.model_revision, before_show)
        preview.dispose()
        self.assertEqual(preview.lease.sequence, 5)
        self.assertEqual(preview.lease.model_revision, real_revision(self.design))
        self.assertNotEqual(preview.lease.model_revision, initial['revision'])
        self.assertEqual(self.f.design.rev, 'source-rev')
        self.assertGreater(len(effects), 5)

    def test_a_moved_scope_plan_of_a_big_design_is_refused_before_any_copy(self):
        with patch.object(plan, 'MAX_INSTANCES', 0):
            generated = plan.exploded_view_plan(self.app, self.args)
            self.assertEqual(generated['plan']['instanceScope'], 'moved')
            with self.assertRaises(c.MechanicalError) as error:
                p.prepare(self.app, self.args, generated['planId'])
        self.assertEqual(error.exception.code, 'preview_budget')
        self.assertIn('exploded_view_stills', str(error.exception))
        self.assertEqual(self.root.customGraphicsGroups.count, 0)

    def test_native_top_group_parent_shape_does_not_replace_inventory_proof(self):
        preview = self.preview()
        preview.create()
        preview.root.parent = Graphics()  # Measured Fusion top-level getter shape.
        preview.sample(2)
        exact = preview.root
        foreign = Graphics(self.root, self.root.customGraphicsGroups)
        self.root.customGraphicsGroups.values[0] = foreign
        with self.assertRaises(c.MechanicalError):
            preview.dispose()
        self.assertTrue(exact.isValid)
        self.assertTrue(foreign.isValid)

    def test_unbracketed_opaque_revision_change_is_never_adopted(self):
        preview = self.preview(); preview.create()
        verified = copy.deepcopy(preview.lease.verified_state)
        self.root.revisionId = 'unrelated-edit'
        self.assertEqual(p.presentation_observation(self.doc, self.design)['witnessDigest'], verified['witnessDigest'])
        for operation in (lambda: preview.sample(0), preview.activate, preview.dispose):
            with self.assertRaises(c.MechanicalError):operation()
        self.assertEqual(preview.lease.verified_state, verified)
        self.assertFalse(preview._delete_issued)

    def test_same_count_external_attributes_and_origin_changes_refuse(self):
        preview = self.preview(); preview.create()
        for owner in (self.doc, self.design, self.root, self.root.originConstructionPoint):
            with self.subTest(owner=type(owner).__name__):
                before = real_revision(self.design)
                owner.attributes.values.append(Entity(objectType='adsk::core::Attribute', groupName='AnotherOwner', name='note', value='changed'))
                if owner is not self.root:self.assertEqual(real_revision(self.design), before)
                with self.assertRaises(c.MechanicalError):preview.sample(0)
                owner.attributes.values.clear()
        self.root.xConstructionAxis.geometry.direction.x = .5
        with self.assertRaises(c.MechanicalError):preview.dispose()
        self.assertFalse(preview._delete_issued)

    def test_mid_effect_metadata_change_cannot_be_rebased_or_cleaned_up(self):
        self.enable_revision_effects()
        preview = self.preview(); preview.create()
        previous = copy.deepcopy(preview.lease.verified_state)
        original = Graphics.transform.fset
        def corrupt(group, matrix):
            original(group, matrix)
            self.doc.attributes.values[:] = [Entity(objectType='adsk::core::Attribute', groupName='user', name='note', value='unrelated change')]
        with patch.object(Graphics, 'transform', property(Graphics.transform.fget, corrupt)), self.assertRaises(c.MechanicalError):
            preview.sample(2)
        self.assertEqual(preview.lease.verified_state, previous)
        self.assertIsNone(preview.lease.cleanup_state)
        self.assertFalse(preview.lease.failure_observation['cleanupOnly'])
        with self.assertRaises(c.MechanicalError):preview.dispose()
        self.assertFalse(preview._delete_issued)

    def test_partial_effect_revision_is_cleanup_only_and_never_ready(self):
        self.enable_revision_effects()
        preview = self.preview(); preview.create(); preview.sample(1)
        last_good = copy.deepcopy(preview.lease.verified_state)
        preview.nodes[2]['group'].fail_transform = True
        with self.assertRaises(RuntimeError):preview.sample(2)
        self.assertEqual(preview.lease.verified_state, last_good)
        self.assertNotEqual(preview.lease.cleanup_state['revision'], last_good['revision'])
        self.assertTrue(preview.lease.failure_observation['cleanupOnly'])
        with self.assertRaises(c.MechanicalError):preview.lease.check()
        self.assertEqual(preview.lease.check(cleanup=True), preview.lease.cleanup_state)
        self.f.doc.isValid = False
        self.app.documents.values.remove(self.f.doc)
        self.assertEqual(preview.dispose()['state'], 'disposed')
        self.assertEqual(preview.lease.model_revision, real_revision(self.design))

    def test_changed_flags_during_owned_effect_are_observed_not_reset(self):
        preview = self.preview(); preview.create()
        before = preview.lease.model_revision
        self.root.customGraphicsGroups.on_effect = lambda: setattr(self.doc, 'isModified', True)
        with self.assertRaises(c.MechanicalError) as caught:preview.sample(2)
        self.assertEqual(caught.exception.details['presentationAfter']['flags'], [False, True])
        self.assertEqual(caught.exception.details['presentationBefore']['flags'], [False, False])
        self.assertFalse(caught.exception.details['rollbackClaimed'])
        self.assertTrue(self.doc.isModified)
        self.assertEqual(preview.lease.model_revision, before)
        with self.assertRaises(c.MechanicalError):preview.dispose()

    def test_throwing_witness_getter_retains_primary_error_and_flag_change(self):
        class ThrowingAttributes:
            @property
            def count(inner):
                self.doc.isModified = True
                raise RuntimeError('attribute inventory failed after dirtying')
        self.doc.attributes = ThrowingAttributes()
        with self.assertRaises(c.MechanicalError) as caught:p.presentation_observation(self.doc, self.design)
        self.assertIn('attribute inventory failed', str(caught.exception))
        self.assertEqual(caught.exception.details['presentationObservation']['flagEffect'], 'changed')
        self.assertFalse(caught.exception.details['rollbackClaimed'])

    def test_bounded_full_aggregate_and_strict_blank_witness(self):
        original = p.presentation_observation(self.doc, self.design)
        self.design.rev = 'different-aggregate'
        with self.assertRaises(c.MechanicalError) as caught:p.presentation_observation(self.doc, self.design)
        self.assertEqual(caught.exception.code, 'preview_revision_unavailable')
        self.design.rev = 'blank'
        for owner, name, value in ((self.doc, 'isModified', 0), (self.root, 'constructionPoints', Collection([object()])),
                                   (self.design, 'isConfiguredDesign', True), (self.doc, 'attributes', NS(count=257)),
                                   (self.root, 'isOriginFolderLightBulbOn', 1)):
            previous = getattr(owner, name)
            setattr(owner, name, value)
            with self.subTest(name=name), self.assertRaises(c.MechanicalError):p.presentation_observation(self.doc, self.design)
            setattr(owner, name, previous)
        self.assertEqual(p.presentation_observation(self.doc, self.design), original)

    def test_flat_definition_copies_and_world_translation_applied_once(self):
        preview=self.preview();self.app.activeDocument=self.doc
        receipt=preview.create()
        self.assertIs(self.app.activeDocument,self.f.doc)
        self.assertEqual(receipt['bodyCopyCount'],5)
        self.assertFalse(preview.root.isSelectable);self.assertFalse(preview.root.isChildrenSelectable)
        self.assertEqual(len(preview.root.values),5)
        by_key={r['instanceId']:r for r in self.generated['plan']['instances']}
        for node in preview.nodes:
            self.assertIs(node['group'].parent,preview.root)
            self.assertEqual(p.matrix_values(node['graphics'][0]),p.IDENTITY)
            self.assertTrue(node['graphics'][0].bRepBody.isTransient)
            if node['instanceId'] is not None:
                expected=list(by_key[node['instanceId']]['homeMatrix'])
                for i in (3,7,11):expected[i]/=10
                self.assertEqual(p.matrix_values(node['group']),expected)
        self.assertEqual(self.f.doc.isModified,True);self.assertEqual(self.f.design.rev,'source-rev')

    def test_sample_uses_server_owned_matrices_with_preview_active(self):
        preview=self.preview();preview.create();preview.activate()
        self.args['steps'][0]['moves'][0]['distance']=99999
        self.generated['samples'][-1]['poses'][0]['matrix'][3]=999999
        receipt=preview.sample(2)
        self.assertIs(self.app.activeDocument,self.doc);self.assertEqual(receipt['sampleIndex'],2)
        pin=next(r for r in self.generated['plan']['instances'] if r['path']=='Assembly:1+Pin:1')
        group=next(n['group'] for n in preview.nodes if n['instanceId']==pin['instanceId'])
        self.assertEqual([p.matrix_values(group)[i] for i in (3,7,11)],[10.,2.5,0.])
        preview.sample(0)
        self.assertEqual([p.matrix_values(group)[i] for i in (3,7,11)],[10.,1.,0.])
        for index in (True,-1,3,1.,{'matrix':p.IDENTITY}):
            with self.assertRaises(c.MechanicalError):preview.sample(index)

    def test_copy_is_independent_and_no_transient_visibility_read(self):
        preview=self.preview();preview.create()
        copied=preview.nodes[0]['graphics'][0].bRepBody
        del copied.isVisible
        preview.sample(1)  # Effective visibility is checked only on source/native proxies.
        self.f.root.bRepBodies.item(0).volume=2
        self.assertEqual(copied.volume,1/6)
        with self.assertRaises(c.MechanicalError):preview.sample(2)
        self.assertEqual(preview.state,'ready')
        self.assertEqual(preview.dispose()['state'],'disposed')  # Stale source must not trap owned cleanup.
        self.assertTrue(self.doc.isValid)  # No document close.

    def test_mesh_hidden_suppressed_surface_and_context_mismatch_refuse_before_graphics(self):
        cases=[(self.f.pin,'meshBodies',Collection([object()])),(self.f.ap,'isVisible',False),
               (self.f.ap,'bodyVisible',False),(self.f.ap,'timelineObject',NS(isSuppressed=True)),
               (self.f.pin.bRepBodies.item(0),'isSolid',False),
               (self.f.pin.bRepBodies.item(0),'nativeObject',object())]
        for obj,key,value in cases:
            with self.subTest(key=key):
                had=hasattr(obj,key);old=getattr(obj,key,None);setattr(obj,key,value)
                with self.assertRaises(c.MechanicalError):self.prepared()
                setattr(obj,key,old) if had else delattr(obj,key)
                self.assertEqual(self.root.customGraphicsGroups.count,0)
        native=self.f.pin.bRepBodies.item(0);native.createForAssemblyContext=lambda o:Entity(objectType='adsk::fusion::BRepBody',isVisible=True,nativeObject=native,assemblyContext=self.f.b)
        with self.assertRaises(c.MechanicalError):self.prepared()

    def test_ownership_semantic_error_foreign_id_saved_or_nonblank_refuses(self):
        for change in ({'success':False},{'documentId':'other'},{'sessionId':'old'}, {'data':{'success':False}}):
            old=copy.deepcopy(self.receipt);self.receipt.update(change)
            with self.assertRaises(c.MechanicalError):self.lease()
            self.receipt=old
        self.doc.isSaved=True
        with self.assertRaises(c.MechanicalError):self.lease()
        self.doc.isSaved=False;self.root.bRepBodies.values.append(body())
        with self.assertRaises(c.MechanicalError):self.lease()

    def test_worker_guard_precedes_native_getters_at_all_entries(self):
        class Trap:
            def __getattribute__(self,key):raise AssertionError('native getter '+key)
        errors=[]
        def run():
            calls=[lambda:p.prepare(Trap(),{},'x'),lambda:p.OwnedPresentationLease(Trap(),Trap(),Trap(),'x',lambda:{}),
                   lambda:p.Preview(Trap(),Trap()),lambda:p.Preview.create(Trap()),lambda:p.Preview.sample(Trap(),0),
                   lambda:p.Preview.activate(Trap()),lambda:p.Preview.dispose(Trap())]
            for call in calls:
                try:call()
                except Exception as exc:errors.append(exc)
        thread=threading.Thread(target=run);thread.start();thread.join()
        self.assertEqual(len(errors),7)
        self.assertTrue(all(isinstance(e,c.MechanicalError) and e.code=='wrong_thread' for e in errors))

    def test_wrong_plan_stale_revision_or_changed_flags_refuses_before_copy(self):
        with self.assertRaises(c.MechanicalError):p.prepare(self.app,self.args,'foreign-plan')
        prepared=self.prepared();self.f.design.rev='changed'
        with self.assertRaises(c.MechanicalError):p.Preview(prepared,self.lease()).create()
        self.f.design.rev='source-rev';self.f.doc.isModified=False
        with self.assertRaises(c.MechanicalError):p.Preview(prepared,self.lease()).create()
        self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_partial_sample_retained_then_exact_dispose_without_source(self):
        preview=self.preview();preview.create();preview.nodes[2]['group'].fail_transform=True
        with self.assertRaises(RuntimeError):preview.sample(2)
        self.assertEqual(preview.state,'retained_failure')
        self.assertNotEqual(p.matrix_values(preview.nodes[1]['group']),p.IDENTITY)
        with self.assertRaises(c.MechanicalError):preview.sample(2)
        self.f.doc.isValid=False;self.app.documents.values.remove(self.f.doc)
        self.assertEqual(preview.dispose()['state'],'disposed')
        self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_ignored_setter_and_corrupt_copy_fail_readback(self):
        preview=self.preview();preview.create();preview.nodes[1]['group'].ignore_transform=True
        with self.assertRaises(c.MechanicalError):preview.sample(2)
        self.assertEqual(preview.state,'retained_failure');preview.dispose()
        preview=self.preview();preview.create();preview.nodes[1]['graphics'][0].bRepBody.volume=99
        with self.assertRaises(c.MechanicalError):preview.sample(1)
        preview.dispose()

    def test_partial_creation_keeps_exact_root_for_disposal(self):
        preview=self.preview()
        original=Graphics.addBRepBody
        counter=[]
        def throwing(group,native):
            counter.append(1)
            if len(counter)==2:raise RuntimeError('copy refused')
            return original(group,native)
        with patch.object(Graphics,'addBRepBody',throwing),self.assertRaises(RuntimeError):preview.create()
        self.assertIsNotNone(preview.root);self.assertEqual(preview.state,'retained_failure')
        self.assertEqual(preview.dispose()['state'],'disposed')

    def test_unacknowledged_added_child_prevents_blind_cleanup(self):
        preview=self.preview()
        def null_after_add(group):
            group.values.append(Graphics(group));return None
        with patch.object(Graphics,'addGroup',null_after_add),self.assertRaises(c.MechanicalError):preview.create()
        with self.assertRaises(c.MechanicalError):preview.dispose()
        self.assertEqual(self.root.customGraphicsGroups.count,1)

    def test_foreign_graphics_or_lease_prevents_deletion(self):
        preview=self.preview();preview.create();preview.root.values.append(Graphics(preview.root))
        with self.assertRaises(c.MechanicalError):preview.dispose()
        preview.root.values.pop();self.receipt['success']=False
        with self.assertRaises(c.MechanicalError):preview.dispose()
        self.assertTrue(preview.root.isValid)

    def test_false_delete_or_missing_absence_proof_retains_exact_refs(self):
        for ignored in (False,True):
            preview=self.preview();preview.create()
            preview.root.delete_ack=ignored;preview.root.ignore_delete=ignored
            with self.assertRaises(c.MechanicalError):preview.dispose()
            self.assertEqual(preview.state,'retained_failure');self.assertTrue(preview.root.isValid)
            preview.root.delete_ack=True;preview.root.ignore_delete=False
            with self.assertRaises(c.MechanicalError):preview.dispose()
            self.assertTrue(preview.root.isValid)
            self.root.customGraphicsGroups.values.clear()  # Isolated fake reset; no second native deletion.

    def test_foreign_active_document_is_never_activated_away(self):
        preview=self.preview();preview.create();foreign=Entity();self.app.activeDocument=foreign
        with self.assertRaises(c.MechanicalError):preview.activate()
        self.assertIs(self.app.activeDocument,foreign)
        preview.dispose()
        preview=self.preview()
        with self.assertRaises(c.MechanicalError):preview.create()
        self.assertIs(self.app.activeDocument,foreign)
        preview.dispose()

    def test_bounds_and_nonfinite_source_refuse_without_native_graphics(self):
        self.f.pin.bRepBodies.item(0).boundingBox.maxPoint.x=float('nan')
        with self.assertRaises(c.MechanicalError):self.prepared()
        self.f.pin.bRepBodies.item(0).boundingBox.maxPoint.x=1.
        with patch.object(p,'MAX_BODIES',2),self.assertRaises(c.MechanicalError):self.prepared()
        with patch.object(p,'MAX_VERTICES',3),self.assertRaises(c.MechanicalError):self.prepared()
        self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_acknowledged_but_corrupt_native_matrix_factory_cannot_become_expectation(self):
        preview=self.preview();preview.create()
        def corrupt(matrix, values):
            matrix.values=list(values)
            for i in (3,7,11):matrix.values[i]=0.
            # Also corrupt caller input: expectations must not alias this list.
            for i in (3,7,11):values[i]=0.
            return True
        with patch.object(Matrix,'setWithArray',corrupt),self.assertRaises(c.MechanicalError) as error:
            preview.sample(2)
        self.assertEqual(error.exception.code,'preview_transform_refused')
        self.assertIsNone(preview.sample_index);self.assertEqual(preview.state,'retained_failure')
        preview.dispose()
        preview=self.preview()
        with patch.object(Matrix,'setWithArray',corrupt),self.assertRaises(c.MechanicalError):preview.create()
        self.assertEqual(preview.state,'retained_failure');preview.dispose()

    def test_nonnull_child_and_body_handles_retained_before_throwing_validity_getter(self):
        class Once:
            def __init__(self, wrapped):object.__setattr__(self,'wrapped',wrapped);object.__setattr__(self,'failed',False)
            def __getattr__(self,key):
                if key=='isValid' and not self.failed:
                    object.__setattr__(self,'failed',True);raise RuntimeError('validity getter failed once')
                return getattr(self.wrapped,key)
            def __setattr__(self,key,value):setattr(self.wrapped,key,value)
        for method in ('addGroup','addBRepBody'):
            with self.subTest(method=method):
                original=getattr(Graphics,method)
                def wrapped(group,*args):
                    value=Once(original(group,*args));group.values[-1]=value;return value
                preview=self.preview()
                with patch.object(Graphics,method,wrapped),self.assertRaises(RuntimeError):preview.create()
                self.assertEqual(preview.state,'retained_failure')
                self.assertEqual(len(preview.nodes),1)
                if method=='addBRepBody':self.assertEqual(len(preview.nodes[0]['graphics']),1)
                self.assertEqual(preview.dispose()['state'],'disposed')
                self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_not_registered_as_public_or_durable_operation(self):
        from test_mechanical import m
        self.assertTrue(p.PROTOTYPE_ONLY)
        self.assertFalse(any(getattr(handler, '__module__', None) == p.__name__ for handler in m.OPERATIONS.values()))
        self.assertFalse(hasattr(p,'READS'));self.assertFalse(hasattr(p,'WRITES'))


if __name__=='__main__':unittest.main()
