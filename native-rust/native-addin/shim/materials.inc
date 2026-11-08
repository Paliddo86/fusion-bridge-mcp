// Typed synchronous catalog reads only. Rust owns bounds, selection validation,
// stability, search, revisions and JSON. No library loading or material copying.
#include <Core/Materials/MaterialLibraries.h>
#include <Core/Materials/MaterialLibrary.h>
#include <Core/Materials/Materials.h>
#include <Core/Materials/Material.h>
static int af_material_libraries(AfContext* c, AfDocumentRef* d, Ptr<adsk::core::MaterialLibraries>& out) {
    Ptr<adsk::fusion::Design> design;
    int status=af_design(c,d,design);if(status)return status;
    out=c->app->materialLibraries();
    if(c->app->getLastError()!=0||!out)return 7;
    if(!out->isValid()||c->app->getLastError()!=0)return 3;
    return 0;
}
static int af_material_source(AfContext* c,AfDocumentRef* d,const char* id,size_t len,Ptr<adsk::core::Materials>& out) {
    if(!af_input(id,len))return 1;
    Ptr<adsk::fusion::Design> design;
    int status=af_design(c,d,design);if(status)return status;
    if(len) {
        Ptr<adsk::core::MaterialLibraries> libraries;
        status=af_material_libraries(c,d,libraries);if(status)return status;
        auto library=libraries->itemById(af_string(id,len));
        if(c->app->getLastError()!=0||!library)return 7;
        if(!library->isValid()||c->app->getLastError()!=0)return 3;
        out=library->materials();
    }else {out=design->materials();}
    if(c->app->getLastError()!=0||!out)return 7;
    if(!out->isValid()||c->app->getLastError()!=0)return 3;
    return 0;
}
extern "C" int af_material_library_count(AfContext* c,AfDocumentRef* d,uint64_t* out) noexcept {
    if(!out)return 1;
    try {Ptr<adsk::core::MaterialLibraries> libraries;int status=af_material_libraries(c,d,libraries);if(status)return status;
        *out=libraries->count();return c->app->getLastError()==0?0:7;
    }catch(...){return 4;}
}
extern "C" int af_material_library_read(AfContext* c,AfDocumentRef* d,uint64_t index,uint64_t* count,AfBytes sink,void* user) noexcept {
    if(!count||!sink||index>INT_MAX)return 1;
    try {Ptr<adsk::core::MaterialLibraries> libraries;int status=af_material_libraries(c,d,libraries);if(status)return status;
        auto size=libraries->count();if(c->app->getLastError()!=0)return 7;if(index>=static_cast<uint64_t>(size))return 6;
        auto library=libraries->item(static_cast<int>(index));if(c->app->getLastError()!=0||!library)return 7;
        if(!library->isValid()||c->app->getLastError()!=0)return 3;
        auto id=library->id();if(c->app->getLastError()!=0)return 7;
        auto name=library->name();if(c->app->getLastError()!=0)return 7;
        auto materials=library->materials();if(c->app->getLastError()!=0||!materials)return 7;
        if(!materials->isValid()||c->app->getLastError()!=0)return 3;
        auto material_count=materials->count();if(c->app->getLastError()!=0||material_count<0)return 7;*count=static_cast<uint64_t>(material_count);
        if(!sink(user,0,id.data(),id.size())||!sink(user,1,name.data(),name.size()))return 5;return 0;
    }catch(...){return 4;}
}
extern "C" int af_material_count(AfContext* c,AfDocumentRef* d,const char* id,size_t len,uint64_t* out) noexcept {
    if(!out)return 1;
    try {Ptr<adsk::core::Materials> materials;int status=af_material_source(c,d,id,len,materials);if(status)return status;
        *out=materials->count();return c->app->getLastError()==0?0:7;
    }catch(...){return 4;}
}
extern "C" int af_material_read(AfContext* c,AfDocumentRef* d,const char* id,size_t len,uint64_t index,AfBytes sink,void* user) noexcept {
    if(!sink||index>INT_MAX)return 1;
    try {Ptr<adsk::core::Materials> materials;int status=af_material_source(c,d,id,len,materials);if(status)return status;
        auto count=materials->count();if(c->app->getLastError()!=0)return 7;if(index>=static_cast<uint64_t>(count))return 6;
        auto material=materials->item(static_cast<int>(index));if(c->app->getLastError()!=0||!material)return 7;
        if(!material->isValid()||c->app->getLastError()!=0)return 3;
        auto material_id=material->id();if(c->app->getLastError()!=0)return 7;
        auto name=material->name();if(c->app->getLastError()!=0)return 7;
        if(!sink(user,0,material_id.data(),material_id.size())||!sink(user,1,name.data(),name.size()))return 5;return 0;
    }catch(...){return 4;}
}
