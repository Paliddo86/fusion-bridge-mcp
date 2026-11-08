"""Bounded two-layer orthogonal A* over the shared conservative copper model.

Executed after routing_script.py in the same namespace. Planning never mutates.
"""
import heapq
import time


def automatic_plan(board,args):
    began=time.monotonic()
    net=identifier(args.get('net'));nets,pads,_,_=objects(board)
    if net not in nets:fail('unknown_net','Unknown net '+net)
    ends=[]
    for key in ('fromPad','toPad'):
        p=pads.get(args.get(key))
        if p is None:fail('unknown_pad',key+' must name an existing pad')
        if p['net']!=net:fail('net_mismatch',key+' does not belong to '+net)
        ends.append(p)
    if ends[0]['name']==ends[1]['name']:fail('invalid_path','Choose two different pads')
    width=number(args.get('width',.25),True)
    dia=number(args.get('viaDiameter',.6),True);drill=number(args.get('viaDrill',.3),True)
    if drill>=dia:fail('invalid_geometry','Via drill must be smaller than diameter')
    grid=number(args.get('grid',.5),True)
    if not .1<=grid<=5:fail('invalid_args','grid must be 0.1..5 mm')
    allow=args.get('allowVias',True)
    if not isinstance(allow,bool):fail('invalid_args','allowVias must be boolean')
    via_cost=number(args.get('viaCost',5),True)
    max_nodes=args.get('maxNodes',25000);timeout=args.get('timeoutMs',3000)
    for v,lo,hi,label in [(max_nodes,1,100000,'maxNodes'),(timeout,10,5000,'timeoutMs')]:
        if type(v)!=int or not lo<=v<=hi:fail('invalid_args',label+' outside supported bounds')
    ctx=routing_context(board,args,net,width,drill)
    clearance=ctx['clearance'];edge=ctx['edgeClearance'];xmin,xmax,ymin,ymax=ctx['bounds']
    def axis(lo,hi,coordinates):
        n=math.floor((hi-lo)/grid)
        return sorted(set([round(lo+i*grid,6) for i in range(n+1)]+coordinates))
    if ((xmax-xmin)/grid+3)*((ymax-ymin)/grid+3)*2>250000:
        fail('search_budget','Grid has too many cells; use a coarser grid')
    xs=axis(xmin,xmax,[p['at'][0] for p in ends]);ys=axis(ymin,ymax,[p['at'][1] for p in ends])
    startxy=(xs.index(ends[0]['at'][0]),ys.index(ends[0]['at'][1]))
    goalxy=(xs.index(ends[1]['at'][0]),ys.index(ends[1]['at'][1]))
    layers=[1,16] if allow else sorted(set(ends[0]['layers'])&set(ends[1]['layers']))
    if not layers:fail('no_route','Endpoints need a via but allowVias is false')
    obstacles=[]
    for other,c,d,r,ls,*_ in ctx['obstacles']:
        if other==net:continue
        obstacles.append((c,d,r,set(ls),(min(c[0],d[0])-r-clearance,max(c[0],d[0])+r+clearance,min(c[1],d[1])-r-clearance,max(c[1],d[1])+r+clearance)))
    cache={}
    def clear(a,b,radius,ls,via=False):
        if outline_problem(ctx,a,b,radius):return False   # any closed outline, not only a rectangle (#123)
        ax,bx=min(a[0],b[0])-radius,max(a[0],b[0])+radius
        ay,by=min(a[1],b[1])-radius,max(a[1],b[1])+radius
        for c,d,r,ol,bounds in obstacles:
            if not ls.intersection(ol):continue
            if bx<bounds[0] or ax>bounds[1] or by<bounds[2] or ay>bounds[3]:continue
            if distance(a,b,c,d)<radius+r+clearance-1e-6:return False
        # Avoid placing new vias inside pads, including same-net SMD terminals.
        if via and any(math.dist(a,p['at'])<radius+p['radius']+clearance for p in pads.values()):return False
        return True
    def edge_clear(u,v):
        key=tuple(sorted((u,v)))
        if key not in cache:
            a=(xs[u[0]],ys[u[1]]);b=(xs[v[0]],ys[v[1]])
            via=u[2]!=v[2]
            cache[key]=clear(a,b,max(dia/2,drill+.3) if via else width/2,set(layers) if via else {u[2]},via)
        return cache[key]
    def heuristic(u):
        return abs(xs[u[0]]-ends[1]['at'][0])+abs(ys[u[1]]-ends[1]['at'][1])+(0 if u[2] in ends[1]['layers'] else via_cost)
    queue=[];costs={};parents={};serial=0
    for l in ends[0]['layers']:
        if l not in layers:continue
        u=(*startxy,l);costs[u]=0;heapq.heappush(queue,(heuristic(u),0,serial,u));serial+=1
    visited=set();goal=None
    while queue:
        if len(visited)>=max_nodes or time.monotonic()-began>timeout/1000:
            fail('search_budget','No route found within the node/time budget; no copper was changed')
        _,g,_,u=heapq.heappop(queue)
        if u in visited:continue
        visited.add(u)
        if u[:2]==goalxy and u[2] in ends[1]['layers']:goal=u;break
        x,y,l=u;neighbors=[]
        for dx,dy in ((1,0),(0,1),(-1,0),(0,-1)):
            nx,ny=x+dx,y+dy
            if 0<=nx<len(xs) and 0<=ny<len(ys):neighbors.append((nx,ny,l))
        if allow:neighbors.append((x,y,17-l))
        for v in neighbors:
            if v in visited or not edge_clear(u,v):continue
            step=via_cost if l!=v[2] else abs(xs[x]-xs[v[0]])+abs(ys[y]-ys[v[1]])
            ng=g+step
            if ng>=costs.get(v,float('inf'))-1e-9:continue
            costs[v]=ng;parents[v]=u;heapq.heappush(queue,(ng+heuristic(v),ng,serial,v));serial+=1
    if goal is None:fail('no_route','No legal route exists on the requested grid; no copper was changed')
    path=[goal]
    while path[-1] in parents:path.append(parents[path[-1]])
    path.reverse();simple=[path[0]]
    for a,b,c in zip(path,path[1:],path[2:]):
        if a[2]==b[2]==c[2] and ((a[0]==b[0]==c[0]) or (a[1]==b[1]==c[1])):continue
        simple.append(b)
    simple.append(path[-1]);points=[]
    for x,y,l in simple:
        xy=[xs[x],ys[y]]
        if points and [points[-1]['x'],points[-1]['y']]==xy:points[-1]['layer']=l
        else:points.append({'x':xy[0],'y':xy[1],'layer':l})
    route={'net':net,'width':width,'layer':path[0][2],'viaDiameter':dia,'viaDrill':drill,'clearance':clearance,'points':points}
    candidate=plan(board,route) # Same authoritative preflight used by the mutation verb.
    length=sum(math.dist(i['start'],i['end']) for i in candidate['items'] if i['kind']=='segment')
    return {**candidate,'route':route,'planner':{'algorithm':'bounded orthogonal two-layer A*','grid':grid,'expandedNodes':len(visited),'elapsedMs':round((time.monotonic()-began)*1000,3),'lengthMm':round(length,6),'viaCount':sum(i['kind']=='via' for i in candidate['items']),'externalRouter':False}}
