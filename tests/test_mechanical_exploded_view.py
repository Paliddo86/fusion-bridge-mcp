"""Independent frame, hierarchy, identity and boundary tests; no native motion."""
import copy
import importlib
import math
import threading
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch
from test_mechanical import m,c,Entity,Collection,mech_exe

plan=importlib.import_module('mechanical_test_commands.mechanical.exploded_view')


def pose(x=0,y=0,z=0,angle=0):
    co,si=math.cos(math.radians(angle)),math.sin(math.radians(angle))
    return [co,-si,0,x/10, si,co,0,y/10, 0,0,1,z/10, 0,0,0,1]


def fixture():
    design=Entity();root=Entity(objectType='adsk::fusion::Component',name='Root',partNumber='',entityToken='root',parentDesign=design,bRepBodies=Collection([object()]),revisionId='r1')
    design.rootComponent=root
    shared=Entity(objectType='adsk::fusion::Component',name='Assembly',partNumber='DUP',entityToken='shared',parentDesign=design,revisionId='r1')
    pin=Entity(objectType='adsk::fusion::Component',name='Pin',partNumber='DUP',entityToken='pin',parentDesign=design,revisionId='r1')
    token_map={'root':[root],'shared':[shared],'pin':[pin]}
    def occ(path,comp,matrix,parent=None,native=None):
        token='occ-'+path if native is None else native.entityToken
        value=Entity(objectType='adsk::fusion::Occurrence',entityToken=token,fullPathName=path,name=path.split('+')[-1],
                     component=comp,assemblyContext=parent,nativeObject=native,transform2=NS(asArray=lambda:list(matrix)),childOccurrences=Collection())
        if token not in token_map:token_map[token]=[value]
        return value
    a=occ('Assembly:1',shared,pose(100,0,angle=90));b=occ('Assembly:2',shared,pose(0,200,angle=180))
    native_pin=occ('Pin:1',pin,pose(10,0,angle=30))
    ap=occ('Assembly:1+Pin:1',pin,pose(100,10,angle=120),a,native_pin)
    bp=occ('Assembly:2+Pin:1',pin,pose(-10,200,angle=210),b,native_pin)
    a.childOccurrences=Collection([ap]);b.childOccurrences=Collection([bp])
    root.occurrences=Collection([b,a]);root.allOccurrences=Collection([bp,b,ap,a])
    design.findEntityByToken=lambda token:token_map.get(token,[])
    document=Entity(isSaved=False,isModified=True)
    value={'documentId':'doc','revision':'revision'}
    return NS(design=design,root=root,a=a,b=b,ap=ap,bp=bp,pin=pin,shared=shared,doc=document,value=value,app=object(),tokens=token_map)


