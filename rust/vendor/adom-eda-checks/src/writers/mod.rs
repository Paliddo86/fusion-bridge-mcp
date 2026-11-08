//! Board-file writers for a bridge whose tool has no native exporter (EDA Bridge Standard #16): GenCAD, DXF and IPC-2581C
//! from the board model, SVG artwork as regions for a layer, and a part's 3D body as STEP from its package dimensions (#20). Like the checks, no I/O and no tool: each returns the file text
//! or the regions as JSON, says it was written from the board model (`writtenFrom`), and the bridge writes the files or
//! places the regions. A bridge whose tool has a native exporter keeps it.
use serde_json::Value;

pub mod body3d;
pub mod dxf;
pub mod gencad;
pub mod ipc2581;
pub mod svgart;

/// The verbs these writers answer, by standard name.
pub const WRITERS: &[&str] = &["export_dxf", "export_gencad", "export_ipc2581", "generate_body", "import_svg"];

pub use dxf::dxf_layers;
pub use gencad::gencad;
pub use svgart::svg_to_regions;

/// Run one writer by its standard verb name (None for a name that is not a writer).
pub fn run(verb: &str, input: &Value) -> Option<Value> {
    Some(match verb {
        "export_dxf" => dxf_layers(input),
        "export_gencad" => gencad(input),
        "export_ipc2581" => ipc2581::export_ipc2581(input),
        "generate_body" => body3d::generate_body(input),
        "import_svg" => svg_to_regions(input),
        _ => return None,
    })
}
