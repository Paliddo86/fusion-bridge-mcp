// Typed read-only exact line/implicit-point inventory. No constraint setters.
#include <Fusion/Sketch/SketchEntityList.h>
#include <Fusion/Sketch/GeometricConstraintList.h>
#include <Fusion/Sketch/SketchDimensionList.h>
#include <Core/Application/Attributes.h>
#include <Fusion/Components/Components.h>
#include "reference_presence.h"
#include "isolated_line_incidence.h"
static_assert(sizeof(AfLineFlags)==48&&offsetof(AfLineFlags,constraints)==24,"line flags ABI");
static_assert(sizeof(AfLinePoint)==80&&offsetof(AfLinePoint,flags)==24,"line point ABI");
static_assert(sizeof(AfLineState)==352&&offsetof(AfLineState,construction)==344,"line state ABI");
static int ls_flags(AfContext*c,AfSketchRef*s,AfExtrudeDiagnostic*d,const Ptr<adsk::fusion::SketchEntity>&e,AfLineFlags*out,bool relation,bool dimension=false){
 if(!e)return 6;bool valid=e->isValid();if(!ex_ok(c,d,401)||!valid)return 3;
 auto parent=e->parentSketch();if(!ex_ok(c,d,402)||parent!=s->sketch)return 6;
 auto proxy=e->assemblyContext();if(!ex_ok(c,d,403)||proxy)return 6;
 out->fixed=e->isFixed();if(!ex_ok(c,d,404))return 7;
 out->reference=e->isReference();if(!ex_ok(c,d,405))return 7;
 // Autodesk scopes referencedEntity() to isReference=true.
 // Ordinary entities must not call that getter (current SDK raises error2).
 af_reference_presence(out->reference!=0,[&](){return e->referencedEntity();},out->has_reference);
 if(out->reference&&!ex_ok(c,d,406))return 7;
 out->fully_constrained=e->isFullyConstrained();if(!ex_ok(c,d,407))return 7;
 out->is_2d=e->is2D();if(!ex_ok(c,d,408))return 7;
 auto constraints=e->geometricConstraints();if(!ex_ok(c,d,409)||!constraints)return 7;
 out->constraints=constraints->count();if(!ex_ok(c,d,410)||out->constraints>(relation?1:0))return 6;
 if(out->constraints){auto actual=constraints->item(0);if(!ex_ok(c,d,415)||actual!=s->retained_relation)return 6;}
 auto dimensions=e->sketchDimensions();if(!ex_ok(c,d,411)||!dimensions)return 7;
 out->dimensions=dimensions->count();if(!ex_ok(c,d,412)||out->dimensions>(dimension?1:0))return 6;
 if(out->dimensions){auto actual=dimensions->item(0);if(!ex_ok(c,d,416)||actual!=s->retained_dimension)return 6;}
 auto attributes=e->attributes();if(!ex_ok(c,d,413)||!attributes)return 7;
 out->attributes=attributes->count();return ex_ok(c,d,414)?0:7;
}
static int af_line_state_impl(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,AfLineState*out,AfExtrudeDiagnostic*diag,bool relation,bool dimension=false)noexcept{
 if(!out||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
 try{
  // Existing helper verifies exact retained document/root/sketch membership;
  // this finite current authoring prerequisite additionally requires active.
  int r=af_geometry_check(c,d,s);if(r)return r;
  if(s->geometry_count!=1||lease>=s->geometry_count||s->geometry_kinds[lease]!=2)return 6;
  Ptr<adsk::fusion::SketchLine>line=s->geometry_entities[lease];if(!line)return 6;
  auto curves=s->sketch->sketchCurves();if(!ex_ok(c,diag,420)||!curves)return 7;
  auto nc=curves->count();if(!ex_ok(c,diag,421)||nc!=1)return 6;
  auto sole=curves->item(0);if(!ex_ok(c,diag,422)||sole!=line)return 6;
  auto points=s->sketch->sketchPoints();if(!ex_ok(c,diag,423)||!points)return 7;
  auto np=points->count();if(!ex_ok(c,diag,424)||np!=3)return 6;
  auto start=line->startSketchPoint();if(!ex_ok(c,diag,425)||!start)return 7;
  auto end=line->endSketchPoint();if(!ex_ok(c,diag,426)||!end)return 7;
  auto origin=s->sketch->originPoint();if(!ex_ok(c,diag,427)||!origin)return 7;
  if(start==end||start==origin||end==origin)return 6;
  const std::array<Ptr<adsk::fusion::SketchPoint>,3> current{start,end,origin};
  if(!s->line_state_bound){s->line_state_points=current;s->line_state_lease=lease;s->line_state_bound=true;}
  else if(s->line_state_lease!=lease||s->line_state_points!=current)return 6;
  uint32_t seen=0;for(size_t i=0;i<3;++i){auto p=points->item(i);if(!ex_ok(c,diag,428)||!p)return 7;
   size_t j=0;for(;j<3;++j)if(p==current[j])break;if(j==3||(seen&(1u<<j)))return 6;seen|=1u<<j;}
  if(seen!=7)return 6;
  auto texts=s->sketch->sketchTexts();if(!ex_ok(c,diag,450)||!texts)return 7;
  out->texts=texts->count();if(!ex_ok(c,diag,451)||out->texts!=0)return 6;
  r=ls_flags(c,s,diag,line,&out->line,relation,dimension);if(r)return r;
  out->construction=line->isConstruction();if(!ex_ok(c,diag,429))return 7;
  for(size_t i=0;i<3;++i){auto p=current[i];r=ls_flags(c,s,diag,p,&out->points[i].flags,relation&&i!=2,dimension&&i!=2);if(r)return r;
   auto geometry=p->geometry();if(!ex_ok(c,diag,430)||!ex_point(c,diag,431,geometry,out->points[i].position))return 7;
   // Complete sole-line/three-point/zero-text inventory proves curve incidence.
   // connectedEntities() may return null; it is not queried or treated as empty.
   if(!af_isolated_line_incidence(start,end,origin,p,out->points[i].connected))return 6;
  }
#define LS_COUNT(field,expression,stage) {auto collection=(expression);if(!ex_ok(c,diag,stage)||!collection)return 7;out->field=collection->count();if(!ex_ok(c,diag,stage+1))return 7;}
  LS_COUNT(bodies,s->root->bRepBodies(),440);LS_COUNT(features,s->root->features(),442);
  LS_COUNT(occurrences,s->root->occurrences(),444);LS_COUNT(meshes,s->root->meshBodies(),446);
  Ptr<adsk::fusion::Design>design;r=af_design(c,d,design);if(r)return r;
  LS_COUNT(components,design->allComponents(),448);LS_COUNT(attributes,s->sketch->attributes(),452);
#undef LS_COUNT
  // Recheck every acquired object's exact current parent and membership after
  // getters. The next complete Rust-driven capture also compares all values.
  r=af_geometry_check(c,d,s);if(r)return r;
  auto final_curves=s->sketch->sketchCurves();if(!ex_ok(c,diag,460)||!final_curves)return 7;
  auto final_count=final_curves->count();if(!ex_ok(c,diag,461)||final_count!=1)return 6;
  auto final_line=final_curves->item(0);if(!ex_ok(c,diag,462)||final_line!=line)return 6;
  auto final_start=line->startSketchPoint();if(!ex_ok(c,diag,463)||final_start!=start)return 6;
  auto final_end=line->endSketchPoint();if(!ex_ok(c,diag,464)||final_end!=end)return 6;
  auto final_origin=s->sketch->originPoint();if(!ex_ok(c,diag,465)||final_origin!=origin)return 6;
  auto final_points=s->sketch->sketchPoints();if(!ex_ok(c,diag,466)||!final_points)return 7;
  auto final_np=final_points->count();if(!ex_ok(c,diag,467)||final_np!=3)return 6;
  seen=0;for(size_t i=0;i<3;++i){auto p=final_points->item(i);if(!ex_ok(c,diag,468)||!p)return 7;size_t j=0;for(;j<3;++j)if(p==current[j])break;if(j==3||(seen&(1u<<j)))return 6;seen|=1u<<j;}
  auto final_texts=s->sketch->sketchTexts();if(!ex_ok(c,diag,469)||!final_texts)return 7;
  auto final_nt=final_texts->count();if(!ex_ok(c,diag,470)||final_nt!=0)return 6;
  return seen==7?0:6;
 }catch(...){return 4;}
}

extern "C" int af_line_state(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,AfLineState*out,AfExtrudeDiagnostic*diag)noexcept{return af_line_state_impl(c,d,s,lease,out,diag,false);}
