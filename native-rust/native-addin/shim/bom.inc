// Typed retained SDK leases/getters only. Rust owns traversal, grouping, policy,
// metadata decoding, projections, bounds, BOM calculation and CSV publication.
#include <Fusion/Components/Occurrence.h>
#include <Fusion/Components/OccurrenceList.h>
#include <Fusion/BRep/BRepBody.h>
#include <Fusion/Fusion/PhysicalProperties.h>
#include <Core/Application/Attributes.h>
#include <Core/Application/Attribute.h>
#include <Core/Materials/Material.h>
#include <vector>
struct AfBomObject { Ptr<adsk::fusion::Component> component; Ptr<adsk::fusion::Occurrence> occurrence; };
struct AfBomView { AfContext* context; AfDocumentRef* document; Ptr<adsk::fusion::Design> design; std::vector<AfBomObject> objects; const char* getter_stage="not_entered"; int sdk_error=0; uint32_t definition_id=UINT32_MAX; std::string sdk_description; };
static int bom_check(AfBomView* v) {
    if(!v) return 1; Ptr<adsk::fusion::Design> design;
    int result=af_design(v->context,v->document,design); if(result) return result;
    return design==v->design?0:6;
}
static int bom_object(AfBomView* v,uint32_t id,AfBomObject*& out) {
    int result=bom_check(v); if(result) return result;
    if(id>=v->objects.size()) return 1; out=&v->objects[id];
    bool valid=out->component?out->component->isValid():out->occurrence->isValid();
    return valid && v->context->app->getLastError()==0?0:3;
}
static int bom_add(AfBomView* v,AfBomObject value,uint32_t* out) {
    if(!out || v->objects.size()>=4096) return 5;
    *out=static_cast<uint32_t>(v->objects.size());v->objects.push_back(value);return 0;
}
static int bom_properties(AfBomView* v,const Ptr<adsk::fusion::PhysicalProperties>& properties,AfBomPhysical* out) {
    if(v->context->app->getLastError()!=0 || !properties || !out) return 7;
    auto accuracy=properties->accuracy();if(v->context->app->getLastError()!=0)return 7;
    out->high_accuracy=accuracy==adsk::fusion::HighCalculationAccuracy?1:0;
    out->mass_kg=properties->mass();if(v->context->app->getLastError()!=0)return 7;
    out->volume_cm3=properties->volume();return v->context->app->getLastError()==0?0:7;
}
extern "C" int af_bom_open(AfContext* c,AfDocumentRef* d,AfBomView** out) noexcept {
    if(!out)return 1;*out=nullptr;try {Ptr<adsk::fusion::Design> design;int s=af_design(c,d,design);if(s)return s;
        *out=new AfBomView{c,d,design,{}};return 0;}catch(...){return 4;}
}
extern "C" int af_bom_close(AfBomView* v) noexcept {
    if(!v)return 1;int s=check(v->context);if(s)return s;try{delete v;return 0;}catch(...){return 4;}
}
// Read-only SDK connectivity fact. Rust decides whether BOM metadata is supported.
extern "C" int af_bom_is_offline(AfBomView* v,uint32_t* out) noexcept {
    if(!v || !out)return 1;*out=0;
    try {int s=bom_check(v);if(s)return s;auto offline=v->context->app->isOffLine();
        if(v->context->app->getLastError()!=0)return 7;*out=offline?1:0;return 0;
    }catch(...){return 4;}
}
extern "C" int af_bom_design(AfBomView* v,AfBomDesign* out,AfBytes sink,void* user) noexcept {
    if(!v || !out || !sink)return 1;*out={};try {v->getter_stage="validation";v->sdk_error=0;v->definition_id=UINT32_MAX;v->sdk_description.clear();int s=bom_check(v);if(s)return s;auto app=v->context->app;
#define BOM_READ(field, expression) {v->getter_stage=#field;auto value=(expression);if(app->getLastError()!=0)return 7;out->field=value;}
        v->getter_stage="active_document";auto active=app->activeDocument();if(app->getLastError()!=0)return 7;out->active=active==v->document->document?1:0;
        v->getter_stage="design_intent";auto intent=v->design->designIntent();if(app->getLastError()!=0)return 7;out->hybrid=intent==adsk::fusion::HybridDesignIntentType?1:0;
        BOM_READ(configured,v->design->isConfiguredDesign());BOM_READ(configuration,v->design->isConfiguration());
        v->getter_stage="configuration_top_table";auto table=v->design->configurationTopTable();if(app->getLastError()!=0)return 7;out->table_present=table?1:0;
        v->getter_stage="configuration_row_id";auto row=v->design->configurationRowId();if(app->getLastError()!=0)return 7;if(!sink(user,0,row.data(),row.size()))return 5;
        v->getter_stage="all_components";auto components=v->design->allComponents();if(app->getLastError()!=0 || !components)return 7;BOM_READ(components,components->count());
        v->getter_stage="timeline";auto timeline=v->design->timeline();if(app->getLastError()!=0 || !timeline)return 7;BOM_READ(timeline_count,timeline->count());BOM_READ(timeline_position,timeline->markerPosition());
#undef BOM_READ
        return 0;}catch(...){return 4;}
}
// Diagnostic only: retained error from the original getLastError call.
// No SDK getter, reset, retry, or ownership change while fetching this detail.
extern "C" int af_bom_error_stage(AfBomView* v,int32_t* sdk_error,uint32_t* definition_id,AfBytes sink,void* user) noexcept {
    if(!v || !sdk_error || !definition_id || !sink)return 1;int s=check(v->context);if(s)return s;
    *sdk_error=v->sdk_error;*definition_id=v->definition_id;
    const char* stage=v->getter_stage;size_t size=0;while(stage[size] && size<64)++size;
    if(size==64 || v->sdk_description.size()>1024)return 5;
    return sink(user,0,stage,size) && sink(user,1,v->sdk_description.data(),v->sdk_description.size())?0:5;
}
extern "C" int af_bom_root(AfBomView* v,uint32_t* out) noexcept {
    try{int s=bom_check(v);if(s)return s;auto root=v->design->rootComponent();if(v->context->app->getLastError()!=0 || !root)return 7;return bom_add(v,{root,{}},out);}catch(...){return 4;}
}
extern "C" int af_bom_same(AfBomView* v,uint32_t a,uint32_t b,uint32_t* out) noexcept {
    if(!out || !v)return 1;try{int s=check(v->context);if(s)return s;if(a>=v->objects.size() || b>=v->objects.size())return 1;auto left=&v->objects[a];auto right=&v->objects[b];
        *out=(left->component && right->component)?left->component==right->component:(left->occurrence && right->occurrence)?left->occurrence==right->occurrence:false;return 0;}catch(...){return 4;}
}
extern "C" int af_bom_component(AfBomView* v,uint32_t id,uint32_t* out) noexcept {
    try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->occurrence)return 1;auto c=o->occurrence->component();if(v->context->app->getLastError()!=0 || !c)return 7;
        auto owner=c->parentDesign();if(v->context->app->getLastError()!=0 || owner!=v->design)return 6;return bom_add(v,{c,{}},out);}catch(...){return 4;}
}
extern "C" int af_bom_definition(AfBomView* v,uint32_t id,uint64_t* meshes,AfBytes sink,void* user) noexcept {
    if(!v || !meshes || !sink)return 1;try{v->getter_stage="validation";v->sdk_error=0;v->definition_id=id;v->sdk_description.clear();AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->component)return 1;auto c=o->component;
#define BOM_TEXT(field, label, expression) {v->getter_stage=label;auto text=(expression);std::string description;int error=v->context->app->getLastError(&description);if(error!=0){v->sdk_error=error;try{v->sdk_description.assign(description,0,1024);}catch(...){v->sdk_description.clear();}return 7;}if(!sink(user,field,text.data(),text.size()))return 5;}
        BOM_TEXT(0,"name",c->name());BOM_TEXT(1,"part_number",c->partNumber());BOM_TEXT(2,"description",c->description());BOM_TEXT(3,"revision_id",c->revisionId());BOM_TEXT(4,"entity_token",c->entityToken());
#undef BOM_TEXT
        v->getter_stage="mesh_bodies";auto collection=c->meshBodies();if(v->context->app->getLastError()!=0 || !collection)return 7;v->getter_stage="mesh_bodies_count";*meshes=collection->count();return v->context->app->getLastError()==0?0:7;
    }catch(...){return 4;}
}
extern "C" int af_bom_material(AfBomView* v,uint32_t id,uint32_t* present,AfBytes sink,void* user) noexcept {
    if(!present || !sink)return 1;try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->component)return 1;
        auto m=o->component->material();if(v->context->app->getLastError()!=0)return 7;*present=m?1:0;if(!m)return 0;
        auto identity=m->id();if(v->context->app->getLastError()!=0)return 7;auto name=m->name();if(v->context->app->getLastError()!=0)return 7;
        return sink(user,0,identity.data(),identity.size())&&sink(user,1,name.data(),name.size())?0:5;
    }catch(...){return 4;}
}
extern "C" int af_bom_attribute_count(AfBomView* v,uint32_t id,uint64_t* out) noexcept {
    if(!out)return 1;try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->component)return 1;auto a=o->component->attributes();if(v->context->app->getLastError()!=0 || !a)return 7;*out=a->count();return v->context->app->getLastError()==0?0:7;}catch(...){return 4;}
}
extern "C" int af_bom_attribute(AfBomView* v,uint32_t id,uint64_t index,AfBytes sink,void* user) noexcept {
    if(!sink || index>128)return 1;try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->component)return 1;auto a=o->component->attributes();if(v->context->app->getLastError()!=0 || !a)return 7;auto value=a->item(static_cast<int>(index));if(v->context->app->getLastError()!=0 || !value)return 7;
        auto group=value->groupName();if(v->context->app->getLastError()!=0)return 7;auto name=value->name();if(v->context->app->getLastError()!=0)return 7;auto text=value->value();if(v->context->app->getLastError()!=0)return 7;
        return sink(user,0,group.data(),group.size())&&sink(user,1,name.data(),name.size())&&sink(user,2,text.data(),text.size())?0:5;
    }catch(...){return 4;}
}
extern "C" int af_bom_instance(AfBomView* v,uint32_t id,AfBomOccurrence* out,AfBytes sink,void* user) noexcept {
    if(!out || !sink)return 1;*out={};try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->occurrence)return 1;auto occurrence=o->occurrence;auto app=v->context->app;