class Exploded(unittest.TestCase):
    def setUp(self):
        c._entities.clear();self.f=fixture()
        self.config={'configured':False,'isConfiguration':False,'configurationRowId':'','table':None,'complete':True}
        self.t=patch.object(plan,'target',return_value=(self.f.doc,self.f.design,self.f.value)).start()
        self.cfg=patch.object(plan,'configuration_snapshot',return_value=self.config).start()
        self.addCleanup(patch.stopall)
    def move(self,occ=None,frame='world',distance=5,direction=None):
        return {'occurrence':c.ref(occ or self.f.ap,self.f.value),'direction':direction or [1,0,0],'distance':distance,'frame':frame}
    def args(self,steps=None,**extra):
        return {'op':'exploded_view_plan','sessionId':plan.SESSION_ID,'target':self.f.value,'unit':'mm',
                'steps':steps or [{'durationSeconds':1,'moves':[self.move()]}],**extra}
    def read(self,**args):return plan.exploded_view_plan(self.f.app,self.args(**args))
    def positions(self,result,index=-1):
        paths={row['instanceId']:row['path'] for row in result['plan']['instances']}
        return {paths[row['instanceId']]:[row['matrix'][i] for i in (3,7,11)] for row in result['samples'][index]['poses']}

    def test_root_parent_local_frames_for_rotated_repeated_instances(self):
        for frame,expected in [('world',[105,10,0]),('parent',[100,15,0]),('local',[97.5,10+5*math.sqrt(3)/2,0])]:
            result=self.read(steps=[{'durationSeconds':1,'moves':[self.move(frame=frame)]}])
            actual=self.positions(result)['Assembly:1+Pin:1']
            for a,b in zip(actual,expected):self.assertAlmostEqual(a,b)
            self.assertEqual(self.positions(result)['Assembly:2+Pin:1'],[-10,200,0])
        result=self.read(steps=[{'durationSeconds':1,'moves':[self.move(self.f.bp,frame='parent')]}])
        for actual,expected in zip(self.positions(result)['Assembly:2+Pin:1'],[-15,200,0]):self.assertAlmostEqual(actual,expected)

    def test_simultaneous_ancestor_child_add_once_then_ordered_child_move(self):
        steps=[{'durationSeconds':2,'title':'Lift subassembly','instruction':'Keep the pin aligned.',
                'moves':[self.move(self.f.a,frame='local',distance=10),self.move(frame='parent')]},
               {'durationSeconds':1,'title':'Remove pin','moves':[self.move(distance=4)]}]
        result=self.read(steps=steps,sampleTimes=[1,2,2.5])
        expected=[[100,10,0],[100,17.5,0],[100,25,0],[102,25,0],[104,25,0]]
        for index,point in enumerate(expected):
            for actual,wanted in zip(self.positions(result,index)['Assembly:1+Pin:1'],point):self.assertAlmostEqual(actual,wanted)
        self.assertEqual(result['plan']['steps'][0]['instruction'],'Keep the pin aligned.')
        self.assertEqual(self.positions(result)['Assembly:2+Pin:1'],[-10,200,0])
        self.assertFalse(result['nativeMoved']);self.assertTrue(result['modelUnchanged'])

    def test_three_level_ancestor_propagation_has_no_duplicate_offsets(self):
        g=Entity(objectType='adsk::fusion::Occurrence',entityToken='g',fullPathName='Assembly:1+Pin:1+Tip:1',component=self.f.pin,
                 assemblyContext=self.f.ap,nativeObject=None,transform2=NS(asArray=lambda:pose(100,10,2)),childOccurrences=Collection())
        self.f.ap.childOccurrences=Collection([g]);self.f.root.allOccurrences.values.append(g);self.f.tokens['g']=[g]
        result=self.read(steps=[{'durationSeconds':1,'moves':[self.move(self.f.a,distance=2),self.move(distance=3),self.move(g,distance=4)]}])
        self.assertEqual(self.positions(result)[g.fullPathName],[109,10,2])

    def test_reverse_samples_are_forward_reversed_and_home_endpoint_exact(self):
        args=self.args(steps=[{'durationSeconds':2,'moves':[self.move(frame='local',distance=12)]}],sampleTimes=[.5,1,1.5])
        before=copy.deepcopy(args)
        forward=plan.exploded_view_plan(self.f.app,args)
        reverse=plan.exploded_view_plan(self.f.app,{**args,'playback':'assemble'})
        self.assertEqual(args,before);self.assertEqual(forward['planId'],reverse['planId'])
        for a,b in zip(forward['samples'],reversed(reverse['samples'])):self.assertEqual(a['poses'],b['poses'])
        homes={r['instanceId']:r['homeMatrix'] for r in reverse['plan']['instances']}
        for row in reverse['samples'][-1]['poses']:
            self.assertEqual(row['matrix'],homes[row['instanceId']]);self.assertEqual(row['translationFromHome'],[0,0,0])

    def test_distinct_instances_and_duplicate_part_numbers_keep_labels_and_quantities_separate(self):
        result=self.read();rows=result['plan']['instances']
        self.assertEqual(len({r['instanceId'] for r in rows}),4)
        self.assertEqual(len({r['label']['instance'] for r in rows}),4)
        self.assertEqual(len({r['label']['item'] for r in rows}),2)
        self.assertEqual({r['partNumber'] for r in rows},{'DUP'})
        self.assertEqual(result['plan']['rootBodyGroup']['bodyCount'],1)
        self.assertFalse(result['plan']['rootBodyGroup']['selectable'])
        self.assertFalse(any('quantity' in r for r in rows))

    def test_home_axes_remain_fixed_across_steps_and_direction_is_normalized(self):
        result=self.read(steps=[{'durationSeconds':1,'moves':[self.move(self.f.a,distance=10)]},
                               {'durationSeconds':1,'moves':[self.move(frame='parent',direction=[100,0,0])]}])
        self.assertEqual(result['plan']['steps'][1]['moves'][0]['direction'],[1,0,0])
        for a,b in zip(self.positions(result)['Assembly:1+Pin:1'],[110,15,0]):self.assertAlmostEqual(a,b)

    def test_invalid_inputs_fail_before_target(self):
        invalid=[{'unit':'cm'},{'playback':'forward'},{'sampleTimes':[float('nan')]},{'sampleTimes':[2]},
                 {'sampleTimes':[True]}, {'steps':[{'durationSeconds':0,'moves':[self.move()]}]},
                 {'steps':[{'durationSeconds':1,'moves':[self.move(direction=[0,0,0])]}]},
                 {'steps':[{'durationSeconds':1,'moves':[self.move(distance=-1)]}]},
                 {'steps':[{'durationSeconds':1,'moves':[self.move(distance=True)]}]},
                 {'steps':[{'durationSeconds':1,'moves':[self.move(direction=[float('inf'),0,0])]}]},
                 {'steps':[{'durationSeconds':1,'instruction':'x'*1025,'moves':[self.move()]}]}]
        for change in invalid:
            with self.subTest(change=change),self.assertRaises(c.MechanicalError):plan.exploded_view_plan(self.f.app,{**self.args(),**change})
        self.t.assert_not_called()

    def test_unrepresentable_duration_and_strict_timing_bounds_refuse(self):
        steps=[{'durationSeconds':3600,'moves':[self.move()]},{'durationSeconds':1e-20,'moves':[self.move()]}]
        with self.assertRaises(c.MechanicalError):self.read(steps=steps)
        self.t.assert_not_called()
        result=self.read(steps=[{'durationSeconds':1,'moves':[self.move()]},{'durationSeconds':1,'moves':[self.move()]}],sampleTimes=[1])
        self.assertEqual(result['samples'][1]['stepProgress'],[{'stepIndex':0,'fraction':1.},{'stepIndex':1,'fraction':0.}])
        self.assertEqual(result['samples'][-1]['stepProgress'],[{'stepIndex':0,'fraction':1.},{'stepIndex':1,'fraction':1.}])

    def test_duplicate_simultaneous_instance_and_foreign_or_stale_refs_refuse(self):
        for moves in ([self.move(),self.move()], [{**self.move(),'occurrence':{**self.move()['occurrence'],'revision':'old'}}],
                      [{**self.move(),'occurrence':c.ref(self.f.pin,self.f.value)}]):
            with self.assertRaises(c.MechanicalError):self.read(steps=[{'durationSeconds':1,'moves':moves}])

    def test_unknown_parent_identity_or_incomplete_tree_refuses(self):
        original=self.f.ap.assemblyContext
        self.f.ap.assemblyContext=self.f.b
        with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'invalid_context')
        self.f.ap.assemblyContext=original
        self.f.a.childOccurrences=Collection()
        with self.assertRaises(c.MechanicalError):self.read()

    def test_same_path_foreign_wrapper_and_duplicate_path_refuse(self):
        self.f.a.childOccurrences=Collection([copy.copy(self.f.ap)])
        with self.assertRaises(c.MechanicalError):self.read()
        self.f.a.childOccurrences=Collection([self.f.ap]);self.f.root.allOccurrences.values.append(self.f.ap)
        with self.assertRaises(c.MechanicalError):self.read()

    def test_nonrigid_nonfinite_and_malformed_native_matrices_refuse(self):
        bad=[]
        x=pose();x[0]=2;bad.append(x)
        x=pose();x[0]=-1;bad.append(x)
        x=pose();x[3]=float('nan');bad.append(x)
        x=pose();x[15]=0;bad.append(x)
        x=pose();x[0]=True;bad.append(x)
        bad.append([1]*15)
        for values in bad:
            self.f.ap.transform2=NS(asArray=lambda:values)
            with self.subTest(values=values),self.assertRaises(c.MechanicalError):self.read()

    def test_budget_refusals_are_explicit_and_never_truncated_success(self):
        with patch.object(plan,'MAX_POSES',1):
            with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'plan_budget')
        self.assertIn('sampleTimes',error.exception.details['hint'])
        with patch.object(plan,'MAX_OUTPUT_BYTES',10):
            with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'plan_budget')
        self.assertGreater(error.exception.details['bytes'],10);self.assertIn('hint',error.exception.details)

    # -- designs over 256 occurrences: the moved occurrences and their parents ------------------------
    def big(self,count=4579):
        """The fixture read as a big design: reading every occurrence fails the test."""
        self.f.root.allOccurrences=NS(count=count,item=lambda _:self.fail('Must not read every occurrence'))

    def test_a_design_over_256_occurrences_plans_the_moved_scope_without_reading_every_occurrence(self):
        steps=[{'durationSeconds':2,'moves':[self.move(frame='local',distance=12)]}]
        full=self.read(steps=steps,sampleTimes=[1])
        self.big()
        result=self.read(steps=steps,sampleTimes=[1])
        rows=result['plan']['instances']
        self.assertEqual((full['plan']['instanceScope'],result['plan']['instanceScope']),('all','moved'))
        self.assertEqual((full['plan']['designInstances'],result['plan']['designInstances']),(4,4579))
        self.assertIn('children follow their parent',result['plan']['scopeNote']);self.assertNotIn('scopeNote',full['plan'])
        self.assertEqual([r['path'] for r in rows],['Assembly:1','Assembly:1+Pin:1'])
        self.assertIsNone(rows[0]['parentInstanceId']);self.assertEqual(rows[1]['parentInstanceId'],rows[0]['instanceId'])
        listed={r['instanceId'] for r in rows}
        self.assertEqual(rows,[r for r in full['plan']['instances'] if r['instanceId'] in listed])
        for mine,theirs in zip(result['samples'],full['samples']):
            self.assertEqual(mine['poses'],[p for p in theirs['poses'] if p['instanceId'] in listed])
        self.assertEqual(result['plan']['steps'],full['plan']['steps'])
        self.assertEqual(result['plan']['rootBodyGroup'],full['plan']['rootBodyGroup'])

    def test_moved_scope_matches_the_full_plan_for_every_listed_instance(self):
        cases=[[{'durationSeconds':2,'moves':[self.move(self.f.a,frame='local',distance=10),self.move(frame='parent')]},
                {'durationSeconds':1,'moves':[self.move(distance=4)]}],
               [{'durationSeconds':1,'moves':[self.move(self.f.bp,frame='parent',direction=[0,1,1])]}],
               [{'durationSeconds':1,'moves':[self.move(self.f.b,distance=3)]},{'durationSeconds':1,'moves':[self.move(self.f.ap,frame='local')]}]]
        for steps in cases:
            with self.subTest(steps=steps):
                full=self.read(steps=steps,sampleTimes=[.5,1])
                with patch.object(plan,'MAX_INSTANCES',0):
                    scoped=self.read(steps=steps,sampleTimes=[.5,1])
                rows={r['instanceId']:r for r in full['plan']['instances']}
                listed=[r['instanceId'] for r in scoped['plan']['instances']]
                for row in scoped['plan']['instances']:
                    self.assertEqual({k:v for k,v in row.items() if k!='label'},{k:v for k,v in rows[row['instanceId']].items() if k!='label'})
                self.assertEqual(listed,[key for key in rows if key in listed])  # the full inventory's order
                for mine,theirs in zip(scoped['samples'],full['samples']):
                    self.assertEqual(mine['poses'],[p for p in theirs['poses'] if p['instanceId'] in listed])
                self.assertEqual(scoped['plan']['steps'],full['plan']['steps'])

    def test_moved_scope_reads_parents_by_name_never_their_siblings(self):
        reads={'item':0,'name':0}
        class Level(Collection):
            def item(self,i):reads['item']+=1;return self.values[i]
            def itemByName(self,name):reads['name']+=1;return super().itemByName(name)
        filler=[Entity(objectType='adsk::fusion::Occurrence',name='Bolt:%d'%i,fullPathName='Bolt:%d'%i) for i in range(300)]
        self.f.root.occurrences=Level(filler+self.f.root.occurrences.values)
        self.f.a.childOccurrences=Level(self.f.a.childOccurrences.values)
        self.big()
        result=self.read()
        self.assertEqual([r['path'] for r in result['plan']['instances']],['Assembly:1','Assembly:1+Pin:1'])
        self.assertEqual(reads['item'],0)
        self.assertLessEqual(reads['name'],6)  # two levels: resolving the move, then each of the two captures

    def test_a_name_holding_a_plus_is_walked_in_the_moved_scope(self):
        washer=Entity(objectType='adsk::fusion::Occurrence',entityToken='occ-washer',fullPathName='M3+washer:1',name='M3+washer:1',
                      component=self.f.pin,assemblyContext=None,nativeObject=None,transform2=NS(asArray=lambda:pose(0,0,5)))
        ring=Entity(objectType='adsk::fusion::Occurrence',entityToken='occ-ring',fullPathName='M3+washer:1+Ring:2',name='Ring:2',
                    component=self.f.pin,assemblyContext=washer,nativeObject=None,transform2=NS(asArray=lambda:pose(0,0,6)),childOccurrences=Collection())
        washer.childOccurrences=Collection([ring]);self.f.root.occurrences.values.append(washer)
        self.f.tokens.update({'occ-washer':[washer],'occ-ring':[ring]})
        self.big()
        result=self.read(steps=[{'durationSeconds':1,'moves':[self.move(ring,distance=2)]}])
        self.assertEqual([r['path'] for r in result['plan']['instances']],['M3+washer:1','M3+washer:1+Ring:2'])
        self.assertEqual(self.positions(result)['M3+washer:1+Ring:2'],[2,0,6])

    def test_a_definition_local_reference_is_refused_in_the_moved_scope(self):
        filler=[Entity(objectType='adsk::fusion::Occurrence',name='Bolt:%d'%i,fullPathName='Bolt:%d'%i,nativeObject=None) for i in range(300)]
        self.f.root.allOccurrences=Collection(filler+self.f.root.allOccurrences.values)
        local=self.f.tokens['occ-Pin:1'][0]  # the pin's own occurrence, outside the root context
        with self.assertRaises(c.MechanicalError) as error:self.read(steps=[{'durationSeconds':1,'moves':[self.move(local)]}])
        self.assertEqual(error.exception.code,'invalid_context')
        self.assertIn('root-context instance',str(error.exception))

    def test_up_front_budgets_refuse_before_reading_any_instance(self):
        rows=[]
        with patch.object(plan,'instance_row',side_effect=lambda *a:rows.append(a)):
            with patch.object(plan,'MAX_POSES',7),self.assertRaises(c.MechanicalError) as error:self.read()  # 4 occurrences x 2 times
            self.assertEqual((error.exception.code,error.exception.details['instances']),('plan_budget',4))
            self.big()
            with patch.object(plan,'MAX_TRACKED',1),self.assertRaises(c.MechanicalError) as error:self.read()
            self.assertEqual((error.exception.code,error.exception.details['tracked']),('plan_budget',2))
            self.assertIn('parent subassembly',error.exception.details['hint'])
            with patch.object(plan,'MAX_POSES',3),self.assertRaises(c.MechanicalError) as error:self.read()  # 2 tracked x 2 times
            self.assertEqual((error.exception.details['instances'],error.exception.details['sampleTimes']),(2,2))
        self.assertEqual(rows,[])

    def test_the_time_budget_refuses_with_a_hint_in_both_scopes(self):
        for big in (False,True):
            if big:self.big()
            with patch.object(plan,'BUDGET_SECONDS',-1),self.assertRaises(c.MechanicalError) as error:self.read()
            self.assertEqual((error.exception.code,error.exception.details['instancesRead']),('plan_budget',0))
            self.assertEqual(error.exception.details['instances'],2 if big else 4)
            self.assertIn('Run it again',error.exception.details['hint'])
            self.assertIsInstance(error.exception.details['revisionSeconds'],float)  # the revision check before any instance

    def test_part_numbers_are_read_once_per_component_revision_and_a_bom_read_is_reused(self):
        state=importlib.import_module('mechanical_test_commands.mechanical.assembly_state')
        plan._part_numbers.clear();state._native_properties.clear()
        counts={'shared':0,'pin':0}
        def counted(name):
            def read(entity):
                counts[name]+=1;return 'PN-'+name
            return property(read)
        self.f.shared.__class__=type('Shared',(Entity,),{'partNumber':counted('shared')})
        self.f.pin.__class__=type('Pin',(Entity,),{'partNumber':counted('pin')})
        first=self.read();self.read()
        self.assertEqual(counts,{'shared':1,'pin':1})  # four instances of two components, two plans of two captures each
        self.assertEqual({r['partNumber'] for r in first['plan']['instances']},{'PN-shared','PN-pin'})
        self.f.pin.revisionId='r2';self.read()
        self.assertEqual(counts['pin'],2)
        state._native_properties[(c.identity(self.f.pin),'r3')]=('PN-from-bom','')
        self.f.pin.revisionId='r3'
        result=self.read()
        self.assertEqual(counts['pin'],2)
        self.assertIn('PN-from-bom',{r['partNumber'] for r in result['plan']['instances']})

    def test_root_bodies_past_256_are_counted_not_refused(self):
        self.f.root.bRepBodies=Collection([object()]*300)
        self.assertEqual(self.read()['plan']['rootBodyGroup']['bodyCount'],300)
        self.big()
        self.assertEqual(self.read()['plan']['rootBodyGroup']['bodyCount'],300)

    def test_a_component_inserted_by_reference_is_planned_and_a_foreign_one_refused(self):
        self.f.pin.parentDesign=Entity(parentDocument=Entity(dataFile=object()),rootComponent=Entity())
        self.assertEqual(len(self.read()['plan']['instances']),4)
        self.f.pin.parentDesign=Entity(parentDocument=Entity(dataFile=None),rootComponent=Entity())
        with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'wrong_component')

    def test_worker_session_and_stale_target_precede_native_snapshot(self):
        errors=[]
        def worker():
            try:self.read()
            except c.MechanicalError as error:errors.append(error.code)
        thread=threading.Thread(target=worker);thread.start();thread.join()
        self.assertEqual(errors,['wrong_thread']);self.t.assert_not_called();self.cfg.assert_not_called()
        with self.assertRaises(c.MechanicalError):plan.exploded_view_plan(self.f.app,{**self.args(),'sessionId':'old'})
        self.t.assert_not_called()
        self.t.side_effect=c.MechanicalError('stale_revision','changed')
        with self.assertRaises(c.MechanicalError):self.read()
        self.cfg.assert_not_called()

    def test_midread_model_and_flags_changes_refuse_without_restore(self):
        original=plan.sample_plan
        def changed(*args):
            self.f.doc.isModified=False
            return original(*args)
        with patch.object(plan,'sample_plan',side_effect=changed):
            with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'plan_source_changed');self.assertFalse(self.f.doc.isModified)
        self.f.doc.isModified=True
        def moved(*args):
            self.f.ap.transform2=NS(asArray=lambda:pose(200,10))
            return original(*args)
        with patch.object(plan,'sample_plan',side_effect=moved):
            with self.assertRaises(c.MechanicalError):self.read()
        self.assertEqual(self.f.ap.transform2.asArray()[3],20)

    def test_incomplete_configuration_refuses_and_exact_configuration_is_bound(self):
        result=self.read();self.assertTrue(result['plan']['configuration']['graphComplete'])
        self.cfg.side_effect=c.MechanicalError('configuration_scope_unsupported','incomplete')
        with self.assertRaises(c.MechanicalError) as error:self.read()
        self.assertEqual(error.exception.code,'configuration_scope_unsupported')

    def test_real_registration_read_schema_and_immutable_sampling(self):
        self.assertEqual(len(m.SCHEMA),73);self.assertNotIn('exploded_view_plan',m.WRITES)
        frozen,write=mech_exe.prepare_request(self.args(),m.SCHEMA,m.WRITES,m.SESSION_ID);self.assertFalse(write)
        result=self.read();snapshot={k:result['plan'][k] for k in ('instances','rootBodyGroup')}
        before=copy.deepcopy(snapshot)
        self.assertEqual(plan.sample_plan(snapshot,result['plan']['steps'],[0,1],1,'explode'),result['samples'])
        self.assertEqual(snapshot,before)
        self.assertFalse(result['graphicsCreated']);self.assertFalse(result['storyboardCreated']);self.assertFalse(result['rendered'])


