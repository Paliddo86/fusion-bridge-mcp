// Pure guard execution on Linux or Windows, no SDK/app objects or effects.
#include "../relation_identity.h"
#include <cassert>
#include <iostream>
struct Line {};
struct Horizontal {Line* same_line;int calls=0;Line* line(){++calls;return same_line;}};
struct Vertical {Line* same_line;int calls=0;Line* line(){++calls;return same_line;}};
template<class T>void verify(){
 Line line;T retained{&line}, substitute{&line};Line*out=nullptr;
 assert(af_exact_relation_line(&retained,&retained,out)==0&&out==&line);
 assert(retained.calls==1);out=nullptr;
 // Same constrained line cannot substitute another typed returned holder.
 assert(af_exact_relation_line(&substitute,&retained,out)==6&&out==nullptr);
 assert(substitute.calls==0&&retained.calls==1);
 T* absent=nullptr;
 assert(af_exact_relation_line(absent,&retained,out)==6&&out==nullptr);
 assert(af_exact_relation_line(&retained,absent,out)==6&&out==nullptr);
}
int main(){verify<Horizontal>();verify<Vertical>();std::cout<<"horizontal+vertical exact typed holder guard: PASS\n";}
