// Thin typed owner-thread assembly calls. Rust owns policy, JSON and effects.
#include <Fusion/Components/Occurrences.h>
#include <Fusion/Components/Components.h>
#include <Fusion/Fusion/Snapshots.h>
#include <Fusion/Fusion/Snapshot.h>
#include <Fusion/Components/Joints.h>
#include <Fusion/Components/AsBuiltJoints.h>
#include <Fusion/Components/RigidGroups.h>
#include <Fusion/Fusion/TimelineObject.h>
#include <Fusion/Components/AssemblyConstraints.h>
#include <Fusion/Components/MotionLinks.h>
#include <Fusion/Components/JointOrigins.h>
#include <array>
struct AfAssembly {AfContext*context;Ptr<adsk::core::Document>document;Ptr<adsk::fusion::Design>design;Ptr<adsk::fusion::Component>root;std::array<Ptr<adsk::fusion::Component>,32>components{};std::array<Ptr<adsk::fusion::Occurrence>,64>occurrences{};std::array<Ptr<adsk::fusion::Snapshot>,64>snapshots{};size_t nc=0,no=0,ns=0;bool release_attempted=false,positions_opened=false;};
static_assert(sizeof(AfAssemblyRoot)==128&&sizeof(AfAssemblyComponent)==80&&sizeof(AfAssemblyOccurrence)==176,"assembly ABI");
static_assert(offsetof(AfAssemblyRoot,flags)==104&&offsetof(AfAssemblyOccurrence,matrix)==16&&offsetof(AfAssemblyOccurrence,flags)==144,"assembly ABI offsets");
static int assembly_check(AfContext*c,AfDocumentRef*d,AfAssembly*v,AfExtrudeDiagnostic*diag){
 if(!v||v->context!=c||v->release_attempted)return 1;Ptr<adsk::fusion::Design>design;int r=af_design(c,d,design);if(r)return r;if(design!=v->design||d->document!=v->document)return 6;
 auto root=design->rootComponent();if(!ex_ok(c,diag,700)||root!=v->root)return 6;
 auto all=design->allComponents();if(!ex_ok(c,diag,701)||!all)return 7;auto n=all->count();if(!ex_ok(c,diag,702)||n!=v->nc+1)return 6;
 std::array<bool,33>seen{};for(size_t i=0;i<n;++i){auto comp=all->item(i);if(!ex_ok(c,diag,703)||!comp)return 7;size_t slot=33;if(comp==root)slot=32;else for(size_t j=0;j<v->nc;++j)if(comp==v->components[j]){if(slot!=33)return 6;slot=j;}if(slot==33||seen[slot])return 6;seen[slot]=true;}
 auto occurrences=root->occurrences();if(!ex_ok(c,diag,704)||!occurrences)return 7;n=occurrences->count();if(!ex_ok(c,diag,705)||n!=v->no)return 6;
 std::array<bool,64>oseen{};for(size_t i=0;i<n;++i){auto occ=occurrences->item(i);if(!ex_ok(c,diag,706)||!occ)return 7;size_t slot=64;for(size_t j=0;j<v->no;++j)if(occ==v->occurrences[j]){if(slot!=64)return 6;slot=j;}if(slot==64||oseen[slot])return 6;oseen[slot]=true;}
 return 0;
}
static int assembly_component(AfContext*c,AfDocumentRef*d,AfAssembly*v,uint64_t index,Ptr<adsk::fusion::Component>&comp,AfExtrudeDiagnostic*diag){int r=assembly_check(c,d,v,diag);if(r)return r;if(index>=v->nc)return 1;comp=v->components[index];if(!comp)return 6;bool valid=comp->isValid();if(!ex_ok(c,diag,707)||!valid)return 3;auto owner=comp->parentDesign();return ex_ok(c,diag,708)&&owner==v->design?0:6;}
extern "C" int af_assembly_open(AfContext*c,AfDocumentRef*d,AfAssembly**out,AfExtrudeDiagnostic*diag)noexcept{if(!out||!diag)return 1;*out=nullptr;*diag={};diag->sdk_error=-1;try{Ptr<adsk::fusion::Design>design;int r=af_design(c,d,design);if(r)return r;*out=new AfAssembly{};auto v=*out;v->context=c;v->document=d->document;v->design=design;v->root=design->rootComponent();if(!ex_ok(c,diag,709)||!v->root)return 7;return assembly_check(c,d,v,diag);}catch(...){return 4;}}
extern "C" int af_assembly_close(AfContext*c,AfAssembly*v)noexcept{if(!v||v->context!=c||v->release_attempted)return 1;int r=check(c);if(r)return r;v->release_attempted=true;try{delete v;return 0;}catch(...){return 4;}}
extern "C" int af_assembly_root(AfContext*c,AfDocumentRef*d,AfAssembly*v,AfAssemblyRoot*out,AfBytes sink,void*u,AfExtrudeDiagnostic*diag)noexcept{if(!out||!sink||!diag)return 1;*out={};try{int r=assembly_check(c,d,v,diag);if(r)return r;auto root=v->root;auto design=v->design;
#define ASCOUNT(i,expr,stage) {auto list=(expr);if(!ex_ok(c,diag,stage)||!list)return 7;out->counts[i]=list->count();if(!ex_ok(c,diag,stage+1))return 7;}
 auto active=c->app->activeDocument();if(!ex_ok(c,diag,710))return 7;out->flags[0]=active==v->document;
 out->flags[1]=design->designType()==adsk::fusion::ParametricDesignType;if(!ex_ok(c,diag,711))return 7;
 out->flags[2]=design->designIntent()==adsk::fusion::HybridDesignIntentType;if(!ex_ok(c,diag,712))return 7;
 auto configured=design->isConfiguredDesign();if(!ex_ok(c,diag,713))return 7;auto configuration=design->isConfiguration();if(!ex_ok(c,diag,714))return 7;out->flags[3]=configured||configuration;
 out->flags[4]=c->app->isOffLine();if(!ex_ok(c,diag,715))return 7;
 auto snapshots=design->snapshots();if(!ex_ok(c,diag,716)||!snapshots)return 7;out->flags[5]=snapshots->hasPendingSnapshot();if(!ex_ok(c,diag,717))return 7;
 ASCOUNT(0,design->allComponents(),718) ASCOUNT(1,root->occurrences(),720)
 auto timeline=design->timeline();if(!ex_ok(c,diag,722)||!timeline)return 7;out->counts[2]=timeline->count();if(!ex_ok(c,diag,723))return 7;out->counts[3]=timeline->markerPosition();if(!ex_ok(c,diag,724))return 7;
 ASCOUNT(4,root->bRepBodies(),725) ASCOUNT(5,root->sketches(),727) ASCOUNT(6,root->meshBodies(),729) ASCOUNT(7,root->joints(),731) ASCOUNT(8,root->asBuiltJoints(),733) ASCOUNT(9,root->rigidGroups(),735) ASCOUNT(10,root->assemblyConstraints(),800) ASCOUNT(11,root->motionLinks(),802) ASCOUNT(12,root->jointOrigins(),804)
 auto revision=root->revisionId();if(!ex_ok(c,diag,737))return 7;auto name=root->name();if(!ex_ok(c,diag,738))return 7;
 return sink(u,0,revision.data(),revision.size())&&sink(u,1,name.data(),name.size())?0:5;
#undef ASCOUNT
 }catch(...){return 4;}}
