// Typed absolute pose and captured-position calls; all policy is in Rust.
#include <cmath>
static_assert(sizeof(AfAssemblyPositions)==800 && offsetof(AfAssemblyPositions,indexes)==32 && offsetof(AfAssemblyPositions,health)==544,"placement ABI");
static int placement_members(AfContext*c,AfAssembly*v,AfExtrudeDiagnostic*d){
 auto snapshots=v->design->snapshots();if(!ex_ok(c,d,850)||!snapshots)return 7;
 auto count=snapshots->count();if(!ex_ok(c,d,851)||count>64)return 5;
 if(!v->positions_opened){v->positions_opened=true;v->ns=count;for(size_t i=0;i<count;++i){v->snapshots[i]=snapshots->item(i);if(!ex_ok(c,d,852)||!v->snapshots[i])return 7;}}
 if(count!=v->ns)return 6;
 std::array<bool,64>seen{};
 for(size_t i=0;i<count;++i){auto item=snapshots->item(i);if(!ex_ok(c,d,853)||!item)return 7;size_t slot=64;for(size_t j=0;j<v->ns;++j)if(v->snapshots[j]==item){if(slot!=64)return 6;slot=j;}if(slot==64||seen[slot])return 6;seen[slot]=true;bool valid=item->isValid();if(!ex_ok(c,d,854)||!valid)return 3;}
 return 0;
}
extern "C" int af_assembly_positions(AfContext*c,AfDocumentRef*doc,AfAssembly*v,AfAssemblyPositions*out,AfExtrudeDiagnostic*d)noexcept{
 if(!out||!d)return 1;*out={};try{
 int r=assembly_check(c,doc,v,d);if(r)return r;r=placement_members(c,v,d);if(r)return r;
 auto timeline=v->design->timeline();if(!ex_ok(c,d,855)||!timeline)return 7;
 out->timeline=timeline->count();if(!ex_ok(c,d,856))return 7;out->timeline_position=timeline->markerPosition();if(!ex_ok(c,d,857))return 7;
 auto snapshots=v->design->snapshots();if(!ex_ok(c,d,858)||!snapshots)return 7;
 out->pending=snapshots->hasPendingSnapshot();if(!ex_ok(c,d,859))return 7;out->count=v->ns;
 for(size_t i=0;i<v->ns;++i){auto t=v->snapshots[i]->timelineObject();if(!ex_ok(c,d,860)||!t)return 7;auto parent=t->parentTimeline();if(!ex_ok(c,d,861)||parent!=timeline)return 6;int index=t->index();if(!ex_ok(c,d,862)||index<0||static_cast<uint64_t>(index)>=out->timeline)return 6;auto at=timeline->item(index);if(!ex_ok(c,d,863)||at!=t)return 6;bool suppressed=t->isSuppressed();if(!ex_ok(c,d,864)||suppressed)return 6;bool rolled=t->isRolledBack();if(!ex_ok(c,d,865)||rolled)return 6;out->indexes[i]=index;out->health[i]=static_cast<uint32_t>(t->healthState());if(!ex_ok(c,d,866))return 7;}
 r=assembly_check(c,doc,v,d);return r?r:placement_members(c,v,d);
 }catch(...){return 4;}
}
extern "C" int af_assembly_translate(AfContext*c,AfDocumentRef*doc,AfAssembly*v,uint64_t oi,const double*m,uint32_t*invoked,uint64_t*component,AfExtrudeDiagnostic*d)noexcept{
 if(!m||!invoked||!component||!d)return 1;*invoked=0;*component=UINT64_MAX;try{
 int r=assembly_check(c,doc,v,d);if(r)return r;r=placement_members(c,v,d);if(r)return r;if(oi>=v->no)return 1;
 auto active=c->app->activeDocument();if(!ex_ok(c,d,867)||active!=v->document)return 6;
 auto snapshots=v->design->snapshots();if(!ex_ok(c,d,868)||!snapshots)return 7;bool pending=snapshots->hasPendingSnapshot();if(!ex_ok(c,d,869)||pending)return 6;
 auto occurrence=v->occurrences[oi];bool valid=occurrence->isValid();if(!ex_ok(c,d,870)||!valid)return 3;bool grounded=occurrence->isGrounded();if(!ex_ok(c,d,871)||grounded)return 6;bool parent=occurrence->isGroundToParent();if(!ex_ok(c,d,872)||parent)return 6;
 auto comp=occurrence->component();if(!ex_ok(c,d,881)||!comp)return 7;size_t ci=0;for(;ci<v->nc;++ci)if(v->components[ci]==comp)break;if(ci==v->nc)return 6;*component=ci;
 auto matrix=adsk::core::Matrix3D::create();if(!ex_ok(c,d,873)||!matrix)return 7;
 for(int row=0;row<4;++row)for(int col=0;col<4;++col){const int i=row*4+col;bool translation=col==3&&row<3;double expected=row==col?1.:0.;if(!std::isfinite(m[i])||(translation?std::abs(m[i])>100000.:m[i]!=expected))return 5;bool set=matrix->setCell(row,col,m[i]);if(!ex_ok(c,d,874)||!set)return 7;double actual=matrix->getCell(row,col);if(!ex_ok(c,d,875)||actual!=m[i])return 6;}
 *invoked=1;bool accepted=occurrence->transform2(matrix);return ex_ok(c,d,876)&&accepted?0:7;
 }catch(...){return 4;}
}
extern "C" int af_assembly_capture_position(AfContext*c,AfDocumentRef*doc,AfAssembly*v,uint32_t*invoked,uint64_t*slot,AfExtrudeDiagnostic*d)noexcept{
 if(!invoked||!slot||!d)return 1;*invoked=0;*slot=UINT64_MAX;try{
 int r=assembly_check(c,doc,v,d);if(r)return r;r=placement_members(c,v,d);if(r)return r;if(v->ns>=64)return 5;
 auto active=c->app->activeDocument();if(!ex_ok(c,d,877)||active!=v->document)return 6;
 auto snapshots=v->design->snapshots();if(!ex_ok(c,d,878)||!snapshots)return 7;bool pending=snapshots->hasPendingSnapshot();if(!ex_ok(c,d,879)||!pending)return 6;
 size_t i=v->ns++;*invoked=1;v->snapshots[i]=snapshots->add();
 if(!ex_ok(c,d,880)||!v->snapshots[i])return 7;*slot=i;
 return placement_members(c,v,d);
 }catch(...){return 4;}
}
