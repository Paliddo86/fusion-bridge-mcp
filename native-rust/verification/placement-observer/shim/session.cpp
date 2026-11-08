#include "session.h"
#include <Windows.h>
#include <Core/Application/Application.h>
#include <Core/Application/Document.h>
#include <Core/Application/Documents.h>
#include <Core/Application/Products.h>
#include <Core/UserInterface/UserInterface.h>
#include <Core/Geometry/Matrix3D.h>
#include <Core/Geometry/Point3D.h>
#include <Core/Geometry/Vector3D.h>
#include <Fusion/Fusion/Design.h>
#include <Fusion/Fusion/Timeline.h>
#include <Fusion/Fusion/AreaProperties.h>
#include <Fusion/Fusion/PhysicalProperties.h>
#include <Fusion/Fusion/ModelParameter.h>
#include <Fusion/Components/Component.h>
#include <Fusion/Sketch/Sketches.h>
#include <Fusion/Sketch/Sketch.h>
#include <Fusion/Sketch/SketchCurves.h>
#include <Fusion/Sketch/SketchPoints.h>
#include <Fusion/Sketch/SketchDimensions.h>
#include <Fusion/Sketch/SketchTexts.h>
#include <Fusion/Sketch/GeometricConstraints.h>
#include <Fusion/Sketch/Profiles.h>
#include <Fusion/Sketch/Profile.h>
#include <Fusion/Sketch/ProfileLoops.h>
#include <Fusion/Sketch/ProfileLoop.h>
#include <Fusion/Sketch/ProfileCurves.h>
#include <Fusion/Construction/ConstructionPlane.h>
#include <Fusion/Features/Features.h>
#include <Fusion/Features/ExtrudeFeatures.h>
#include <Fusion/Features/ExtrudeFeature.h>
#include <Fusion/Features/DistanceExtentDefinition.h>
#include <Fusion/Features/ProfilePlaneStartDefinition.h>
#include <Fusion/BRep/BRepBody.h>
#include <Fusion/BRep/BRepBodies.h>
#include <array>
#include <cstring>
#include <cmath>
#include <Fusion/Sketch/SketchLine.h>
#include <Fusion/Sketch/SketchLinearDimension.h>
#include <Fusion/Fusion/ModelParameters.h>
#include <Fusion/Sketch/SketchLines.h>
#include <Fusion/Sketch/SketchPoint.h>
#include <Fusion/Sketch/SketchEntityList.h>
#include <Fusion/Sketch/GeometricConstraint.h>
#include <Fusion/Sketch/GeometricConstraintList.h>
#include <Fusion/Sketch/HorizontalConstraint.h>
#include <Fusion/Sketch/VerticalConstraint.h>
#include <Fusion/Components/Components.h>
#include <Fusion/Components/Occurrences.h>
#include <Fusion/Fusion/ParameterList.h>
#include <Fusion/Fusion/UserParameters.h>
#include <Fusion/MeshBody/MeshBodies.h>
#include <Fusion/Graphics/CustomGraphicsGroups.h>
#include <Fusion/Construction/ConstructionPlanes.h>
#include <Fusion/Construction/ConstructionAxes.h>
#include <Fusion/Construction/ConstructionPoints.h>
using adsk::core::Ptr;
#include <Fusion/BRep/BRepBody.h>

