"""Real dispatcher/journal + fake native graphics: no host or native execution."""
import copy
import importlib
import json
import threading
from types import SimpleNamespace as NS
import unittest
import uuid
from unittest.mock import patch
from test_mechanical import m, c, Entity, isolated_history
import test_exploded_preview_prototype as native_fixture
Graphics = native_fixture.Graphics
body = native_fixture.body

controller = importlib.import_module('mechanical_test_commands.mechanical.exploded_preview')
from mech_reference import journal  # faults are injected into the exe's journal, which now writes history
p = controller.backend


class PublicPreview(unittest.TestCase):
    def setUp(self):
        self.native=native_fixture.PreviewTests('test_flat_definition_copies_and_world_translation_applied_once')
        self.native.setUp();self.addCleanup(self.native.doCleanups)
        self.f=self.native.f;self.app=self.native.app;self.doc=self.native.doc;self.design=self.native.design
        self.root=self.native.root
        isolated_history(self)
        self.h=self.exe
        self.control=controller.Controller()
        for item in [patch.object(controller,'_controller',self.control),
                     patch.object(c,'document_identity',side_effect=lambda doc:doc.doc_id),
                     patch.object(c,'revision',side_effect=lambda design:design.rev)]:
            item.start();self.addCleanup(item.stop)
        self.f.doc.creationId='native-source';self.doc.creationId='native-presentation'
        self.f.doc.products=NS(itemByProductType=lambda name:self.f.design)
        self.doc.products=NS(itemByProductType=lambda name:self.design)
        c._documents[:]=[(self.f.doc,'source'),(self.doc,'presentation')]
        self.target={'documentId':'presentation','revision':self.design.rev}
        self.creation={'op':'new_design','sessionId':m.SESSION_ID,'operationId':'document-created'}
        self.created_data={'target':self.target,'document':{'creationId':'native-presentation','saved':False}}
        with patch.dict(m.OPERATIONS,new_design=lambda app,args:copy.deepcopy(self.created_data)):
            made=self.exe.call(self.app,self.creation)
        self.assertTrue(made['success'])
        self.app.activeDocument=self.doc
        self.create_args={'op':'exploded_preview_create','sessionId':m.SESSION_ID,'operationId':'preview-created',
            'target':self.target,'sourceTarget':self.f.value,'creationOperationId':'document-created',
            'expectedPlanId':self.native.generated['planId'],
            'planner':{key:copy.deepcopy(value) for key,value in self.native.args.items() if key in ('unit','steps','sampleTimes','playback')}}
        self.number=0
    def call(self,args):return self.exe.call(self.app,copy.deepcopy(args))
    def create(self):
        result=self.call(self.create_args)
        self.assertTrue(result['success'],result)
        self.key=result['data']['previewId']
        return result
    def args(self,op,**extra):
        self.number+=1
        entry=self.control.entries[self.key]
        return {'op':'exploded_preview_'+op,'sessionId':m.SESSION_ID,'operationId':'action-'+str(self.number),
                'target':self.target,'previewId':self.key,'expectedGeneration':entry['generation'],**extra}
    def inspect(self):return self.call({'op':'exploded_preview_inspect','sessionId':m.SESSION_ID,'previewId':self.key})

    def test_native_revision_effects_preserve_creation_receipt_and_disposal_reuse_chain(self):
        self.native.enable_revision_effects()
        creation_record = copy.deepcopy(self.h.store().lookup(m.SESSION_ID, 'document-created'))
        original_target = copy.deepcopy(self.target)
        created = self.create(); first_key = self.key
        self.assertNotEqual(created['data']['lastVerifiedPresentationRevision'], original_target['revision'])
        for index in (2, 0, 1):
            result = self.call(self.args('sample', sampleIndex=index))
            self.assertTrue(result['success'], result)
            self.assertEqual(result['data']['presentationTarget'], original_target)
            self.assertEqual(result['data']['lastVerifiedPresentationRevision'], self.design.rev)
        disposed = self.call(self.args('dispose'))
        self.assertTrue(disposed['success'], disposed)
        self.assertEqual(disposed['data']['observedEmptyTarget']['revision'], self.design.rev)
        self.app.activeDocument = self.doc
        reused = self.call({**self.create_args, 'operationId': 'reuse-after-dispose'})
        self.assertTrue(reused['success'], reused)
        self.assertEqual(reused['data']['predecessorPreviewId'], first_key)
        self.assertEqual(reused['data']['presentationTarget'], original_target)
        self.assertEqual(self.h.store().lookup(m.SESSION_ID, 'document-created'), creation_record)
        self.assertEqual(self.call(self.create_args)['data'], created['data'])

    def test_same_empty_model_with_unowned_revision_does_not_authorize_reuse(self):
        self.native.enable_revision_effects()
        self.create()
        disposed = self.call(self.args('dispose'))
        self.assertTrue(disposed['success'], disposed)
        self.root.revisionId = 'foreign-revision-after-dispose'
        self.app.activeDocument = self.doc
        refused = self.call({**self.create_args, 'operationId': 'reuse-stale-disposed'})
        self.assertEqual(refused['errorCode'], 'stale_revision')
        self.assertEqual(self.root.customGraphicsGroups.count, 0)
        self.assertEqual(len(self.control.entries), 1)

    def test_first_claim_still_requires_original_creation_revision(self):
        self.root.revisionId = 'intervening-edit'
        refused = self.call(self.create_args)
        self.assertEqual(refused['errorCode'], 'stale_revision')
        self.assertEqual(self.control.entries, {})
        self.assertEqual(self.root.customGraphicsGroups.count, 0)

    def test_partial_sample_revision_cleanup_preserves_last_verified_pose_and_history(self):
        self.native.enable_revision_effects()
        self.create()
        sampled = self.call(self.args('sample', sampleIndex=1))
        self.assertTrue(sampled['success'], sampled)
        entry = self.control.entries[self.key]
        entry['backend'].nodes[2]['group'].fail_transform = True
        request = self.args('sample', sampleIndex=2)
        failed = self.call(request)
        self.assertFalse(failed['success'])
        self.assertEqual(failed['preview']['pose'], 'unverified')
        self.assertIsNone(failed['preview']['sampleIndex'])
        self.assertEqual(failed['preview']['lastVerifiedSampleIndex'], 1)
        self.assertTrue(failed['preview']['presentationFailureObservation']['cleanupOnly'])
        failure_record = copy.deepcopy(self.h.store().lookup(m.SESSION_ID, request['operationId']))
        disposed = self.call(self.args('dispose'))
        self.assertTrue(disposed['success'], disposed)
        self.assertEqual(self.h.store().lookup(m.SESSION_ID, request['operationId']), failure_record)
        self.assertEqual(self.root.customGraphicsGroups.count, 0)

    def test_public_lifecycle_and_historical_replay_after_disposal(self):
        created=self.create();self.assertIs(self.app.activeDocument,self.f.doc)
        self.assertEqual(created['data']['generation'],1);self.assertEqual(self.root.customGraphicsGroups.count,1)
        request=self.args('sample',sampleIndex=2)
        sampled=self.call(request);self.assertTrue(sampled['success'],sampled)
        self.assertEqual(sampled['data']['generation'],2)
        replay=self.call(request);self.assertTrue(replay['replayed']);self.assertEqual(replay['data'],sampled['data'])
        shown=self.call(self.args('show'));self.assertTrue(shown['success'],shown);self.assertIs(self.app.activeDocument,self.doc)
        current=self.inspect()['data'];self.assertTrue(current['usable']);self.assertEqual(current['receiptScope'],'live_observation')
        self.assertTrue(current['history']['terminalRecorded'])
        disposed=self.call(self.args('dispose'));self.assertTrue(disposed['success'],disposed)
        self.assertEqual(disposed['data']['state'],'disposed');self.assertEqual(self.root.customGraphicsGroups.count,0)
        self.assertTrue(self.doc.isValid);self.assertEqual(disposed['data']['documentDisposition'],'open_for_caller')
        replay_create=self.call(self.create_args);self.assertTrue(replay_create['replayed'])
        self.assertEqual(replay_create['data'],created['data']);self.assertEqual(self.root.customGraphicsGroups.count,0)
        live=self.inspect()['data'];self.assertEqual(live['state'],'disposed');self.assertFalse(live['nativeResourceRights'])
        self.assertEqual(self.call(self.args('dispose'))['errorCode'],'preview_state')

    def test_conflicting_digest_and_stale_generation_do_not_write(self):
        self.create();entry=self.control.entries[self.key];before=copy.deepcopy(entry['backend'].receipt())
        conflict=self.call({**self.create_args,'expectedPlanId':'changed'})
        self.assertEqual(conflict['errorCode'],'operation_conflict');self.assertEqual(conflict['outcome'],'not_executed')
        stale=self.call(self.args('sample',sampleIndex=1,expectedGeneration=0))
        self.assertEqual(stale['errorCode'],'preview_generation_conflict')
        self.assertEqual(entry['backend'].receipt(),before);self.assertEqual(entry['generation'],1)

    def test_failed_durable_admission_has_no_activation_reservation_or_graphics(self):
        with patch.object(self.h.store(),'admit',side_effect=journal.JournalError('operation_storage_failed','disk fault')):
            result=self.call(self.create_args)
        self.assertFalse(result['success']);self.assertEqual(result['outcome'],'unknown')
        self.assertEqual(self.control.entries,{});self.assertIs(self.app.activeDocument,self.doc)
        self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_executing_duplicate_never_enters_controller(self):
        descriptor,reply=self.h.admit(self.create_args,'first-dispatch');self.assertIsNone(reply)
        self.assertIsNone(self.h.start(descriptor,self.create_args,{'documentId':'presentation'}))
        result=self.call(self.create_args)
        self.assertEqual(result['errorCode'],'operation_in_progress');self.assertEqual(self.control.entries,{})
        self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_creation_history_is_server_proof_not_observed_previous_document(self):
        record=self.h.store().lookup(m.SESSION_ID,'document-created')
        self.assertEqual(record['observedDocument']['documentId'],'owned-offline-document')
        created=self.create();self.assertTrue(created['success'])
        self.assertNotEqual(record['observedDocument']['documentId'],created['data']['presentationTarget']['documentId'])

    def test_foreign_creation_identity_saved_nonblank_and_stale_targets_refuse(self):
        variants=[{'creationOperationId':'unknown'}, {'target':{'documentId':'foreign','revision':'blank'}},
                  {'sourceTarget':{'documentId':'source','revision':'old'}}, {'sourceTarget':self.target}]
        for index,change in enumerate(variants):
            result=self.call({**self.create_args,'operationId':'refusal-'+str(index),**change})
            self.assertFalse(result['success']);self.assertEqual(self.control.entries,{})
        self.doc.creationId='different'
        self.assertFalse(self.call({**self.create_args,'operationId':'wrong-native-id'})['success'])
        self.doc.creationId='native-presentation';self.doc.isSaved=True
        self.assertFalse(self.call({**self.create_args,'operationId':'saved'})['success'])
        self.doc.isSaved=False;self.root.bRepBodies.values.append(body())
        self.assertFalse(self.call({**self.create_args,'operationId':'nonblank'})['success'])
        self.assertEqual(self.control.entries,{});self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_wrong_expected_plan_after_activation_releases_only_known_no_graphics_reservation(self):
        result=self.call({**self.create_args,'expectedPlanId':'wrong'})
        self.assertFalse(result['success']);self.assertEqual(result['errorCode'],'preview_plan_mismatch')
        self.assertEqual(result['effectState'],'activation_only');self.assertFalse(result['rollbackClaimed'])
        entry=next(iter(self.control.entries.values()))
        self.assertEqual(entry['state'],'disposed');self.assertEqual(entry['generation'],1)
        self.assertEqual(entry['disposition'],'reservation_released_no_graphics')
        self.assertFalse(entry['factoryMayHaveBeenIssued']);self.assertEqual(self.control.claims,{})
        self.assertIs(self.app.activeDocument,self.f.doc);self.assertEqual(self.root.customGraphicsGroups.count,0)
        self.app.activeDocument=self.doc
        self.create_args['operationId']='fresh-create'
        self.create()

    def test_unknown_graphics_creation_does_not_use_zero_count_release(self):
        with patch.object(p.Preview,'create',side_effect=RuntimeError('factory outcome unknown')):
            result=self.call(self.create_args)
        self.assertFalse(result['success']);entry=next(iter(self.control.entries.values()))
        self.assertEqual(entry['state'],'retained_failure');self.assertTrue(entry['factoryMayHaveBeenIssued'])
        self.assertEqual(len(self.control.claims),1);self.assertEqual(self.root.customGraphicsGroups.count,0)
        self.key=entry['previewId']
        result=self.call(self.args('dispose'));self.assertFalse(result['success']);self.assertEqual(len(self.control.claims),1)

    def test_partial_sample_retains_then_disposes_after_source_closes(self):
        self.create();entry=self.control.entries[self.key]
        entry['backend'].nodes[2]['group'].fail_transform=True
        result=self.call(self.args('sample',sampleIndex=2))
        self.assertFalse(result['success']);self.assertEqual(entry['state'],'retained_failure')
        self.assertFalse(entry['nativeWorkInFlight']);self.assertTrue(entry['lastReturned'])
        self.f.doc.isValid=False;self.app.documents.values.remove(self.f.doc)
        disposed=self.call(self.args('dispose'));self.assertTrue(disposed['success'],disposed)
        self.assertEqual(self.root.customGraphicsGroups.count,0);self.assertTrue(self.doc.isValid)

    def test_terminal_storage_failure_allows_only_explicit_exact_owned_disposal(self):
        store=self.h.store();real=store.transition
        def fail_finish(*args,**kwargs):
            if args[3]=='succeeded' and args[1]=='preview-created':raise journal.JournalError('operation_storage_failed','lost terminal commit')
            return real(*args,**kwargs)
        with patch.object(store,'transition',side_effect=fail_finish):result=self.call(self.create_args)
        self.assertEqual(result['errorCode'],'operation_reconciliation_required')
        self.key=next(iter(self.control.entries));entry=self.control.entries[self.key]
        self.assertTrue(entry['lastReturned']);self.assertEqual(self.root.customGraphicsGroups.count,1)
        original=store.lookup(m.SESSION_ID,'preview-created')
        self.assertEqual(original['phase'],'executing')
        self.assertFalse(self.inspect()['data']['usable'])
        self.assertEqual(self.call(self.args('sample',sampleIndex=1))['errorCode'],'preview_history_unresolved')
        self.assertEqual(self.call(self.args('show'))['errorCode'],'preview_history_unresolved')
        disposed=self.call(self.args('dispose'));self.assertTrue(disposed['success'],disposed)
        self.assertTrue(disposed['data']['priorOperationUnresolved'])
        self.assertEqual(disposed['data']['priorUnresolvedOperation']['operationId'],'preview-created')
        self.assertIn('observedResultDigest',disposed['data']['priorUnresolvedOperation'])
        self.assertEqual(store.lookup(m.SESSION_ID,'preview-created'),original)
        self.assertEqual(self.root.customGraphicsGroups.count,0)
        again=self.call(self.create_args);self.assertFalse(again['success']);self.assertEqual(self.root.customGraphicsGroups.count,0)

    def test_unknown_history_cannot_authorize_disposal_of_inflight_or_foreign_descendant(self):
        self.create();entry=self.control.entries[self.key]
        entry['nativeWorkInFlight']=True
        self.assertFalse(self.call(self.args('dispose'))['success'])
        entry['nativeWorkInFlight']=False;entry['backend'].root.values.append(Graphics(entry['backend'].root))
        self.assertFalse(self.call(self.args('dispose'))['success']);self.assertEqual(self.root.customGraphicsGroups.count,1)

    def test_dispose_commit_failure_preserves_unknown_and_blocks_replacement_create(self):
        self.create();request=self.args('dispose');store=self.h.store();real=store.transition
        def fail(*args,**kwargs):
            if args[3]=='succeeded' and args[1]==request['operationId']:raise journal.JournalError('operation_storage_failed','terminal fault')
            return real(*args,**kwargs)
        with patch.object(store,'transition',side_effect=fail):result=self.call(request)
        self.assertEqual(result['errorCode'],'operation_reconciliation_required');self.assertEqual(self.root.customGraphicsGroups.count,0)
        self.assertFalse(self.inspect()['data']['history']['terminalRecorded'])
        self.app.activeDocument=self.doc
        result=self.call({**self.create_args,'operationId':'replacement'})
        self.assertEqual(result['errorCode'],'preview_history_unresolved');self.assertEqual(self.root.customGraphicsGroups.count,0)
        self.assertFalse(self.call(request)['success'])  # No second delete.

    def test_invalid_indices_foreign_active_doc_and_capacity_fail_without_effect(self):
        with patch.object(controller,'MAX_ACTIVE',0):
            result=self.call({**self.create_args,'operationId':'capacity'})
        self.assertEqual(result['errorCode'],'preview_capacity');self.assertEqual(self.control.entries,{})
        self.create();entry=self.control.entries[self.key];generation=entry['generation']
        for value in (-1,True,1.5,64,3):
            result=self.call(self.args('sample',sampleIndex=value));self.assertFalse(result['success'])
            self.assertEqual(entry['generation'],generation)
        foreign=Entity();self.app.activeDocument=foreign
        result=self.call(self.args('show'));self.assertEqual(result['errorCode'],'preview_activation_refused')
        self.assertIs(self.app.activeDocument,foreign);self.assertEqual(entry['generation'],generation)

    def test_restart_does_not_rebind_registry_or_replay_native_effects(self):
        original=self.create();key=self.key
        with patch.object(controller,'_controller',controller.Controller()):
            result=self.inspect();self.assertEqual(result['errorCode'],'preview_session_expired')
            replay=self.call(self.create_args);self.assertTrue(replay['replayed']);self.assertEqual(replay['data'],original['data'])
            self.assertEqual(self.root.customGraphicsGroups.count,1)
            self.assertEqual(self.call(self.args('dispose'))['errorCode'],'preview_session_expired')
        self.assertIn(key,self.control.entries)

    def test_worker_and_old_session_guard_before_native_getters(self):
        class Trap:
            def __getattribute__(self,key):raise AssertionError('native access '+key)
        errors=[]
        def run():
            for method,args in [(self.control.create,{}),(self.control.inspect,{}),
                                (lambda app,args,h:self.control.action('dispose',app,args,h),{})]:
                try:method(Trap(),args,self.h)
                except Exception as error:errors.append(error)
        worker=threading.Thread(target=run);worker.start();worker.join()
        self.assertEqual(len(errors),3);self.assertTrue(all(getattr(e,'code',None)=='wrong_thread' for e in errors))
        result=self.call({**self.create_args,'sessionId':'old'})
        self.assertEqual(result['errorCode'],'session_mismatch');self.assertEqual(self.control.entries,{})

    def test_uncertain_delete_is_never_reissued_with_fresh_ids_or_lost_terminal_storage(self):
        for mode in ('false','throw','deleted_readback_failed','false_finish_failure'):
            with self.subTest(mode=mode):
                self.app.activeDocument=self.doc
                self.create_args['operationId']='create-'+mode
                self.create();entry=self.control.entries[self.key];group=entry['backend'].root
                calls=[];original=group.deleteMe
                def deletion():
                    calls.append(1)
                    if mode=='throw':raise RuntimeError('delete outcome unknown')
                    if mode=='deleted_readback_failed':
                        result=original();self.design.rev='changed-after-delete';return result
                    return False
                store=self.h.store();transition=store.transition
                request=self.args('dispose')
                def finish(*args,**kwargs):
                    if mode=='false_finish_failure' and args[1]==request['operationId'] and args[3]=='failed':
                        raise journal.JournalError('operation_storage_failed','lost failed terminal')
                    return transition(*args,**kwargs)
                with patch.object(group,'deleteMe',side_effect=deletion),patch.object(store,'transition',side_effect=finish):
                    failed=self.call(request)
                    self.assertFalse(failed['success'])
                    again=self.call(self.args('dispose'))
                    self.assertEqual(again['errorCode'],'preview_disposal_unresolved')
                    self.assertEqual(len(calls),1)
                self.assertEqual(entry['disposalOutcome'],'unknown')
                # Offline fixture reset only, never production cleanup/retry.
                self.root.customGraphicsGroups.values.clear();self.design.rev='blank'
                self.control=controller.Controller();controller._controller=self.control

    def test_combined_retained_json_budget_refuses_before_any_graphics_factory(self):
        prepared=self.native.prepared();lease=self.native.lease();preview=p.Preview(prepared,lease)
        individual=max(len(prepared._result_json.encode()),len(preview._sample_json.encode()))
        combined=controller.retained_json_bytes(prepared,preview)
        self.assertGreater(combined,individual)
        with patch.object(controller,'MAX_PLAN_BYTES',individual),patch.object(p.Preview,'create') as native_create:
            result=self.call(self.create_args)
        self.assertEqual(result['errorCode'],'preview_capacity');native_create.assert_not_called()
        entry=next(iter(self.control.entries.values()))
        self.assertEqual(entry['state'],'disposed');self.assertEqual(entry['planBytes'],0)
        self.assertEqual(self.control.claims,{})
        self.assertEqual(self.root.customGraphicsGroups.count,0)
        aliased=NS(_a_json='same-string')
        aliased._b_json=aliased._a_json
        self.assertEqual(controller.retained_json_bytes(aliased),len('same-string'))

    def test_partial_later_sample_reports_unverified_pose_with_last_good_index(self):
        self.create();self.assertTrue(self.call(self.args('sample',sampleIndex=1))['success'])
        entry=self.control.entries[self.key];entry['backend'].nodes[2]['group'].fail_transform=True
        failed=self.call(self.args('sample',sampleIndex=2))
        self.assertFalse(failed['success']);receipt=failed['preview']
        self.assertEqual(receipt['pose'],'unverified');self.assertIsNone(receipt['sampleIndex'])
        self.assertEqual(receipt['lastVerifiedSampleIndex'],1)
        live=self.inspect()['data'];self.assertEqual(live['pose'],'unverified')
        self.assertFalse(live['usable'])

    def test_disposed_inspection_is_explicit_tombstone_after_close_or_document_reuse(self):
        self.create();self.assertTrue(self.call(self.args('dispose'))['success'])
        old_key=self.key
        self.doc.isValid=False;self.app.documents.values.remove(self.doc)
        result=self.inspect()['data']
        self.assertEqual(result['receiptScope'],'registry_tombstone')
        self.assertEqual(result['nativeObservation'],'NOT_PERFORMED')
        self.assertFalse(result['observationComplete']);self.assertEqual(result['documentDisposition'],'unknown_current')
        self.assertEqual(result['lastKnownDocumentDisposition'],'open_for_caller')
        self.doc.isValid=True;self.app.documents.values.append(self.doc);self.app.activeDocument=self.doc
        self.create_args['operationId']='new-preview-same-doc';self.create()
        self.assertEqual(self.root.customGraphicsGroups.count,1)
        result=self.call({'op':'exploded_preview_inspect','sessionId':m.SESSION_ID,'previewId':old_key})['data']
        self.assertEqual(result['receiptScope'],'registry_tombstone');self.assertFalse(result['nativeResourceRights'])
        self.assertEqual(self.root.customGraphicsGroups.count,1)

    def test_two_admitted_generation_claims_apply_only_first_sample(self):
        self.create()
        first=self.args('sample',sampleIndex=1);second=self.args('sample',sampleIndex=2)
        a,reply=self.h.admit(first,uuid.uuid4().hex);self.assertIsNone(reply)
        b,reply=self.h.admit(second,uuid.uuid4().hex);self.assertIsNone(reply)
        result=self.exe.dispatch(self.app,first,a);self.assertTrue(result['success'],result)
        rejected=self.exe.dispatch(self.app,second,b)
        self.assertEqual(rejected['errorCode'],'preview_generation_conflict')
        entry=self.control.entries[self.key]
        self.assertEqual(entry['generation'],2);self.assertEqual(entry['sampleIndex'],1)
        self.assertEqual(self.h.store().lookup(m.SESSION_ID,second['operationId'])['phase'],'failed')

    def test_contradictory_terminal_result_cannot_make_live_preview_usable(self):
        self.create();lookup=m._history.store().lookup
        def conflicting(session,operation):
            record=lookup(session,operation)
            if operation=='preview-created':record['result']['data']['generation']=999
            return record
        with patch.object(m._history.store(),'lookup',side_effect=conflicting):
            result=self.inspect()['data']
            self.assertFalse(result['usable']);self.assertFalse(result['history']['terminalRecorded'])
            rejected=self.call(self.args('sample',sampleIndex=1))
            self.assertEqual(rejected['errorCode'],'preview_history_unresolved')
        self.assertEqual(self.control.entries[self.key]['generation'],1)

    def test_controller_is_direct_write_and_all_schemas_present(self):
        self.assertEqual(set(m.OPERATIONS),set(m.SCHEMA))
        for name in controller.WRITES:self.assertIn(name,m.WRITES)
        self.assertNotIn('exploded_preview_inspect',m.WRITES)
        with patch.object(m.transactions,'execute',side_effect=AssertionError('PTransaction must not wrap presentation effects')):
            self.create();self.assertTrue(self.call(self.args('dispose'))['success'])


if __name__=='__main__':unittest.main()
