use super::*;
use crate::sketches::{
    dimensions::{Dimension, DimensionBackend, State},
    relations::{Attempt, RelationState},
};
#[repr(C)]
#[derive(Default)]
struct RawDimension {
    relation: relations::RawRelation,
    model_parameters: u64,
    user_parameters: u64,
    present: u32,
    driving: u32,
    aligned: u32,
    reserved: u32,
    value: f64,
    parameter_value: f64,
    text: [f64; 3],
}
const _: () = assert!(
    std::mem::size_of::<RawDimension>() == 432
        && std::mem::offset_of!(RawDimension, model_parameters) == 360
        && std::mem::offset_of!(RawDimension, value) == 392
);
extern "C" {
    fn af_dimension_state(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        out: *mut RawDimension,
        sink: Sink,
        user: *mut c_void,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_dimension_add(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        position: *const f64,
        invoked: *mut u32,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_dimension_set(
        c: *mut c_void,
        d: *mut c_void,
        h: *mut c_void,
        lease: u64,
        value: f64,
        invoked: *mut u32,
        diag: *mut RawExtrudeDiagnostic,
    ) -> i32;
}
fn decode(raw: RawDimension, fields: &mut Fields) -> Result<State, String> {
    if raw.reserved != 0 {
        return Err("native_dimension_invalid_abi".into());
    }
    let present = flag(raw.present)?;
    let driving = flag(raw.driving)?;
    let aligned = flag(raw.aligned)?;
    let dimension = if present {
        Some(Dimension {
            driving,
            aligned,
            value_cm: raw.value,
            parameter_value_cm: raw.parameter_value,
            text_position_cm: raw.text,
            parameter_name: field(fields, 0)?,
            parameter_expression: field(fields, 1)?,
            parameter_unit: field(fields, 2)?,
        })
    } else {
        if driving || aligned || raw.value != 0. || raw.parameter_value != 0. || raw.text != [0.; 3]
        {
            return Err("native_dimension_invalid_absent_fields".into());
        }
        None
    };
    Ok(State {
        relation: RelationState {
            line: line_state::decode(raw.relation.line)?,
            kind: raw.relation.kind,
        },
        dimension,
        model_parameters: raw.model_parameters,
        user_parameters: raw.user_parameters,
    })
}
impl Adapter<'_> {
    fn dimension_diagnostic(&self, status: i32, diag: &RawExtrudeDiagnostic) {
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
    }
}
impl DimensionBackend for Adapter<'_> {
    fn dimension_state(&self, h: &Handle, lease: u64) -> Result<State, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawDimension::default();
        let mut fields = Fields::default();
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_dimension_state(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                lease,
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
                &mut diag,
            )
        };
        self.dimension_diagnostic(status, &diag);
        if status != 0 {
            return Err(format!(
                "native_dimension_sdk_read_failed_status_{status}_stage_{}_error_{}",
                diag.stage, diag.sdk_error
            ));
        }
        decode(raw, &mut fields)
    }
    fn add_dimension(&self, h: &Handle, lease: u64, position: [f64; 3]) -> Attempt {
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
            af_dimension_add(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                lease,
                position.as_ptr(),
                &mut invoked,
                &mut diag,
            )
        };
        self.dimension_diagnostic(status, &diag);
        Attempt {
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1,
        }
    }
    fn set_dimension_value(&self, h: &Handle, lease: u64, value: f64) -> Attempt {
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
            af_dimension_set(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                lease,
                value,
                &mut invoked,
                &mut diag,
            )
        };
        self.dimension_diagnostic(status, &diag);
        Attempt {
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_flags_and_absent_payload() {
        for which in 0..4 {
            let mut r = RawDimension::default();
            match which {
                0 => r.present = 2,
                1 => r.driving = 2,
                2 => r.aligned = 2,
                _ => r.reserved = 1,
            };
            assert!(decode(r, &mut Fields::default()).is_err());
        }
        let mut r = RawDimension::default();
        r.value = 1.;
        assert!(decode(r, &mut Fields::default()).is_err());
    }
    #[test]
    fn present_requires_all_strings() {
        let mut r = RawDimension::default();
        r.present = 1;
        r.driving = 1;
        r.aligned = 1;
        assert!(decode(r, &mut Fields::default()).is_err());
    }
}
