use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub document_id: String,
    pub revision: String,
}

/// Native entity tokens are opaque references, never definition identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityRef {
    pub document_id: String,
    pub revision: String,
    pub token: String,
    pub object_type: String,
    pub occurrence_token: Option<String>,
    pub occurrence_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_component_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Material {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    pub part_number: String,
    pub description: String,
    pub attributes: BTreeMap<String, String>,
    pub material: Option<Material>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplier_part_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_purchased: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_from_bom: Option<bool>,
}

impl Metadata {
    /// `attributes` contains only the native AdomMechanical group. Recognized
    /// attributes are JSON-encoded strings/booleans, as in the legacy add-in.
    pub fn from_attributes(
        name: String,
        part_number: String,
        description: String,
        attributes: BTreeMap<String, String>,
        material: Option<Material>,
    ) -> Result<Self> {
        fn read<T: serde::de::DeserializeOwned>(
            attrs: &BTreeMap<String, String>,
            key: &str,
        ) -> Result<Option<T>> {
            attrs
                .get(key)
                .map(|raw| {
                    serde_json::from_str(raw).map_err(|error| {
                        Error::new(
                            "invalid_metadata",
                            format!("Attribute {key} has an invalid type or JSON: {error}"),
                        )
                    })
                })
                .transpose()
        }
        Ok(Self {
            name,
            part_number,
            description,
            material,
            supplier: read(&attributes, "supplier")?,
            supplier_part_number: read(&attributes, "supplierPartNumber")?,
            source_url: read(&attributes, "sourceUrl")?,
            source_file: read(&attributes, "sourceFile")?,
            sha256: read(&attributes, "sha256")?,
            as_purchased: read(&attributes, "asPurchased")?,
            exclude_from_bom: read(&attributes, "excludeFromBom")?,
            attributes,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let decoded = Self::from_attributes(
            self.name.clone(),
            self.part_number.clone(),
            self.description.clone(),
            self.attributes.clone(),
            self.material.clone(),
        )?;
        if &decoded != self {
            return Err(Error::new(
                "invalid_metadata",
                "Decoded metadata disagrees with native attribute values",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhysicalProperties {
    pub mass_kg: f64,
    pub volume_mm3: f64,
}

impl PhysicalProperties {
    pub fn from_fusion_units(mass_kg: f64, volume_cm3: f64) -> Result<Self> {
        let value = Self {
            mass_kg,
            volume_mm3: volume_cm3 * 1000.0,
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn validate(self) -> Result<()> {
        if !self.mass_kg.is_finite()
            || !self.volume_mm3.is_finite()
            || self.mass_kg < 0.0
            || self.volume_mm3 < 0.0
        {
            return Err(Error::new(
                "invalid_physical_properties",
                "Mass and volume must be finite and nonnegative",
            ));
        }
        Ok(())
    }

    pub(crate) fn add(&mut self, other: Self) -> Result<()> {
        self.mass_kg += other.mass_kg;
        self.volume_mm3 += other.volume_mm3;
        self.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    /// Session-local SDK equality identity, never a name, token or part number.
    pub id: String,
    /// Alternate wrapper identities already proven equal by the native adapter.
    pub aliases: Vec<String>,
    pub metadata: Metadata,
    pub reference: EntityRef,
    pub direct_bodies: Vec<PhysicalProperties>,
    /// Native component.getPhysicalProperties(High), required for a collapsed
    /// purchased row. This is not inferred from purchasing child rows.
    pub subtree_properties: Option<PhysicalProperties>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceLink {
    pub entity_id: String,
    pub object_type: String,
    pub owner_id: String,
    pub owner_scope: OwnerScope,
    pub occurrence_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerScope {
    Component,
    Design,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Occurrence {
    pub id: String,
    pub path: String,
    pub component_id: String,
    pub reference: EntityRef,
    pub source: SourceLink,
    /// None means the adapter successfully observed there is no timeline object.
    /// A failed suppression read must instead make the witness incomplete.
    pub timeline_suppressed: Option<bool>,
    /// Visibility has no effect on purchasing or physical inclusion.
    pub visible: bool,
    /// Context-specific occurrence.bRepBodies physical observations.
    pub direct_bodies: Vec<PhysicalProperties>,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Root {
    pub component_id: String,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalculationAccuracy {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotWitness {
    pub session_id: String,
    /// SDK identity of the Design object, distinct from document identity.
    pub design_id: String,
    pub target: Target,
    pub complete: bool,
    pub read_errors: Vec<String>,
    pub calculation_accuracy: CalculationAccuracy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssemblySnapshot {
    pub target: Target,
    pub witness: SnapshotWitness,
    pub configuration: crate::configuration::ConfigurationWitness,
    pub root: Root,
    pub components: Vec<Component>,
    pub occurrences: Vec<Occurrence>,
}
