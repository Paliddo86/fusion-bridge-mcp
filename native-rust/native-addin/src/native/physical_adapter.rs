use super::*;
use crate::physical::{Backend, Result, Snapshot, MAX_BODIES, MAX_FACES};
#[repr(C)]
#[derive(Default)]
struct State {
    root_bodies: u64,
    root_occurrences: u64,
    mesh_bodies: u64,
    face_count: u64,
    non_solid_bodies: u64,
    design_intent: u32,
    configured: u32,
    accuracy: u32,
    reserved: u32,
    mass_kg: f64,
    volume_cm3: f64,
    area_cm2: f64,
    center_cm: [f64; 3],
}
#[repr(C)]
struct Diagnostic {
    sdk_error: i32,
    stage: u32,
    length: u32,
    truncated: u32,
    text: [u8; 4096],
}
const _: () = assert!(
    std::mem::size_of::<State>() == 104
        && std::mem::offset_of!(State, mass_kg) == 56
        && std::mem::offset_of!(State, center_cm) == 80
);
const _: () = assert!(
    std::mem::size_of::<Diagnostic>() == 4112 && std::mem::offset_of!(Diagnostic, text) == 16
);
extern "C" {
    fn af_root_physical(
        c: *mut c_void,
        d: *mut c_void,
        max_bodies: u64,
        max_faces: u64,
        out: *mut State,
        sink: Sink,
        user: *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
}
pub(super) struct Adapter<'a> {
    pub context: &'a Context,
    pub document: &'a DocumentReference,
}
impl Backend for Adapter<'_> {
    fn snapshot(&self) -> Result<Snapshot> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut out = State::default();
        let mut fields = Fields::default();
        let mut diagnostic = Diagnostic {
            sdk_error: -1,
            stage: 0,
            length: 0,
            truncated: 0,
            text: [0; 4096],
        };
        let status = unsafe {
            af_root_physical(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                MAX_BODIES,
                MAX_FACES,
                &mut out,
                collect,
                (&mut fields as *mut Fields).cast(),
                &mut diagnostic,
            )
        };
        if status != 0 {
            let length = (diagnostic.length as usize).min(diagnostic.text.len());
            return Err(format!(
                "native_physical_sdk_failure:{}",
                json!({"abiStatus":status,"stage":diagnostic.stage,"sdkError":diagnostic.sdk_error,"description":String::from_utf8_lossy(&diagnostic.text[..length]),"truncated":diagnostic.truncated!=0})
            ));
        }
        if out.configured > 1 || out.reserved != 0 {
            return Err("native_physical_invalid_sdk_flags".into());
        }
        Ok(Snapshot {
            revision_before: field(&mut fields, 0)?,
            revision_after: field(&mut fields, 1)?,
            design_intent: out.design_intent,
            configured: out.configured == 1,
            root_bodies: out.root_bodies,
            root_occurrences: out.root_occurrences,
            mesh_bodies: out.mesh_bodies,
            face_count: out.face_count,
            non_solid_bodies: out.non_solid_bodies,
            accuracy: out.accuracy,
            mass_kg: out.mass_kg,
            volume_cm3: out.volume_cm3,
            area_cm2: out.area_cm2,
            center_cm: out.center_cm,
        })
    }
}