#include <Fusion/Components/Occurrence.h>
#include <Fusion/Components/OccurrenceList.h>
#include <Fusion/Components/Joints.h>
#include <Fusion/Components/AsBuiltJoints.h>
#include <Fusion/Components/RigidGroups.h>
#include <Fusion/Components/AssemblyConstraints.h>
#include <Fusion/Components/MotionLinks.h>
#include <Fusion/Components/JointOrigins.h>
#include <Fusion/Fusion/Snapshots.h>
#include <Fusion/Fusion/TimelineObject.h>
#include <Core/Application/Attributes.h>
#include <Core/Application/Attribute.h>
struct EoContext {
 DWORD thread;bool release_attempted=false,roster_bound=false,prepare_attempted=false,bound=false;size_t document_count=0;
 EoDiagnostic diagnostic{};Ptr<adsk::core::Application>app;
 std::array<Ptr<adsk::core::Document>,16>documents{};
 Ptr<adsk::core::Document>document;Ptr<adsk::fusion::Design>design;Ptr<adsk::fusion::Component>root;
 std::array<Ptr<adsk::fusion::Component>,2>components{};
 std::array<Ptr<adsk::fusion::Occurrence>,3>occurrences{};
};
static_assert(sizeof(EoFacts)==16&&sizeof(EoInit)==16,"initial ABI");
static_assert(sizeof(EoDiagnostic)==4112&&offsetof(EoDiagnostic,text)==16,"diagnostic ABI");
static_assert(sizeof(EoAssemblyFacts)==968&&offsetof(EoAssemblyFacts,matrices)==536&&offsetof(EoAssemblyFacts,snapshot_count)==920,"assembly ABI");
static bool ok(EoContext*c,uint32_t stage){auto &d=c->diagnostic;d={};d.stage=stage;std::string text;d.sdk_error=c->app->getLastError(&text);if(d.sdk_error){d.length=static_cast<uint32_t>(text.size()>sizeof(d.text)?sizeof(d.text):text.size());d.truncated=text.size()>sizeof(d.text)?1:0;if(d.length)std::memcpy(d.text,text.data(),d.length);return false;}return true;}
static int check(EoContext*c){if(!c||c->thread!=GetCurrentThreadId()||c->release_attempted)return 2;c->diagnostic={};c->diagnostic.sdk_error=-1;c->diagnostic.stage=1;return c->app?0:3;}
extern "C" uint32_t eo_thread()noexcept{return GetCurrentThreadId();}
extern "C" int eo_open(EoContext**out)noexcept{if(!out)return 1;*out=nullptr;try{*out=new EoContext{};auto c=*out;c->thread=GetCurrentThreadId();c->app=adsk::core::Application::get();if(!c->app)return 3;bool valid=c->app->isValid();return ok(c,2)&&valid?0:3;}catch(...){return 4;}}
extern "C" int eo_guard(EoContext*c,EoBytes sink,void*u,uint32_t*jobs)noexcept{try{int r=check(c);if(r)return r;if(!sink||!jobs)return 1;auto ui=c->app->userInterface();if(!ok(c,10)||!ui)return 7;auto command=ui->activeCommand();if(!ok(c,11))return 7;*jobs=c->app->hasActiveJobs()?1:0;if(!ok(c,12))return 7;return sink(u,0,command.data(),command.size())?0:5;}catch(...){return 4;}}
extern "C" int eo_roster(EoContext*c,uint64_t*out)noexcept{try{int r=check(c);if(r)return r;if(!out)return 1;auto ds=c->app->documents();if(!ok(c,20)||!ds)return 7;auto n=ds->count();if(!ok(c,21)||n>16)return 6;*out=n;if(c->roster_bound&&n!=c->document_count)return 6;bool initial=!c->roster_bound;c->roster_bound=true;if(initial)c->document_count=n;for(size_t i=0;i<n;++i){auto d=ds->item(i);if(!ok(c,22)||!d)return 7;for(size_t j=0;j<i;++j)if(c->documents[j]==d)return 6;if(initial)c->documents[i]=d;else if(c->documents[i]!=d)return 6;}auto after=ds->count();return ok(c,23)&&after==n?0:6;}catch(...){return 4;}}
extern "C" int eo_document(EoContext*c,uint64_t index,EoFacts*out,EoBytes sink,void*u)noexcept{try{int r=check(c);if(r)return r;if(!out||!sink||!c->roster_bound||index>=c->document_count)return 1;*out={};auto d=c->documents[index];bool valid=d&&d->isValid();if(!ok(c,30)||!valid)return 7;out->saved=d->isSaved()?1:0;if(!ok(c,31))return 7;out->modified=d->isModified()?1:0;if(!ok(c,32))return 7;auto active=c->app->activeDocument();if(!ok(c,33))return 7;out->active=active==d?1:0;auto file=d->dataFile();if(!ok(c,34))return 7;out->has_data_file=file?1:0;auto name=d->name();if(!ok(c,35))return 7;auto id=d->creationId();if(!ok(c,36))return 7;return sink(u,0,name.data(),name.size())&&sink(u,1,id.data(),id.size())?0:5;}catch(...){return 4;}}

