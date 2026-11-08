#pragma once
#include <cstdint>
// Call only after complete sole-SketchLine, three-point and zero-text inventory.
// This derives curve incidence from retained typed identities. It does not
// classify an arbitrary graph or interpret a nullable SDK collection.
template<class Point>bool af_isolated_line_incidence(const Point&start,const Point&end,const Point&origin,const Point&point,uint64_t&out){
 out=0;
 if(!start||!end||!origin||!point||start==end||start==origin||end==origin)return false;
 if(point==start||point==end){out=1;return true;}
 return point==origin;
}