extern "C" int af_assembly_component(AfContext*c,AfDocumentRef*d,AfAssembly*v,uint64_t index,uint32_t cloud,AfAssemblyComponent*out,AfBytes sink,void*u,AfExtrudeDiagnostic*diag)noexcept{if(!out||!sink||!diag||cloud>1)return 1;*out={};try{Ptr<adsk::fusion::Component>comp;int r=assembly_component(c,d,v,index,comp,diag);if(r)return r;
#define ACTEXT(i,expr,stage) {auto value=(expr);if(!ex_ok(c,diag,stage))return 7;if(!sink(u,i,value.data(),value.size()))return 5;}
 ACTEXT(0,comp->name(),740) ACTEXT(1,comp->revisionId(),741)
 if(cloud){bool offline=c->app->isOffLine();if(!ex_ok(c,diag,742))return 7;if(offline)return 8;ACTEXT(2,comp->partNumber(),743) ACTEXT(3,comp->description(),744)}
#define ACCOUNT(i,expr,stage) {auto list=(expr);if(!ex_ok(c,diag,stage)||!list)return 7;out->counts[i]=list->count();if(!ex_ok(c,diag,stage+1))return 7;}
 ACCOUNT(0,comp->bRepBodies(),745) ACCOUNT(1,comp->sketches(),747) ACCOUNT(2,comp->occurrences(),749) ACCOUNT(3,comp->meshBodies(),751) ACCOUNT(4,comp->attributes(),753) ACCOUNT(5,comp->joints(),806) ACCOUNT(6,comp->asBuiltJoints(),808) ACCOUNT(7,comp->rigidGroups(),810) ACCOUNT(8,comp->assemblyConstraints(),812) ACCOUNT(9,comp->motionLinks(),814)
#undef ACCOUNT
#undef ACTEXT
 return 0;}catch(...){return 4;}}
