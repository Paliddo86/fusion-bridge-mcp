"""Read-only exploded assembly plans. Translation math never moves native objects.

A design of up to MAX_INSTANCES occurrences gets a plan of every root-context occurrence
(instanceScope "all"). A bigger design gets a plan of the occurrences the steps move and their
parents (instanceScope "moved"), each found by name level by level from the root, so a
4,579-occurrence assembly pays only for the parts its steps move. Every other occurrence keeps its
home pose plus the offset of its nearest listed parent: children follow their parent."""
import json
import math
import threading
import time

from .assembly_state import native_properties
from .core import SESSION_ID, concrete, digest, identity, ref, require, resolve, same, target
from .configurations import provenance, snapshot as configuration_snapshot

MAX_INSTANCES = 256      # a design with up to this many occurrences gets the whole inventory
MAX_TRACKED = 4096       # the most occurrences a moved-scope plan, render or still tracks (moved ones and their parents)
MAX_DEPTH = 32
MAX_STEPS = 32
MAX_MOVES = 128
MAX_SAMPLES = 64
MAX_POSES = 4096
MAX_ROOT_BODIES = 20000
MAX_OUTPUT_BYTES = 2 * 1024 * 1024
# native reads stop past this many seconds from the start: ab's relay gives a read about 60 s
BUDGET_SECONDS = 40.0
IDENTITY = [1.,0.,0.,0., 0.,1.,0.,0., 0.,0.,1.,0., 0.,0.,0.,1.]
SPLIT_HINT = ('Split the steps into several plans, or move a parent subassembly instead of each of its parts '
              '(children follow their parent).')
SAMPLES_HINT = 'Ask for fewer sampleTimes (both endpoints are always included), or split the steps into several plans.'
BUDGET_HINT = ('Run it again: the part numbers it read are kept for this session (on a cloud assembly each is a cloud read '
               'of about 0.5 s), so the next run goes further. Or move fewer parts per plan.')
SCOPE_NOTE = ('instances lists the occurrences the steps move and their parents. Every other occurrence keeps its home pose '
              'plus the translation of its nearest listed parent: children follow their parent.')


def bounded(collection):
    count = collection.count
    require(type(count) is int and 0 <= count <= MAX_INSTANCES, 'plan_budget', 'Occurrence collection exceeds 256 instances or has an invalid count.')
    return [concrete(collection.item(i)) for i in range(count)]


def text(value, limit=2048):
    require(isinstance(value,str) and len(value)<=limit, 'plan_identity_unavailable', 'Instance metadata is unavailable or exceeds its bound.')
    return value


