// Bounded typed root physical-property observation. No setters or SDK pointers
// cross the ABI; all Ptr ownership is local to this owning-thread invocation.
#include <array>
#include <Fusion/BRep/BRepFaces.h>
#include <Fusion/Components/Occurrences.h>
#include <Fusion/MeshBody/MeshBodies.h>
static_assert(sizeof(AfPhysicalState)==104 && offsetof(AfPhysicalState,mass_kg)==56 && offsetof(AfPhysicalState,center_cm)==80,"physical ABI");
static_assert(static_cast<uint32_t>(adsk::fusion::HighCalculationAccuracy)==2,"SDK high accuracy enum");
static int ph_body(AfContext*c,AfExtrudeDiagnostic*diag,const Ptr<adsk::fusion::Component>&root,
                   const Ptr<adsk::fusion::BRepBody>&body,uint64_t&faces) {
    if(!body)return 7;
    bool live=body->isValid();if(!ex_ok(c,diag,130)||!live)return 3;
    auto parent=body->parentComponent();if(!ex_ok(c,diag,131)||parent!=root)return 6;
    bool transient=body->isTransient();if(!ex_ok(c,diag,132)||transient)return 6;
    auto context=body->assemblyContext();if(!ex_ok(c,diag,133)||context)return 6;
    bool solid=body->isSolid();if(!ex_ok(c,diag,134)||!solid)return 6;
    auto list=body->faces();if(!ex_ok(c,diag,135)||!list)return 7;
    faces=list->count();if(!ex_ok(c,diag,136)||faces==0)return 6;
    return 0;
}
extern "C" int af_root_physical(AfContext*c,AfDocumentRef*d,uint64_t max_bodies,uint64_t max_faces,AfPhysicalState*out,AfBytes sink,void*user,AfExtrudeDiagnostic*diag) noexcept {
    if(!out||!sink||!diag||max_bodies>128||max_faces>20000)return 1;
    *out={};*diag={};diag->sdk_error=-1;diag->stage=1;
    try{
        Ptr<adsk::fusion::Design> design;int status=af_design(c,d,design);if(status)return status;
        auto root=design->rootComponent();if(!ex_ok(c,diag,100)||!root)return 7;
        bool live=root->isValid();if(!ex_ok(c,diag,101)||!live)return 3;
        auto before=root->revisionId();if(!ex_ok(c,diag,102))return 7;
        if(!sink(user,0,before.data(),before.size()))return 5;
        auto intent=design->designIntent();if(!ex_ok(c,diag,103))return 7;out->design_intent=static_cast<uint32_t>(intent);
        bool configured=design->isConfiguredDesign();if(!ex_ok(c,diag,104))return 7;
        bool configuration=design->isConfiguration();if(!ex_ok(c,diag,105))return 7;out->configured=configured||configuration?1:0;
        if(intent!=adsk::fusion::HybridDesignIntentType||out->configured)return 6;
        auto occurrences=root->occurrences();if(!ex_ok(c,diag,106)||!occurrences)return 7;
        out->root_occurrences=occurrences->count();if(!ex_ok(c,diag,107)||out->root_occurrences)return 6;
        auto mesh=root->meshBodies();if(!ex_ok(c,diag,108)||!mesh)return 7;
        out->mesh_bodies=mesh->count();if(!ex_ok(c,diag,109)||out->mesh_bodies)return 6;
        auto bodies=root->bRepBodies();if(!ex_ok(c,diag,110)||!bodies)return 7;
        out->root_bodies=bodies->count();if(!ex_ok(c,diag,111)||out->root_bodies>max_bodies)return 6;
        std::array<Ptr<adsk::fusion::BRepBody>,128> retained;
        std::array<uint64_t,128> face_counts{};
        for(size_t i=0;i<out->root_bodies;++i){
            retained[i]=bodies->item(i);if(!ex_ok(c,diag,112)||!retained[i])return 7;
            for(size_t j=0;j<i;++j)if(retained[j]==retained[i])return 6;
            status=ph_body(c,diag,root,retained[i],face_counts[i]);if(status)return status;
            if(face_counts[i]>max_faces-out->face_count)return 6;out->face_count+=face_counts[i];
        }
        // No physical calculation until the complete finite body/face admission.
        auto properties=root->getPhysicalProperties(adsk::fusion::HighCalculationAccuracy);if(!ex_ok(c,diag,150)||!properties)return 7;
        live=properties->isValid();if(!ex_ok(c,diag,151)||!live)return 3;
        out->accuracy=static_cast<uint32_t>(properties->accuracy());if(!ex_ok(c,diag,152))return 7;
        out->mass_kg=properties->mass();if(!ex_ok(c,diag,153))return 7;
        out->volume_cm3=properties->volume();if(!ex_ok(c,diag,154))return 7;
        out->area_cm2=properties->area();if(!ex_ok(c,diag,155))return 7;
        auto center=properties->centerOfMass();if(!ex_ok(c,diag,156)||!center)return 7;
        auto xyz=center->asArray();if(!ex_ok(c,diag,157)||xyz.size()!=3)return 7;
        std::copy(xyz.begin(),xyz.end(),out->center_cm);
        // Reacquire this exact document/design/root and every body membership.
        Ptr<adsk::fusion::Design> current;status=af_design(c,d,current);if(status)return status;
        if(current!=design)return 6;
        auto current_root=current->rootComponent();if(!ex_ok(c,diag,170)||current_root!=root)return 6;
        live=root->isValid();if(!ex_ok(c,diag,171)||!live)return 3;
        auto current_bodies=root->bRepBodies();if(!ex_ok(c,diag,172)||!current_bodies)return 7;
        auto count=current_bodies->count();if(!ex_ok(c,diag,173)||count!=out->root_bodies)return 6;
        for(size_t i=0;i<count;++i){
            auto member=current_bodies->item(i);if(!ex_ok(c,diag,174)||member!=retained[i])return 6;
            uint64_t faces=0;status=ph_body(c,diag,root,retained[i],faces);if(status)return status;
            if(faces!=face_counts[i])return 6;
        }
        auto current_occurrences=root->occurrences();if(!ex_ok(c,diag,175)||!current_occurrences)return 7;
        auto occurrence_count=current_occurrences->count();if(!ex_ok(c,diag,176)||occurrence_count!=out->root_occurrences)return 6;
        auto current_mesh=root->meshBodies();if(!ex_ok(c,diag,177)||!current_mesh)return 7;
        auto mesh_count=current_mesh->count();if(!ex_ok(c,diag,178)||mesh_count!=out->mesh_bodies)return 6;
        auto current_intent=design->designIntent();if(!ex_ok(c,diag,179)||current_intent!=intent)return 6;
        configured=design->isConfiguredDesign();if(!ex_ok(c,diag,180)||configured)return 6;
        configuration=design->isConfiguration();if(!ex_ok(c,diag,181)||configuration)return 6;
        auto after=root->revisionId();if(!ex_ok(c,diag,182))return 7;
        if(!sink(user,1,after.data(),after.size()))return 5;
        return 0;
    }catch(...){return 4;}
}