extern "C" int af_assembly_attribute(AfContext*c,AfDocumentRef*d,AfAssembly*v,uint64_t ci,uint64_t ai,AfBytes sink,void*u,AfExtrudeDiagnostic*diag)noexcept{if(!sink||!diag||ai>=128)return 1;try{Ptr<adsk::fusion::Component>comp;int r=assembly_component(c,d,v,ci,comp,diag);if(r)return r;auto attrs=comp->attributes();if(!ex_ok(c,diag,755)||!attrs)return 7;auto a=attrs->item(ai);if(!ex_ok(c,diag,756)||!a)return 7;auto group=a->groupName();if(!ex_ok(c,diag,757))return 7;auto name=a->name();if(!ex_ok(c,diag,758))return 7;auto value=a->value();if(!ex_ok(c,diag,759))return 7;return sink(u,0,group.data(),group.size())&&sink(u,1,name.data(),name.size())&&sink(u,2,value.data(),value.size())?0:5;}catch(...){return 4;}}
extern "C" int af_assembly_occurrence(AfContext*c,AfDocumentRef*d,AfAssembly*v,uint64_t index,AfAssemblyOccurrence*out,AfBytes sink,void*u,AfExtrudeDiagnostic*diag)noexcept{if(!out||!sink||!diag)return 1;*out={};try{int r=assembly_check(c,d,v,diag);if(r)return r;if(index>=v->no)return 1;auto occ=v->occurrences[index];if(!occ)return 6;bool valid=occ->isValid();if(!ex_ok(c,diag,760)||!valid)return 3;auto comp=occ->component();if(!ex_ok(c,diag,761)||!comp)return 7;size_t ci=0;for(;ci<v->nc;++ci)if(v->components[ci]==comp)break;if(ci==v->nc)return 6;out->component=ci;
 auto ctx=occ->assemblyContext();if(!ex_ok(c,diag,762))return 7;out->flags[6]=!ctx;
 out->flags[0]=occ->isGrounded();if(!ex_ok(c,diag,763))return 7;out->flags[1]=occ->isGroundToParent();if(!ex_ok(c,diag,764))return 7;out->flags[2]=occ->isReferencedComponent();if(!ex_ok(c,diag,765))return 7;out->flags[3]=occ->isConfiguration();if(!ex_ok(c,diag,766))return 7;
 auto timeline=occ->timelineObject();if(!ex_ok(c,diag,767)||!timeline)return 7;out->flags[4]=timeline->isSuppressed();if(!ex_ok(c,diag,768))return 7;out->flags[5]=timeline->isRolledBack();if(!ex_ok(c,diag,769))return 7;
 auto children=occ->childOccurrences();if(!ex_ok(c,diag,770)||!children)return 7;out->children=children->count();if(!ex_ok(c,diag,771))return 7;
 auto matrix=occ->transform2();if(!ex_ok(c,diag,772)||!matrix)return 7;for(int row=0;row<4;++row)for(int col=0;col<4;++col){out->matrix[row*4+col]=matrix->getCell(row,col);if(!ex_ok(c,diag,773))return 7;}
 auto name=occ->name();if(!ex_ok(c,diag,774))return 7;auto path=occ->fullPathName();if(!ex_ok(c,diag,775))return 7;return sink(u,0,name.data(),name.size())&&sink(u,1,path.data(),path.size())?0:5;
 }catch(...){return 4;}}
extern "C" int af_assembly_add(AfContext*c,AfDocumentRef*d,AfAssembly*v,int64_t existing,uint32_t*invoked,uint64_t*ci,uint64_t*oi,AfExtrudeDiagnostic*diag)noexcept{if(!invoked||!ci||!oi||!diag)return 1;*invoked=0;*ci=*oi=UINT64_MAX;try{int r=assembly_check(c,d,v,diag);if(r)return r;if(existing< -1||existing>=static_cast<int64_t>(v->nc)||v->no>=64||(existing<0&&v->nc>=32))return 5;auto occurrences=v->root->occurrences();if(!ex_ok(c,diag,780)||!occurrences)return 7;auto identity=adsk::core::Matrix3D::create();if(!ex_ok(c,diag,781)||!identity)return 7;
 size_t occurrence=v->no++;size_t component=existing<0?v->nc++:static_cast<size_t>(existing);
 *invoked=1;v->occurrences[occurrence]=existing<0?occurrences->addNewComponent(identity):occurrences->addExistingComponent(v->components[component],identity);
 if(!ex_ok(c,diag,782)||!v->occurrences[occurrence])return 7;*oi=occurrence;
 auto returned=v->occurrences[occurrence]->component();if(existing<0)v->components[component]=returned;if(!ex_ok(c,diag,783)||!returned||returned!=v->components[component])return 6;*ci=component;return assembly_check(c,d,v,diag);
 }catch(...){return 4;}}
extern "C" int af_assembly_change(AfContext*c,AfDocumentRef*d,AfAssembly*v,uint64_t ci,uint32_t kind,const char*key,size_t keylen,const char*value,size_t valuelen,uint32_t*invoked,AfExtrudeDiagnostic*diag)noexcept{if(!invoked||!diag||!key||!value||keylen>64||valuelen>16384||kind>1)return 1;*invoked=0;try{Ptr<adsk::fusion::Component>comp;int r=assembly_component(c,d,v,ci,comp,diag);if(r)return r;std::string k(key,keylen),s(value,valuelen);if(kind==1){if(k!="name"&&k!="partNumber"&&k!="description")return 1;if(k!="name"){bool offline=c->app->isOffLine();if(!ex_ok(c,diag,790))return 7;if(offline)return 8;}*invoked=1;bool accepted=k=="name"?comp->name(s):k=="partNumber"?comp->partNumber(s):comp->description(s);return ex_ok(c,diag,791)&&accepted?0:7;}
 auto attrs=comp->attributes();if(!ex_ok(c,diag,792)||!attrs)return 7;*invoked=1;auto a=attrs->add("AdomMechanical",k,s);return ex_ok(c,diag,793)&&a?0:7;
 }catch(...){return 4;}}
