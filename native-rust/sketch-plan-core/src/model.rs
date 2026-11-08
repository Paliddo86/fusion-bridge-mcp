use serde::{Deserialize, Serialize};

fn construction_flag<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

pub const MAX_ITEMS: usize = 128;
pub const MAX_SIDES: usize = 128;
pub const MAX_FIT_POINTS: usize = 128;
pub const MAX_ENTITIES: usize = 4096;
pub const MAX_STORED_POINTS: usize = 16384;
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ABS_MM: f64 = 1e9;
pub const MAX_ABS_CM: f64 = MAX_ABS_MM / 10.;
/// Conservative preparation threshold; it is not an SDK tolerance guarantee.
pub const MIN_SPAN_CM: f64 = 1e-8;
pub const MAX_ANGLE_DEGREES: f64 = 1e9;

/// Input coordinates in sketch-space millimeters. No model/world transform is inferred.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PointMm {
    Xy([f64; 2]),
    Xyz([f64; 3]),
}
impl From<[f64; 2]> for PointMm {
    fn from(v: [f64; 2]) -> Self {
        Self::Xy(v)
    }
}
impl From<[f64; 3]> for PointMm {
    fn from(v: [f64; 3]) -> Self {
        Self::Xyz(v)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Primitive {
    Point {
        position: PointMm,
    },
    Line {
        start: PointMm,
        end: PointMm,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Rectangle {
        start: PointMm,
        end: PointMm,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Circle {
        center: PointMm,
        radius: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Arc {
        center: PointMm,
        start: PointMm,
        sweep: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Polygon {
        center: PointMm,
        radius: f64,
        sides: usize,
        #[serde(default)]
        angle: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Slot {
        start: PointMm,
        end: PointMm,
        width: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
    Spline {
        points: Vec<PointMm>,
        #[serde(default)]
        closed: bool,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "construction_flag"
        )]
        construction: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub items: Vec<Primitive>,
}

/// Prepared geometry in centimeters/radians, not created native entities.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    Point {
        position_cm: [f64; 3],
    },
    Line {
        start_cm: [f64; 3],
        end_cm: [f64; 3],
    },
    Circle {
        center_cm: [f64; 3],
        radius_cm: f64,
    },
    Arc {
        center_cm: [f64; 3],
        start_cm: [f64; 3],
        end_cm: [f64; 3],
        sweep_radians: f64,
    },
    /// No interpolation, knots, solver validity or closed-curve shape is claimed.
    FittedSpline {
        fit_points_cm: Vec<[f64; 3]>,
        closed: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entity {
    pub geometry: Geometry,
    /// None means no requested flag assignment, distinct from explicit false.
    pub construction: Option<bool>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveKind {
    Point,
    Line,
    Rectangle,
    Circle,
    Arc,
    Polygon,
    Slot,
    Spline,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreparedItem {
    pub input_index: usize,
    pub primitive: PrimitiveKind,
    pub entities: Vec<Entity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_entities: usize,
    pub max_stored_points: usize,
    pub max_output_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entities: MAX_ENTITIES,
            max_stored_points: MAX_STORED_POINTS,
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }
}