// Independently retained PUBLIC objects. Enumeration establishes holders once;
// subsequent complete set comparisons cannot substitute index/name/token identity.
static int membership(EoContext*c){int r=check(c);if(r)return r;uint64_t n=0;r=eo_roster(c,&n);if(r)return r;
 if(!c->bound||!c->document||!c->design||!c->root)return 6;
 bool valid=c->document->isValid();if(!ok(c,100)||!valid)return 3;
 auto ps=c->document->products();if(!ok(c,101)||!ps)return 7;
 Ptr<adsk::fusion::Design>d=ps->itemByProductType("DesignProductType");if(!ok(c,102)||d!=c->design)return 6;
 valid=d->isValid();if(!ok(c,103)||!valid)return 3;
 auto parent=d->parentDocument();if(!ok(c,104)||parent!=c->document)return 6;
 auto root=d->rootComponent();if(!ok(c,105)||root!=c->root)return 6;
 auto all=d->allComponents();if(!ok(c,106)||!all)return 7;n=all->count();if(!ok(c,107)||n!=3)return 6;
 unsigned seen=0;for(size_t i=0;i<3;++i){auto comp=all->item(i);if(!ok(c,108)||!comp)return 7;unsigned j=comp==root?0:comp==c->components[0]?1:comp==c->components[1]?2:3;if(j==3||(seen&(1u<<j)))return 6;seen|=1u<<j;
  valid=comp->isValid();if(!ok(c,109)||!valid)return 3;auto design=comp->parentDesign();if(!ok(c,110)||design!=d)return 6;
 }if(seen!=7)return 6;
 auto occurrences=root->occurrences();if(!ok(c,111)||!occurrences)return 7;n=occurrences->count();if(!ok(c,112)||n!=3)return 6;
 seen=0;for(size_t i=0;i<3;++i){auto occ=occurrences->item(i);if(!ok(c,113)||!occ)return 7;size_t j=0;for(;j<3;++j)if(occ==c->occurrences[j])break;if(j==3||(seen&(1u<<j)))return 6;seen|=1u<<j;
  valid=occ->isValid();if(!ok(c,114)||!valid)return 3;auto component=occ->component();if(!ok(c,115)||component!=c->components[j==1?1:0])return 6;
  auto context=occ->assemblyContext();if(!ok(c,116)||context)return 6;
 }return seen==7?0:6;
}
extern "C" int eo_prepare(EoContext*c,uint64_t index,EoInit*out)noexcept{try{int r=check(c);if(r)return r;if(!out||!c->roster_bound||c->prepare_attempted||index>=c->document_count)return 1;c->prepare_attempted=true;*out={};
 c->document=c->documents[index];auto ps=c->document->products();if(!ok(c,40)||!ps)return 7;
 c->design=ps->itemByProductType("DesignProductType");if(!ok(c,41)||!c->design)return 7;
 auto parent=c->design->parentDocument();if(!ok(c,42)||parent!=c->document)return 6;
 c->root=c->design->rootComponent();if(!ok(c,43)||!c->root)return 7;
 auto all=c->design->allComponents();if(!ok(c,44)||!all)return 7;out->components=all->count();if(!ok(c,45)||out->components!=3)return 6;
 auto list=c->root->occurrences();if(!ok(c,46)||!list)return 7;out->occurrences=list->count();if(!ok(c,47)||out->occurrences!=3)return 6;
 for(size_t i=0;i<3;++i){c->occurrences[i]=list->item(i);if(!ok(c,48)||!c->occurrences[i])return 7;for(size_t j=0;j<i;++j)if(c->occurrences[i]==c->occurrences[j])return 6;}
 for(size_t i=0;i<2;++i){c->components[i]=c->occurrences[i]->component();if(!ok(c,49)||!c->components[i]||c->components[i]==c->root)return 6;}
 if(c->components[0]==c->components[1])return 6;
 auto shared=c->occurrences[2]->component();if(!ok(c,50)||shared!=c->components[0])return 6;
 c->bound=true;return membership(c);
 }catch(...){return 4;}}
extern "C" int eo_revisions(EoContext*c,EoBytes sink,void*u)noexcept{try{int r=membership(c);if(r)return r;if(!sink)return 1;
 std::array<Ptr<adsk::fusion::Component>,3> comps{c->root,c->components[0],c->components[1]};
 for(size_t i=0;i<3;++i){auto rev=comps[i]->revisionId();if(!ok(c,200+static_cast<uint32_t>(i))||rev.empty())return 7;if(!sink(u,static_cast<uint32_t>(i),rev.data(),rev.size()))return 5;}
 return membership(c);}catch(...){return 4;}}
