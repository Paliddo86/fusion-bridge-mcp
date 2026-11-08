//! Bounded native assembly collector. Identity equality, traversal, metadata,
//! revision witnesses and calculation policy are Rust. No SDK mutation or I/O.
use fusion_bom_core as core;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const MAX_COMPONENTS: usize = 128;
pub const MAX_OCCURRENCES: usize = 512;
pub const MAX_BODIES: usize = 2048;
pub const MAX_DEPTH: usize = 32;
pub const MAX_BYTES: usize = 512 * 1024;
pub const REVISION_SCOPE: &str = "bom-projection-v1: two exact-object assembly observations, component revisions, AdomMechanical metadata, material identity and high-accuracy definition/occurrence body properties; not a complete geometry or document revision";
pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Design {
    pub active: bool,
    pub hybrid: bool,
    pub configured: bool,
    pub configuration: bool,
    pub table_present: bool,
    pub row_id: String,
    pub components: usize,
    pub timeline_count: usize,
    pub timeline_position: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Definition {
    pub name: String,
    pub part_number: String,
    pub description: String,
    pub revision: String,
    pub token: String,
    pub attributes: BTreeMap<String, String>,
    pub material: Option<core::Material>,
    pub mesh_bodies: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Instance {
    pub path: String,
    pub token: String,
    pub referenced: bool,
    pub configured: bool,
    pub timeline_present: bool,
    pub suppressed: bool,
    pub rolled_back: bool,
    pub visible: bool,
}
/// Handle is a typed retained view index, never an address or identity. The
/// backend retains all acquisitions until checked owner-thread close. same()
/// compares retained SDK references of the same static type, not token text.
pub trait Backend {
    type Handle: Copy;
    fn is_offline(&self) -> Result<bool>;
    fn design(&self) -> Result<Design>;
    fn root(&self) -> Result<Self::Handle>;
    fn same(&self, a: Self::Handle, b: Self::Handle) -> Result<bool>;
    fn definition(&self, component: Self::Handle) -> Result<Definition>;
    fn component(&self, occurrence: Self::Handle) -> Result<Self::Handle>;
    fn instance(&self, occurrence: Self::Handle) -> Result<Instance>;
    fn child_count(&self, owner: Self::Handle) -> Result<usize>;
    fn child(&self, owner: Self::Handle, index: usize) -> Result<Self::Handle>;
    fn body_count(&self, owner: Self::Handle) -> Result<usize>;
    /// Must read occurrence-context bodies when owner is an occurrence; high
    /// accuracy and solid-body support must be verified, never guessed.
    fn body(&self, owner: Self::Handle, index: usize) -> Result<core::PhysicalProperties>;
    fn subtree_properties(&self, component: Self::Handle) -> Result<core::PhysicalProperties>;
}
#[derive(Serialize, PartialEq)]
struct Def {
    data: Definition,
    metadata: core::Metadata,
    bodies: Vec<core::PhysicalProperties>,
    subtree: Option<core::PhysicalProperties>,
}
#[derive(Serialize, PartialEq)]
struct Occ {
    data: Instance,
    component: usize,
    children: Vec<usize>,
    bodies: Vec<core::PhysicalProperties>,
}
#[derive(Serialize, PartialEq)]
struct Projection {
    design: Design,
    components: Vec<Def>,
    occurrences: Vec<Occ>,
    children: Vec<usize>,
}
struct Collector<'a, B: Backend> {
    backend: &'a B,
    projection: Projection,
    definitions: Vec<B::Handle>,
    instances: Vec<B::Handle>,
    body_reads: usize,
    text_bytes: usize,
}
fn require(ok: bool, code: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(code.into())
    }
}
fn physical(value: core::PhysicalProperties) -> Result<core::PhysicalProperties> {
    require(
        value.mass_kg.is_finite()
            && value.volume_mm3.is_finite()
            && value.mass_kg >= 0.0
            && value.volume_mm3 >= 0.0,
        "native_bom_invalid_physical",
    )?;
    Ok(value)
}
impl<'a, B: Backend> Collector<'a, B> {
    fn text(&mut self, text: &str) -> Result<()> {
        require(
            text.len() <= 16 * 1024 && !text.contains('\0'),
            "native_bom_text_bound",
        )?;
        self.text_bytes = self
            .text_bytes
            .checked_add(text.len())
            .ok_or("native_bom_byte_bound")?;
        require(self.text_bytes <= MAX_BYTES / 2, "native_bom_byte_bound")
    }
    fn bodies(&mut self, owner: B::Handle) -> Result<Vec<core::PhysicalProperties>> {
        let count = self.backend.body_count(owner)?;
        self.body_reads = self
            .body_reads
            .checked_add(count)
            .ok_or("native_bom_body_bound")?;
        require(self.body_reads <= MAX_BODIES, "native_bom_body_bound")?;
        (0..count)
            .map(|i| self.backend.body(owner, i).and_then(physical))
            .collect()
    }
    fn definition(&mut self, handle: B::Handle) -> Result<usize> {
        for (index, previous) in self.definitions.iter().enumerate() {
            if self.backend.same(*previous, handle)? {
                return Ok(index);
            }
        }
        require(
            self.definitions.len() < MAX_COMPONENTS,
            "native_bom_component_bound",
        )?;
        let data = self.backend.definition(handle)?;
        require(
            data.mesh_bodies == 0 && !data.revision.is_empty() && !data.token.is_empty(),
            "native_bom_definition_scope",
        )?;
        for text in [
            &data.name,
            &data.part_number,
            &data.description,
            &data.revision,
            &data.token,
        ] {
            self.text(text)?;
        }
        require(data.attributes.len() <= 128, "native_bom_metadata_bound")?;
        for (key, value) in &data.attributes {
            self.text(key)?;
            self.text(value)?;
        }
        if let Some(material) = &data.material {
            self.text(&material.id)?;
            self.text(&material.name)?;
        }
        let metadata = core::Metadata::from_attributes(
            data.name.clone(),
            data.part_number.clone(),
            data.description.clone(),
            data.attributes.clone(),
            data.material.clone(),
        )
        .map_err(|_| "native_bom_invalid_metadata")?;
        let bodies = self.bodies(handle)?;
        let subtree = if metadata.as_purchased == Some(true) {
            Some(physical(self.backend.subtree_properties(handle)?)?)
        } else {
            None
        };
        let index = self.definitions.len();
        self.definitions.push(handle);
        self.projection.components.push(Def {
            data,
            metadata,
            bodies,
            subtree,
        });
        Ok(index)
    }
    fn children(
        &mut self,
        owner: B::Handle,
        depth: usize,
        ancestry: &[usize],
    ) -> Result<Vec<usize>> {
        require(depth <= MAX_DEPTH, "native_bom_depth_bound")?;
        let count = self.backend.child_count(owner)?;
        require(
            count <= MAX_OCCURRENCES.saturating_sub(self.instances.len()),
            "native_bom_occurrence_bound",
        )?;
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let handle = self.backend.child(owner, index)?;
            for previous in &self.instances {
                require(
                    !self.backend.same(*previous, handle)?,
                    "native_bom_duplicate_occurrence",
                )?;
            }
            let data = self.backend.instance(handle)?;
            require(
                !data.referenced && !data.configured && !data.suppressed && !data.rolled_back,
                "native_bom_occurrence_scope",
            )?;
            require(
                !data.path.is_empty() && !data.token.is_empty(),
                "native_bom_missing_reference",
            )?;
            self.text(&data.path)?;
            self.text(&data.token)?;
            let component_handle = self.backend.component(handle)?;
            let component = self.definition(component_handle)?;
            require(!ancestry.contains(&component), "native_bom_cycle")?;
            let bodies = self.bodies(handle)?;
            require(
                bodies.len() == self.projection.components[component].bodies.len(),
                "native_bom_context_body_mismatch",
            )?;
            let serial = self.instances.len();
            require(serial < MAX_OCCURRENCES, "native_bom_occurrence_bound")?;
            self.instances.push(handle);
            self.projection.occurrences.push(Occ {
                data,
                component,
                children: vec![],
                bodies,
            });
            let mut next = ancestry.to_vec();
            next.push(component);
            let children = self.children(handle, depth + 1, &next)?;
            self.projection.occurrences[serial].children = children;
            result.push(serial);
        }
        require(
            self.backend.child_count(owner)? == count,
            "native_bom_changed_collection",
        )?;
        Ok(result)
    }
    fn capture(backend: &'a B) -> Result<Self> {
        require(
            !backend.is_offline()?,
            "native_bom_online_required_for_component_metadata",
        )?;
        let design = backend.design()?;
        require(
            design.active
                && design.hybrid
                && !design.configured
                && !design.configuration
                && !design.table_present
                && design.row_id.is_empty(),
            "native_bom_design_scope",
        )?;
        require(
            design.components > 0
                && design.components <= MAX_COMPONENTS
                && design.timeline_count <= 1024
                && design.timeline_position == design.timeline_count,
            "native_bom_design_bound_or_rollback",
        )?;
        let mut capture = Self {
            backend,
            projection: Projection {
                design,
                components: vec![],
                occurrences: vec![],
                children: vec![],
            },
            definitions: vec![],
            instances: vec![],
            body_reads: 0,
            text_bytes: 0,
        };
        let root = backend.root()?;
        require(capture.definition(root)? == 0, "native_bom_root_identity")?;
        capture.projection.children = capture.children(root, 0, &[0])?;
        require(
            capture.definitions.len() == capture.projection.design.components,
            "native_bom_unreachable_definition",
        )?;
        require(
            backend.design()? == capture.projection.design,
            "native_bom_changed_design",
        )?;
        Ok(capture)
    }
}
fn entity(
    target: &core::Target,
    token: &str,
    kind: &str,
    path: Option<&str>,
    root: &str,
) -> core::EntityRef {
    core::EntityRef {
        document_id: target.document_id.clone(),
        revision: target.revision.clone(),
        token: token.into(),
        object_type: format!("adsk::fusion::{kind}"),
        occurrence_token: if kind == "Occurrence" {
            Some(token.into())
        } else {
            None
        },
        occurrence_path: path.map(str::to_owned),
        root_component_id: Some(root.into()),
    }
}
/// Complete only for the explicitly supported local/unconfigured/unsuppressed
/// BRep subset. Two passes compare actual references as well as all BOM fields.
pub fn generate<B: Backend>(
    backend: &B,
    session: &str,
    document: &str,
    expected_revision: Option<&str>,
    explode_purchased: bool,
) -> Result<Value> {
    capture(
        backend,
        session,
        document,
        expected_revision,
        explode_purchased,
    )
    .map(|result| result.data)
}
#[derive(Debug)]
pub struct Captured {
    pub(crate) data: Value,
    pub(crate) calculation: core::Bom,
}
impl Captured {
    pub fn data(&self) -> &Value {
        &self.data
    }
    pub fn into_data(self) -> Value {
        self.data
    }
}
pub fn capture<B: Backend>(
    backend: &B,
    session: &str,
    document: &str,
    expected_revision: Option<&str>,
    explode_purchased: bool,
) -> Result<Captured> {
    require(
        !session.is_empty() && !document.is_empty(),
        "native_bom_identity_missing",
    )?;
    let first = Collector::capture(backend)?;
    let second = Collector::capture(backend)?;
    require(
        first.projection == second.projection
            && first.definitions.len() == second.definitions.len()
            && first.instances.len() == second.instances.len(),
        "native_bom_changed_snapshot",
    )?;
    for (a, b) in first
        .definitions
        .iter()
        .zip(&second.definitions)
        .chain(first.instances.iter().zip(&second.instances))
    {
        require(backend.same(*a, *b)?, "native_bom_replaced_identity")?;
    }
    let projection = first.projection;
    let encoded = serde_json::to_vec(&projection).map_err(|_| "native_bom_encoding_failed")?;
    require(encoded.len() <= MAX_BYTES, "native_bom_byte_bound")?;
    let revision = format!(
        "bom-projection-v1:{}",
        fusion_mechanical_journal::request_digest(&encoded)
    );
    require(
        expected_revision.is_none_or(|expected| expected == revision),
        "target_revision_mismatch",
    )?;
    let target = core::Target {
        document_id: document.into(),
        revision,
    };
    // IDs are explicitly scoped to this successful capture. A fresh nonce
    // prevents accidentally treating collection slots as cross-capture identity.
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "native_bom_entropy_unavailable")?;
    let scope: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let cid = |index| format!("bom:{scope}:c:{index}");
    let oid = |index| format!("bom:{scope}:o:{index}");
    let root_id = cid(0);
    let components = projection
        .components
        .iter()
        .enumerate()
        .map(|(index, c)| core::Component {
            id: cid(index),
            aliases: vec![],
            metadata: c.metadata.clone(),
            reference: entity(&target, &c.data.token, "Component", None, &root_id),
            direct_bodies: c.bodies.clone(),
            subtree_properties: c.subtree,
        })
        .collect();
    let occurrences = projection
        .occurrences
        .iter()
        .enumerate()
        .map(|(index, o)| core::Occurrence {
            id: oid(index),
            path: o.data.path.clone(),
            component_id: cid(o.component),
            reference: entity(
                &target,
                &o.data.token,
                "Occurrence",
                Some(&o.data.path),
                &root_id,
            ),
            source: core::SourceLink {
                entity_id: oid(index),
                object_type: "adsk::fusion::Occurrence".into(),
                owner_id: cid(o.component),
                owner_scope: core::OwnerScope::Component,
                occurrence_id: Some(oid(index)),
            },
            timeline_suppressed: o.data.timeline_present.then_some(false),
            visible: o.data.visible,
            direct_bodies: o.bodies.clone(),
            children: o.children.iter().map(|i| oid(*i)).collect(),
        })
        .collect();
    let snapshot = core::AssemblySnapshot {
        target: target.clone(),
        witness: core::SnapshotWitness {
            session_id: session.into(),
            design_id: format!("bom:{scope}:design"),
            target: target.clone(),
            complete: true,
            read_errors: vec![],
            calculation_accuracy: core::CalculationAccuracy::High,
        },
        configuration: core::ConfigurationWitness {
            target: target.clone(),
            configured: false,
            is_configuration: false,
            configuration_row_id: None,
            complete: true,
            unsupported: vec![],
            read_errors: vec![],
            configured_occurrences: vec![],
            table: None,
        },
        root: core::Root {
            component_id: root_id,
            children: projection.children.iter().map(|i| oid(*i)).collect(),
        },
        components,
        occurrences,
    };
    let mut bom = core::generate_with_limits(
        &snapshot,
        core::Options { explode_purchased },
        core::Limits {
            max_depth: MAX_DEPTH,
            max_component_visits: 2 * (MAX_OCCURRENCES + 1),
            max_physical_body_instances: MAX_BODIES,
        },
    )
    .map_err(|error| error.code.to_owned())?;
    bom.identity_scope = format!("one complete native BOM snapshot {scope}; exact retained SDK equality within capture; no cross-capture definition-ID stability");
    let mut result = serde_json::to_value(&bom).map_err(|_| "native_bom_encoding_failed")?;
    result["revisionScope"] = json!(REVISION_SCOPE);
    result["nativeScope"] = json!("online Fusion; local unconfigured hybrid assemblies; unsuppressed current timeline; solid BRep only; no meshes or external/configured occurrences");
    result["legacyParity"] = json!(false);
    require(
        serde_json::to_vec(&result)
            .map_err(|_| "native_bom_encoding_failed")?
            .len()
            <= MAX_BYTES,
        "native_bom_byte_bound",
    )?;
    Ok(Captured {
        data: result,
        calculation: bom,
    })
}