#define BOM_FLAG(field, expression) {auto value=(expression);if(app->getLastError()!=0)return 7;out->field=value?1:0;}
        BOM_FLAG(referenced,occurrence->isReferencedComponent());BOM_FLAG(configured,occurrence->isConfiguration());BOM_FLAG(visible,occurrence->isVisible());
        auto timeline=occurrence->timelineObject();if(app->getLastError()!=0)return 7;out->timeline_present=timeline?1:0;
        if(timeline){BOM_FLAG(suppressed,timeline->isSuppressed());BOM_FLAG(rolled_back,timeline->isRolledBack());}
#undef BOM_FLAG
        auto path=occurrence->fullPathName();if(app->getLastError()!=0)return 7;auto token=occurrence->entityToken();if(app->getLastError()!=0)return 7;
        return sink(user,0,path.data(),path.size())&&sink(user,1,token.data(),token.size())?0:5;
    }catch(...){return 4;}
}
extern "C" int af_bom_child_count(AfBomView* v,uint32_t id,uint64_t* out) noexcept {
    if(!out)return 1;try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;
        if(o->component){auto c=o->component->occurrences();if(v->context->app->getLastError()!=0 || !c)return 7;*out=c->count();}
        else{auto c=o->occurrence->childOccurrences();if(v->context->app->getLastError()!=0 || !c)return 7;*out=c->count();}
        return v->context->app->getLastError()==0?0:7;
    }catch(...){return 4;}
}
extern "C" int af_bom_child(AfBomView* v,uint32_t id,uint64_t index,uint32_t* out) noexcept {
    if(index>512)return 1;try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;Ptr<adsk::fusion::Occurrence> child;
        if(o->component){auto c=o->component->occurrences();if(v->context->app->getLastError()!=0 || !c)return 7;child=c->item(static_cast<int>(index));}
        else{auto c=o->occurrence->childOccurrences();if(v->context->app->getLastError()!=0 || !c)return 7;child=c->item(static_cast<int>(index));}
        if(v->context->app->getLastError()!=0 || !child)return 7;return bom_add(v,{{},child},out);
    }catch(...){return 4;}
}
static int bom_bodies(AfBomView* v,uint32_t id,Ptr<adsk::fusion::BRepBodies>& bodies) {
    AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;bodies=o->component?o->component->bRepBodies():o->occurrence->bRepBodies();return v->context->app->getLastError()==0 && bodies?0:7;
}
extern "C" int af_bom_body_count(AfBomView* v,uint32_t id,uint64_t* out) noexcept {
    if(!out)return 1;try{Ptr<adsk::fusion::BRepBodies> bodies;int s=bom_bodies(v,id,bodies);if(s)return s;*out=bodies->count();return v->context->app->getLastError()==0?0:7;}catch(...){return 4;}
}
extern "C" int af_bom_body(AfBomView* v,uint32_t id,uint64_t index,AfBomPhysical* out) noexcept {
    if(!out || index>2048)return 1;try{Ptr<adsk::fusion::BRepBodies> bodies;int s=bom_bodies(v,id,bodies);if(s)return s;auto body=bodies->item(static_cast<int>(index));if(v->context->app->getLastError()!=0 || !body)return 7;
        bool solid=body->isSolid();if(v->context->app->getLastError()!=0 || !solid)return 6;
        return bom_properties(v,body->getPhysicalProperties(adsk::fusion::HighCalculationAccuracy),out);
    }catch(...){return 4;}
}
extern "C" int af_bom_subtree(AfBomView* v,uint32_t id,AfBomPhysical* out) noexcept {
    try{AfBomObject* o;int s=bom_object(v,id,o);if(s)return s;if(!o->component)return 1;return bom_properties(v,o->component->getPhysicalProperties(adsk::fusion::HighCalculationAccuracy),out);}catch(...){return 4;}
}
