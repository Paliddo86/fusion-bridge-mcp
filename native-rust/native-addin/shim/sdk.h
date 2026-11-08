#pragma once
#include <stddef.h>
#include <stdint.h>

// This is the entire initial ABI. No STL, SDK object, ownership, JSON or Python
// representation crosses it. SDK pointers remain scoped to the opening thread.
struct AfContext;
struct AfEvent;
struct AfDocumentRef;
struct AfBomView;
struct AfBomDesign { uint32_t active, hybrid, configured, configuration, table_present; uint64_t components, timeline_count, timeline_position; };
struct AfBomOccurrence { uint32_t referenced, configured, timeline_present, suppressed, rolled_back, visible; };
struct AfBomPhysical { double mass_kg, volume_cm3; uint32_t high_accuracy; };
struct AfDocument { uint32_t active; uint32_t saved; uint32_t modified; };
struct AfDesignState {
    uint32_t active;
    uint32_t parametric;
    uint64_t component_count;
    uint64_t body_count;
    uint64_t sketch_count;
    uint64_t timeline_count;
    uint64_t timeline_position;
};
struct AfParameter {
    double value;
    uint32_t value_type; // 0 numeric database units, 1 text (sink field 4).
    uint32_t is_user;
    uint32_t is_deletable;
};
struct AfScratchProfile {
    uint32_t parametric, hybrid;
    uint64_t components, bodies, sketches, occurrences, timeline, graphics, mesh_bodies,
             features, planes, axes, points, parameters, user_parameters;
};
typedef int (*AfBytes)(void*, uint32_t, const char*, size_t);
typedef void (*AfWake)(void*);
extern "C" {
int af_open(AfContext** out) noexcept;
int af_close(AfContext* context) noexcept;
int af_version(AfContext* context, AfBytes sink, void* user) noexcept;
int af_document_count(AfContext* context, uint64_t* out) noexcept;
int af_document_retain(AfContext* context, uint64_t index, AfDocumentRef** out) noexcept;
int af_document_ref_read(AfContext* context, AfDocumentRef* document, AfDocument* out, AfBytes sink, void* user) noexcept;
int af_document_same(AfContext* context, AfDocumentRef* left, AfDocumentRef* right, uint32_t* out) noexcept;
int af_document_valid(AfContext* context, AfDocumentRef* document, uint32_t* out) noexcept;
int af_document_release(AfContext* context, AfDocumentRef* document) noexcept;
// Design sink fields: 0 root name, 1 root revisionId, 2 default length units.
int af_design_state(AfContext*, AfDocumentRef*, AfDesignState*, AfBytes, void*) noexcept;
int af_parameter_count(AfContext*, AfDocumentRef*, uint64_t*) noexcept;
// Parameter sink fields: 0 name, 1 expression, 2 unit, 3 comment, 4 text value.
int af_parameter_read(AfContext*, AfDocumentRef*, uint64_t, AfParameter*, AfBytes, void*) noexcept;
// Health sink field 0: error/warning message; state is FeatureHealthStates 0..5.
int af_timeline_health(AfContext*, AfDocumentRef*, uint64_t, uint32_t*, AfBytes, void*) noexcept;
int af_parameter_evaluate(AfContext*, AfDocumentRef*, const char*, size_t, const char*, size_t, double*) noexcept;
int af_parameter_create(AfContext*, AfDocumentRef*, const char*, size_t, const char*, size_t, const char*, size_t, const char*, size_t) noexcept;
int af_parameter_set(AfContext*, AfDocumentRef*, const char*, size_t, const char*, size_t) noexcept;
int af_design_compute(AfContext*, AfDocumentRef*) noexcept;
int af_active_command(AfContext*, AfBytes, void*) noexcept;
int af_document_create(AfContext*, AfDocumentRef**, uint32_t* invoked) noexcept;
int af_document_parametric(AfContext*, AfDocumentRef*, uint32_t* invoked, uint32_t* ack) noexcept;
int af_document_data_file(AfContext*, AfDocumentRef*, uint32_t*) noexcept;
int af_scratch_profile(AfContext*, AfDocumentRef*, AfScratchProfile*) noexcept;
int af_document_discard(AfContext*, AfDocumentRef*, uint32_t* invoked, uint32_t* ack) noexcept;
int af_document_activate(AfContext*, AfDocumentRef*, uint32_t* invoked, uint32_t* ack) noexcept;
int af_bom_open(AfContext*, AfDocumentRef*, AfBomView**) noexcept;
int af_bom_close(AfBomView*) noexcept;
int af_bom_design(AfBomView*, AfBomDesign*, AfBytes, void*) noexcept;
int af_bom_root(AfBomView*, uint32_t*) noexcept;
int af_bom_same(AfBomView*, uint32_t, uint32_t, uint32_t*) noexcept;
int af_bom_component(AfBomView*, uint32_t, uint32_t*) noexcept;
int af_bom_definition(AfBomView*, uint32_t, uint64_t*, AfBytes, void*) noexcept;
int af_bom_material(AfBomView*, uint32_t, uint32_t*, AfBytes, void*) noexcept;
int af_bom_attribute_count(AfBomView*, uint32_t, uint64_t*) noexcept;
int af_bom_attribute(AfBomView*, uint32_t, uint64_t, AfBytes, void*) noexcept;
int af_bom_instance(AfBomView*, uint32_t, AfBomOccurrence*, AfBytes, void*) noexcept;
int af_bom_child_count(AfBomView*, uint32_t, uint64_t*) noexcept;
int af_bom_child(AfBomView*, uint32_t, uint64_t, uint32_t*) noexcept;
int af_bom_body_count(AfBomView*, uint32_t, uint64_t*) noexcept;
int af_bom_body(AfBomView*, uint32_t, uint64_t, AfBomPhysical*) noexcept;
int af_bom_subtree(AfBomView*, uint32_t, AfBomPhysical*) noexcept;
uint32_t af_thread_id() noexcept;
int af_event_open(const char* event_id, size_t len, AfWake callback, void* user, AfEvent** out) noexcept;
// Only this specific SDK operation may be called from a worker thread. The
// owner must join every worker before af_event_close releases SDK references.
int af_event_fire(AfEvent* event) noexcept;
int af_event_close(AfEvent* event) noexcept;
}
struct AfSketchRef;
struct AfSketchRoot { uint32_t active,parametric,hybrid,configured; uint64_t sketch_count,timeline_count,timeline_position; };
struct AfSketchState { double transform[16]; uint64_t curves,points,profiles,dimensions,constraints; uint32_t plane,health,deferred; };
extern "C" {
int af_sketch_root(AfContext*,AfDocumentRef*,AfSketchRoot*,AfBytes,void*) noexcept;
int af_sketch_create(AfContext*,AfDocumentRef*,uint32_t,AfSketchRef**,uint32_t*) noexcept;
int af_sketch_read(AfContext*,AfDocumentRef*,AfSketchRef*,AfSketchState*,AfBytes,void*) noexcept;
int af_sketch_release(AfContext*,AfSketchRef*) noexcept;
struct AfSketchGeometry { uint32_t kind,construction,closed,point_count,reference; double scalar,normal[3],points[384]; };
static_assert(sizeof(AfSketchGeometry)==3128,"geometry ABI size");
static_assert(offsetof(AfSketchGeometry,points)==56,"geometry ABI points offset");
int af_geometry_empty(AfContext*,AfDocumentRef*,AfSketchRef*,uint32_t*) noexcept;
int af_geometry_insert(AfContext*,AfDocumentRef*,AfSketchRef*,const AfSketchGeometry*,uint64_t*,uint32_t*,uint32_t*) noexcept;
int af_geometry_equalities(AfContext*,AfDocumentRef*,AfSketchRef*,const uint64_t*,uint32_t,uint8_t*) noexcept;
int af_geometry_read(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,AfSketchGeometry*) noexcept;
struct AfExtrudeDiagnostic { int32_t sdk_error; uint32_t stage,length,truncated; char text[4096]; };
struct AfProfileState { double area,perimeter,centroid[3]; uint64_t loops,curves,root_bodies,root_extrudes; uint32_t on_plane,outer_only,high_accuracy; };
struct AfSolidState { double volume,centroid_sketch[3],distance,taper; uint32_t health,body_solid,feature_solid,new_body,one_side,profile_plane_start,high_accuracy; };
int af_profile_read(AfContext*,AfDocumentRef*,AfSketchRef*,AfProfileState*,AfExtrudeDiagnostic*) noexcept;
int af_extrude_create(AfContext*,AfDocumentRef*,AfSketchRef*,double,uint32_t,uint32_t*,AfExtrudeDiagnostic*) noexcept;
int af_extrude_verify_profile(AfContext*,AfDocumentRef*,AfSketchRef*,AfExtrudeDiagnostic*) noexcept;
int af_extrude_read(AfContext*,AfDocumentRef*,AfSketchRef*,AfSolidState*,AfExtrudeDiagnostic*) noexcept;
}

