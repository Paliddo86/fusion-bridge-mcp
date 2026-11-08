use super::*;
use crate::purchased::{Backend, Command, Result, Snapshot};
#[repr(C)]
#[derive(Default)]
struct State {
    design_intent: u32,
    available: u32,
    native: u32,
}
#[repr(C)]
struct Diagnostic {
    sdk_error: i32,
    stage: u32,
    length: u32,
    truncated: u32,
    text: [u8; 4096],
}
const _: () = assert!(std::mem::size_of::<State>() == 12);
const _: () = assert!(
    std::mem::size_of::<Diagnostic>() == 4112 && std::mem::offset_of!(Diagnostic, text) == 16
);
extern "C" {
    fn af_mcmaster_discover(
        c: *mut c_void,
        d: *mut c_void,
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
            af_mcmaster_discover(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut out,
                collect,
                (&mut fields as *mut Fields).cast(),
                &mut diagnostic,
            )
        };
        if status != 0 {
            let len = (diagnostic.length as usize).min(diagnostic.text.len());
            return Err(format!(
                "native_mcmaster_sdk_failure:{}",
                json!({"abiStatus":status,"stage":diagnostic.stage,"sdkError":diagnostic.sdk_error,"description":String::from_utf8_lossy(&diagnostic.text[..len]),"truncated":diagnostic.truncated!=0})
            ));
        }
        if out.available > 1 || out.native > 1 {
            return Err("native_mcmaster_invalid_sdk_boolean".into());
        }
        let command = if out.available == 1 {
            Some(Command {
                id: field(&mut fields, 0)?,
                name: field(&mut fields, 1)?,
                native: out.native == 1,
            })
        } else {
            None
        };
        Ok(Snapshot {
            design_intent: out.design_intent,
            command,
        })
    }
}
