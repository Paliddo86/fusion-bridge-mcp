//! Pure bounded numeric preparation. No native sketch, identity, solver,
//! transaction, model-unchanged proof or SDK effect is produced by this crate.
mod model;
pub use model::*;
use serde::Serialize;
use std::{
    fmt,
    io::{self, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Numeric,
    Degenerate,
    NonCoplanar,
    Count,
    Budget,
    Serialization,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Numeric => "nonfinite, out-of-bound or unrepresentable geometry",
            Self::Degenerate => "degenerate sketch geometry",
            Self::NonCoplanar => "unsupported sketch-space Z coordinates",
            Self::Count => "invalid item, side or fit-point count",
            Self::Budget => "sketch preparation budget exceeded",
            Self::Serialization => "sketch plan serialization failed",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn require(ok: bool, error: Error) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(error)
    }
}

/// Finite mm to cm conversion with an explicit bound and underflow refusal.
pub fn millimeters_to_centimeters(mm: f64) -> Result<f64> {
    require(mm.is_finite() && mm.abs() <= MAX_ABS_MM, Error::Numeric)?;
    let cm = mm / 10.;
    require(cm.is_finite() && (mm == 0. || cm != 0.), Error::Numeric)?;
    Ok(cm)
}
pub fn point_cm(point: PointMm) -> Result<[f64; 3]> {
    let xyz = match point {
        PointMm::Xy([x, y]) => [x, y, 0.],
        PointMm::Xyz(xyz) => xyz,
    };
    Ok([
        millimeters_to_centimeters(xyz[0])?,
        millimeters_to_centimeters(xyz[1])?,
        millimeters_to_centimeters(xyz[2])?,
    ])
}
fn point_valid(p: [f64; 3]) -> Result<[f64; 3]> {
    require(
        p.iter().all(|v| v.is_finite() && v.abs() <= MAX_ABS_CM),
        Error::Numeric,
    )?;
    Ok(p)
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
fn span(a: [f64; 3], b: [f64; 3]) -> Result<()> {
    require(distance(a, b) > MIN_SPAN_CM, Error::Degenerate)
}
fn radius(mm: f64) -> Result<f64> {
    let cm = millimeters_to_centimeters(mm)?;
    require(cm > MIN_SPAN_CM, Error::Degenerate)?;
    Ok(cm)
}
fn same_z(a: [f64; 3], b: [f64; 3]) -> Result<()> {
    require(a[2] == b[2], Error::NonCoplanar)
}
// Conservative full-circle envelope also bounds arcs, even when the requested
// subarc would occupy a smaller region. This avoids hidden out-of-bound geometry.
fn circle_bounds(center: [f64; 3], radius: f64) -> Result<()> {
    for axis in [0, 1] {
        for sign in [-1., 1.] {
            let mut edge = center;
            edge[axis] += sign * radius;
            point_valid(edge)?;
            span(center, edge)?;
        }
    }
    Ok(())
}
fn line(a: [f64; 3], b: [f64; 3]) -> Result<Geometry> {
    point_valid(a)?;
    point_valid(b)?;
    span(a, b)?;
    Ok(Geometry::Line {
        start_cm: a,
        end_cm: b,
    })
}
fn arc(center: [f64; 3], start: [f64; 3], sweep: f64) -> Result<Geometry> {
    same_z(center, start)?;
    span(center, start)?;
    let dx = start[0] - center[0];
    let dy = start[1] - center[1];
    circle_bounds(center, dx.hypot(dy))?;
    let (sin, cos) = sweep.sin_cos();
    let end = point_valid([
        center[0] + dx * cos - dy * sin,
        center[1] + dx * sin + dy * cos,
        center[2],
    ])?;
    span(start, end)?;
    Ok(Geometry::Arc {
        center_cm: center,
        start_cm: start,
        end_cm: end,
        sweep_radians: sweep,
    })
}
fn cost(item: &Primitive) -> Result<(usize, usize)> {
    match item {
        Primitive::Point { .. } => Ok((1, 1)),
        Primitive::Line { .. } => Ok((1, 2)),
        Primitive::Rectangle { .. } => Ok((4, 8)),
        Primitive::Circle { .. } => Ok((1, 1)),
        Primitive::Arc { .. } => Ok((1, 3)),
        Primitive::Polygon { sides, .. } => {
            require((3..=MAX_SIDES).contains(sides), Error::Count)?;
            Ok((*sides, sides * 2))
        }
        Primitive::Slot { .. } => Ok((4, 10)),
        Primitive::Spline { points, closed, .. } => {
            require(
                ((if *closed { 3 } else { 2 })..=MAX_FIT_POINTS).contains(&points.len()),
                Error::Count,
            )?;
            Ok((1, points.len()))
        }
    }
}
fn kind(item: &Primitive) -> PrimitiveKind {
    match item {
        Primitive::Point { .. } => PrimitiveKind::Point,
        Primitive::Line { .. } => PrimitiveKind::Line,
        Primitive::Rectangle { .. } => PrimitiveKind::Rectangle,
        Primitive::Circle { .. } => PrimitiveKind::Circle,
        Primitive::Arc { .. } => PrimitiveKind::Arc,
        Primitive::Polygon { .. } => PrimitiveKind::Polygon,
        Primitive::Slot { .. } => PrimitiveKind::Slot,
        Primitive::Spline { .. } => PrimitiveKind::Spline,
    }
}
fn compile(item: &Primitive) -> Result<Vec<Entity>> {
    let (geometries, construction) = match item {
        Primitive::Point { position } => (
            vec![Geometry::Point {
                position_cm: point_cm(*position)?,
            }],
            None,
        ),
        Primitive::Line {
            start,
            end,
            construction,
        } => (
            vec![line(point_cm(*start)?, point_cm(*end)?)?],
            *construction,
        ),
        Primitive::Rectangle {
            start,
            end,
            construction,
        } => {
            let a = point_cm(*start)?;
            let c = point_cm(*end)?;
            same_z(a, c)?;
            let b = [c[0], a[1], a[2]];
            let d = [a[0], c[1], a[2]];
            (
                vec![line(a, b)?, line(b, c)?, line(c, d)?, line(d, a)?],
                *construction,
            )
        }
        Primitive::Circle {
            center,
            radius: mm,
            construction,
        } => {
            let center = point_cm(*center)?;
            let radius = radius(*mm)?;
            circle_bounds(center, radius)?;
            (
                vec![Geometry::Circle {
                    center_cm: center,
                    radius_cm: radius,
                }],
                *construction,
            )
        }
        Primitive::Arc {
            center,
            start,
            sweep,
            construction,
        } => {
            require(
                sweep.is_finite() && sweep.abs() > 0. && sweep.abs() < 360.,
                Error::Numeric,
            )?;
            let radians = sweep.to_radians();
            require(
                radians.is_finite() && radians != 0. && radians.abs() < std::f64::consts::TAU,
                Error::Numeric,
            )?;
            (
                vec![arc(point_cm(*center)?, point_cm(*start)?, radians)?],
                *construction,
            )
        }
        Primitive::Polygon {
            center,
            radius: mm,
            sides,
            angle,
            construction,
        } => {
            let center = point_cm(*center)?;
            let radius = radius(*mm)?;
            circle_bounds(center, radius)?;
            require(
                angle.is_finite() && angle.abs() <= MAX_ANGLE_DEGREES,
                Error::Numeric,
            )?;
            // Reduce in degrees before trig. Bounded angles avoid arbitrary
            // argument-reduction loss while preserving orientation modulo turns.
            let angle = angle.rem_euclid(360.).to_radians();
            let vertices: Result<Vec<_>> = (0..*sides)
                .map(|i| {
                    let theta = angle + std::f64::consts::TAU * (i as f64) / (*sides as f64);
                    let (sin, cos) = theta.sin_cos();
                    point_valid([
                        center[0] + radius * cos,
                        center[1] + radius * sin,
                        center[2],
                    ])
                })
                .collect();
            let vertices = vertices?;
            let edges: Result<Vec<_>> = (0..*sides)
                .map(|i| line(vertices[i], vertices[(i + 1) % sides]))
                .collect();
            (edges?, *construction)
        }
        Primitive::Slot {
            start,
            end,
            width,
            construction,
        } => {
            let start = point_cm(*start)?;
            let end = point_cm(*end)?;
            require((start[2] - end[2]).abs() < 1e-9, Error::NonCoplanar)?;
            let radius = radius(*width)? / 2.;
            require(radius > MIN_SPAN_CM, Error::Degenerate)?;
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            let length = dx.hypot(dy);
            require(length > MIN_SPAN_CM, Error::Degenerate)?;
            // Normalize first, avoiding an unnecessary length*radius product.
            let ox = -dy / length * radius;
            let oy = dx / length * radius;
            let offset =
                |c: [f64; 3], sign: f64| point_valid([c[0] + sign * ox, c[1] + sign * oy, c[2]]);
            let a = offset(start, 1.)?;
            let b = offset(end, 1.)?;
            let c = offset(end, -1.)?;
            let d = offset(start, -1.)?;
            // Explicit opposite endpoints preserve exact chain connectivity;
            // computing sin(-PI) would introduce a spurious tiny endpoint gap.
            circle_bounds(start, radius)?;
            circle_bounds(end, radius)?;
            span(a, d)?;
            span(b, c)?;
            (
                vec![
                    line(a, b)?,
                    Geometry::Arc {
                        center_cm: end,
                        start_cm: b,
                        end_cm: c,
                        sweep_radians: -std::f64::consts::PI,
                    },
                    line(c, d)?,
                    Geometry::Arc {
                        center_cm: start,
                        start_cm: d,
                        end_cm: a,
                        sweep_radians: -std::f64::consts::PI,
                    },
                ],
                *construction,
            )
        }
        Primitive::Spline {
            points,
            closed,
            construction,
        } => {
            let points: Result<Vec<_>> = points.iter().copied().map(point_cm).collect();
            let points = points?;
            for pair in points.windows(2) {
                span(pair[0], pair[1])?;
            }
            if *closed {
                span(points[0], points[points.len() - 1])?;
            }
            (
                vec![Geometry::FittedSpline {
                    fit_points_cm: points,
                    closed: *closed,
                }],
                *construction,
            )
        }
    };
    Ok(geometries
        .into_iter()
        .map(|geometry| Entity {
            geometry,
            construction,
        })
        .collect())
}

/// Immutable, completely validated supplied-data instructions. Getters cannot
/// mutate the validated plan; cloned individual geometry is no longer this plan.
#[derive(Clone, Debug, Serialize)]
pub struct PreparedPlan {
    kind: &'static str,
    evidence_scope: &'static str,
    units: &'static str,
    items: Vec<PreparedItem>,
    entity_count: usize,
    stored_point_count: usize,
}
impl PreparedPlan {
    pub fn items(&self) -> &[PreparedItem] {
        &self.items
    }
    pub fn entity_count(&self) -> usize {
        self.entity_count
    }
    pub fn stored_point_count(&self) -> usize {
        self.stored_point_count
    }
    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| Error::Serialization)
    }
}
struct Counter {
    bytes: usize,
    limit: usize,
}
impl Write for Counter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(data.len())
            .ok_or_else(|| io::Error::other("size overflow"))?;
        if self.bytes > self.limit {
            return Err(io::Error::other("output limit"));
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn prepare(request: &Request) -> Result<PreparedPlan> {
    prepare_with_limits(request, Limits::default())
}
pub fn prepare_with_limits(request: &Request, limits: Limits) -> Result<PreparedPlan> {
    require((1..=MAX_ITEMS).contains(&request.items.len()), Error::Count)?;
    require(
        limits.max_entities > 0
            && limits.max_entities <= MAX_ENTITIES
            && limits.max_stored_points > 0
            && limits.max_stored_points <= MAX_STORED_POINTS
            && limits.max_output_bytes > 0
            && limits.max_output_bytes <= MAX_OUTPUT_BYTES,
        Error::Budget,
    )?;
    let mut entities = 0usize;
    let mut points = 0usize;
    // Validate/count expansion before constructing any generated geometry.
    for item in &request.items {
        let (e, p) = cost(item)?;
        entities = entities.checked_add(e).ok_or(Error::Budget)?;
        points = points.checked_add(p).ok_or(Error::Budget)?;
        require(
            entities <= limits.max_entities && points <= limits.max_stored_points,
            Error::Budget,
        )?;
    }
    let items: Result<Vec<_>> = request
        .items
        .iter()
        .enumerate()
        .map(|(input_index, item)| {
            Ok(PreparedItem {
                input_index,
                primitive: kind(item),
                entities: compile(item)?,
            })
        })
        .collect();
    let result=PreparedPlan{kind:"SUPPLIED_SKETCH_GEOMETRY_PLAN",
        evidence_scope:"Supplied numeric geometry only; no native identity, solver validity, transaction, mutation or modelUnchanged proof.",
        units:"cm_and_radians",items:items?,entity_count:entities,stored_point_count:points};
    let mut counter = Counter {
        bytes: 0,
        limit: limits.max_output_bytes,
    };
    serde_json::to_writer(&mut counter, &result).map_err(|_| Error::Budget)?;
    Ok(result)
}
