use super::*;
use crate::bom::{self, Backend as BomBackend};
use fusion_bom_core::{Material, PhysicalProperties};
use std::collections::BTreeMap;

#[repr(C)]
#[derive(Default)]
struct RawDesign {
    active: u32,
    hybrid: u32,
    configured: u32,
    configuration: u32,
    table_present: u32,
    components: u64,
    timeline_count: u64,
    timeline_position: u64,
}
#[repr(C)]
#[derive(Default)]
struct RawInstance {
    referenced: u32,
    configured: u32,
    timeline_present: u32,
    suppressed: u32,
    rolled_back: u32,
    visible: u32,
}
#[repr(C)]
#[derive(Default)]
struct RawPhysical {
    mass_kg: f64,
    volume_cm3: f64,
    high_accuracy: u32,
}
extern "C" {
    fn af_bom_open(c: *mut c_void, d: *mut c_void, out: *mut *mut c_void) -> i32;
    fn af_bom_close(v: *mut c_void) -> i32;
    fn af_bom_is_offline(v: *mut c_void, out: *mut u32) -> i32;
    fn af_bom_design(v: *mut c_void, out: *mut RawDesign, sink: Sink, user: *mut c_void) -> i32;
    fn af_bom_error_stage(
        v: *mut c_void,
        sdk_error: *mut i32,
        definition_id: *mut u32,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_bom_root(v: *mut c_void, out: *mut u32) -> i32;
    fn af_bom_same(v: *mut c_void, a: u32, b: u32, out: *mut u32) -> i32;
    fn af_bom_component(v: *mut c_void, id: u32, out: *mut u32) -> i32;
    fn af_bom_definition(
        v: *mut c_void,
        id: u32,
        meshes: *mut u64,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_bom_material(
        v: *mut c_void,
        id: u32,
        present: *mut u32,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_bom_attribute_count(v: *mut c_void, id: u32, out: *mut u64) -> i32;
    fn af_bom_attribute(v: *mut c_void, id: u32, index: u64, sink: Sink, user: *mut c_void) -> i32;
    fn af_bom_instance(
        v: *mut c_void,
        id: u32,
        out: *mut RawInstance,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_bom_child_count(v: *mut c_void, id: u32, out: *mut u64) -> i32;
    fn af_bom_child(v: *mut c_void, id: u32, index: u64, out: *mut u32) -> i32;
    fn af_bom_body_count(v: *mut c_void, id: u32, out: *mut u64) -> i32;
    fn af_bom_body(v: *mut c_void, id: u32, index: u64, out: *mut RawPhysical) -> i32;
    fn af_bom_subtree(v: *mut c_void, id: u32, out: *mut RawPhysical) -> i32;
}
fn number(value: u64) -> bom::Result<usize> {
    usize::try_from(value).map_err(|_| "native_bom_count_overflow".into())
}
// Lossy decoding is confined to optional diagnostic text. Ordinary SDK data
// continues to use the strict shared collector.
unsafe extern "C" fn collect_diagnostic(
    user: *mut c_void,
    field: u32,
    data: *const std::ffi::c_char,
    len: usize,
) -> i32 {
    if user.is_null() || (data.is_null() && len != 0) || len > 1024 || field > 1 {
        return 0;
    }
    let fields = &mut *user.cast::<bom::DiagnosticFields>();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bytes = if len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(data.cast::<u8>(), len)
        };
        fields.store(field, bytes).is_ok()
    }))
    .unwrap_or(false);
    i32::from(result)
}
struct View(NonNull<c_void>);
impl View {
    fn checked(&self, call: &str, code: i32) -> bom::Result<()> {
        if code != 0 {
            let mut diagnostic = bom::DiagnosticFields::default();
            let mut sdk_error = 0;
            let mut definition_id = u32::MAX;
            // Out-parameters/fields are written only after the ABI owner check.
            // Preserve each copied field even if optional description fails.
            let _observed = unsafe {
                af_bom_error_stage(
                    self.0.as_ptr(),
                    &mut sdk_error,
                    &mut definition_id,
                    collect_diagnostic,
                    (&mut diagnostic as *mut bom::DiagnosticFields).cast(),
                )
            };
            let stage = diagnostic.stage;
            let description = diagnostic.description;
            let detail = if sdk_error != 0 {
                Some((sdk_error, definition_id, description.as_deref()))
            } else {
                None
            };
            return Err(bom::sdk_diagnostic(call, code, stage.as_deref(), detail));
        }
        Ok(())
    }
}
impl BomBackend for View {
    type Handle = u32;
    fn is_offline(&self) -> bom::Result<bool> {
        let mut offline = 0;
        unsafe {
            bom::sdk_status(
                "connectivity",
                af_bom_is_offline(self.0.as_ptr(), &mut offline),
            )?;
        }
        checked_bool(offline)
    }
    fn design(&self) -> bom::Result<bom::Design> {
        let mut raw = RawDesign::default();
        let mut fields = Fields::default();
        let code = unsafe {
            af_bom_design(
                self.0.as_ptr(),
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            )
        };
        self.checked("design", code)?;
        Ok(bom::Design {
            active: checked_bool(raw.active)?,
            hybrid: checked_bool(raw.hybrid)?,
            configured: checked_bool(raw.configured)?,
            configuration: checked_bool(raw.configuration)?,
            table_present: checked_bool(raw.table_present)?,
            row_id: field(&mut fields, 0)?,
            components: number(raw.components)?,
            timeline_count: number(raw.timeline_count)?,
            timeline_position: number(raw.timeline_position)?,
        })
    }
    fn root(&self) -> bom::Result<u32> {
        let mut out = 0;
        unsafe {
            bom::sdk_status("root", af_bom_root(self.0.as_ptr(), &mut out))?;
        }
        Ok(out)
    }
    fn same(&self, a: u32, b: u32) -> bom::Result<bool> {
        let mut out = 0;
        unsafe {
            bom::sdk_status("same", af_bom_same(self.0.as_ptr(), a, b, &mut out))?;
        }
        checked_bool(out)
    }
    fn component(&self, id: u32) -> bom::Result<u32> {
        let mut out = 0;
        unsafe {
            bom::sdk_status("component", af_bom_component(self.0.as_ptr(), id, &mut out))?;
        }
        Ok(out)
    }
    fn definition(&self, id: u32) -> bom::Result<bom::Definition> {
        let mut fields = Fields::default();
        let mut meshes = 0;
        unsafe {
            self.checked(
                "definition",
                af_bom_definition(
                    self.0.as_ptr(),
                    id,
                    &mut meshes,
                    collect,
                    (&mut fields as *mut Fields).cast(),
                ),
            )?;
        }
        let name = field(&mut fields, 0)?;
        let part_number = field(&mut fields, 1)?;
        let description = field(&mut fields, 2)?;
        let revision = field(&mut fields, 3)?;
        let token = field(&mut fields, 4)?;
        let mut count = 0;
        unsafe {
            bom::sdk_status(
                "attribute_count",
                af_bom_attribute_count(self.0.as_ptr(), id, &mut count),
            )?;
        }
        if count > 128 {
            return Err("native_bom_metadata_bound".into());
        }
        let mut attributes = BTreeMap::new();
        let mut attribute_bytes = 0usize;
        for index in 0..count {
            let mut fields = Fields::default();
            unsafe {
                bom::sdk_status(
                    "attribute",
                    af_bom_attribute(
                        self.0.as_ptr(),
                        id,
                        index,
                        collect,
                        (&mut fields as *mut Fields).cast(),
                    ),
                )?;
            }
            let group = field(&mut fields, 0)?;
            let key = field(&mut fields, 1)?;
            let value = field(&mut fields, 2)?;
            if [group.len(), key.len(), value.len()]
                .iter()
                .any(|length| *length > 16 * 1024)
            {
                return Err("native_bom_text_bound".into());
            }
            attribute_bytes = attribute_bytes
                .checked_add(group.len() + key.len() + value.len())
                .ok_or("native_bom_byte_bound")?;
            if attribute_bytes > bom::MAX_BYTES / 2 {
                return Err("native_bom_byte_bound".into());
            }
            if group == "AdomMechanical" && attributes.insert(key, value).is_some() {
                return Err("native_bom_duplicate_attribute".into());
            }
        }
        let mut after = 0;
        unsafe {
            bom::sdk_status(
                "attribute_count",
                af_bom_attribute_count(self.0.as_ptr(), id, &mut after),
            )?;
        }
        if after != count {
            return Err("native_bom_changed_attributes".into());
        }
        let mut fields = Fields::default();
        let mut present = 0;
        unsafe {
            bom::sdk_status(
                "material",
                af_bom_material(
                    self.0.as_ptr(),
                    id,
                    &mut present,
                    collect,
                    (&mut fields as *mut Fields).cast(),
                ),
            )?;
        }
        let material = if checked_bool(present)? {
            Some(Material {
                id: field(&mut fields, 0)?,
                name: field(&mut fields, 1)?,
            })
        } else {
            None
        };
        Ok(bom::Definition {
            name,
            part_number,
            description,
            revision,
            token,
            attributes,
            material,
            mesh_bodies: number(meshes)?,
        })
    }
    fn instance(&self, id: u32) -> bom::Result<bom::Instance> {
        let mut raw = RawInstance::default();
        let mut fields = Fields::default();
        unsafe {
            bom::sdk_status(
                "instance",
                af_bom_instance(
                    self.0.as_ptr(),
                    id,
                    &mut raw,
                    collect,
                    (&mut fields as *mut Fields).cast(),
                ),
            )?;
        }
        Ok(bom::Instance {
            path: field(&mut fields, 0)?,
            token: field(&mut fields, 1)?,
            referenced: checked_bool(raw.referenced)?,
            configured: checked_bool(raw.configured)?,
            timeline_present: checked_bool(raw.timeline_present)?,
            suppressed: checked_bool(raw.suppressed)?,
            rolled_back: checked_bool(raw.rolled_back)?,
            visible: checked_bool(raw.visible)?,
        })
    }
    fn child_count(&self, id: u32) -> bom::Result<usize> {
        let mut out = 0;
        unsafe {
            bom::sdk_status(
                "child_count",
                af_bom_child_count(self.0.as_ptr(), id, &mut out),
            )?;
        }
        number(out)
    }
    fn child(&self, id: u32, index: usize) -> bom::Result<u32> {
        let mut out = 0;
        unsafe {
            bom::sdk_status(
                "child",
                af_bom_child(self.0.as_ptr(), id, index as u64, &mut out),
            )?;
        }
        Ok(out)
    }
    fn body_count(&self, id: u32) -> bom::Result<usize> {
        let mut out = 0;
        unsafe {
            bom::sdk_status(
                "body_count",
                af_bom_body_count(self.0.as_ptr(), id, &mut out),
            )?;
        }
        number(out)
    }
    fn body(&self, id: u32, index: usize) -> bom::Result<PhysicalProperties> {
        let mut out = RawPhysical::default();
        unsafe {
            bom::sdk_status(
                "body",
                af_bom_body(self.0.as_ptr(), id, index as u64, &mut out),
            )?;
        }
        properties(out)
    }
    fn subtree_properties(&self, id: u32) -> bom::Result<PhysicalProperties> {
        let mut out = RawPhysical::default();
        unsafe {
            bom::sdk_status("subtree", af_bom_subtree(self.0.as_ptr(), id, &mut out))?;
        }
        properties(out)
    }
}
fn properties(value: RawPhysical) -> bom::Result<PhysicalProperties> {
    if value.high_accuracy != 1 {
        return Err("native_bom_accuracy_unconfirmed".into());
    }
    PhysicalProperties::from_fusion_units(value.mass_kg, value.volume_cm3)
        .map_err(|_| "native_bom_invalid_physical".into())
}
pub(super) fn generate(
    context: &Context,
    document: &DocumentReference,
    session: &str,
    id: &str,
    revision: Option<&str>,
    explode: bool,
) -> bom::Result<bom::Captured> {
    let lease = context.child_leases.begin()?;
    let mut raw = std::ptr::null_mut();
    let opened = unsafe { af_bom_open(context.ptr.as_ptr(), document.ptr.as_ptr(), &mut raw) };
    let Some(raw) = NonNull::new(raw) else {
        if opened == 0 {
            panic!("native BOM open acknowledged without lease; parent teardown fenced");
        }
        // The C++ opener leaves out=null on every preacquisition failure.
        lease.unopened();
        return Err("native_bom_open_failed".into());
    };
    let result = lease.run(
        View(raw),
        |view| {
            bom::sdk_status("open", opened)?;
            bom::capture(view, session, id, revision, explode)
        },
        |view| unsafe { bom::sdk_status("close", af_bom_close(view.0.as_ptr())) },
    );
    match result {
        Ok(value) => value,
        Err(_) => panic!("native BOM lease release unconfirmed; parent teardown fenced"),
    }
}
