// Typed SDK access only. Rust owns bounded identity, projection, plans and policy.
#include <Fusion/Sketch/Sketch.h>
#include <Fusion/Sketch/SketchCurves.h>
#include <Fusion/Sketch/SketchPoints.h>
#include <Fusion/Sketch/Profiles.h>
#include <Fusion/Sketch/SketchDimensions.h>
#include <Fusion/Sketch/GeometricConstraints.h>
#include <Fusion/Construction/ConstructionPlane.h>
#include <Core/Geometry/Matrix3D.h>
#include <Fusion/Sketch/SketchEntity.h>
#include <Fusion/Sketch/SketchPoint.h>
#include <Fusion/Sketch/GeometricConstraint.h>
#include <Fusion/Sketch/HorizontalConstraint.h>
#include <Fusion/Sketch/VerticalConstraint.h>
#include <Fusion/Sketch/SketchLinearDimension.h>
#include <Fusion/Fusion/ModelParameter.h>
#include <Fusion/Fusion/ModelParameters.h>
#include <Fusion/Sketch/Profile.h>
#include <Fusion/Features/ExtrudeFeature.h>
#include <Fusion/BRep/BRepBody.h>
#include <array>
struct AfSketchRef {
    DWORD thread; Ptr<adsk::core::Document> document; Ptr<adsk::fusion::Component> root; Ptr<adsk::fusion::Sketch> sketch;
    std::array<Ptr<adsk::fusion::SketchEntity>,64> geometry_entities{};
    std::array<uint32_t,64> geometry_kinds{};
    uint64_t geometry_count=0;
    // Released first (reverse field order), before geometry/sketch/root/document.
    Ptr<adsk::fusion::Profile> retained_profile;
    Ptr<adsk::fusion::ExtrudeFeature> retained_extrude;
    Ptr<adsk::fusion::BRepBody> retained_solid;
    bool extrusion_attempted=false;
    bool profile_verification_attempted=false,profile_relationship_confirmed=false;
    // Fixed bounded read leases; destroyed before parent geometry/sketch refs.
    std::array<Ptr<adsk::fusion::SketchPoint>,3> line_state_points{};
    bool line_state_bound=false;
    uint64_t line_state_lease=UINT64_MAX;
    // One returned constraint, released before endpoints and parent sketch.
    Ptr<adsk::fusion::HorizontalConstraint> retained_horizontal;
    Ptr<adsk::fusion::VerticalConstraint> retained_vertical;
    Ptr<adsk::fusion::GeometricConstraint> retained_relation;
    uint32_t relation_kind=0;
    bool relation_attempted=false;
    // Exact typed children retained immediately after add, released before parents.
    Ptr<adsk::fusion::SketchLinearDimension> retained_dimension;
    Ptr<adsk::fusion::ModelParameter> retained_dimension_parameter;
    bool dimension_add_attempted=false,dimension_value_attempted=false;
};
extern "C" int af_sketch_root(AfContext* c,AfDocumentRef* d,AfSketchRoot* out,AfBytes sink,void* user) noexcept {
    if(!out || !sink)return 1;*out={};try {Ptr<adsk::fusion::Design> design;int s=af_design(c,d,design);if(s)return s;
        auto app=c->app;auto active=app->activeDocument();if(app->getLastError()!=0)return 7;out->active=active==d->document;
#define SK_ROOT(field,expr) {auto value=(expr);if(app->getLastError()!=0)return 7;out->field=value;}
        SK_ROOT(parametric,design->designType()==adsk::fusion::ParametricDesignType);
        SK_ROOT(hybrid,design->designIntent()==adsk::fusion::HybridDesignIntentType);
        auto configured=design->isConfiguredDesign();if(app->getLastError()!=0)return 7;
        auto configuration=design->isConfiguration();if(app->getLastError()!=0)return 7;
        auto table=design->configurationTopTable();if(app->getLastError()!=0)return 7;
        auto row=design->configurationRowId();if(app->getLastError()!=0)return 7;out->configured=configured||configuration||table||!row.empty();
        auto root=design->rootComponent();if(app->getLastError()!=0||!root)return 7;
        auto revision=root->revisionId();if(app->getLastError()!=0||revision.empty())return 7;
        auto sketches=root->sketches();if(app->getLastError()!=0||!sketches)return 7;SK_ROOT(sketch_count,sketches->count());
        if(out->parametric) {auto timeline=design->timeline();if(app->getLastError()!=0||!timeline)return 7;
            SK_ROOT(timeline_count,timeline->count());auto position=timeline->markerPosition();if(app->getLastError()!=0||position<0)return 7;out->timeline_position=position;}
#undef SK_ROOT
        return sink(user,0,revision.data(),revision.size())?0:5;
    }catch(...){return 4;}
}
extern "C" int af_sketch_create(AfContext* c,AfDocumentRef* d,uint32_t plane,AfSketchRef** out,uint32_t* invoked) noexcept {
    if(!out||!invoked)return 1;*out=nullptr;*invoked=0;try {Ptr<adsk::fusion::Design> design;int s=af_design(c,d,design);if(s)return s;
        auto root=design->rootComponent();if(c->app->getLastError()!=0||!root)return 7;
        Ptr<adsk::fusion::ConstructionPlane> support;
        if(plane==1)support=root->xYConstructionPlane();else if(plane==2)support=root->xZConstructionPlane();else if(plane==3)support=root->yZConstructionPlane();else return 1;
        if(c->app->getLastError()!=0||!support)return 7;
        auto sketches=root->sketches();if(c->app->getLastError()!=0||!sketches)return 7;
        // Holder is observable before add, so null/exception/error cannot lose ownership.
        *out=new AfSketchRef{GetCurrentThreadId(),d->document,root,{}};
        *invoked=1;(*out)->sketch=sketches->add(support);
        if(c->app->getLastError()!=0||!(*out)->sketch)return 7;return 0;
    }catch(...){return 4;}
}
extern "C" int af_sketch_read(AfContext* c,AfDocumentRef* d,AfSketchRef* ref,AfSketchState* out,AfBytes sink,void* user) noexcept {
    if(!ref||!out||!sink)return 1;*out={};try {int s=check(c);if(s)return s;if(ref->thread!=GetCurrentThreadId())return 2;
        Ptr<adsk::fusion::Design> design;s=af_design(c,d,design);if(s)return s;
        if(ref->document!=d->document||!ref->sketch)return 6;auto app=c->app;
        auto root=design->rootComponent();if(app->getLastError()!=0||root!=ref->root)return 6;
        if(!ref->sketch->isValid()||app->getLastError()!=0)return 3;
        auto parent=ref->sketch->parentComponent();if(app->getLastError()!=0||parent!=root)return 6;
        auto sketches=root->sketches();if(app->getLastError()!=0||!sketches)return 7;
        auto count=sketches->count();if(app->getLastError()!=0||count>512)return 5;
        bool member=false;for(size_t i=0;i<count;++i){auto value=sketches->item(i);if(app->getLastError()!=0||!value)return 7;if(value==ref->sketch)member=true;}if(!member)return 6;
        auto sketch=ref->sketch;auto name=sketch->name();if(app->getLastError()!=0)return 7;
        auto revision=sketch->revisionId();if(app->getLastError()!=0||revision.empty())return 7;
        Ptr<adsk::fusion::ConstructionPlane> plane=sketch->referencePlane();if(app->getLastError()!=0)return 7;
        auto xy=root->xYConstructionPlane();if(app->getLastError()!=0||!xy)return 7;
        auto xz=root->xZConstructionPlane();if(app->getLastError()!=0||!xz)return 7;
        auto yz=root->yZConstructionPlane();if(app->getLastError()!=0||!yz)return 7;
        out->plane=plane==xy?1:plane==xz?2:plane==yz?3:0;
        auto transform=sketch->transform();if(app->getLastError()!=0||!transform)return 7;
        auto array=transform->asArray();if(app->getLastError()!=0||array.size()!=16)return 7;for(size_t i=0;i<16;++i)out->transform[i]=array[i];
#define SK_COUNT(field,expr) {auto values=(expr);if(app->getLastError()!=0||!values)return 7;out->field=values->count();if(app->getLastError()!=0)return 7;}
        SK_COUNT(curves,sketch->sketchCurves());SK_COUNT(points,sketch->sketchPoints());SK_COUNT(profiles,sketch->profiles());SK_COUNT(dimensions,sketch->sketchDimensions());SK_COUNT(constraints,sketch->geometricConstraints());
#undef SK_COUNT
        out->health=sketch->healthState();if(app->getLastError()!=0)return 7;
        out->deferred=sketch->isComputeDeferred();if(app->getLastError()!=0)return 7;
        return sink(user,0,name.data(),name.size())&&sink(user,1,revision.data(),revision.size())?0:5;
    }catch(...){return 4;}
}
extern "C" int af_sketch_release(AfContext* c,AfSketchRef* ref) noexcept {
    if(!ref)return 1;int s=check(c);if(s)return s;if(ref->thread!=GetCurrentThreadId())return 2;try{delete ref;return 0;}catch(...){return 4;}
}
