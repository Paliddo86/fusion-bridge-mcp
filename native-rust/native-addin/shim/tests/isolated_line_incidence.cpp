#include "../isolated_line_incidence.h"
#include <cassert>
int main(){
 int a=1,b=2,c=3,d=4;int *start=&a,*end=&b,*origin=&c,*foreign=&d,*nil=nullptr;
 uint64_t count=99;
 assert(af_isolated_line_incidence(start,end,origin,start,count)&&count==1);
 assert(af_isolated_line_incidence(start,end,origin,end,count)&&count==1);
 assert(af_isolated_line_incidence(start,end,origin,origin,count)&&count==0);
 assert(!af_isolated_line_incidence(start,end,origin,foreign,count)&&count==0);
 assert(!af_isolated_line_incidence(start,end,origin,nil,count)&&count==0);
 assert(!af_isolated_line_incidence(nil,end,origin,end,count)&&count==0);
 assert(!af_isolated_line_incidence(start,nil,origin,start,count)&&count==0);
 assert(!af_isolated_line_incidence(start,end,nil,start,count)&&count==0);
 assert(!af_isolated_line_incidence(start,start,origin,start,count)&&count==0);
 assert(!af_isolated_line_incidence(start,end,start,start,count)&&count==0);
 assert(!af_isolated_line_incidence(start,end,end,start,count)&&count==0);
 // Coordinates are immaterial: pointer identity, not coincident coordinates.
 int same_value=1;int*coincident=&same_value;
 assert(!af_isolated_line_incidence(start,end,origin,coincident,count)&&count==0);
}
