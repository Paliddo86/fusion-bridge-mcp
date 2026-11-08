//! adom-eda-checks: the EDA Bridge Standard's checks tier, computed from the board model and its companions as plain
//! JSON, plus board-file writers (`writers`: GenCAD, DXF, SVG to regions) for a bridge whose tool has no native
//! exporter. No bridge calls, no tool, no I/O: a bridge links this crate (or runs the `adom-eda-checks` command) and
//! answers the verb under its own prefix (`kicad_return_path`). Replies keep the standard envelope (`success`,
//! `errorCode`, `error`, `_hint`) and name verbs without a prefix.
//!
//! Every check takes one input bundle, `{board, stack?, pours?, rules?, silk?, fab?, bodies?, prices?, sheetParts?, netlist?, sheetDrawing?, library?, args?}` (see `input`).
use serde_json::Value;

pub mod assembly;
pub mod autoclass;
pub mod bodies;
pub mod bom;
pub mod coppermap;
pub mod cost;
pub mod current;
pub mod fab;
pub mod features;
pub mod geom;
pub mod impedance;
pub mod input;
pub mod landpattern;
pub mod lengths;
pub mod lengthtune;
pub mod liblint;
pub mod pairroute;
pub mod panel;
pub mod report;
pub mod review;
pub mod placement;
pub mod placetools;
pub mod route;
pub mod schreview;
pub mod silkplace;
pub mod space;
pub mod teardrops;
pub mod writers;

/// This engine's version. Every reply carries it as `engine` ("adom-eda-checks 0.3.0"), so a bridge never writes it by hand
/// (kicad-lane's replies said 0.1.2 for three releases after re-vendoring).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The checks this build answers, by standard verb name.
pub const CHECKS: &[&str] = &["assembly_check", "auto_classes", "board_report", "bom", "check_clearance", "check_orientation", "copper_balance", "cross_check", "current_capacity", "estimate_cost", "fab_check", "find_space", "footprint_from_land_pattern", "height_check", "impedance", "length_tune", "lint_library", "net_lengths", "panelize", "pin_pad_check", "place_decoupling", "placement_check", "plan_fanout", "plan_route", "return_path", "review_board", "route_pair", "schematic_review", "silk_designators", "teardrops", "thermal_vias", "via_fence"];

/// Run one check or writer by its standard name. The reply carries `engine`.
pub fn run(check: &str, input: &Value) -> Value {
    let mut out = answer(check, input);
    if let Some(o) = out.as_object_mut() { o.entry("engine").or_insert(Value::String(format!("adom-eda-checks {VERSION}"))); }
    out
}

fn answer(check: &str, input: &Value) -> Value {
    if let Some(out) = writers::run(check, input) { return out; }
    match check {
        "assembly_check" => assembly::assembly_check(input),
        "auto_classes" => autoclass::auto_classes(input),
        "board_report" => report::board_report(input),
        "bom" => bom::bom(input),
        "check_clearance" => placetools::check_clearance(input),
        "check_orientation" => space::check_orientation(input),
        "copper_balance" => coppermap::copper_balance(input),
        "cross_check" => bom::cross_check(input),
        "current_capacity" => current::current_capacity(input),
        "estimate_cost" => cost::estimate_cost(input),
        "find_space" => space::find_space(input),
        "footprint_from_land_pattern" => landpattern::footprint_from_land_pattern(input),
        "fab_check" => fab::fab_check(input),
        "height_check" => bodies::height_check(input),
        "impedance" => impedance::impedance(input),
        "length_tune" => lengthtune::length_tune(input),
        "lint_library" => liblint::lint_library(input),
        "net_lengths" => lengths::net_lengths(input),
        "panelize" => panel::panelize(input),
        "pin_pad_check" => schreview::pin_pad_check(input),
        "place_decoupling" => placetools::place_decoupling(input),
        "plan_fanout" => placetools::plan_fanout(input),
        "placement_check" => placement::placement_check(input),
        "plan_route" => route::plan_route(input),
        "return_path" => coppermap::return_path(input),
        "review_board" => review::review_board(input),
        "route_pair" => pairroute::route_pair(input),
        "schematic_review" => schreview::schematic_review(input),
        "silk_designators" => silkplace::silk_designators(input),
        "teardrops" => teardrops::teardrops(input),
        "thermal_vias" => features::thermal_vias(input),
        "via_fence" => features::via_fence(input),
        _ => input::refuse("not_supported", &format!("{check} is not a verb this engine answers (checks: {}; writers: {})", CHECKS.join(", "), writers::WRITERS.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_reply_names_the_engine_version() {
        let r = super::run("copper_balance", &serde_json::json!({}));
        assert_eq!(r["engine"], format!("adom-eda-checks {}", super::VERSION));
        assert_eq!(super::run("no_such_check", &serde_json::json!({}))["engine"], r["engine"]);
    }
}
