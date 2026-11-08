#include "../dimension_creator.h"
#include <cassert>
#include <stdexcept>
struct Sketch { bool valid; bool native; };
struct Base { enum Kind { Other, SketchKind } kind; Sketch* sketch; };
static Sketch* typed(const Base& b){return b.kind==Base::SketchKind?b.sketch:nullptr;}
int main(){
 Sketch owned{true,true},foreign{true,true};
 Sketch*retained=&owned;
 int validations=0,setters=0;
 auto execute=[&](Sketch*creator,int getter_error=0){
  int r=af_exact_dimension_creator(creator,retained,[&](Sketch*p){
   ++validations;
   if(getter_error) return getter_error;
   if(!p->valid)return 3;
   return p->native?0:6;
  });
  if(r==0)++setters;
  return r;
 };
 assert(execute(nullptr)==6&&validations==0&&setters==0);
 assert(execute(typed({Base::Other,&owned}))==6&&validations==0&&setters==0);
 assert(execute(&foreign)==6&&validations==0&&setters==0);
 retained=nullptr;assert(execute(&owned)==6&&validations==0&&setters==0);retained=&owned;
 owned.valid=false;assert(execute(&owned)==3&&setters==0);
 owned.valid=true;owned.native=false;assert(execute(&owned)==6&&setters==0);
 owned.native=true;assert(execute(&owned,7)==7&&setters==0);
 bool caught=false;
 try{af_exact_dimension_creator(&owned,retained,[](Sketch*)->int{throw std::runtime_error("getter");});}
 catch(const std::runtime_error&){caught=true;}assert(caught&&setters==0);
 assert(execute(typed({Base::SketchKind,&owned}))==0&&setters==1);
}
