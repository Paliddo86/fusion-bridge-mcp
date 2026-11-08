#pragma once
#include <cstdint>
// SDK getter precondition: referencedEntity is applicable only for references.
// No setter, retry, error clearing or semantic reference comparison.
template<class Getter>void af_reference_presence(bool is_reference,Getter&& getter,uint32_t&out){
 out=0;
 if(is_reference){auto entity=getter();out=entity?1:0;}
}
