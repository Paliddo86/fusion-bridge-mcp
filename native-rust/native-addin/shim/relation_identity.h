#pragma once
// Both typed SDK branches use the same guard. The exact returned typed wrapper
// must remain held; an equivalent constraint on the same line is insufficient.
template<class RelationPtr,class LinePtr>
int af_exact_relation_line(const RelationPtr& observed,const RelationPtr& retained,LinePtr& line) {
    if(!observed||!retained||observed!=retained)return 6;
    line=observed->line();
    return 0;
}
