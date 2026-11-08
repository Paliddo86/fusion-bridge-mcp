#pragma once
// Typed SDK cast occurs at the call site. A null/wrong-type/foreign creator must
// not reach even the validation getters, let alone the later dimension setter.
template<class SketchPtr,class Validate>
int af_exact_dimension_creator(const SketchPtr& creator,const SketchPtr& retained,Validate&& validate) {
    if(!creator||!retained||creator!=retained)return 6;
    return validate(creator);
}
