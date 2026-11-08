// Typed geometry effects and exact-reference readback only. Rust owns plan policy.
#include <Core/Geometry/Point3D.h>
#include <Core/Geometry/Line3D.h>
#include <Core/Geometry/Circle3D.h>
#include <Core/Geometry/Arc3D.h>
#include <Core/Geometry/Vector3D.h>
#include <Core/Application/ObjectCollection.h>
#include <Fusion/Sketch/SketchPoint.h>
#include <Fusion/Sketch/SketchPointList.h>
#include <Fusion/Sketch/SketchLine.h>
#include <Fusion/Sketch/SketchLines.h>
#include <Fusion/Sketch/SketchCircle.h>
#include <Fusion/Sketch/SketchCircles.h>
#include <Fusion/Sketch/SketchArc.h>
#include <Fusion/Sketch/SketchArcs.h>
#include <Fusion/Sketch/SketchFittedSpline.h>
#include <Fusion/Sketch/SketchFittedSplines.h>
#include <Fusion/Sketch/SketchTexts.h>
#include <cmath>
static int af_geometry_sink(void*,uint32_t,const char*,size_t) {return 1;}
static int af_geometry_check(AfContext*c,AfDocumentRef*d,AfSketchRef*s){
    AfSketchState state{};
    int result=af_sketch_read(c,d,s,&state,af_geometry_sink,nullptr);if(result)return result;
    auto active=c->app->activeDocument();if(c->app->getLastError()!=0||active!=s->document)return 7;
    return 0;
}
extern "C" int af_geometry_empty(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint32_t*out) noexcept {
    if(!out)return 1;*out=0;
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        if(s->geometry_count!=0)return 0;
        auto texts=s->sketch->sketchTexts();if(c->app->getLastError()!=0||!texts)return 7;
        auto nt=texts->count();if(c->app->getLastError()!=0)return 7;if(nt!=0)return 0;
        auto points=s->sketch->sketchPoints();if(c->app->getLastError()!=0||!points)return 7;
        auto n=points->count();if(c->app->getLastError()!=0)return 7;if(n>1)return 0;
        auto origin=s->sketch->originPoint();if(c->app->getLastError()!=0||!origin)return 7;
        if(n==1){auto point=points->item(0);if(c->app->getLastError()!=0||!point||point!=origin)return 7;}
        *out=1;return 0;
    }catch(...){return 4;}
}
extern "C" int af_geometry_insert(AfContext*c,AfDocumentRef*d,AfSketchRef*s,const AfSketchGeometry*in,uint64_t*lease,uint32_t*invoked,uint32_t*stage) noexcept {
    if(!in||!lease||!invoked||!stage)return 1;
    *lease=UINT64_MAX;*invoked=0;*stage=1;
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        if(in->kind<1||in->kind>5||in->point_count>128||in->construction>2||in->closed>1||s->geometry_count>=64)return 6;
        if((in->kind==1&&in->point_count!=1)||(in->kind==2&&in->point_count!=2)||(in->kind==3&&in->point_count!=1)||(in->kind==4&&in->point_count!=2)||(in->kind==5&&in->point_count<2))return 6;
        std::array<Ptr<adsk::core::Point3D>,128> points;
        for(uint32_t i=0;i<in->point_count;++i){
            points[i]=adsk::core::Point3D::create(in->points[3*i],in->points[3*i+1],in->points[3*i+2]);
            if(c->app->getLastError()!=0||!points[i])return 7;
        }
        auto curves=s->sketch->sketchCurves();if(c->app->getLastError()!=0||!curves)return 7;
        // A fixed empty slot is reserved before invocation. Every returned SDK
        // reference is assigned directly into this parent-owned slot, including
        // setter/readback failures; no temporary registry or new allocation owns it.
        const uint64_t index=s->geometry_count++;
        auto& kept=s->geometry_entities[index];s->geometry_kinds[index]=in->kind;
        if(in->kind==1){
            auto collection=s->sketch->sketchPoints();if(c->app->getLastError()!=0||!collection)return 7;
            *stage=2;*invoked=1;kept=collection->add(points[0]);
        }else if(in->kind==2){
            auto collection=curves->sketchLines();if(c->app->getLastError()!=0||!collection)return 7;
            *stage=2;*invoked=1;kept=collection->addByTwoPoints(points[0],points[1]);
        }else if(in->kind==3){
            auto collection=curves->sketchCircles();if(c->app->getLastError()!=0||!collection)return 7;
            *stage=2;*invoked=1;kept=collection->addByCenterRadius(points[0],in->scalar);
        }else if(in->kind==4){
            auto collection=curves->sketchArcs();if(c->app->getLastError()!=0||!collection)return 7;
            *stage=2;*invoked=1;kept=collection->addByCenterStartSweep(points[0],points[1],in->scalar);
        }else{
            auto collection=curves->sketchFittedSplines();if(c->app->getLastError()!=0||!collection)return 7;
            auto input=adsk::core::ObjectCollection::create();if(c->app->getLastError()!=0||!input)return 7;
            for(uint32_t i=0;i<in->point_count;++i){bool ok=input->add(points[i]);if(c->app->getLastError()!=0||!ok)return 7;}
            *stage=2;*invoked=1;kept=collection->add(input);
        }
        if(c->app->getLastError()!=0||!kept)return 7;
        *lease=index;
        if(in->kind!=1&&in->construction!=0){
            Ptr<adsk::fusion::SketchCurve> curve=kept;if(!curve)return 7;
            *stage=3;bool ok=curve->isConstruction(in->construction==2);if(c->app->getLastError()!=0||!ok)return 7;
        }
        if(in->kind==5&&in->closed==1){
            Ptr<adsk::fusion::SketchFittedSpline> spline=kept;if(!spline)return 7;
            *stage=4;bool ok=spline->isClosed(true);if(c->app->getLastError()!=0||!ok)return 7;
        }
        *stage=5;return 0;
    }catch(...){return 4;}
}
static bool af_geometry_point(AfContext*c,const Ptr<adsk::core::Point3D>&p,double*out){
    if(!p)return false;
    auto xyz=p->asArray();if(c->app->getLastError()!=0||xyz.size()!=3)return false;
    for(size_t i=0;i<3;++i){if(!std::isfinite(xyz[i]))return false;out[i]=xyz[i];}return true;
}
static bool af_geometry_normal(AfContext*c,const Ptr<adsk::core::Vector3D>&p,double*out){
    if(!p)return false;
    auto xyz=p->asArray();if(c->app->getLastError()!=0||xyz.size()!=3)return false;
    for(size_t i=0;i<3;++i){if(!std::isfinite(xyz[i]))return false;out[i]=xyz[i];}return true;
}
extern "C" int af_geometry_read(AfContext*c,AfDocumentRef*d,AfSketchRef*s,uint64_t lease,AfSketchGeometry*out) noexcept {
    if(!out)return 1;*out=AfSketchGeometry{};
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        if(lease>=s->geometry_count||lease>=64)return 6;
        auto& kept=s->geometry_entities[lease];if(!kept)return 7;
        bool is_valid=kept->isValid();if(c->app->getLastError()!=0||!is_valid)return 7;
        auto parent=kept->parentSketch();if(c->app->getLastError()!=0||parent!=s->sketch)return 7;
        out->reference=kept->isReference()?1:0;if(c->app->getLastError()!=0)return 7;
        out->kind=s->geometry_kinds[lease];
        bool member=false;
        if(out->kind==1){
            auto collection=s->sketch->sketchPoints();if(c->app->getLastError()!=0||!collection)return 7;
            auto n=collection->count();if(c->app->getLastError()!=0||n>16384)return 7;
            for(size_t i=0;i<n;++i){Ptr<adsk::fusion::SketchEntity> current=collection->item(i);if(c->app->getLastError()!=0||!current)return 7;if(current==kept)member=true;}
        }else{
            auto collection=s->sketch->sketchCurves();if(c->app->getLastError()!=0||!collection)return 7;
            auto n=collection->count();if(c->app->getLastError()!=0||n>16384)return 7;
            for(size_t i=0;i<n;++i){Ptr<adsk::fusion::SketchEntity> current=collection->item(i);if(c->app->getLastError()!=0||!current)return 7;if(current==kept)member=true;}
            Ptr<adsk::fusion::SketchCurve> curve=kept;if(!curve)return 7;
            out->construction=curve->isConstruction()?2:1;if(c->app->getLastError()!=0)return 7;
        }
        if(!member)return 7;
        if(out->kind==1){
            Ptr<adsk::fusion::SketchPoint> point=kept;if(!point)return 7;
            auto p=point->geometry();if(c->app->getLastError()!=0||!af_geometry_point(c,p,out->points))return 7;out->point_count=1;
        }else if(out->kind==2){
            Ptr<adsk::fusion::SketchLine> line=kept;if(!line)return 7;
            auto g=line->geometry();if(c->app->getLastError()!=0||!g)return 7;
            auto a=g->startPoint();if(c->app->getLastError()!=0||!af_geometry_point(c,a,out->points))return 7;
            auto b=g->endPoint();if(c->app->getLastError()!=0||!af_geometry_point(c,b,out->points+3))return 7;out->point_count=2;
        }else if(out->kind==3){
            Ptr<adsk::fusion::SketchCircle> circle=kept;if(!circle)return 7;
            auto g=circle->geometry();if(c->app->getLastError()!=0||!g)return 7;
            auto center=g->center();if(c->app->getLastError()!=0||!af_geometry_point(c,center,out->points))return 7;
            out->scalar=g->radius();if(c->app->getLastError()!=0)return 7;
            auto normal=g->normal();if(c->app->getLastError()!=0||!af_geometry_normal(c,normal,out->normal))return 7;out->point_count=1;
        }else if(out->kind==4){
            Ptr<adsk::fusion::SketchArc> arc=kept;if(!arc)return 7;
            auto g=arc->geometry();if(c->app->getLastError()!=0||!g)return 7;
            auto center=g->center();if(c->app->getLastError()!=0||!af_geometry_point(c,center,out->points))return 7;
            auto a=g->startPoint();if(c->app->getLastError()!=0||!af_geometry_point(c,a,out->points+3))return 7;
            auto b=g->endPoint();if(c->app->getLastError()!=0||!af_geometry_point(c,b,out->points+6))return 7;
            const double start=g->startAngle();if(c->app->getLastError()!=0)return 7;
            const double end=g->endAngle();if(c->app->getLastError()!=0)return 7;out->scalar=end-start;
            auto normal=g->normal();if(c->app->getLastError()!=0||!af_geometry_normal(c,normal,out->normal))return 7;out->point_count=3;
        }else if(out->kind==5){
            Ptr<adsk::fusion::SketchFittedSpline> spline=kept;if(!spline)return 7;
            out->closed=spline->isClosed()?1:0;if(c->app->getLastError()!=0)return 7;
            auto points=spline->fitPoints();if(c->app->getLastError()!=0||!points)return 7;
            auto n=points->count();if(c->app->getLastError()!=0||n<2||n>128)return 7;out->point_count=static_cast<uint32_t>(n);
            for(size_t i=0;i<n;++i){auto p=points->item(i);if(c->app->getLastError()!=0||!p)return 7;auto g=p->geometry();if(c->app->getLastError()!=0||!af_geometry_point(c,g,out->points+3*i))return 7;}
        }else{return 6;}
        return std::isfinite(out->scalar)?0:7;
    }catch(...){return 4;}
}

// One bounded typed equality observation, no new holders or caller selectors.
extern "C" int af_geometry_equalities(AfContext*c,AfDocumentRef*d,AfSketchRef*s,const uint64_t*leases,uint32_t n,uint8_t*out) noexcept {
    if(!out||!leases||n==0||n>64)return 1;
    for(uint32_t i=0;i<n*n;++i)out[i]=0;
    try{
        int result=af_geometry_check(c,d,s);if(result)return result;
        for(uint32_t i=0;i<n;++i){
            if(leases[i]>=s->geometry_count||leases[i]>=64)return 6;
            auto& x=s->geometry_entities[leases[i]];if(!x)return 7;
            bool valid=x->isValid();if(c->app->getLastError()!=0||!valid)return 7;
            auto parent=x->parentSketch();if(c->app->getLastError()!=0||parent!=s->sketch)return 7;
        }
        for(uint32_t i=0;i<n;++i)for(uint32_t j=0;j<n;++j)
            out[i*n+j]=s->geometry_entities[leases[i]]==s->geometry_entities[leases[j]]?1:0;
        return 0;
    }catch(...){return 4;}
}
