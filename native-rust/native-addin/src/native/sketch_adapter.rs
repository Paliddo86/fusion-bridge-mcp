mod line_state;
use super::*;
use crate::sketches::{Backend, CreateAttempt, RootState, SketchState};
#[repr(C)]
#[derive(Default)]
struct RawRoot {
    active: u32,
    parametric: u32,
    hybrid: u32,
    configured: u32,
    sketch_count: u64,
    timeline_count: u64,
    timeline_position: u64,
}
#[repr(C)]
#[derive(Default)]
struct RawSketch {
    transform: [f64; 16],
    curves: u64,
    points: u64,
    profiles: u64,
    dimensions: u64,
    constraints: u64,
    plane: u32,
    health: u32,
    deferred: u32,
}
extern "C" {
    fn af_sketch_root(
        c: *mut c_void,
        d: *mut c_void,
        out: *mut RawRoot,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_sketch_create(
        c: *mut c_void,
        d: *mut c_void,
        plane: u32,
        out: *mut *mut c_void,
        invoked: *mut u32,
    ) -> i32;
    fn af_sketch_read(
        c: *mut c_void,
        d: *mut c_void,
        sketch: *mut c_void,
        out: *mut RawSketch,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_sketch_release(c: *mut c_void, sketch: *mut c_void) -> i32;
}
// No Drop: consuming release is explicit before parent document/context release.
pub(super) struct Handle(NonNull<c_void>);
pub(super) struct Adapter<'a> {
    pub context: &'a Context,
    pub document: &'a DocumentReference,
}
impl Backend for Adapter<'_> {
    type Handle = Handle;
    fn root(&self) -> Result<RootState, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawRoot::default();
        let mut fields = Fields::default();
        unsafe {
            checked(af_sketch_root(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        Ok(RootState {
            active: flag(raw.active)?,
            parametric: flag(raw.parametric)?,
            hybrid: flag(raw.hybrid)?,
            configured: flag(raw.configured)?,
            root_revision: field(&mut fields, 0)?,
            sketch_count: raw.sketch_count,
            timeline_count: raw.timeline_count,
            timeline_position: raw.timeline_position,
        })
    }
    fn command(&self) -> Result<String, String> {
        self.context.active_command()
    }
    fn create(&self, plane: u32) -> CreateAttempt<Handle> {
        let mut ptr = std::ptr::null_mut();
        let mut invoked = 0;
        let status = unsafe {
            af_sketch_create(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                plane,
                &mut ptr,
                &mut invoked,
            )
        };
        CreateAttempt {
            handle: NonNull::new(ptr).map(Handle),
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1,
        }
    }
    fn read(&self, handle: &Handle) -> Result<SketchState, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawSketch::default();
        let mut fields = Fields::default();
        unsafe {
            checked(af_sketch_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        Ok(SketchState {
            name: field(&mut fields, 0)?,
            sdk_revision: field(&mut fields, 1)?,
            plane: raw.plane,
            transform: raw.transform,
            curves: raw.curves,
            points: raw.points,
            profiles: raw.profiles,
            dimensions: raw.dimensions,
            constraints: raw.constraints,
            health: raw.health,
            deferred: flag(raw.deferred)?,
        })
    }
}
fn flag(value: u32) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid SDK boolean".into()),
    }
}
pub(super) fn release(context: &Context, handle: Handle) -> Result<(), String> {
    unsafe { checked(af_sketch_release(context.ptr.as_ptr(), handle.0.as_ptr())) }
}

use crate::sketches::geometry::{GeometryBackend, InsertAttempt, ObservedEntity};
use crate::sketches::geometry_preparation::{Entity, Geometry};
#[repr(C)]
struct RawGeometry {
    kind: u32,
    construction: u32,
    closed: u32,
    point_count: u32,
    reference: u32,
    scalar: f64,
    normal: [f64; 3],
    points: [f64; 384],
}
const _: () = assert!(std::mem::size_of::<RawGeometry>() == 3128);
const _: () = assert!(std::mem::offset_of!(RawGeometry, points) == 56);
impl Default for RawGeometry {
    fn default() -> Self {
        Self {
            kind: 0,
            construction: 0,
            closed: 0,
            point_count: 0,
            reference: 0,
            scalar: 0.,
            normal: [0.; 3],
            points: [0.; 384],
        }
    }
}
extern "C" {
    fn af_geometry_equalities(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        leases: *const u64,
        n: u32,
        out: *mut u8,
    ) -> i32;
    fn af_geometry_empty(c: *mut c_void, d: *mut c_void, s: *mut c_void, out: *mut u32) -> i32;
    fn af_geometry_insert(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        input: *const RawGeometry,
        lease: *mut u64,
        invoked: *mut u32,
        stage: *mut u32,
    ) -> i32;
    fn af_geometry_read(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        lease: u64,
        out: *mut RawGeometry,
    ) -> i32;
}
impl RawGeometry {
    fn input(entity: &Entity) -> Self {
        let mut raw = Self {
            construction: match entity.construction {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            },
            ..Self::default()
        };
        let points = match &entity.geometry {
            Geometry::Point { position_cm } => {
                raw.kind = 1;
                vec![*position_cm]
            }
            Geometry::Line { start_cm, end_cm } => {
                raw.kind = 2;
                vec![*start_cm, *end_cm]
            }
            Geometry::Circle {
                center_cm,
                radius_cm,
            } => {
                raw.kind = 3;
                raw.scalar = *radius_cm;
                vec![*center_cm]
            }
            Geometry::Arc {
                center_cm,
                start_cm,
                sweep_radians,
                ..
            } => {
                raw.kind = 4;
                raw.scalar = *sweep_radians;
                vec![*center_cm, *start_cm]
            }
            Geometry::FittedSpline {
                fit_points_cm,
                closed,
            } => {
                raw.kind = 5;
                raw.closed = u32::from(*closed);
                fit_points_cm.clone()
            }
        };
        raw.point_count = points.len() as u32;
        for (i, p) in points.iter().enumerate() {
            raw.points[i * 3..i * 3 + 3].copy_from_slice(p);
        }
        raw
    }
    fn observed(&self) -> Result<ObservedEntity, String> {
        let n = self.point_count as usize;
        if n > 128
            || !self.scalar.is_finite()
            || !self.points[..n * 3]
                .iter()
                .all(|p| p.is_finite() && p.abs() <= 10000.)
        {
            return Err("native_geometry_invalid_sdk_geometry".into());
        }
        let p = |i: usize| {
            [
                self.points[i * 3],
                self.points[i * 3 + 1],
                self.points[i * 3 + 2],
            ]
        };
        if matches!(self.kind, 3 | 4)
            && self
                .normal
                .iter()
                .zip([0., 0., 1.])
                .any(|(a, b)| !a.is_finite() || (*a - b).abs() > 1e-9)
        {
            return Err("native_geometry_invalid_sdk_plane".into());
        }
        let geometry = match self.kind {
            1 if n == 1 => Geometry::Point { position_cm: p(0) },
            2 if n == 2 => Geometry::Line {
                start_cm: p(0),
                end_cm: p(1),
            },
            3 if n == 1 => Geometry::Circle {
                center_cm: p(0),
                radius_cm: self.scalar,
            },
            4 if n == 3 => Geometry::Arc {
                center_cm: p(0),
                start_cm: p(1),
                end_cm: p(2),
                sweep_radians: self.scalar,
            },
            5 if n >= 2 => Geometry::FittedSpline {
                fit_points_cm: (0..n).map(p).collect(),
                closed: flag(self.closed)?,
            },
            _ => return Err("native_geometry_invalid_sdk_kind".into()),
        };
        Ok(ObservedEntity {
            geometry,
            construction: match self.construction {
                0 => None,
                1 => Some(false),
                2 => Some(true),
                _ => return Err("native_geometry_invalid_sdk_flag".into()),
            },
            reference: flag(self.reference)?,
        })
    }
}
impl GeometryBackend for Adapter<'_> {
    fn entity_equalities(&self, handle: &Handle, leases: &[u64]) -> Result<Vec<Vec<bool>>, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        if leases.is_empty() || leases.len() > 64 {
            return Err("native_entities_bound".into());
        }
        let mut matrix = [0u8; 4096];
        unsafe {
            checked(af_geometry_equalities(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                leases.as_ptr(),
                leases.len() as u32,
                matrix.as_mut_ptr(),
            ))?;
        }
        matrix[..leases.len() * leases.len()]
            .chunks_exact(leases.len())
            .map(|row| row.iter().map(|v| flag(*v as u32)).collect())
            .collect()
    }
    fn empty_origin_only(&self, handle: &Handle) -> Result<bool, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut empty = 0;
        unsafe {
            checked(af_geometry_empty(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &mut empty,
            ))?;
        }
        flag(empty)
    }
    fn insert(&self, handle: &Handle, entity: &Entity) -> InsertAttempt {
        let raw = RawGeometry::input(entity);
        let mut lease = u64::MAX;
        let mut invoked = 0;
        let mut stage = 0;
        let status = unsafe {
            af_geometry_insert(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &raw,
                &mut lease,
                &mut invoked,
                &mut stage,
            )
        };
        InsertAttempt {
            lease: (lease != u64::MAX).then_some(lease),
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1 && stage == 5,
            stage: match stage {
                1 => "prepare",
                2 => "add",
                3 => "construction",
                4 => "periodic_closure",
                5 => "complete",
                _ => "unconfirmed",
            },
        }
    }
    fn entity(&self, handle: &Handle, lease: u64) -> Result<ObservedEntity, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawGeometry::default();
        let status = unsafe {
            af_geometry_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                lease,
                &mut raw,
            )
        };
        if status != 0 {
            return Err(format!("native_geometry_read_status_{status}"));
        }
        raw.observed()
    }
}

