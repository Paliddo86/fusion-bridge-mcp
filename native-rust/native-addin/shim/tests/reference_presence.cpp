#include "../reference_presence.h"
#include <cassert>
#include <iostream>
#include <stdexcept>
int main(){
 uint32_t out=99;int calls=0;int entity=1;
 af_reference_presence(false,[&]()->int*{++calls;throw std::runtime_error("getter must not run for ordinary geometry");},out);
 assert(calls==0&&out==0);
 af_reference_presence(true,[&]()->int*{++calls;return nullptr;},out);assert(calls==1&&out==0);
 af_reference_presence(true,[&]()->int*{++calls;return &entity;},out);assert(calls==2&&out==1);
 bool propagated=false;try{af_reference_presence(true,[&]()->int*{++calls;throw std::runtime_error("preserve SDK failure");},out);}catch(const std::runtime_error&){propagated=true;}
 assert(propagated&&calls==3&&out==0);
 std::cout<<"conditional reference getter precondition/no-call/null/present/failure: PASS\n";
}
