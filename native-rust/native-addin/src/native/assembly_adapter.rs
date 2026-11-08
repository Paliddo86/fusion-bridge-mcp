use super::*;
use crate::assembly::{
    self, Attempt, Backend, Change, Component, Occurrence, Open, Projection, Root,
};
use std::collections::BTreeMap;
#[repr(C)]
#[derive(Default)]
struct RawRoot {
    counts: [u64; 13],
    flags: [u32; 6],
}
#[repr(C)]
#[derive(Default)]
struct RawComponent {
    counts: [u64; 10],
}
#[repr(C)]
#[derive(Default)]
struct RawOccurrence {
    component: u64,
    children: u64,
    matrix: [f64; 16],
    flags: [u32; 7],
}
#[repr(C)]
struct RawPositions {
    count: u64,
    timeline: u64,
    timeline_position: u64,
    pending: u32,
    reserved: u32,
    indexes: [i64; 64],
    health: [u32; 64],
}
impl Default for RawPositions {
    fn default() -> Self {
        Self {
            count: 0,
            timeline: 0,
            timeline_position: 0,
            pending: 0,
            reserved: 0,
            indexes: [0; 64],
            health: [0; 64],
        }
    }
}
#[repr(C)]
struct Diagnostic {
    sdk_error: i32,
    stage: u32,
    length: u32,
    truncated: u32,
    text: [u8; 4096],
}
impl Default for Diagnostic {
    fn default() -> Self {
        Self {
            sdk_error: -1,
            stage: 0,
            length: 0,
            truncated: 0,
            text: [0; 4096],
        }
    }
}
const _: () = assert!(
    std::mem::size_of::<RawPositions>() == 800
        && std::mem::offset_of!(RawPositions, indexes) == 32
        && std::mem::offset_of!(RawPositions, health) == 544
        && std::mem::size_of::<RawRoot>() == 128
        && std::mem::size_of::<RawComponent>() == 80
        && std::mem::size_of::<RawOccurrence>() == 176
        && std::mem::offset_of!(RawOccurrence, matrix) == 16
        && std::mem::offset_of!(RawOccurrence, flags) == 144
        && std::mem::size_of::<Diagnostic>() == 4112
);
extern "C" {
    fn af_assembly_positions(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        out: *mut RawPositions,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_translate(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        oi: u64,
        matrix: *const f64,
        invoked: *mut u32,
        component: *mut u64,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_capture_position(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        invoked: *mut u32,
        slot: *mut u64,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_open(
        c: *mut c_void,
        d: *mut c_void,
        out: *mut *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_close(c: *mut c_void, v: *mut c_void) -> i32;
    fn af_assembly_root(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        out: *mut RawRoot,
        sink: Sink,
        user: *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_component(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        index: u64,
        cloud: u32,
        out: *mut RawComponent,
        sink: Sink,
        user: *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_attribute(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        ci: u64,
        ai: u64,
        sink: Sink,
        user: *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_occurrence(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        index: u64,
        out: *mut RawOccurrence,
        sink: Sink,
        user: *mut c_void,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_add(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        existing: i64,
        invoked: *mut u32,
        ci: *mut u64,
        oi: *mut u64,
        diag: *mut Diagnostic,
    ) -> i32;
    fn af_assembly_change(
        c: *mut c_void,
        d: *mut c_void,
        v: *mut c_void,
        ci: u64,
        kind: u32,
        key: *const c_char,
        keylen: usize,
        value: *const c_char,
        valuelen: usize,
        invoked: *mut u32,
        diag: *mut Diagnostic,
    ) -> i32;
}
// No implicit SDK release. Book owns this exact view until explicit shutdown.
pub(super) struct Handle(NonNull<c_void>);
pub(super) struct Adapter<'a> {
    pub context: &'a Context,
    pub document: &'a DocumentReference,
}
impl Adapter<'_> {
    fn checked(&self, status: i32, diag: Diagnostic) -> Result<(), String> {
        if status == 0 {
            return Ok(());
        }
        let n = (diag.length as usize).min(diag.text.len());
        self.context.extrusion_diagnostic.replace(json!({"abiStatus":status,"stage":diag.stage,"sdkError":diag.sdk_error,"description":String::from_utf8_lossy(&diag.text[..n]),"truncated":diag.truncated!=0}));
        if status == 8 {
            return Err("native_assembly_native_metadata_requires_online".into());
        }
        Err(format!("native_assembly_sdk_status_{status}"))
    }
    fn root(&self, h: &Handle) -> Result<Root, String> {
        let mut out = RawRoot::default();
        let mut fields = Fields::default();
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_root(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                &mut out,
                collect,
                (&mut fields as *mut Fields).cast(),
                &mut diag,
            )
        };
        self.checked(status, diag)?;
        let f = out.flags;
        let n = out.counts;
        Ok(Root {
            revision: field(&mut fields, 0)?,
            name: field(&mut fields, 1)?,
            active: flag(f[0])?,
            parametric: flag(f[1])?,
            hybrid: flag(f[2])?,
            configured: flag(f[3])?,
            offline: flag(f[4])?,
            pending_snapshot: flag(f[5])?,
            components: n[0],
            occurrences: n[1],
            timeline: n[2],
            timeline_position: n[3],
            bodies: n[4],
            sketches: n[5],
            mesh_bodies: n[6],
            joints: n[7],
            as_built_joints: n[8],
            rigid_groups: n[9],
            assembly_constraints: n[10],
            motion_links: n[11],
            joint_origins: n[12],
        })
    }
}
impl Backend for Adapter<'_> {
    type Handle = Handle;
    fn open(&self) -> Open<Handle> {
        let Ok(fence) = self.context.child_leases.begin() else {
            return Open {
                handle: None,
                acknowledged: false,
            };
        };
        let mut out = std::ptr::null_mut();
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_open(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut out,
                &mut diag,
            )
        };
        let acknowledged = self.checked(status, diag).is_ok();
        let result = Open {
            handle: NonNull::new(out).map(Handle),
            acknowledged,
        };
        // No fallible work between relinquishing the acquisition fence and the Book
        // storing the already-built Open value. A panic above keeps teardown fenced.
        if result.handle.is_some() || status != 0 {
            fence.unopened();
        }
        result
    }
    fn command(&self) -> Result<String, String> {
        self.context.active_command()
    }
    fn snapshot(&self, h: &Handle, cloud: bool) -> Result<Projection, String> {
        let captured = self.capture(h, cloud)?;
        if let Some(diagnostic) =
            assembly::root_capture_diagnostic(&captured.projection.root, &captured.root_after)
        {
            self.context.extrusion_diagnostic.replace(diagnostic);
            return Err("native_assembly_root_changed_during_capture".into());
        }
        Ok(captured.projection)
    }
    fn capture(&self, h: &Handle, cloud: bool) -> Result<assembly::Capture, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let root = self.root(h)?;
        if cloud && root.offline {
            return Err("native_assembly_native_metadata_requires_online".into());
        }
        if root.components == 0
            || root.components > assembly::MAX_COMPONENTS as u64 + 1
            || root.occurrences > assembly::MAX_OCCURRENCES as u64
        {
            return Err("native_assembly_projection_bound".into());
        }
        let mut components = Vec::with_capacity(root.components as usize - 1);
        for ci in 0..root.components - 1 {
            let mut out = RawComponent::default();
            let mut fields = Fields::default();
            let mut diag = Diagnostic::default();
            let status = unsafe {
                af_assembly_component(
                    self.context.ptr.as_ptr(),
                    self.document.ptr.as_ptr(),
                    h.0.as_ptr(),
                    ci,
                    u32::from(cloud),
                    &mut out,
                    collect,
                    (&mut fields as *mut Fields).cast(),
                    &mut diag,
                )
            };
            self.checked(status, diag)?;
            let n = out.counts;
            if n[4] > 128 {
                return Err("native_assembly_metadata_bound".into());
            }
            let mut attributes = BTreeMap::new();
            for ai in 0..n[4] {
                let mut a = Fields::default();
                let mut diag = Diagnostic::default();
                let status = unsafe {
                    af_assembly_attribute(
                        self.context.ptr.as_ptr(),
                        self.document.ptr.as_ptr(),
                        h.0.as_ptr(),
                        ci,
                        ai,
                        collect,
                        (&mut a as *mut Fields).cast(),
                        &mut diag,
                    )
                };
                self.checked(status, diag)?;
                let group = field(&mut a, 0)?;
                let key = field(&mut a, 1)?;
                let value = field(&mut a, 2)?;
                if group == "AdomMechanical" && attributes.insert(key, value).is_some() {
                    return Err("native_assembly_duplicate_attribute".into());
                }
            }
            components.push(Component {
                name: field(&mut fields, 0)?,
                revision: field(&mut fields, 1)?,
                part_number: if cloud {
                    Some(field(&mut fields, 2)?)
                } else {
                    None
                },
                description: if cloud {
                    Some(field(&mut fields, 3)?)
                } else {
                    None
                },
                attributes,
                bodies: n[0],
                sketches: n[1],
                occurrences: n[2],
                mesh_bodies: n[3],
                joints: n[5],
                as_built_joints: n[6],
                rigid_groups: n[7],
                assembly_constraints: n[8],
                motion_links: n[9],
            });
        }
        let mut occurrences = Vec::with_capacity(root.occurrences as usize);
        for i in 0..root.occurrences {
            let mut out = RawOccurrence::default();
            let mut fields = Fields::default();
            let mut diag = Diagnostic::default();
            let status = unsafe {
                af_assembly_occurrence(
                    self.context.ptr.as_ptr(),
                    self.document.ptr.as_ptr(),
                    h.0.as_ptr(),
                    i,
                    &mut out,
                    collect,
                    (&mut fields as *mut Fields).cast(),
                    &mut diag,
                )
            };
            self.checked(status, diag)?;
            let f = out.flags;
            occurrences.push(Occurrence {
                component: usize::try_from(out.component)
                    .map_err(|_| "native_assembly_index_bound")?,
                children: out.children,
                matrix: out.matrix,
                grounded: flag(f[0])?,
                grounded_to_parent: flag(f[1])?,
                referenced: flag(f[2])?,
                configured: flag(f[3])?,
                suppressed: flag(f[4])?,
                rolled_back: flag(f[5])?,
                native: flag(f[6])?,
                name: field(&mut fields, 0)?,
                path: field(&mut fields, 1)?,
            });
        }
        let after = self.root(h)?;
        Ok(assembly::Capture {
            root_after: after,
            projection: Projection {
                root,
                components,
                occurrences,
                cloud_metadata: cloud,
            },
        })
    }
    fn add(&self, h: &Handle, existing: Option<usize>) -> Attempt {
        let mut invoked = 0;
        let mut ci = u64::MAX;
        let mut oi = u64::MAX;
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_add(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                existing.map_or(-1, |i| i as i64),
                &mut invoked,
                &mut ci,
                &mut oi,
                &mut diag,
            )
        };
        let acknowledged = self.checked(status, diag).is_ok() && invoked == 1;
        Attempt {
            invoked: invoked != 0,
            acknowledged,
            component: (ci != u64::MAX).then_some(ci as usize),
            occurrence: (oi != u64::MAX).then_some(oi as usize),
        }
    }
    fn change(&self, h: &Handle, ci: usize, c: &Change) -> Attempt {
        let mut invoked = 0;
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_change(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                ci as u64,
                u32::from(c.native),
                c.key.as_ptr().cast(),
                c.key.len(),
                c.value.as_ptr().cast(),
                c.value.len(),
                &mut invoked,
                &mut diag,
            )
        };
        let acknowledged = self.checked(status, diag).is_ok() && invoked == 1;
        Attempt {
            invoked: invoked != 0,
            acknowledged,
            component: Some(ci),
            occurrence: None,
        }
    }
    fn positions(&self, h: &Handle) -> Result<assembly::placement::PositionState, String> {
        let mut out = RawPositions::default();
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_positions(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                &mut out,
                &mut diag,
            )
        };
        self.checked(status, diag)?;
        if out.count > 64 || out.reserved != 0 {
            return Err("native_placement_position_state_bound".into());
        }
        let pending = flag(out.pending)?;
        let mut snapshots = Vec::with_capacity(out.count as usize);
        for i in 0..out.count as usize {
            snapshots.push(assembly::placement::SnapshotFact {
                slot: i,
                timeline_index: u64::try_from(out.indexes[i])
                    .map_err(|_| "native_placement_snapshot_index")?,
                health: out.health[i],
            });
        }
        Ok(assembly::placement::PositionState {
            pending,
            timeline: out.timeline,
            timeline_position: out.timeline_position,
            snapshots,
        })
    }
    fn translate(&self, h: &Handle, oi: usize, matrix: &[f64; 16]) -> Attempt {
        let mut invoked = 0;
        let mut ci = u64::MAX;
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_translate(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                oi as u64,
                matrix.as_ptr(),
                &mut invoked,
                &mut ci,
                &mut diag,
            )
        };
        let acknowledged = self.checked(status, diag).is_ok() && invoked == 1;
        Attempt {
            invoked: invoked != 0,
            acknowledged,
            component: (ci != u64::MAX).then_some(ci as usize),
            occurrence: Some(oi),
        }
    }
    fn capture_position(&self, h: &Handle) -> assembly::placement::SnapshotAttempt {
        let mut invoked = 0;
        let mut slot = u64::MAX;
        let mut diag = Diagnostic::default();
        let status = unsafe {
            af_assembly_capture_position(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                h.0.as_ptr(),
                &mut invoked,
                &mut slot,
                &mut diag,
            )
        };
        let acknowledged = self.checked(status, diag).is_ok() && invoked == 1;
        assembly::placement::SnapshotAttempt {
            attempt: Attempt {
                invoked: invoked != 0,
                acknowledged,
                component: None,
                occurrence: None,
            },
            snapshot_slot: (slot != u64::MAX).then_some(slot as usize),
        }
    }
    fn record_observation_diagnostic(&self, value: Value) {
        self.context.extrusion_diagnostic.replace(value);
    }
    fn diagnostic(&self) -> Value {
        self.context.extrusion_diagnostic.borrow().clone()
    }
}
pub(super) fn release(c: &Context, h: Handle) -> Result<(), String> {
    unsafe { checked(af_assembly_close(c.ptr.as_ptr(), h.0.as_ptr())) }
}
