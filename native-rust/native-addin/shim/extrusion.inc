#include <Fusion/Sketch/ProfileLoops.h>
#include <Fusion/Sketch/ProfileLoop.h>
#include <Fusion/Sketch/ProfileCurves.h>
#include <Fusion/Fusion/AreaProperties.h>
#include <Fusion/Fusion/PhysicalProperties.h>
#include <Fusion/Features/ExtrudeFeatures.h>
#include <Fusion/Features/ExtrudeFeatureInput.h>
#include <Fusion/Features/DistanceExtentDefinition.h>
#include <Fusion/Features/ProfilePlaneStartDefinition.h>
#include <Fusion/Fusion/ModelParameter.h>
#include <cstring>
static_assert(sizeof(AfExtrudeDiagnostic)==4112 && offsetof(AfExtrudeDiagnostic,text)==16,"diagnostic ABI");
static_assert(sizeof(AfProfileState)==88 && offsetof(AfProfileState,loops)==40 && offsetof(AfProfileState,on_plane)==72,"profile ABI");
static_assert(sizeof(AfSolidState)==80 && offsetof(AfSolidState,health)==48,"solid ABI");
// Consume exactly the existing immediate SDK error observation; never reset/retry.
static bool ex_ok(AfContext*c,AfExtrudeDiagnostic*d,uint32_t stage){
    d->stage=stage;std::string text;d->sdk_error=c->app->getLastError(&text);
    if(d->sdk_error!=0){d->length=static_cast<uint32_t>(text.size()>sizeof(d->text)?sizeof(d->text):text.size());d->truncated=text.size()>sizeof(d->text)?1:0;if(d->length)std::memcpy(d->text,text.data(),d->length);return false;}return true;
}
static bool ex_point(AfContext*c,AfExtrudeDiagnostic*d,uint32_t stage,const Ptr<adsk::core::Point3D>&p,double*out){
    if(!p)return false;auto xyz=p->asArray();if(!ex_ok(c,d,stage)||xyz.size()!=3)return false;for(size_t i=0;i<3;++i){if(!std::isfinite(xyz[i]))return false;out[i]=xyz[i];}return true;
}
extern "C" int af_profile_read(AfContext*c,AfDocumentRef*d,AfSketchRef*s,AfProfileState*out,AfExtrudeDiagnostic*diag) noexcept {
    if(!out||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        auto profiles=s->sketch->profiles();if(!ex_ok(c,diag,100)||!profiles)return 7;
        auto count=profiles->count();if(!ex_ok(c,diag,101)||count!=1)return 6;
        auto current=profiles->item(0);if(!ex_ok(c,diag,102)||!current)return 7;
        if(!s->retained_profile)s->retained_profile=current;
        else if(s->retained_profile!=current)return 6;
        auto& p=s->retained_profile;
        bool valid_profile=p->isValid();if(!ex_ok(c,diag,103)||!valid_profile)return 7;
        auto parent=p->parentSketch();if(!ex_ok(c,diag,104)||parent!=s->sketch)return 7;
        auto context=p->assemblyContext();if(!ex_ok(c,diag,105)||context)return 6;
        out->on_plane=p->isOnSketchPlane()?1:0;if(!ex_ok(c,diag,106))return 7;
        auto loops=p->profileLoops();if(!ex_ok(c,diag,107)||!loops)return 7;
        out->loops=loops->count();if(!ex_ok(c,diag,108)||out->loops!=1)return 6;
        auto loop=loops->item(0);if(!ex_ok(c,diag,109)||!loop)return 7;
        out->outer_only=loop->isOuter()?1:0;if(!ex_ok(c,diag,110))return 7;
        auto curves=loop->profileCurves();if(!ex_ok(c,diag,111)||!curves)return 7;
        out->curves=curves->count();if(!ex_ok(c,diag,112)||out->curves>256)return 6;
        auto area=p->areaProperties(adsk::fusion::HighCalculationAccuracy);if(!ex_ok(c,diag,113)||!area)return 7;
        out->area=area->area();if(!ex_ok(c,diag,114))return 7;
        out->perimeter=area->perimeter();if(!ex_ok(c,diag,115))return 7;
        auto centroid=area->centroid();if(!ex_ok(c,diag,116)||!ex_point(c,diag,117,centroid,out->centroid))return 7;
        auto accuracy=area->accuracy();if(!ex_ok(c,diag,118))return 7;out->high_accuracy=accuracy==adsk::fusion::HighCalculationAccuracy?1:0;
        auto bodies=s->root->bRepBodies();if(!ex_ok(c,diag,119)||!bodies)return 7;
        out->root_bodies=bodies->count();if(!ex_ok(c,diag,120))return 7;
        auto features=s->root->features();if(!ex_ok(c,diag,121)||!features)return 7;
        auto extrudes=features->extrudeFeatures();if(!ex_ok(c,diag,122)||!extrudes)return 7;
        out->root_extrudes=extrudes->count();if(!ex_ok(c,diag,123))return 7;
        return 0;
    }catch(...){return 4;}
}
extern "C" int af_extrude_create(AfContext*c,AfDocumentRef*d,AfSketchRef*s,double distance,uint32_t negative,uint32_t*invoked,AfExtrudeDiagnostic*diag) noexcept {
    if(!invoked||!diag)return 1;*invoked=0;*diag={};
    try{
        AfProfileState profile{};int result=af_profile_read(c,d,s,&profile,diag);if(result)return result;
        if(s->extrusion_attempted||s->retained_extrude||s->retained_solid||profile.root_bodies!=0||profile.root_extrudes!=0||negative>1||!std::isfinite(distance)||distance<=0.)return 6;
        auto features=s->root->features();if(!ex_ok(c,diag,200)||!features)return 7;
        auto group=features->extrudeFeatures();if(!ex_ok(c,diag,201)||!group)return 7;
        auto input=group->createInput(s->retained_profile,adsk::fusion::NewBodyFeatureOperation);if(!ex_ok(c,diag,202)||!input)return 7;
        bool solid=input->isSolid(true);if(!ex_ok(c,diag,203)||!solid)return 7;
        auto value=adsk::core::ValueInput::createByReal(distance);if(!ex_ok(c,diag,204)||!value)return 7;
        auto extent=adsk::fusion::DistanceExtentDefinition::create(value);if(!ex_ok(c,diag,205)||!extent)return 7;
        auto zero=adsk::core::ValueInput::createByReal(0.);if(!ex_ok(c,diag,206)||!zero)return 7;
        bool defined=input->setOneSideExtent(extent,negative?adsk::fusion::NegativeExtentDirection:adsk::fusion::PositiveExtentDirection,zero);if(!ex_ok(c,diag,207)||!defined)return 7;
        // The live SDK rejected reading/downcasting the default
        // start extent (legacy stage 208). Define the requested input explicitly;
        // never retry/reset the failed getter or infer a successful feature.
        auto start=adsk::fusion::ProfilePlaneStartDefinition::create();if(!ex_ok(c,diag,213)||!start)return 7;
        bool started=input->startExtent(start);if(!ex_ok(c,diag,214)||!started)return 7;
        // One invocation only; captured reference remains owned even if the
        // acknowledgment or subsequent sole-body getter fails.
        s->extrusion_attempted=true;*invoked=1;diag->stage=209;
        s->retained_extrude=group->add(input);if(!ex_ok(c,diag,209)||!s->retained_extrude)return 7;
        auto bodies=s->retained_extrude->bodies();if(!ex_ok(c,diag,210)||!bodies)return 7;
        auto count=bodies->count();if(!ex_ok(c,diag,211)||count!=1)return 7;
        s->retained_solid=bodies->item(0);if(!ex_ok(c,diag,212)||!s->retained_solid)return 7;
        return 0;
    }catch(...){return 4;}
}
// The profile getter advances the parent component revision in the installed
// SDK. Capture its exact relationship once before Rust's post-add root fence.
// The booleans record this call's fact on these immutable retained handles;
// they are not a fresh profile observation or permission to retry.
extern "C" int af_extrude_verify_profile(AfContext*c,AfDocumentRef*d,AfSketchRef*s,AfExtrudeDiagnostic*diag) noexcept {
    if(!diag)return 1;*diag={};diag->sdk_error=-1;diag->stage=1;
    try {
        int result=af_geometry_check(c,d,s);if(result)return result;
        if(!s->extrusion_attempted||s->profile_verification_attempted||!s->retained_profile||!s->retained_extrude||!s->retained_solid)return 6;
        s->profile_verification_attempted=true;
        auto& feature=s->retained_extrude;
        bool valid=feature->isValid();if(!ex_ok(c,diag,300)||!valid)return 7;
        auto owner=feature->parentComponent();if(!ex_ok(c,diag,302)||owner!=s->root)return 7;
        Ptr<adsk::fusion::Profile> profile=feature->profile();if(!ex_ok(c,diag,304)||profile!=s->retained_profile)return 7;
        s->profile_relationship_confirmed=true;return 0;
    }catch(...){return 4;}
}
extern "C" int af_extrude_read(AfContext*c,AfDocumentRef*d,AfSketchRef*s,AfSolidState*out,AfExtrudeDiagnostic*diag) noexcept {
    if(!out||!diag)return 1;*out={};*diag={};diag->sdk_error=-1;diag->stage=1;
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        if(!s->profile_verification_attempted||!s->profile_relationship_confirmed||!s->retained_profile||!s->retained_extrude||!s->retained_solid)return 6;
        auto& feature=s->retained_extrude;auto& body=s->retained_solid;
        bool valid_feature=feature->isValid();if(!ex_ok(c,diag,300)||!valid_feature)return 7;
        bool valid_body=body->isValid();if(!ex_ok(c,diag,301)||!valid_body)return 7;
        auto owner=feature->parentComponent();if(!ex_ok(c,diag,302)||owner!=s->root)return 7;
        auto body_owner=body->parentComponent();if(!ex_ok(c,diag,303)||body_owner!=s->root)return 7;
        auto features=s->root->features();if(!ex_ok(c,diag,305)||!features)return 7;
        auto extrudes=features->extrudeFeatures();if(!ex_ok(c,diag,306)||!extrudes)return 7;
        auto nf=extrudes->count();if(!ex_ok(c,diag,307)||nf!=1)return 7;
        auto current_feature=extrudes->item(0);if(!ex_ok(c,diag,308)||current_feature!=feature)return 7;
        auto bodies=s->root->bRepBodies();if(!ex_ok(c,diag,309)||!bodies)return 7;
        auto nb=bodies->count();if(!ex_ok(c,diag,310)||nb!=1)return 7;
        auto current_body=bodies->item(0);if(!ex_ok(c,diag,311)||current_body!=body)return 7;
        auto feature_bodies=feature->bodies();if(!ex_ok(c,diag,312)||!feature_bodies)return 7;
        auto nfb=feature_bodies->count();if(!ex_ok(c,diag,313)||nfb!=1)return 7;
        auto feature_body=feature_bodies->item(0);if(!ex_ok(c,diag,314)||feature_body!=body)return 7;
        out->health=static_cast<uint32_t>(feature->healthState());if(!ex_ok(c,diag,315))return 7;
        out->body_solid=body->isSolid()?1:0;if(!ex_ok(c,diag,316))return 7;
        out->feature_solid=feature->isSolid()?1:0;if(!ex_ok(c,diag,317))return 7;
        auto operation=feature->operation();if(!ex_ok(c,diag,318))return 7;out->new_body=operation==adsk::fusion::NewBodyFeatureOperation?1:0;
        auto extent_type=feature->extentType();if(!ex_ok(c,diag,319))return 7;out->one_side=extent_type==adsk::fusion::OneSideFeatureExtentType?1:0;
        Ptr<adsk::fusion::ProfilePlaneStartDefinition> start=feature->startExtent();if(!ex_ok(c,diag,320))return 7;out->profile_plane_start=start?1:0;
        Ptr<adsk::fusion::DistanceExtentDefinition> extent=feature->extentOne();if(!ex_ok(c,diag,321)||!extent)return 7;
        auto distance=extent->distance();if(!ex_ok(c,diag,322)||!distance)return 7;
        out->distance=distance->value();if(!ex_ok(c,diag,323))return 7;
        auto taper=feature->taperAngleOne();if(!ex_ok(c,diag,324)||!taper)return 7;
        out->taper=taper->value();if(!ex_ok(c,diag,325))return 7;
        auto properties=body->getPhysicalProperties(adsk::fusion::HighCalculationAccuracy);if(!ex_ok(c,diag,326)||!properties)return 7;
        out->volume=properties->volume();if(!ex_ok(c,diag,327))return 7;
        auto accuracy=properties->accuracy();if(!ex_ok(c,diag,328))return 7;out->high_accuracy=accuracy==adsk::fusion::HighCalculationAccuracy?1:0;
        auto center=properties->centerOfMass();if(!ex_ok(c,diag,329)||!center)return 7;
        auto sketch_center=s->sketch->modelToSketchSpace(center);if(!ex_ok(c,diag,330)||!ex_point(c,diag,331,sketch_center,out->centroid_sketch))return 7;
        return 0;
    }catch(...){return 4;}
}
