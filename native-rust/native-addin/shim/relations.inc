// Typed ABI only: exact returned relationship and once-only add, no retry/delete.
#include <Fusion/Sketch/HorizontalConstraint.h>
#include <Fusion/Sketch/VerticalConstraint.h>
#include "relation_identity.h"
static_assert(sizeof(AfRelationState)==360&&offsetof(AfRelationState,kind)==352,"relation ABI");
static int relation_member(AfContext*c,AfSketchRef*s,AfExtrudeDiagnostic*diag){
 auto all=s->sketch->geometricConstraints();if(!ex_ok(c,diag,500)||!all)return 7;
 auto n=all->count();if(!ex_ok(c,diag,501)||n!=(s->retained_relation?1:0))return 6;
 if(!s->retained_relation)return s->relation_attempted?6:0;
 if(!s->relation_attempted||s->relation_kind<1||s->relation_kind>2)return 6;
 auto only=all->item(0);if(!ex_ok(c,diag,502)||only!=s->retained_relation)return 6;
 bool valid=only->isValid();if(!ex_ok(c,diag,503)||!valid)return 3;
 auto parent=only->parentSketch();if(!ex_ok(c,diag,504)||parent!=s->sketch)return 6;
 auto context=only->assemblyContext();if(!ex_ok(c,diag,505)||context)return 6;
 Ptr<adsk::fusion::SketchLine>line;
 if(s->relation_kind==1){Ptr<adsk::fusion::HorizontalConstraint>typed=only;int r=af_exact_relation_line(typed,s->retained_horizontal,line);if(r)return r;}
 else {Ptr<adsk::fusion::VerticalConstraint>typed=only;int r=af_exact_relation_line(typed,s->retained_vertical,line);if(r)return r;}
 if(!ex_ok(c,diag,506)||s->geometry_count!=1||line!=s->geometry_entities[0])return 6;
 return 0;
}
extern "C" int af_relation_state(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,AfRelationState*out,AfExtrudeDiagnostic*diag)noexcept{
 if(!out||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
 try{int r=af_geometry_check(c,d,s);if(r)return r;
  r=relation_member(c,s,diag);if(r)return r;
  r=af_line_state_impl(c,d,s,lease,&out->line,diag,s->retained_relation!=nullptr);if(r)return r;
  r=relation_member(c,s,diag);if(r)return r;out->kind=s->retained_relation?s->relation_kind:0;return 0;
 }catch(...){return 4;}
}
extern "C" int af_relation_add(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,uint32_t kind,uint32_t*invoked,AfExtrudeDiagnostic*diag)noexcept{
 if(!invoked||!diag)return 1;*invoked=0;*diag={};diag->sdk_error=-1;diag->stage=1;
 try{int r=af_geometry_check(c,d,s);if(r)return r;
  if(kind<1||kind>2||s->relation_attempted||s->retained_relation||!s->line_state_bound||s->line_state_lease!=lease)return 6;
  // Fixed holder exists already; complete exact endpoint membership is refreshed
  // before entering the setter. No allocation follows the returned SDK object.
  AfLineState before{};r=af_line_state_impl(c,d,s,lease,&before,diag,false);if(r)return r;
  if(before.line.fixed||before.line.fully_constrained||before.points[0].flags.fixed||before.points[1].flags.fixed
     ||before.points[0].flags.fully_constrained||before.points[1].flags.fully_constrained)return 6;
  r=relation_member(c,s,diag);if(r)return r;
  Ptr<adsk::fusion::SketchLine>line=s->geometry_entities[lease];if(!line)return 6;
  auto constraints=s->sketch->geometricConstraints();if(!ex_ok(c,diag,520)||!constraints)return 7;
  auto count=constraints->count();if(!ex_ok(c,diag,521)||count!=0)return 6;
  s->relation_attempted=true;s->relation_kind=kind;*invoked=1;
  if(kind==1){s->retained_horizontal=constraints->addHorizontal(line);s->retained_relation=s->retained_horizontal;}
  else{s->retained_vertical=constraints->addVertical(line);s->retained_relation=s->retained_vertical;}
  if(!ex_ok(c,diag,522)||!s->retained_relation)return 7;
  return 0;
 }catch(...){return 4;}
}
