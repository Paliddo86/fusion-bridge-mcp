//! Legacy contracts plus explicitly named native lifecycle extensions.
//! Classification is not a list of implemented native operations.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// `health` becomes a write only when `recompute` is true.
    Health,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationContract {
    pub name: &'static str,
    pub access: Access,
}

macro_rules! contracts {
    ($($name:literal => $access:ident),* $(,)?) => {
        pub const OPERATION_CONTRACTS: &[OperationContract] = &[
            $(OperationContract { name: $name, access: Access::$access }),*
        ];
    }
}

contracts! {
    "assembly_properties" => Read,
    "bom" => Read,
    "bom_export" => Write,
    "cad_import" => Write,
    "component_create" => Write,
    "component_metadata" => Write,
    "configuration_inspect" => Read,
    "construction_axis" => Write,
    "entity_delete" => Write,
    "exploded_preview_create" => Write,
    "exploded_preview_dispose" => Write,
    "exploded_preview_inspect" => Read,
    "exploded_preview_sample" => Write,
    "exploded_preview_show" => Write,
    "exploded_view_plan" => Read,
    "export" => Write,
    "feature_create" => Write,
    "feature_edit" => Write,
    "geometry_query" => Read,
    "health" => Health,
    "inspect" => Read,
    "inspect_entity" => Read,
    "interference" => Read,
    "joint_create" => Write,
    "joint_delete" => Write,
    "joint_edit" => Write,
    "joint_origin_create" => Write,
    "joint_origin_delete" => Write,
    "joint_origin_edit" => Write,
    "material_assign" => Write,
    "material_list" => Read,
    "mcmaster_discover" => Read,
    "mcmaster_open" => Write,
    "measure" => Read,
    "motion_link_create" => Write,
    "motion_link_delete" => Write,
    "motion_link_edit" => Write,
    "new_design" => Write,
    "native_scratch_discard" => Write,
    "native_document_activate" => Write,
    "native_sketch_inspect" => Read,
    "native_sketch_profiles" => Read,
    "native_sketch_entities" => Read,
    "native_sketch_line_state" => Read,
    "native_sketch_relations" => Read,
    "native_sketch_dimensions" => Read,
    "native_assembly_inspect" => Read,
    "occurrence_delete" => Write,
    "occurrence_insert" => Write,
    "occurrence_update" => Write,
    "parameter_batch" => Write,
    "parameter_create" => Write,
    "parameter_set" => Write,
    "plane_create" => Write,
    "regular_joint_create" => Write,
    "regular_joint_edit" => Write,
    "render_scene_inspect" => Read,
    "rigid_group_create" => Write,
    "rigid_group_delete" => Write,
    "rigid_group_edit" => Write,
    "sketch_constraint" => Write,
    "sketch_create" => Write,
    "sketch_curve_edit" => Write,
    "sketch_diagnostics" => Read,
    "sketch_dimension" => Write,
    "sketch_edit" => Write,
    "sketch_geometry" => Write,
    "sketch_offset" => Write,
    "sketch_project" => Write,
    "transaction_probe" => Write,
}

pub fn operation_contract(name: &str) -> Option<&'static OperationContract> {
    OPERATION_CONTRACTS.iter().find(|entry| entry.name == name)
}
