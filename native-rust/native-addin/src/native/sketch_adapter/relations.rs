use super::*;
use crate::sketches::relations::{Attempt, RelationBackend, RelationState};
#[repr(C)]
#[derive(Default)]
pub(super) struct RawRelation {
    pub(super) line: line_state::RawState,
    pub(super) kind: u32,
}
const _: () = assert!(
    std::mem::size_of::<RawRelation>() == 360 && std::mem::offset_of!(RawRelation, kind) == 352
);
extern "C" {
    fn af_relation_state(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        out: *mut RawRelation,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_relation_add(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        kind: u32,
        invoked: *mut u32,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
}
impl RelationBackend for Adapter<'_> {
    fn clear_relation_diagnostic(&self) {
        self.context.extrusion_diagnostic.replace(Value::Null);
    }
    fn relation_diagnostic(&self) -> Value {
        self.context.extrusion_diagnostic.borrow().clone()
    }
    fn relation_state(&self, h: &Handle, lease: u64) -> Result<RelationState, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawRelation::default();
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_relation_state(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                lease,
                &mut raw,
                &mut diag,
            )
        };
        if status != 0 {
            self.context
                .extrusion_diagnostic
                .replace(crate::sketches::extrusion::sdk_diagnostic(
                    status,
                    diag.stage,
                    diag.sdk_error,
                    &diag.text,
                    diag.length as usize,
                    diag.truncated != 0,
                ));
            return Err(format!(
                "native_relation_sdk_read_failed_status_{status}_stage_{}_error_{}",
                diag.stage, diag.sdk_error
            ));
        }
        Ok(RelationState {
            kind: raw.kind,
            line: line_state::decode(raw.line)?,
        })
    }
    fn constrain(&self, h: &Handle, lease: u64, kind: u32) -> Attempt {
        if self.context.close.ensure_open().is_err()
            || self.context.child_leases.ensure_clear().is_err()
        {
            return Attempt {
                invoked: false,
                acknowledged: false,
            };
        }
        let mut invoked = 0;
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_relation_add(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                lease,
                kind,
                &mut invoked,
                &mut diag,
            )
        };
        if status != 0 {
            self.context
                .extrusion_diagnostic
                .replace(crate::sketches::extrusion::sdk_diagnostic(
                    status,
                    diag.stage,
                    diag.sdk_error,
                    &diag.text,
                    diag.length as usize,
                    diag.truncated != 0,
                ));
        }
        Attempt {
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1,
        }
    }
}