#[cfg(test)]
pub(crate) mod tests;

/// Preserve the exact ABI getter scope in an SDK refusal. No success or scope guard is relaxed.
pub(crate) fn sdk_status(call: &str, value: i32) -> Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(format!("native_bom_sdk_{call}_status_{value}"))
    }
}

/// Diagnostic metadata never changes the original failed status into success.
/// Description is bounded and JSON escaped to keep arbitrary SDK text unambiguous.
pub(crate) fn sdk_diagnostic(
    call: &str,
    code: i32,
    stage: Option<&str>,
    detail: Option<(i32, u32, Option<&str>)>,
) -> String {
    let base = format!(
        "native_bom_sdk_{call}_{}_status_{code}",
        stage.unwrap_or("unavailable")
    );
    match detail {
        Some((number, object, description)) => {
            let bounded: String = description
                .unwrap_or("unavailable")
                .chars()
                .take(1024)
                .collect();
            format!(
                "{base}; sdkError={number}; definitionHandle={object}; description={}",
                serde_json::to_string(&bounded).unwrap_or_else(|_| "\"unavailable\"".into())
            )
        }
        None => base,
    }
}

/// Independent fields prevent optional-description failure from erasing stage.
#[derive(Default)]
pub(crate) struct DiagnosticFields {
    pub stage: Option<String>,
    pub description: Option<String>,
}
impl DiagnosticFields {
    pub fn store(&mut self, field: u32, bytes: &[u8]) -> std::result::Result<(), ()> {
        match field {
            0 if self.stage.is_none() && bytes.len() < 64 => {
                self.stage = Some(std::str::from_utf8(bytes).map_err(|_| ())?.to_owned());
            }
            1 if self.description.is_none() && bytes.len() <= 1024 => {
                self.description = Some(String::from_utf8_lossy(bytes).into_owned());
            }
            _ => return Err(()),
        }
        Ok(())
    }
}
