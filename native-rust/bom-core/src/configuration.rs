use crate::{Error, Result, SourceLink, Target};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationWitness {
    pub target: Target,
    pub configured: bool,
    pub is_configuration: bool,
    pub configuration_row_id: Option<String>,
    pub complete: bool,
    pub unsupported: Vec<String>,
    pub read_errors: Vec<String>,
    /// Nonempty means externally configured inserted occurrences; unsupported
    /// in the legacy complete-BOM contract and deliberately still refused.
    pub configured_occurrences: Vec<ConfiguredOccurrence>,
    pub table: Option<ConfigurationTable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfiguredOccurrence {
    pub occurrence_id: String,
    pub path: String,
    pub table_id: String,
    pub row_id: String,
    pub row_name: String,
    pub source_file_id: String,
    pub source_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationTable {
    pub id: String,
    pub active_row_id: String,
    pub active_row_name: String,
    pub has_custom_columns: bool,
    pub theme_table_ids: Vec<String>,
    /// Includes the optional reserved test row; it cannot be the active row.
    pub rows: Vec<ConfigurationRow>,
    pub columns: Vec<ConfigurationColumn>,
    pub cells: Vec<ConfigurationCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowScope {
    #[serde(rename = "ordinary")]
    Ordinary,
    #[serde(rename = "test")]
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationRow {
    pub id: String,
    pub name: String,
    pub scope: RowScope,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnKind {
    Parameter,
    Suppress,
    Visibility,
    Property,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationColumn {
    pub id: String,
    pub kind: ColumnKind,
    pub source: Option<SourceLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CellValue {
    ConfigurationSuppressCell {
        #[serde(rename = "isSuppressed")]
        is_suppressed: bool,
        #[serde(rename = "isEffectivelySuppressed")]
        is_effectively_suppressed: bool,
    },
    ConfigurationVisibilityCell {
        #[serde(rename = "isVisible")]
        is_visible: bool,
    },
    /// The native adapter's complete configuration witness must separately
    /// validate parameter value types/units. BOM only needs its cell address.
    ConfigurationParameterCell {
        expression: String,
    },
    ConfigurationPropertyCell {
        value: String,
    },
}

impl CellValue {
    fn kind(&self) -> ColumnKind {
        match self {
            Self::ConfigurationSuppressCell { .. } => ColumnKind::Suppress,
            Self::ConfigurationVisibilityCell { .. } => ColumnKind::Visibility,
            Self::ConfigurationParameterCell { .. } => ColumnKind::Parameter,
            Self::ConfigurationPropertyCell { .. } => ColumnKind::Property,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationCell {
    pub row_id: String,
    pub column_id: String,
    pub scope: RowScope,
    #[serde(flatten)]
    pub value: CellValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationProvenance {
    pub configured: bool,
    pub is_configuration: bool,
    pub table_id: Option<String>,
    pub row_id: Option<String>,
    pub row_name: Option<String>,
    pub scope: String,
    pub graph_complete: bool,
}

pub(crate) struct ConfigurationContext<'a> {
    pub provenance: ConfigurationProvenance,
    pub suppression_cells: Vec<ConfigurationCell>,
    pub suppressed_sources: Vec<&'a SourceLink>,
}

impl ConfigurationWitness {
    pub(crate) fn validate(&self, target: &Target) -> Result<ConfigurationContext<'_>> {
        let unsupported = |message: &str| Error::new("configuration_scope_unsupported", message);
        let invalid = |message: &str| Error::new("configuration_read_failed", message);
        if &self.target != target {
            return Err(Error::new(
                "configuration_source_mismatch",
                "Configuration witness belongs to another target/revision",
            ));
        }
        if !self.complete
            || !self.unsupported.is_empty()
            || !self.read_errors.is_empty()
            || self.is_configuration
            || !self.configured_occurrences.is_empty()
        {
            return Err(unsupported(
                "A complete supported configuration witness is required for BOM generation",
            ));
        }
        if self
            .configuration_row_id
            .as_ref()
            .is_some_and(|id| !id.is_empty())
        {
            return Err(invalid("Native configuration flag and row ID disagree"));
        }
        if self.configured != self.table.is_some() {
            return Err(invalid(
                "Configured-design flag and table inventory disagree",
            ));
        }
        let mut context = ConfigurationContext {
            provenance: ConfigurationProvenance {
                configured: self.configured,
                is_configuration: false,
                table_id: None,
                row_id: None,
                row_name: None,
                scope: "current active configuration only".into(),
                graph_complete: true,
            },
            suppression_cells: Vec::new(),
            suppressed_sources: Vec::new(),
        };
        let Some(table) = &self.table else {
            return Ok(context);
        };
        if table.has_custom_columns || !table.theme_table_ids.is_empty() {
            return Err(unsupported(
                "Custom columns and theme-table contents remain unsupported",
            ));
        }
        if table.rows.is_empty()
            || table.rows.len() > 128
            || table.columns.len() > 128
            || table.rows.len().saturating_mul(table.columns.len()) > 4096
            || table.cells.len() > 4096
        {
            return Err(Error::new(
                "configuration_budget",
                "Configuration inventory exceeds its budget",
            ));
        }
        if table.id.is_empty() {
            return Err(invalid("Configuration table ID is missing"));
        }
        let mut rows = BTreeMap::new();
        for row in &table.rows {
            if row.id.is_empty() || rows.insert(row.id.as_str(), row).is_some() {
                return Err(invalid("Missing or duplicate configuration row IDs"));
            }
            if row.parent_id.is_some() || !row.child_ids.is_empty() {
                return Err(unsupported(
                    "Hierarchical configuration rows remain unsupported",
                ));
            }
        }
        if rows
            .values()
            .filter(|row| row.scope == RowScope::Test)
            .count()
            > 1
        {
            return Err(invalid("Multiple reserved test rows"));
        }
        let active = rows
            .get(table.active_row_id.as_str())
            .ok_or_else(|| invalid("Active row is outside ordinary rows"))?;
        if active.scope != RowScope::Ordinary || active.name != table.active_row_name {
            return Err(invalid("Active row identity/name is inconsistent"));
        }
        let mut columns = BTreeMap::new();
        for column in &table.columns {
            if column.id.is_empty() || columns.insert(column.id.as_str(), column).is_some() {
                return Err(invalid("Missing or duplicate configuration column IDs"));
            }
            if column.kind != ColumnKind::Property && column.source.is_none() {
                return Err(invalid(
                    "Configuration controlled-source witness is missing",
                ));
            }
        }
        let mut addresses = BTreeSet::new();
        for cell in &table.cells {
            let row = rows
                .get(cell.row_id.as_str())
                .ok_or_else(|| invalid("Cell row is outside table"))?;
            let column = columns
                .get(cell.column_id.as_str())
                .ok_or_else(|| invalid("Cell column is outside table"))?;
            if !addresses.insert((&cell.row_id, &cell.column_id))
                || cell.scope != row.scope
                || cell.value.kind() != column.kind
            {
                return Err(invalid("Cell address, scope, or type is inconsistent"));
            }
            if cell.row_id == table.active_row_id {
                if let CellValue::ConfigurationSuppressCell {
                    is_effectively_suppressed,
                    ..
                } = cell.value
                {
                    context.suppression_cells.push(cell.clone());
                    if is_effectively_suppressed {
                        context
                            .suppressed_sources
                            .push(column.source.as_ref().expect("source checked"));
                    }
                }
            }
        }
        if addresses.len() != rows.len() * columns.len() {
            return Err(invalid("Configuration cell inventory is incomplete"));
        }
        context.provenance.table_id = Some(table.id.clone());
        context.provenance.row_id = Some(table.active_row_id.clone());
        context.provenance.row_name = Some(table.active_row_name.clone());
        Ok(context)
    }
}