extern "C" {
int af_material_library_count(AfContext*,AfDocumentRef*,uint64_t*) noexcept;
int af_material_library_read(AfContext*,AfDocumentRef*,uint64_t,uint64_t*,AfBytes,void*) noexcept;
int af_material_count(AfContext*,AfDocumentRef*,const char*,size_t,uint64_t*) noexcept;
int af_material_read(AfContext*,AfDocumentRef*,const char*,size_t,uint64_t,AfBytes,void*) noexcept;
}

struct AfMcmasterState { uint32_t design_intent,available,native; };
extern "C" int af_mcmaster_discover(AfContext*,AfDocumentRef*,AfMcmasterState*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
struct AfPhysicalState {
    uint64_t root_bodies,root_occurrences,mesh_bodies,face_count,non_solid_bodies;
    uint32_t design_intent,configured,accuracy,reserved;
    double mass_kg,volume_cm3,area_cm2,center_cm[3];
};
extern "C" int af_root_physical(AfContext*,AfDocumentRef*,uint64_t,uint64_t,AfPhysicalState*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
struct AfLineFlags {uint32_t fixed,reference,has_reference,fully_constrained,is_2d;uint64_t constraints,dimensions,attributes;};
struct AfLinePoint {double position[3];AfLineFlags flags;uint64_t connected;};
struct AfLineState {AfLineFlags line;AfLinePoint points[3];uint64_t bodies,features,occurrences,meshes,components,texts,attributes;uint32_t construction;};
extern "C" int af_line_state(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,AfLineState*,AfExtrudeDiagnostic*) noexcept;

struct AfRelationState { AfLineState line; uint32_t kind; };
extern "C" int af_relation_state(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,AfRelationState*,AfExtrudeDiagnostic*) noexcept;
extern "C" int af_relation_add(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,uint32_t,uint32_t*,AfExtrudeDiagnostic*) noexcept;

struct AfDimensionState {AfRelationState relation;uint64_t model_parameters,user_parameters;uint32_t present,driving,aligned,reserved;double value,parameter_value,text[3];};
extern "C" int af_dimension_state(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,AfDimensionState*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
extern "C" int af_dimension_add(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,const double*,uint32_t*,AfExtrudeDiagnostic*) noexcept;
extern "C" int af_dimension_set(AfContext*,AfDocumentRef*,AfSketchRef*,uint64_t,double,uint32_t*,AfExtrudeDiagnostic*) noexcept;

struct AfAssembly;
struct AfAssemblyRoot {uint64_t counts[13];uint32_t flags[6];};
struct AfAssemblyComponent {uint64_t counts[10];};
struct AfAssemblyOccurrence {uint64_t component,children;double matrix[16];uint32_t flags[7];};
struct AfAssemblyPositions {uint64_t count,timeline,timeline_position;uint32_t pending,reserved;int64_t indexes[64];uint32_t health[64];};
extern "C" {
int af_assembly_positions(AfContext*,AfDocumentRef*,AfAssembly*,AfAssemblyPositions*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_translate(AfContext*,AfDocumentRef*,AfAssembly*,uint64_t,const double*,uint32_t*,uint64_t*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_capture_position(AfContext*,AfDocumentRef*,AfAssembly*,uint32_t*,uint64_t*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_open(AfContext*,AfDocumentRef*,AfAssembly**,AfExtrudeDiagnostic*) noexcept;
int af_assembly_close(AfContext*,AfAssembly*) noexcept;
int af_assembly_root(AfContext*,AfDocumentRef*,AfAssembly*,AfAssemblyRoot*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_component(AfContext*,AfDocumentRef*,AfAssembly*,uint64_t,uint32_t,AfAssemblyComponent*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_attribute(AfContext*,AfDocumentRef*,AfAssembly*,uint64_t,uint64_t,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_occurrence(AfContext*,AfDocumentRef*,AfAssembly*,uint64_t,AfAssemblyOccurrence*,AfBytes,void*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_add(AfContext*,AfDocumentRef*,AfAssembly*,int64_t,uint32_t*,uint64_t*,uint64_t*,AfExtrudeDiagnostic*) noexcept;
int af_assembly_change(AfContext*,AfDocumentRef*,AfAssembly*,uint64_t,uint32_t,const char*,size_t,const char*,size_t,uint32_t*,AfExtrudeDiagnostic*) noexcept;
}