#include <Fusion/Fusion/Snapshot.h>
extern "C" int eo_assembly(EoContext*c,EoAssemblyFacts*out,EoBytes sink,void*u)noexcept{if(!out||!sink)return 1;*out={};try{int r=membership(c);if(r)return r;auto design=c->design;auto root=c->root;
#define COUNT(DEST,EXPR,STAGE) {auto collection=(EXPR);if(!ok(c,STAGE)||!collection)return 7;DEST=collection->count();if(!ok(c,STAGE+1))return 7;}
 COUNT(out->root_counts[0],design->allComponents(),300) COUNT(out->root_counts[1],root->occurrences(),302)
 auto timeline=design->timeline();if(!ok(c,304)||!timeline)return 7;
 out->root_counts[2]=timeline->count();if(!ok(c,305))return 7;out->root_counts[3]=timeline->markerPosition();if(!ok(c,306))return 7;
 COUNT(out->root_counts[4],root->bRepBodies(),307) COUNT(out->root_counts[5],root->sketches(),309) COUNT(out->root_counts[6],root->meshBodies(),311)
 COUNT(out->root_counts[7],root->joints(),313) COUNT(out->root_counts[8],root->asBuiltJoints(),315) COUNT(out->root_counts[9],root->rigidGroups(),317)
 COUNT(out->root_counts[10],root->assemblyConstraints(),319) COUNT(out->root_counts[11],root->motionLinks(),321) COUNT(out->root_counts[12],root->jointOrigins(),323)
 COUNT(out->root_counts[13],root->features(),325) COUNT(out->root_counts[14],root->customGraphicsGroups(),327)
 COUNT(out->root_counts[15],design->allParameters(),329) COUNT(out->root_counts[16],design->userParameters(),331) COUNT(out->root_counts[17],root->modelParameters(),333)
 out->flags[0]=design->designType()==adsk::fusion::ParametricDesignType;if(!ok(c,335))return 7;
 out->flags[1]=design->designIntent()==adsk::fusion::HybridDesignIntentType;if(!ok(c,336))return 7;
 bool configured=design->isConfiguredDesign();if(!ok(c,337))return 7;bool instance=design->isConfiguration();if(!ok(c,338))return 7;out->flags[2]=configured||instance;
 auto active=c->app->activeDocument();if(!ok(c,339))return 7;out->flags[3]=active==c->document;
 auto snapshots=design->snapshots();if(!ok(c,340)||!snapshots)return 7;out->flags[4]=snapshots->hasPendingSnapshot();if(!ok(c,341))return 7;
 out->flags[5]=c->app->isOffLine();if(!ok(c,343))return 7;
 auto name=root->name();if(!ok(c,342))return 7;if(!sink(u,0,name.data(),name.size()))return 5;
 for(size_t i=0;i<2;++i){auto comp=c->components[i];auto& a=out->component_counts[i];
  COUNT(a[0],comp->bRepBodies(),350) COUNT(a[1],comp->sketches(),352) COUNT(a[2],comp->occurrences(),354) COUNT(a[3],comp->meshBodies(),356)
  COUNT(a[4],comp->attributes(),358) COUNT(a[5],comp->joints(),360) COUNT(a[6],comp->asBuiltJoints(),362) COUNT(a[7],comp->rigidGroups(),364)
  COUNT(a[8],comp->assemblyConstraints(),366) COUNT(a[9],comp->motionLinks(),368) COUNT(a[10],comp->jointOrigins(),370) COUNT(a[11],comp->features(),372) COUNT(a[12],comp->modelParameters(),374)
  if(a[4]>128)return 6;auto name=comp->name();if(!ok(c,376))return 7;if(!sink(u,static_cast<uint32_t>(i+1),name.data(),name.size()))return 5;
 }
 std::array<Ptr<adsk::fusion::Component>,3> observed{};
 for(size_t i=0;i<3;++i){auto occ=c->occurrences[i];observed[i]=occ->component();if(!ok(c,400)||!observed[i])return 7;
  auto component=observed[i];out->component_slots[i]=component==c->components[0]?0:component==c->components[1]?1:2;
  auto& flags=out->occurrence_flags[i];flags[0]=occ->isGrounded();if(!ok(c,401))return 7;flags[1]=occ->isGroundToParent();if(!ok(c,402))return 7;
  flags[2]=occ->isReferencedComponent();if(!ok(c,403))return 7;flags[3]=occ->isConfiguration();if(!ok(c,404))return 7;
  auto entry=occ->timelineObject();if(!ok(c,405)||!entry)return 7;flags[4]=entry->isSuppressed();if(!ok(c,406))return 7;flags[5]=entry->isRolledBack();if(!ok(c,407))return 7;
  auto ctx=occ->assemblyContext();if(!ok(c,408))return 7;flags[6]=!ctx;
  COUNT(out->children[i],occ->childOccurrences(),409)
  auto matrix=occ->transform2();if(!ok(c,411)||!matrix)return 7;
  for(int row=0;row<4;++row)for(int col=0;col<4;++col){out->matrices[i][row*4+col]=matrix->getCell(row,col);if(!ok(c,412))return 7;}
  auto name=occ->name();if(!ok(c,413))return 7;auto path=occ->fullPathName();if(!ok(c,414))return 7;
  if(!sink(u,static_cast<uint32_t>(3+i*2),name.data(),name.size())||!sink(u,static_cast<uint32_t>(4+i*2),path.data(),path.size()))return 5;
 }
 for(size_t i=0;i<3;++i)for(size_t j=0;j<3;++j)out->same_component[i*3+j]=observed[i]==observed[j];
 out->snapshot_count=snapshots->count();if(!ok(c,480)||out->snapshot_count!=2)return 6;
 for(size_t i=0;i<2;++i){
  auto snap=snapshots->item(i);if(!ok(c,481)||!snap)return 7;
  bool valid=snap->isValid();if(!ok(c,482)||!valid)return 3;
  auto row=snap->timelineObject();if(!ok(c,483)||!row)return 7;
  auto parent=row->parentTimeline();if(!ok(c,484)||parent!=timeline)return 6;
  int index=row->index();if(!ok(c,485)||index<0||static_cast<uint64_t>(index)>=out->root_counts[2])return 6;
  auto found=timeline->item(index);if(!ok(c,486)||found!=row)return 6;
  out->snapshot_indexes[i]=index;
  out->snapshot_health[i]=static_cast<uint32_t>(row->healthState());if(!ok(c,487))return 7;
  out->snapshot_flags[i][0]=row->isSuppressed();if(!ok(c,488))return 7;
  out->snapshot_flags[i][1]=row->isRolledBack();if(!ok(c,489))return 7;
 }
#undef COUNT
 return membership(c);
 }catch(...){return 4;}}