use crate::sketches::extrusion::{ExtrudeAttempt, ExtrusionBackend, ProfileState, SolidState};
#[repr(C)]
#[derive(Default)]
struct RawProfile {
    area: f64,
    perimeter: f64,
    centroid: [f64; 3],
    loops: u64,
    curves: u64,
    root_bodies: u64,
    root_extrudes: u64,
    on_plane: u32,
    outer_only: u32,
    high_accuracy: u32,
}
#[repr(C)]
#[derive(Default)]
struct RawSolid {
    volume: f64,
    centroid_sketch: [f64; 3],
    distance: f64,
    taper: f64,
    health: u32,
    body_solid: u32,
    feature_solid: u32,
    new_body: u32,
    one_side: u32,
    profile_plane_start: u32,
    high_accuracy: u32,
}
#[repr(C)]
struct RawExtrudeDiagnostic {
    sdk_error: i32,
    stage: u32,
    length: u32,
    truncated: u32,
    text: [u8; 4096],
}
impl Default for RawExtrudeDiagnostic {
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
const _: () = assert!(std::mem::size_of::<RawProfile>() == 88);
const _: () = assert!(std::mem::size_of::<RawSolid>() == 80);
const _: () = assert!(std::mem::size_of::<RawExtrudeDiagnostic>() == 4112);
extern "C" {
    fn af_profile_read(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        out: *mut RawProfile,
        diagnostic: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_extrude_create(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        distance: f64,
        negative: u32,
        invoked: *mut u32,
        diagnostic: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_extrude_verify_profile(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        diagnostic: *mut RawExtrudeDiagnostic,
    ) -> i32;
    fn af_extrude_read(
        c: *mut c_void,
        d: *mut c_void,
        s: *mut c_void,
        out: *mut RawSolid,
        diagnostic: *mut RawExtrudeDiagnostic,
    ) -> i32;
}
impl Adapter<'_> {
    fn extrusion_checked(
        &self,
        status: i32,
        diagnostic: &RawExtrudeDiagnostic,
    ) -> Result<(), String> {
        if status == 0 {
            return Ok(());
        }
        let value = crate::sketches::extrusion::sdk_diagnostic(
            status,
            diagnostic.stage,
            diagnostic.sdk_error,
            &diagnostic.text,
            diagnostic.length as usize,
            diagnostic.truncated != 0,
        );
        self.context.extrusion_diagnostic.replace(value);
        Err(format!(
            "native_extrude_sdk_status_{status}_stage_{}_error_{}",
            diagnostic.stage, diagnostic.sdk_error
        ))
    }
}
impl ExtrusionBackend for Adapter<'_> {
    fn clear_diagnostic(&self) {
        self.context.extrusion_diagnostic.replace(Value::Null);
    }
    fn diagnostic(&self) -> Value {
        self.context.extrusion_diagnostic.borrow().clone()
    }
    fn profile(&self, handle: &Handle) -> Result<ProfileState, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawProfile::default();
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_profile_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &mut raw,
                &mut diag,
            )
        };
        self.extrusion_checked(status, &diag)?;
        Ok(ProfileState {
            area_cm2: raw.area,
            perimeter_cm: raw.perimeter,
            centroid_cm: raw.centroid,
            loops: raw.loops,
            curves: raw.curves,
            root_bodies: raw.root_bodies,
            root_extrudes: raw.root_extrudes,
            on_plane: flag(raw.on_plane)?,
            outer_only: flag(raw.outer_only)?,
            high_accuracy: flag(raw.high_accuracy)?,
        })
    }
    fn extrude(&self, handle: &Handle, distance_cm: f64, negative: bool) -> ExtrudeAttempt {
        let mut invoked = 0;
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_extrude_create(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                distance_cm,
                u32::from(negative),
                &mut invoked,
                &mut diag,
            )
        };
        let _ = self.extrusion_checked(status, &diag);
        ExtrudeAttempt {
            invoked: invoked != 0,
            acknowledged: status == 0 && invoked == 1,
            stage: match diag.stage {
                200..=208 | 213..=214 => "feature_input",
                209 => "feature_add",
                210..=212 => "returned_body_retention",
                _ => "profile_or_shared_guard",
            },
        }
    }
    fn verify_extrude_profile(&self, handle: &Handle) -> Result<(), String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_extrude_verify_profile(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &mut diag,
            )
        };
        self.extrusion_checked(status, &diag)
    }
    fn solid(&self, handle: &Handle) -> Result<SolidState, String> {
        self.context.close.ensure_open()?;
        self.context.child_leases.ensure_clear()?;
        let mut raw = RawSolid::default();
        let mut diag = RawExtrudeDiagnostic::default();
        let status = unsafe {
            af_extrude_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                handle.0.as_ptr(),
                &mut raw,
                &mut diag,
            )
        };
        self.extrusion_checked(status, &diag)?;
        Ok(SolidState {
            volume_cm3: raw.volume,
            centroid_sketch_cm: raw.centroid_sketch,
            distance_cm: raw.distance,
            taper_radians: raw.taper,
            feature_health: raw.health,
            body_solid: flag(raw.body_solid)?,
            feature_solid: flag(raw.feature_solid)?,
            new_body: flag(raw.new_body)?,
            one_side: flag(raw.one_side)?,
            profile_plane_start: flag(raw.profile_plane_start)?,
            high_accuracy: flag(raw.high_accuracy)?,
        })
    }
}

mod relations;

mod dimensions;