class Node:
    """A root-context occurrence whose native reads are counted."""
    objectType='adsk::fusion::Occurrence'
    reads=0
    def __init__(self,name,component,parent,x):
        self.name,self.component,self.assemblyContext,self.nativeObject,self.isValid=name,component,parent,None,True
        self.childOccurrences=Collection();self.x=x
        self.path=(parent.path+'+' if parent else '')+name
        self.entityToken='occ-'+self.path
    @property
    def fullPathName(self):Node.reads+=1;return self.path
    @property
    def transform2(self):Node.reads+=1;return NS(asArray=lambda:pose(self.x))


class Scale(unittest.TestCase):
    """The scratch layout measured live: a subassembly of 6 parts and 2 kits of 4 parts (17 occurrences
    with itself) placed many times at the root."""
    def tree(self,copies):
        design=Entity();root=Entity(objectType='adsk::fusion::Component',name='Root',partNumber='',entityToken='root',parentDesign=design,
                                    bRepBodies=Collection(),revisionId='r1')
        design.rootComponent=root
        comps={name:Entity(objectType='adsk::fusion::Component',name=name,partNumber='',entityToken=name,parentDesign=design,revisionId='r1')
               for name in ('Sub','Kit','Part')}
        tokens={c.entityToken:[c] for c in comps.values()};tokens['root']=[root]
        def add(parent,name,comp,x):
            node=Node(name,comp,parent,x);tokens[node.entityToken]=[node]
            (parent.childOccurrences if parent else root.occurrences).values.append(node)
            return node
        root.occurrences=Collection()
        for s in range(1,copies+1):
            sub=add(None,'Sub:%d'%s,comps['Sub'],s*220)
            for k in range(1,7):add(sub,'Part:%d'%k,comps['Part'],s*220+k*30)
            for kit_index in (1,2):
                kit=add(sub,'Kit:%d'%kit_index,comps['Kit'],s*220)
                for k in range(1,5):add(kit,'Part:%d'%k,comps['Part'],s*220+k*25)
        root.allOccurrences=NS(count=17*copies,item=lambda _:self.fail('Must not read every occurrence'))
        design.findEntityByToken=lambda token:tokens.get(token,[])
        return design
    def plan(self,copies):
        c._entities.clear();design=self.tree(copies);value={'documentId':'doc','revision':'revision'}
        config={'configured':False,'isConfiguration':False,'configurationRowId':'','table':None,'complete':True}
        with patch.object(plan,'target',return_value=(Entity(isSaved=False,isModified=True),design,value)),\
             patch.object(plan,'configuration_snapshot',return_value=config):
            ref=lambda path:c.ref(c.occurrence_at(design,path),value)
            steps=[{'durationSeconds':1,'moves':[{'occurrence':ref('Sub:7+Kit:2+Part:3'),'direction':[0,0,1],'distance':20,'frame':'world'},
                                                  {'occurrence':ref('Sub:7+Kit:1'),'direction':[1,0,0],'distance':30,'frame':'parent'}]},
                   {'durationSeconds':1,'moves':[{'occurrence':ref('Sub:9'),'direction':[0,1,0],'distance':50,'frame':'world'}]}]
            Node.reads=0
            result=plan.exploded_view_plan(object(),{'op':'exploded_view_plan','sessionId':plan.SESSION_ID,'target':value,'unit':'mm','steps':steps})
            return result,Node.reads
    def test_the_moved_scope_reads_the_same_whatever_the_design_size(self):
        small,small_reads=self.plan(64)    # 1,088 occurrences, as measured live
        big,big_reads=self.plan(270)       # 4,590 occurrences
        self.assertEqual([r['path'] for r in big['plan']['instances']],['Sub:7','Sub:7+Kit:1','Sub:7+Kit:2','Sub:7+Kit:2+Part:3','Sub:9'])
        self.assertEqual((small['plan']['designInstances'],big['plan']['designInstances']),(1088,4590))
        self.assertEqual(small_reads,big_reads)
        self.assertLess(big_reads,200)


if __name__=='__main__':unittest.main()
