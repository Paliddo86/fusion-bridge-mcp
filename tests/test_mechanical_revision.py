"""Actual revision/target/new-design code with lazy native getter fault models."""
import importlib
import threading
import copy
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

from test_mechanical import adsk, c, m, Collection, MeshCollection, Entity, isolated_history

assembly = importlib.import_module(c.__package__ + '.assembly_state')
readback = importlib.import_module(c.__package__ + '.readback')
configurations = importlib.import_module(c.__package__ + '.configurations')


class LazyComponent(Entity):
    def __init__(self, name, events, document, ancestors=()):
        super().__init__(name=name, objectType='adsk::fusion::Component', entityToken='token-' + name,
                         bRepBodies=Collection(), sketches=Collection(), features=Collection(),
                         occurrences=Collection(), allOccurrences=Collection(), constructionAxes=Collection(),
                         joints=Collection(), asBuiltJoints=Collection(), rigidGroups=Collection(), jointOrigins=Collection())
        self.events, self.document, self.ancestors = events, document, ancestors
        self.version, self.initialized, self.raise_mesh, self.dirty_on_mesh = 0, False, False, False
        self.throw_after_dirty = False
        self.meshes = MeshCollection()

    @property
    def meshBodies(self):
        self.events.append(('mesh', self.name))
        if self.raise_mesh:
            raise RuntimeError('injected mesh collection acquisition failure')
        if not self.initialized:
            self.initialized = True
            for component in (self,) + self.ancestors:
                component.version += 1
            if self.dirty_on_mesh:
                self.document.isModified = True
            if self.throw_after_dirty:
                raise RuntimeError('injected getter threw after initialization/dirtying')
        return self.meshes

    @property
    def revisionId(self):
        self.events.append(('revision', self.name))
        return str(self.version)


