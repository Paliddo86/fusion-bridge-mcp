//! Pure graph-to-BOM business logic. Native collection, API identity equality,
//! target freshness, configuration inspection and file publication remain
//! adapter responsibilities. This crate never calls Fusion or Python.

pub mod configuration;
mod csv;
mod engine;
mod model;

pub use configuration::*;
pub use csv::{csv_bytes, CsvMode};
pub use engine::{generate, generate_with_limits};
pub use model::*;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}
impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

pub const MAX_GRAPH_ENTITIES: usize = 20_000;
pub const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CSV_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub explode_purchased: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    /// Shared purchasing and physical component-instance visit budget.
    pub max_component_visits: usize,
    pub max_physical_body_instances: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 100,
            max_component_visits: 20_000,
            max_physical_body_instances: 200_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartRow {
    pub component_id: String,
    #[serde(flatten)]
    pub metadata: Metadata,
    pub quantity: u64,
    pub occurrence_paths: Vec<String>,
    pub scope: String,
    pub unit_mass: f64,
    pub unit_volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRow {
    pub path: String,
    pub parent_path: Option<String>,
    pub depth: usize,
    pub component_id: String,
    pub metadata: Metadata,
    pub body_count: usize,
    pub child_count: usize,
    pub as_purchased: bool,
    pub component_ref: EntityRef,
    pub occurrence_ref: Option<EntityRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exclusion {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalTotals {
    pub mass: f64,
    pub volume: f64,
    pub body_instances: usize,
    pub scope: String,
    pub material_basis: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Units {
    pub mass: String,
    pub volume: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bom {
    pub target: Target,
    /// Complete calculation over the supplied witness. This does not certify
    /// that a future native adapter captured the witness correctly.
    pub complete: bool,
    pub configuration: ConfigurationProvenance,
    pub configuration_suppression: Vec<ConfigurationCell>,
    pub parts: Vec<PartRow>,
    pub instances: Vec<InstanceRow>,
    pub excluded: Vec<Exclusion>,
    pub part_count: usize,
    pub total_purchasing_instances: u64,
    pub physical: PhysicalTotals,
    pub units: Units,
    pub calculation_accuracy: String,
    pub identity_scope: String,
}

pub(crate) fn bounded_serialized<T: Serialize>(value: &T, limit: usize) -> Result<()> {
    serialized_len(value, limit).map(|_| ())
}

pub(crate) fn serialized_len<T: Serialize>(value: &T, limit: usize) -> Result<usize> {
    struct Sink(usize);
    impl std::io::Write for Sink {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            if value.len() > self.0 {
                return Err(std::io::Error::other("byte budget exceeded"));
            }
            self.0 -= value.len();
            Ok(value.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink(limit);
    serde_json::to_writer(&mut sink, value).map_err(|_| {
        Error::new(
            "bom_budget",
            "Serialized BOM input/output exceeds its byte budget",
        )
    })?;
    Ok(limit - sink.0)
}