extern "C" int eo_attribute(EoContext*c,uint64_t ci,uint64_t ai,EoBytes sink,void*u)noexcept{try{int r=membership(c);if(r)return r;if(!sink||ci>=2||ai>=128)return 1;
 auto attrs=c->components[ci]->attributes();if(!ok(c,450)||!attrs)return 7;auto n=attrs->count();if(!ok(c,451)||n>128||ai>=n)return 6;
 auto attr=attrs->item(ai);if(!ok(c,452)||!attr)return 7;bool valid=attr->isValid();if(!ok(c,453)||!valid)return 3;
 Ptr<adsk::fusion::Component>parent=attr->parent();if(!ok(c,454)||parent!=c->components[ci])return 6;
 auto group=attr->groupName();if(!ok(c,455))return 7;auto name=attr->name();if(!ok(c,456))return 7;auto value=attr->value();if(!ok(c,457))return 7;
 return sink(u,0,group.data(),group.size())&&sink(u,1,name.data(),name.size())&&sink(u,2,value.data(),value.size())?0:5;
 }catch(...){return 4;}}
extern "C" int eo_capture_end(EoContext*c,const EoAssemblyFacts*f)noexcept{if(!f)return 1;try{int r=membership(c);if(r)return r;for(size_t i=0;i<2;++i){auto attrs=c->components[i]->attributes();if(!ok(c,460)||!attrs)return 7;auto n=attrs->count();if(!ok(c,461)||n!=f->component_counts[i][4])return 6;}return 0;}catch(...){return 4;}}
extern "C" int eo_diagnostic(EoContext*c,EoDiagnostic*out)noexcept{if(!c||c->thread!=GetCurrentThreadId()||!out)return 2;*out=c->diagnostic;return 0;}
extern "C" int eo_release(EoContext*c)noexcept{if(!c||c->thread!=GetCurrentThreadId()||c->release_attempted)return 2;c->release_attempted=true;try{delete c;return 0;}catch(...){return 4;}}
