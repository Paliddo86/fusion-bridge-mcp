#pragma once
#include <cstdint>
#include <cstddef>
struct EoContext;
using EoBytes=int(*)(void*,uint32_t,const char*,size_t);
struct EoFacts{uint32_t saved,modified,active,has_data_file;};
struct EoInit{uint64_t components,occurrences;};
struct EoDiagnostic{int32_t sdk_error;uint32_t stage,length,truncated;uint8_t text[4096];};
struct EoAssemblyFacts{uint64_t root_counts[18],component_counts[2][13];uint32_t flags[6],occurrence_flags[3][7];uint64_t children[3];uint32_t component_slots[3],same_component[9];double matrices[3][16];uint64_t snapshot_count;int64_t snapshot_indexes[2];uint32_t snapshot_health[2],snapshot_flags[2][2];};