def matrix(value):
    out = list(value.asArray())
    require(len(out)==16 and all(type(v) in (int,float) and math.isfinite(v) for v in out),
            'plan_frame_invalid', 'Home pose must be a complete finite matrix.')
    require(all(abs(out[12+i]-v)<=1e-9 for i,v in enumerate((0,0,0,1))), 'plan_frame_invalid', 'Home pose is not affine.')
    axes = [[out[r*4+c] for r in range(3)] for c in range(3)]
    require(all(abs(sum(v*v for v in axis)-1)<=1e-6 for axis in axes)
            and all(abs(sum(a*b for a,b in zip(axes[i],axes[j])))<=1e-6 for i,j in ((0,1),(0,2),(1,2))),
            'plan_frame_invalid', 'Home pose is not orthonormal; scaled instances are unsupported.')
    a,b,c=axes
    cross=[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
    require(sum(x*y for x,y in zip(cross,c))>1-1e-6, 'plan_frame_invalid', 'Reflected home poses are unsupported.')
    for index in (3,7,11):
        out[index] *= 10
        require(math.isfinite(out[index]) and abs(out[index])<=1e9, 'plan_frame_invalid', 'Home translation exceeds 1e9 mm.')
    return out


class Budget:
    """The native reads' time budget, counted from the start of the operation, which is what ab's relay
    waits on: a read that outlives it is lost to the caller and keeps Fusion busy. revision_seconds is
    the revision check every guarded operation runs before the plan reads any instance."""
    def __init__(self):
        self.started = time.perf_counter()
        self.revision_seconds = None

    def check(self, done, total):
        elapsed = time.perf_counter() - self.started
        require(elapsed <= BUDGET_SECONDS, 'plan_budget',
                'The plan read %d of %d instances in %.1f s, past its %g s budget.' % (done, total, elapsed, BUDGET_SECONDS),
                hint=BUDGET_HINT, instancesRead=done, instances=total, elapsedSeconds=round(elapsed, 1),
                revisionSeconds=self.revision_seconds)


_part_numbers = {}  # (component identity, revisionId) -> part number


def part_number(comp):
    """A component's part number, read once per component revision this session (each read is a cloud
    round trip of about 0.5 s on a cloud assembly, and repeated instances share their component); what
    a bom run read is reused."""
    key = (identity(comp), str(comp.revisionId))
    if key not in _part_numbers:
        known = native_properties(comp, read=False)
        if len(_part_numbers) > 50000:
            _part_numbers.clear()
        _part_numbers[key] = text(known[0] if known is not None else comp.partNumber, 256)
    return _part_numbers[key]


def owned(comp, design):
    """A component of this design, or one inserted by reference from another saved design: read-only
    here, as the revision digest treats it."""
    if same(comp.parentDesign, design):
        return True
    source = getattr(comp.parentDesign, 'parentDocument', None)
    return not same(comp, design.rootComponent) and source is not None and getattr(source, 'dataFile', None) is not None


def instance_row(design, value, occurrence, path, parent, parent_handle, refs):
    """One instance: its exact reference, component identity and home pose. Its parent is the native
    assembly context, checked against the parent row, never inferred from the path. refs holds one
    component reference per component, so repeated instances read their component's token once."""
    context=concrete(occurrence.assemblyContext)
    require((parent is None and context is None) or
            (parent is not None and context is not None and text(context.fullPathName)==parent['path']
             and same(context,parent_handle)),
            'invalid_context', 'Occurrence parent context disagrees with the native hierarchy.')
    comp=concrete(occurrence.component)
    require(comp is not None and comp.isValid and owned(comp,design), 'wrong_component', 'Occurrence component belongs to another design.')
    comp_id=identity(comp)
    if comp_id not in refs:
        refs[comp_id]=ref(comp,value)
    return {'instanceId':digest([SESSION_ID,value['documentId'],path,comp_id]),'path':path,'occurrence':ref(occurrence,value),
            'parentInstanceId':parent['instanceId'] if parent else None,
            'componentId':comp_id,'component':refs[comp_id],'componentName':text(comp.name,256),
            'partNumber':part_number(comp),'homeMatrix':matrix(occurrence.transform2)}


def labelled(rows):
    # Numbers are presentation labels within this source-bound plan, not BOM quantities.
    labels={}
    for row in rows:
        labels.setdefault(row['componentId'],'C'+str(len(labels)+1).zfill(3))
        row['label']={'item':labels[row['componentId']],'instance':row['path'],
                      'componentId':row['componentId'],'partNumber':row['partNumber'],
                      'scope':'presentation component label; not an engineering BOM item number'}
    return rows


def root_group(design, value):
    root=design.rootComponent
    root_bodies=root.bRepBodies.count
    require(type(root_bodies) is int and 0<=root_bodies<=MAX_ROOT_BODIES,'plan_budget','Root body inventory exceeds its bound.')
    return {'componentId':identity(root),'component':ref(root,value),'bodyCount':root_bodies,
            'homeMatrix':list(IDENTITY),'selectable':False,'scope':'fixed root-owned direct bodies'}


def capture(design, value, budget=None, refs=None):
    """Capture the actual root-context tree; never infer parents from names."""
    budget = Budget() if budget is None else budget
    refs = {} if refs is None else refs
    root = design.rootComponent
    candidates = bounded(root.allOccurrences)
    by_path = {}
    for occurrence in candidates:
        require(occurrence is not None and occurrence.isValid and occurrence.objectType=='adsk::fusion::Occurrence',
                'plan_identity_unavailable', 'Invalid occurrence in root-context inventory.')
        path=text(occurrence.fullPathName)
        require(path and path not in by_path, 'ambiguous_context', 'Every root-context occurrence needs a unique full path.')
        by_path[path]=occurrence
    rows, seen, handles = [], set(), {}
    def walk(occurrence,parent,depth):
        require(depth<=MAX_DEPTH, 'plan_budget', 'Occurrence hierarchy exceeds 32 levels.')
        path=text(occurrence.fullPathName)
        require(path in by_path and same(occurrence,by_path[path]) and path not in seen,
                'ambiguous_context', 'Child traversal does not identify one exact root-context instance.')
        occurrence=by_path[path]
        budget.check(len(rows),len(by_path))
        row=instance_row(design,value,occurrence,path,parent,handles[parent['instanceId']] if parent else None,refs)
        rows.append(row);seen.add(path);handles[row['instanceId']]=occurrence
        for child in sorted(bounded(occurrence.childOccurrences),key=lambda item:text(item.fullPathName)):
            walk(child,row,depth+1)
    for occurrence in sorted(bounded(root.occurrences),key=lambda item:text(item.fullPathName)):
        walk(occurrence,None,1)
    require(seen==set(by_path), 'invalid_context', 'Native hierarchy did not cover every root-context occurrence.')
    return {'instances':labelled(rows),'rootBodyGroup':root_group(design,value)}, handles


def chain_at(design, path, found):
    """The root-context occurrences along a full path, outermost first, as (path, names, occurrence):
    found level by level by name (itemByName) instead of by reading every occurrence. A name can hold
    '+', so each level takes the shortest name that exists, as core.occurrence_at does. found caches
    the levels already walked (path -> occurrence), so parts in one subassembly read their shared
    parents once. None when the path does not resolve."""
    level, parts, names, chain = design.rootComponent.occurrences, path.split('+') if path else [], [], []
    try:
        while parts:
            for size in range(1, len(parts) + 1):
                name = '+'.join(parts[:size])
                key = '+'.join(names + [name])
                occurrence = found.get(key)
                if occurrence is None:
                    occurrence = level.itemByName(name)
                if occurrence is not None:
                    break
            else:
                return None
            found[key] = occurrence
            names.append(name)
            parts = parts[size:]
            chain.append((key, tuple(names), occurrence))
            if parts:
                level = occurrence.childOccurrences
    except (AttributeError, RuntimeError):
        return None
    return chain or None


def moved_nodes(design, paths):
    """The occurrences at these full paths and all their parents: full path -> (names, occurrence,
    parent path). Nothing here reads every occurrence."""
    found, nodes = {}, {}
    for path in paths:
        chain = chain_at(design, path, found)
        require(chain is not None, 'invalid_context',
                'A moved occurrence is not a root-context instance of this design, or one of its parents is gone.')
        parent = None
        for key, names, occurrence in chain:
            require(len(names) <= MAX_DEPTH, 'plan_budget', 'Occurrence hierarchy exceeds 32 levels.')
            nodes.setdefault(key, (names, occurrence, parent))
            parent = key
    require(len(nodes) <= MAX_TRACKED, 'plan_budget',
            'The steps move %d occurrences with their parents; a plan tracks at most %d.' % (len(nodes), MAX_TRACKED),
            hint=SPLIT_HINT, tracked=len(nodes), limit=MAX_TRACKED)
    return nodes


def capture_moved(design, value, nodes, budget=None, refs=None):
    """The plan's rows for just the moved occurrences and their parents, in the full inventory's order
    (each parent before its children, siblings by name). Rows read the same facts as the full capture."""
    budget = Budget() if budget is None else budget
    refs = {} if refs is None else refs
    rows, handles, index = [], {}, {}
    for key, (names, occurrence, parent_key) in sorted(nodes.items(), key=lambda item: item[1][0]):
        budget.check(len(rows), len(nodes))
        occurrence = concrete(occurrence)
        require(occurrence is not None and occurrence.isValid and occurrence.objectType=='adsk::fusion::Occurrence',
                'plan_identity_unavailable', 'Invalid occurrence in the moved scope.')
        path = text(occurrence.fullPathName)
        require(path == key, 'ambiguous_context', 'A path does not identify one exact root-context instance.')
        parent = rows[index[parent_key]] if parent_key is not None else None
        row = instance_row(design, value, occurrence, path, parent, handles[parent['instanceId']] if parent else None, refs)
        index[key] = len(rows)
        rows.append(row)
        handles[row['instanceId']] = occurrence
    return {'instances': labelled(rows), 'rootBodyGroup': root_group(design, value)}, handles


def resolve_moves(design, value, steps):
    """Each move's occurrence, resolved once (compile_steps reuses them), and their full paths."""
    resolved, paths = [], []
    for step in steps:
        occurrences = [resolve(design, value, move['occurrence'], ('Occurrence',)) for move in step['moves']]
        resolved.append(occurrences)
        paths.extend(text(occurrence.fullPathName) for occurrence in occurrences)
    return resolved, paths


def request_arguments(args, max_steps=MAX_STEPS, max_moves=MAX_MOVES):
    require(args.get('unit')=='mm', 'invalid_input', 'Exploded planning distances require explicit mm.')
    playback=args.get('playback','explode')
    require(playback in ('explode','assemble'), 'invalid_input', 'playback must be explode or assemble.')
    steps=args.get('steps')
    require(isinstance(steps,list) and 1<=len(steps)<=max_steps,'invalid_input','Supply 1–%d ordered steps.'%max_steps)
    total=0.;count=0
    for step in steps:
        require(isinstance(step,dict) and set(step)<= {'durationSeconds','moves','title','instruction'} and {'durationSeconds','moves'}<=set(step),
                'invalid_input','Each step requires durationSeconds and moves.')
        duration=step['durationSeconds']
        require(type(duration) in (int,float) and math.isfinite(duration) and 0<duration<=3600,
                'invalid_input','Step duration must be finite, positive and at most 3600 seconds.')
        if 'title' in step:require(isinstance(step['title'],str) and len(step['title'])<=256,'invalid_input','Step title must be at most 256 characters.')
        if 'instruction' in step:require(isinstance(step['instruction'],str) and len(step['instruction'])<=1024,'invalid_input','Step instruction must be at most 1024 characters.')
        end=total+duration
        require(math.isfinite(end) and end>total,
                'invalid_input', 'Step duration is too small to represent after preceding steps.')
        total=end
        moves=step['moves'];require(isinstance(moves,list) and 1<=len(moves)<=max_moves,'invalid_input','Each step requires 1–%d moves.'%max_moves)
        count+=len(moves)
        for move in moves:
            require(isinstance(move,dict) and set(move)=={'occurrence','direction','distance','frame'}, 'invalid_input','Move fields must be occurrence, direction, distance and frame.')
            require(move['frame'] in ('world','parent','local'),'invalid_input','Unknown translation frame.')
            direction=move['direction'];distance=move['distance']
            require(isinstance(direction,list) and len(direction)==3 and all(type(v) in (int,float) and math.isfinite(v) and abs(v)<=1e9 for v in direction),
                    'invalid_input','Direction requires three finite bounded numbers.')
            require(math.hypot(*direction)>1e-12,'invalid_input','Direction must be nonzero.')
            require(type(distance) in (int,float) and math.isfinite(distance) and 0<distance<=1e6,
                    'invalid_input','Distance must be positive and at most 1e6 mm; use direction for sign.')
    require(count<=max_moves and total<=3600,'plan_budget','Plan exceeds %d moves or 3600 seconds.'%max_moves)
    times=args.get('sampleTimes',[])
    require(isinstance(times,list) and len(times)<=MAX_SAMPLES-2 and all(type(v) in (int,float) and math.isfinite(v) and 0<=v<=total for v in times),
            'invalid_input','Supply at most 62 finite sample times within the plan duration.')
    # Endpoints always appear, and exact endpoint handling preserves home matrices.
    return total, sorted(set([0.,total]+times)), playback


def compile_steps(design,value,snapshot,handles,steps,resolved=None):
    """resolved: each step's move occurrences already resolved, in order (resolve_moves); else each
    move's reference is resolved here."""
    lookup={row['instanceId']:row for row in snapshot['instances']}
    by_path={row['path']:row for row in snapshot['instances']}
    result=[];start=0.
    for index,step in enumerate(steps):
        compiled=[];seen=set()
        for position,move in enumerate(step['moves']):
            occurrence=resolved[index][position] if resolved is not None else resolve(design,value,move['occurrence'],('Occurrence',))
            path=text(occurrence.fullPathName);row=by_path.get(path)
            require(row is not None and same(occurrence,handles[row['instanceId']]), 'invalid_context','Selected reference is not this exact root-context instance.')
            key=row['instanceId'];require(key not in seen,'invalid_input','An instance may occur only once within a simultaneous step.');seen.add(key)
            norm=math.hypot(*move['direction']);direction=[v/norm for v in move['direction']]
            frame=IDENTITY if move['frame']=='world' else row['homeMatrix'] if move['frame']=='local' else lookup[row['parentInstanceId']]['homeMatrix'] if row['parentInstanceId'] else IDENTITY
            world=[sum(frame[r*4+c]*direction[c] for c in range(3))*move['distance'] for r in range(3)]
            compiled.append({'instanceId':key,'frame':move['frame'],'direction':direction,'distance':move['distance'],'worldTranslation':world})
        end=start+step['durationSeconds']
        result.append({'index':index,'title':step.get('title','Step '+str(index+1)), 'instruction':step.get('instruction',''), 'startSeconds':start,'endSeconds':end,
                       'durationSeconds':step['durationSeconds'],'moves':compiled})
        start=end
    return result


def pose_budget(instances, samples):
    require(instances*samples<=MAX_POSES,'plan_budget',
            'Plan samples exceed %d instance poses (%d instances x %d sample times).'%(MAX_POSES,instances,samples),
            hint=SAMPLES_HINT,instances=instances,sampleTimes=samples,limit=MAX_POSES)


def sample_plan(snapshot,steps,times,total,playback):
    """Pure deterministic sampling from immutable root home matrices."""
    pose_budget(len(snapshot['instances']),len(times))
    results=[]
    for time_point in times:
        forward=time_point if playback=='explode' else total-time_point
        own={row['instanceId']:[0.,0.,0.] for row in snapshot['instances']}
        progress=[]
        for step in steps:
            fraction=0. if forward<=step['startSeconds'] else 1. if forward>=step['endSeconds'] else (forward-step['startSeconds'])/step['durationSeconds']
            progress.append({'stepIndex':step['index'],'fraction':fraction})
            for move in step['moves']:
                for axis in range(3):own[move['instanceId']][axis]+=move['worldTranslation'][axis]*fraction
        composed={};poses=[]
        for row in snapshot['instances']:
            key=row['instanceId'];parent=composed[row['parentInstanceId']] if row['parentInstanceId'] else [0.,0.,0.]
            offset=[own[key][axis]+parent[axis] for axis in range(3)];composed[key]=offset
            pose=list(row['homeMatrix'])
            if forward!=0:
                for axis,index in enumerate((3,7,11)):pose[index]+=offset[axis]
            require(all(math.isfinite(v) and abs(v)<=2e9 for v in pose),'plan_frame_invalid','Sample pose exceeds finite output bounds.')
            poses.append({'instanceId':key,'matrix':pose,'translationFromHome':offset})
        results.append({'timeSeconds':time_point,'forwardTimeSeconds':forward,'stepProgress':progress,'poses':poses})
    return results


def exploded_view_plan(app,args):
    require(threading.current_thread() is threading.main_thread(),'wrong_thread','Exploded plan native reads require the main thread.')
    require(args.get('sessionId')==SESSION_ID,'session_mismatch','Exploded planning requires the current session.')
    wanted=args.get('target')
    require(isinstance(wanted,dict) and isinstance(wanted.get('documentId'),str) and isinstance(wanted.get('revision'),str),
            'invalid_input','Exploded planning requires exact target documentId and revision.')
    total,times,playback=request_arguments(args)
    budget=Budget()
    document,design,value=target(app,args)
    budget.revision_seconds=round(time.perf_counter()-budget.started,1)
    flags=(document.isSaved,document.isModified)
    require(all(type(v) is bool for v in flags),'plan_state_unavailable','Document flags are unavailable.')
    config=configuration_snapshot(design,strict=True)
    count=design.rootComponent.allOccurrences.count
    require(type(count) is int and count>=0,'plan_identity_unavailable','The occurrence count is unavailable.')
    whole=count<=MAX_INSTANCES
    refs={}  # component references, read once per component for both captures
    if whole:
        pose_budget(count,len(times))  # before any per-instance read
        resolved=nodes=None
    else:
        resolved,paths=resolve_moves(design,value,args['steps'])
        nodes=moved_nodes(design,paths)
        pose_budget(len(nodes),len(times))
    def take(walked=None):
        if whole:
            return capture(design,value,budget,refs)
        return capture_moved(design,value,walked or moved_nodes(design,paths),budget,refs)
    snapshot,handles=take(nodes)
    steps=compile_steps(design,value,snapshot,handles,args['steps'],resolved)
    samples=sample_plan(snapshot,steps,times,total,playback)
    after,_=take()
    after_flags=(document.isSaved,document.isModified)
    require(after==snapshot and configuration_snapshot(design,strict=True)==config and all(type(v) is bool for v in after_flags) and after_flags==flags,
            'plan_source_changed','Model instance, configuration or document state changed during planning.')
    _,_,current=target(app,args)
    require(current==value,'plan_source_changed','Model target changed during planning.')
    plan={'schemaVersion':1,'sessionId':SESSION_ID,'target':value,'configuration':provenance(config),
          'configurationGraphDigest':digest(config),'units':{'translation':'mm','time':'s'},'space':'root/world',
          'matrixLayout':'row-major; column-vector multiplication; translation indices 3,7,11',
          'framePolicy':'directions normalized; axes fixed at home; parent translations inherited once; explicit child translations add',
          'instanceScope':'all' if whole else 'moved','designInstances':count,
          **({} if whole else {'scopeNote':SCOPE_NOTE}),
          'durationSeconds':total,'steps':steps,**snapshot}
    result={'kind':'PRESENTATION_PLAN','planId':digest(plan),'plan':plan,'playback':playback,'samples':samples,
            'modelUnchanged':True,'nativeMoved':False,'graphicsCreated':False,'rendered':False,'storyboardCreated':False,
            'bomQuantities':'not computed or changed; labels link component identities and exact instance paths',
            'limits':['Explicit directions only; no automatic layout, rotation interpolation, collision or physical disassembly proof.',
                      'Over %d occurrences, instances lists only the moved occurrences and their parents (instanceScope moved).'%MAX_INSTANCES,
                      'Root-owned direct bodies are a fixed group and cannot be selected as occurrences.',
                      'No geometry copies, graphics preview, trails, video, native storyboard or archive-persistence claim.',
                      'Plan identity is bound to this session and model revision; no cross-session reference replay.']}
    size=len(json.dumps(result,allow_nan=False).encode('utf-8'))
    require(size<=MAX_OUTPUT_BYTES,'plan_budget','Plan is %d bytes, past its 2 MiB output bound.'%size,
            hint=SAMPLES_HINT,bytes=size,limit=MAX_OUTPUT_BYTES)
    return result


READS={'exploded_view_plan':exploded_view_plan}
