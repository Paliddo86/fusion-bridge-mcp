use super::*;
use crate::sketches::line_state::{Flags, LineBackend, Point, State};
#[repr(C)]
#[derive(Default)]
struct RawFlags {
    fixed: u32,
    reference: u32,
    has_reference: u32,
    fully_constrained: u32,
    is_2d: u32,
    constraints: u64,
    dimensions: u64,
    attributes: u64,
}
#[repr(C)]
#[derive(Default)]
struct RawPoint {
    position: [f64; 3],
    flags: RawFlags,
    connected: u64,
}
#[repr(C)]
#[derive(Default)]
pub(super) struct RawState {
    line: RawFlags,
    points: [RawPoint; 3],
    bodies: u64,
    features: u64,
    occurrences: u64,
    meshes: u64,
    components: u64,
    texts: u64,
    attributes: u64,
    construction: u32,
}
const _: () = assert!(
    std::mem::size_of::<RawFlags>() == 48 && std::mem::offset_of!(RawFlags, constraints) == 24
);
const _: () =
    assert!(std::mem::size_of::<RawPoint>() == 80 && std::mem::offset_of!(RawPoint, flags) == 24);
const _: () = assert!(
    std::mem::size_of::<RawState>() == 352 && std::mem::offset_of!(RawState, construction) == 344
);
extern "C" {
    fn af_line_state(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        out: *mut RawState,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
}
fn flags(f: RawFlags) -> Result<Flags, String> {
    Ok(Flags {
        fixed: flag(f.fixed)?,
        reference: flag(f.reference)?,
        has_referenced_entity: flag(f.has_reference)?,
        fully_constrained: flag(f.fully_constrained)?,
        is_2d: flag(f.is_2d)?,
        constraints: f.constraints,
        dimensions: f.dimensions,
        attributes: f.attributes,
    })
}
fn point(p: RawPoint) -> Result<Point, String> {
    Ok(Point {
        position_cm: p.position,
        flags: flags(p.flags)?,
        connected_entities: p.connected,
    })
}
impl LineBackend for Adapter<'_> {
    fn line_state(&self, handle: &Handle, lease: u64) -> Result<State, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawState::default();
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_line_state(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                lease,
                &mut raw,
                &mut diag,
            )
        };
        if status != 0 {
            let diagnostic = crate::sketches::extrusion::sdk_diagnostic(
                status,
                diag.stage,
                diag.sdk_error,
                &diag.text,
                diag.length as usize,
                diag.truncated != 0,
            );
            self.context.extrusion_diagnostic.replace(diagnostic);
            return Err(format!(
                "native_line_sdk_read_failed_status_{status}_stage_{}_error_{}",
                diag.stage, diag.sdk_error
            ));
        }
        decode(raw)
    }
}
pub(super) fn decode(raw: RawState) -> Result<State, String> {
    let [start, end, origin] = raw.points;
    Ok(State {
        line: flags(raw.line)?,
        construction: flag(raw.construction)?,
        start: point(start)?,
        end: point(end)?,
        origin: point(origin)?,
        root_bodies: raw.bodies,
        root_features: raw.features,
        root_occurrences: raw.occurrences,
        root_meshes: raw.meshes,
        design_components: raw.components,
        sketch_texts: raw.texts,
        sketch_attributes: raw.attributes,
    })
}