class RevisionInitialization(unittest.TestCase):
    def setUp(self):
        c._documents.clear()
        c._entities.clear()
        self.events = []
        self.doc = Entity(objectType='adsk::core::Document', name='Owned saved source', creationId='owned-native-id', isSaved=True, isModified=False)
        self.root = LazyComponent('root', self.events, self.doc)
        self.carrier = LazyComponent('Carrier', self.events, self.doc, (self.root,))
        self.part = LazyComponent('Part', self.events, self.doc, (self.carrier, self.root))
        self.components = [self.root, self.carrier, self.part]
        self.design = Entity(parentDocument=self.doc, allComponents=Collection(self.components),
                             rootComponent=self.root, allParameters=Collection(),
                             timeline=NS(count=0, markerPosition=0))
        for component in self.components:
            component.parentDesign = self.design
        self.app = NS(activeDocument=self.doc, activeProduct=self.design)
        for change in (patch.object(assembly, 'component_state', return_value={}),
                       patch.object(readback, 'construction_state', return_value={}),
                       patch.object(configurations, 'snapshot', return_value={'complete': True, 'configured': False, 'isConfiguration': False})):
            change.start()
            self.addCleanup(change.stop)

    def test_all_components_initialized_before_any_revision_snapshot(self):
        c.revision(self.design)
        before_first_revision = self.events[:next(i for i, row in enumerate(self.events) if row[0] == 'revision')]
        self.assertEqual({row[1] for row in before_first_revision if row[0] == 'mesh'}, {'root', 'Carrier', 'Part'})

    def test_current_target_survives_later_mesh_inventory(self):
        issued = c.current_target(self.app)
        for component in self.components:
            self.assertEqual(component.meshBodies.count, 0)
        self.assertEqual(c.current_target(self.app), issued)

    def test_child_first_acquisition_cannot_invalidate_earlier_ancestor_hash(self):
        first = c.revision(self.design)
        self.part.meshBodies
        self.assertEqual(c.revision(self.design), first)
        self.assertGreater(self.root.version, self.carrier.version)
        self.assertGreater(self.carrier.version, self.part.version)

    def test_real_later_native_revision_still_refuses_stale_target(self):
        target = c.current_target(self.app)
        self.part.version += 10
        with self.assertRaises(c.MechanicalError) as error:
            c.target(self.app, {'target': target})
        self.assertEqual(error.exception.code, 'stale_revision')

    def test_mesh_acquisition_error_prevents_revision_issuance(self):
        self.part.raise_mesh = True
        with self.assertRaises(Exception):
            c.revision(self.design)
        self.assertFalse(any(row[0] == 'revision' for row in self.events))

    def test_saved_clean_flag_transition_refuses_without_reset(self):
        self.part.dirty_on_mesh = True
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertTrue(self.doc.isModified)
        self.assertTrue(self.doc.isSaved)
        self.assertEqual(error.exception.code, 'revision_document_changed')
        self.assertEqual(error.exception.details['revisionObservation']['documentFlagEffect'], 'changed')
        self.assertEqual(error.exception.details['revisionObservation']['changedFlags'], ['isModified'])
        self.assertFalse(error.exception.details['rollbackClaimed'])

    def test_clean_saved_source_unchanged_and_inactive_ownership_supported(self):
        self.app.activeDocument = Entity(isSaved=False, isModified=True)
        first = c.revision(self.design)
        self.assertEqual(c.revision(self.design), first)
        self.assertEqual((self.doc.isSaved, self.doc.isModified), (True, False))
        self.assertTrue(all(component.initialized for component in self.components))

    def test_throwing_getter_still_reports_changed_flags(self):
        self.part.dirty_on_mesh = self.part.throw_after_dirty = True
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertIn('threw after initialization', str(error.exception))
        observation = error.exception.details['revisionObservation']
        self.assertEqual(observation['documentFlagEffect'], 'changed')
        self.assertEqual(observation['changedFlags'], ['isModified'])
        self.assertTrue(observation['observations'][-1]['isModified'])
        self.assertFalse(error.exception.details['rollbackClaimed'])
        self.assertFalse(any(row[0] == 'revision' for row in self.events))

    def test_flag_read_failure_reports_unknown_instead_of_unchanged(self):
        class Document(Entity):
            @property
            def isModified(self):
                raise RuntimeError('native flags unavailable')
        self.doc = Document(isSaved=True)
        self.design.parentDocument = self.doc
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        observation = error.exception.details['revisionObservation']
        self.assertEqual(observation['documentFlagEffect'], 'unknown')
        self.assertFalse(observation['observations'][0]['complete'])
        self.assertEqual(self.events, [])

    def test_nonboolean_flags_refuse_before_collections(self):
        self.doc.isModified = 0
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertEqual(error.exception.details['revisionObservation']['documentFlagEffect'], 'unknown')
        self.assertEqual(self.events, [])

    def test_mesh_count_failure_is_not_skipped(self):
        class Meshes(MeshCollection):
            @property
            def count(self):
                raise RuntimeError('native count unavailable')
        self.part.meshes = Meshes()
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertIn('native count unavailable', str(error.exception))
        self.assertEqual(error.exception.details['revisionObservation']['documentFlagEffect'], 'unchanged')
        self.assertFalse(any(row[0] == 'revision' for row in self.events))

    def test_malformed_or_excessive_mesh_counts_refuse(self):
        for count in (-1, True, 0.0, c.MAX_ENTITIES + 1):
            with self.subTest(count=count):
                self.part.meshes = NS(count=count, isValid=True, objectType='adsk::fusion::MeshBodies')
                self.events.clear()
                with self.assertRaises(c.MechanicalError) as error:
                    c.revision(self.design)
                self.assertEqual(error.exception.code, 'inspection_budget')
                self.assertFalse(any(row[0] == 'revision' for row in self.events))

    def test_foreign_component_owner_refuses_without_mesh_access(self):
        self.root.parentDesign = Entity()
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertEqual(error.exception.code, 'revision_unavailable')
        self.assertEqual(self.events, [])

    def test_a_component_inserted_by_reference_enters_by_its_source_version(self):
        # 2026-10-28: a production cell inserts 23 designs by reference; every operation refused
        source = Entity(objectType='adsk::core::Document', name='Referenced', dataFile=NS(id='urn:ref', versionNumber=7))
        self.part.parentDesign = Entity(parentDocument=source)
        first = c.revision(self.design)
        self.assertNotIn('Part', {row[1] for row in self.events if row[0] == 'mesh'})
        self.assertEqual(c.revision(self.design), first)
        source.dataFile = NS(id='urn:ref', versionNumber=8)
        self.assertNotEqual(c.revision(self.design), first)
        # a foreign component from an unsaved document is not a reference: still refused
        self.part.parentDesign = Entity(parentDocument=Entity(dataFile=None))
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertEqual(error.exception.code, 'revision_unavailable')

    def test_invalid_or_malformed_mesh_collection_never_counts_as_empty(self):
        for meshes in (None, NS(count=0, isValid=False, objectType='adsk::fusion::MeshBodies'),
                       NS(count=0, isValid=True, objectType='adsk::fusion::BRepBodies'), NS(count=0)):
            with self.subTest(meshes=meshes):
                self.part.meshes = meshes
                self.events.clear()
                with self.assertRaises(c.MechanicalError) as error:
                    c.revision(self.design)
                self.assertEqual(error.exception.code, 'revision_unavailable')
                self.assertFalse(any(row[0] == 'revision' for row in self.events))

    def test_initialization_does_not_reject_existing_mesh_or_enumerate_mesh_geometry(self):
        self.part.meshes = NS(count=1, isValid=True, objectType='adsk::fusion::MeshBodies',
                              item=lambda index: self.fail('Mesh geometry need not be materialized'))
        self.assertEqual(c.revision(self.design), c.revision(self.design))

    def test_actual_new_design_journal_target_survives_blank_check_and_lease(self):
        isolated_history(self)
        source_target = c.current_target(self.app)
        source_doc, source_design = self.doc, self.design
        document = Entity(objectType='adsk::core::Document', name='Untitled', isSaved=False, isModified=True, creationId='created-blank')
        root = LazyComponent('blank', self.events, document)
        root.customGraphicsGroups = Collection()
        root.attributes = Collection()
        def add_attribute(group, name, value):
            root.attributes.values.append(Entity(objectType='adsk::core::Attribute', groupName=group, name=name, value=value))
            root.version += 1
        root.attributes.add = add_attribute
        design = Entity(parentDocument=document, allComponents=Collection([root]), rootComponent=root,
                        allParameters=Collection(), timeline=Collection(), computeAll=lambda: True)
        design.timeline.markerPosition = 0
        root.parentDesign = design
        from test_exploded_preview_prototype import enrich_blank
        enrich_blank(document, design, root)
        root.attributes.add = add_attribute
        document.products = NS(itemByProductType=lambda name: design)
        source_doc.products = NS(itemByProductType=lambda name: source_design)
        documents = Collection([source_doc])
        def add_document(kind):
            documents.values.append(document)
            self.app.activeDocument, self.app.activeProduct = document, design
            return document
        documents.add = add_document
        self.app.documents = documents
        tx = importlib.import_module(c.__package__ + '.transactions')
        controller = importlib.import_module(c.__package__ + '.exploded_preview')
        with patch.object(adsk.core, 'DocumentTypes', NS(FusionDesignDocumentType=0), create=True), \
             patch.object(adsk.fusion, 'DesignTypes', NS(ParametricDesignType=0), create=True), \
             patch.object(tx, 'capability', return_value={'verified': True}):
            request = {'op': 'new_design', 'sessionId': m.SESSION_ID, 'operationId': 'real-creation', 'name': 'Preview fixture'}
            created = self.exe.call(self.app, request)
        self.assertTrue(created['success'], created)
        saved_record = copy.deepcopy(m._history.store().lookup(m.SESSION_ID, 'real-creation'))
        original_target = copy.deepcopy(created['data']['target'])
        self.assertTrue(root.initialized)
        registry = controller.Controller()
        entry = registry.reserve(self.app, {'operationId': 'preview-create', 'op': 'exploded_preview_create',
                                'sessionId': m.SESSION_ID, 'target': original_target,
                                'sourceTarget': source_target, 'creationOperationId': 'real-creation'}, m._history)
        registry.lease(entry).check()
        controller.blank_model(design)
        self.assertEqual(c.current_target(self.app), original_target)
        self.assertEqual(m._history.store().lookup(m.SESSION_ID, 'real-creation'), saved_record)
        root.version += 1
        with self.assertRaises(c.MechanicalError) as error:
            registry.lease(entry)
        self.assertEqual(error.exception.code, 'preview_not_blank')

    def test_component_budget_fails_before_any_mesh_getter(self):
        self.design.allComponents = NS(count=c.MAX_ENTITIES + 1, item=lambda index: self.fail('Budget must precede item'))
        with self.assertRaises(c.MechanicalError) as error:
            c.revision(self.design)
        self.assertEqual(error.exception.code, 'inspection_budget')
        self.assertEqual(self.events, [])

    def test_worker_refuses_before_any_native_component_getter(self):
        errors = []
        def run():
            try:
                c.revision(self.design)
            except Exception as error:
                errors.append(error)
        thread = threading.Thread(target=run)
        thread.start()
        thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], c.MechanicalError)
        self.assertEqual(errors[0].code, 'wrong_thread')
        self.assertEqual(self.events, [])


if __name__ == '__main__':
    unittest.main(verbosity=2)
