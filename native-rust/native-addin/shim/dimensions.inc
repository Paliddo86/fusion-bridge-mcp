// Typed exact-child ABI. Rust controls arguments, stages, units and result policy.
#include "dimension_creator.h"
static_assert(sizeof(AfDimensionState)==432&&offsetof(AfDimensionState,model_parameters)==360&&offsetof(AfDimensionState,value)==392,"dimension ABI");
static int dimension_member(AfContext*c,AfDocumentRef*d,AfSketchRef*s,AfExtrudeDiagnostic*diag){
 int r=relation_member(c,s,diag);if(r)return r;if(!s->retained_relation)return 6;
 auto dims=s->sketch->sketchDimensions();if(!ex_ok(c,diag,600)||!dims)return 7;
 auto count=dims->count();if(!ex_ok(c,diag,601)||count!=(s->retained_dimension?1:0))return 6;
 auto models=s->root->modelParameters();if(!ex_ok(c,diag,602)||!models)return 7;
 auto nm=models->count();if(!ex_ok(c,diag,603)||nm!=(s->retained_dimension?1:0))return 6;
 Ptr<adsk::fusion::Design>design;r=af_design(c,d,design);if(r)return r;
 auto users=design->userParameters();if(!ex_ok(c,diag,604)||!users)return 7;
 auto nu=users->count();if(!ex_ok(c,diag,605)||nu!=0)return 6;
 auto all=design->allParameters();if(!ex_ok(c,diag,606)||!all)return 7;
 auto na=all->count();if(!ex_ok(c,diag,607)||na!=nm)return 6;
 if(!s->retained_dimension)return s->dimension_add_attempted||s->retained_dimension_parameter?6:0;
 if(!s->dimension_add_attempted||!s->retained_dimension_parameter)return 6;
 Ptr<adsk::fusion::SketchLinearDimension>dim=dims->item(0);if(!ex_ok(c,diag,608)||dim!=s->retained_dimension)return 6;
 bool valid=dim->isValid();if(!ex_ok(c,diag,609)||!valid)return 3;
 auto parent=dim->parentSketch();if(!ex_ok(c,diag,610)||parent!=s->sketch)return 6;
 auto context=dim->assemblyContext();if(!ex_ok(c,diag,611)||context)return 6;
 Ptr<adsk::fusion::SketchPoint>one=dim->entityOne();if(!ex_ok(c,diag,612)||one!=s->line_state_points[0])return 6;
 Ptr<adsk::fusion::SketchPoint>two=dim->entityTwo();if(!ex_ok(c,diag,613)||two!=s->line_state_points[1])return 6;
 auto orientation=dim->orientation();if(!ex_ok(c,diag,614)||orientation!=adsk::fusion::AlignedDimensionOrientation)return 6;
 bool driving=dim->isDriving();if(!ex_ok(c,diag,615)||!driving)return 6;
 auto param=dim->parameter();if(!ex_ok(c,diag,616)||param!=s->retained_dimension_parameter)return 6;
 valid=param->isValid();if(!ex_ok(c,diag,617)||!valid)return 3;
 auto component=param->component();if(!ex_ok(c,diag,618)||component!=s->root)return 6;
 Ptr<adsk::fusion::Sketch>creator=param->createdBy();if(!ex_ok(c,diag,619))return 6;
 r=af_exact_dimension_creator(creator,s->sketch,[&](const auto&owner)->int{
  bool creator_valid=owner->isValid();if(!ex_ok(c,diag,622))return 7;if(!creator_valid)return 3;
  auto creator_context=owner->assemblyContext();if(!ex_ok(c,diag,623))return 7;return creator_context?6:0;
 });if(r)return r;
 auto only=models->item(0);if(!ex_ok(c,diag,620)||only!=param)return 6;
 auto any=all->item(0);if(!ex_ok(c,diag,621)||any!=param)return 6;
 return 0;
}
extern "C" int af_dimension_state(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,AfDimensionState*out,AfBytes sink,void*user,AfExtrudeDiagnostic*diag)noexcept{
 if(!out||!sink||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
 try{int r=af_geometry_check(c,d,s);if(r)return r;r=dimension_member(c,d,s,diag);if(r)return r;
  r=af_line_state_impl(c,d,s,lease,&out->relation.line,diag,true,s->retained_dimension!=nullptr);if(r)return r;
  out->relation.kind=s->relation_kind;out->present=s->retained_dimension?1:0;out->model_parameters=out->present;
  if(out->present){auto dim=s->retained_dimension;auto param=s->retained_dimension_parameter;
   out->driving=dim->isDriving();if(!ex_ok(c,diag,630))return 7;
   out->aligned=dim->orientation()==adsk::fusion::AlignedDimensionOrientation;if(!ex_ok(c,diag,631))return 7;
   out->value=dim->value();if(!ex_ok(c,diag,632))return 7;
   out->parameter_value=param->value();if(!ex_ok(c,diag,633))return 7;
   auto text=dim->textPosition();if(!ex_ok(c,diag,634)||!ex_point(c,diag,635,text,out->text))return 7;
   auto name=param->name();if(!ex_ok(c,diag,639))return 7;
   auto expression=param->expression();if(!ex_ok(c,diag,640))return 7;
   auto unit=param->unit();if(!ex_ok(c,diag,641))return 7;
   if(!sink(user,0,name.data(),name.size())||!sink(user,1,expression.data(),expression.size())||!sink(user,2,unit.data(),unit.size()))return 5;
  }
  r=af_geometry_check(c,d,s);if(r)return r;return dimension_member(c,d,s,diag);
 }catch(...){return 4;}
}
extern "C" int af_dimension_add(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,const double*position,uint32_t*invoked,AfExtrudeDiagnostic*diag)noexcept{
 if(!invoked||!diag||!position)return 1;*invoked=0;*diag={};diag->sdk_error=-1;diag->stage=1;
 try{int r=af_geometry_check(c,d,s);if(r)return r;
  if(s->dimension_add_attempted||s->retained_dimension||!s->line_state_bound||s->line_state_lease!=lease)return 6;
  r=dimension_member(c,d,s,diag);if(r)return r;
  AfLineState before{};r=af_line_state_impl(c,d,s,lease,&before,diag,true);if(r)return r;
  if(before.line.fixed||before.line.fully_constrained||before.points[0].flags.fixed||before.points[1].flags.fixed||before.points[0].flags.fully_constrained||before.points[1].flags.fully_constrained)return 6;
  for(size_t i=0;i<3;++i)if(!std::isfinite(position[i])||std::abs(position[i])>10000)return 1;if(position[2]!=0)return 1;
  auto text=adsk::core::Point3D::create(position[0],position[1],position[2]);if(!ex_ok(c,diag,650)||!text)return 7;
  auto dims=s->sketch->sketchDimensions();if(!ex_ok(c,diag,651)||!dims)return 7;
  s->dimension_add_attempted=true;*invoked=1;
  s->retained_dimension=dims->addDistanceDimension(s->line_state_points[0],s->line_state_points[1],adsk::fusion::AlignedDimensionOrientation,text,true);
  if(!ex_ok(c,diag,652)||!s->retained_dimension)return 7;
  // Retain returned parameter before any subsequent getter/setter can fail.
  s->retained_dimension_parameter=s->retained_dimension->parameter();
  if(!ex_ok(c,diag,653)||!s->retained_dimension_parameter)return 7;return 0;
 }catch(...){return 4;}
}
extern "C" int af_dimension_set(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,double value,uint32_t*invoked,AfExtrudeDiagnostic*diag)noexcept{
 if(!invoked||!diag)return 1;*invoked=0;*diag={};diag->sdk_error=-1;diag->stage=1;
 try{int r=af_geometry_check(c,d,s);if(r)return r;
  if(!s->dimension_add_attempted||s->dimension_value_attempted||!s->line_state_bound||s->line_state_lease!=lease||!std::isfinite(value)||value<0.0001||value>1000)return 6;
  r=dimension_member(c,d,s,diag);if(r)return r;
  s->dimension_value_attempted=true;*invoked=1;
  bool accepted=s->retained_dimension->value(value);if(!ex_ok(c,diag,660)||!accepted)return 7;return 0;
 }catch(...){return 4;}
}
