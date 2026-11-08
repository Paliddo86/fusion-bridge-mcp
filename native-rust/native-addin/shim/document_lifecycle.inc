// Typed SDK ABI only. Rust owns command policy, retained identity, scratch
// ownership, bounds, journal proof and outcome classification.
#include <Core/UserInterface/UserInterface.h>
#include <Fusion/Components/Occurrences.h>
#include <Fusion/Graphics/CustomGraphicsGroups.h>
#include <Fusion/MeshBody/MeshBodies.h>
#include <Fusion/Features/Features.h>
#include <Fusion/Construction/ConstructionPlanes.h>
#include <Fusion/Construction/ConstructionAxes.h>
#include <Fusion/Construction/ConstructionPoints.h>
extern "C" int af_active_command(AfContext* c,AfBytes sink,void* user) noexcept {
    try { int result=check(c); if(result) return result; if(!sink || !valid(c)) return 3;
        auto ui=c->app->userInterface(); if(c->app->getLastError()!=0 || !ui) return 7;
        auto command=ui->activeCommand(); if(c->app->getLastError()!=0) return 7;
        return sink(user,0,command.data(),command.size())?0:5;
    } catch(...) {return 4;}
}
extern "C" int af_document_create(AfContext* c,AfDocumentRef** out,uint32_t* invoked) noexcept {
    if(!out || !invoked) return 1; *out=nullptr; *invoked=0;
    try { int result=check(c); if(result) return result; if(!valid(c)) return 3;
        auto docs=c->app->documents(); if(c->app->getLastError()!=0 || !docs) return 7;
        // Prepublish an owned holder before invoking the effect. Any resulting
        // reference remains available to Rust even on a failed acknowledgement.
        *out=new AfDocumentRef{c,{}}; *invoked=1;
        (*out)->document=docs->add(adsk::core::FusionDesignDocumentType,true);
        if(c->app->getLastError()!=0 || !(*out)->document) return 7;
        return 0;
    } catch(...) {return 4;}
}
extern "C" int af_document_parametric(AfContext* c,AfDocumentRef* d,uint32_t* invoked,uint32_t* ack) noexcept {
    if(!invoked || !ack) return 1; *invoked=0; *ack=0;
    try { Ptr<adsk::fusion::Design> design; int result=af_design(c,d,design); if(result) return result;
        *invoked=1; *ack=design->designType(adsk::fusion::ParametricDesignType)?1:0;
        return c->app->getLastError()==0?0:7;
    } catch(...) {return 4;}
}
extern "C" int af_document_data_file(AfContext* c,AfDocumentRef* d,uint32_t* out) noexcept {
    if(!out) return 1;
    try { int result=check_document(c,d); if(result) return result; if(!valid(c)) return 3;
        auto data=d->document->dataFile(); if(c->app->getLastError()!=0) return 7;
        *out=data?1:0; return 0;
    } catch(...) {return 4;}
}
extern "C" int af_scratch_profile(AfContext* c,AfDocumentRef* d,AfScratchProfile* out) noexcept {
    if(!out) return 1; *out={};
    try { Ptr<adsk::fusion::Design> design; int result=af_design(c,d,design); if(result) return result;
        auto type=design->designType(); if(c->app->getLastError()!=0) return 7;
        out->parametric=type==adsk::fusion::ParametricDesignType?1:0;
        auto intent=design->designIntent(); if(c->app->getLastError()!=0) return 7;
        out->hybrid=intent==adsk::fusion::HybridDesignIntentType?1:0;
        auto root=design->rootComponent(); if(c->app->getLastError()!=0 || !root) return 7;
#define AF_LIFECYCLE_COUNT(member, expression) {auto collection=(expression); if(c->app->getLastError()!=0 || !collection) return 7; out->member=collection->count(); if(c->app->getLastError()!=0) return 7;}
        AF_LIFECYCLE_COUNT(components,design->allComponents());
        AF_LIFECYCLE_COUNT(bodies,root->bRepBodies()); AF_LIFECYCLE_COUNT(sketches,root->sketches());
        AF_LIFECYCLE_COUNT(occurrences,root->occurrences()); AF_LIFECYCLE_COUNT(timeline,design->timeline());
        AF_LIFECYCLE_COUNT(graphics,root->customGraphicsGroups()); AF_LIFECYCLE_COUNT(mesh_bodies,root->meshBodies());
        AF_LIFECYCLE_COUNT(features,root->features()); AF_LIFECYCLE_COUNT(planes,root->constructionPlanes());
        AF_LIFECYCLE_COUNT(axes,root->constructionAxes()); AF_LIFECYCLE_COUNT(points,root->constructionPoints());
        AF_LIFECYCLE_COUNT(parameters,design->allParameters()); AF_LIFECYCLE_COUNT(user_parameters,design->userParameters());
#undef AF_LIFECYCLE_COUNT
        return 0;
    } catch(...) {return 4;}
}
extern "C" int af_document_discard(AfContext* c,AfDocumentRef* d,uint32_t* invoked,uint32_t* ack) noexcept {
    if(!invoked || !ack) return 1; *invoked=0; *ack=0;
    try { int result=check_document(c,d); if(result) return result; if(!valid(c)) return 3;
        if(!d->document->isValid() || c->app->getLastError()!=0) return 3;
        *invoked=1; *ack=d->document->close(false)?1:0;
        return c->app->getLastError()==0?0:7;
    } catch(...) {return 4;}
}
extern "C" int af_document_activate(AfContext* c,AfDocumentRef* d,uint32_t* invoked,uint32_t* ack) noexcept {
    if(!invoked || !ack) return 1; *invoked=0; *ack=0;
    try { int result=check_document(c,d); if(result) return result; if(!valid(c)) return 3;
        if(!d->document->isValid() || c->app->getLastError()!=0) return 3;
        *invoked=1; *ack=d->document->activate()?1:0;
        return c->app->getLastError()==0?0:7;
    } catch(...) {return 4;}
}
