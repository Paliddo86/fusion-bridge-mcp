use crate::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub fn generate(snapshot: &AssemblySnapshot, options: Options) -> Result<Bom> {
    generate_with_limits(snapshot, options, Limits::default())
}

pub fn generate_with_limits(
    snapshot: &AssemblySnapshot,
    options: Options,
    limits: Limits,
) -> Result<Bom> {
    let maximum = Limits::default();
    if limits.max_depth > maximum.max_depth
        || limits.max_component_visits == 0
        || limits.max_component_visits > maximum.max_component_visits
        || limits.max_physical_body_instances > maximum.max_physical_body_instances
    {
        return Err(Error::new(
            "invalid_limits",
            "BOM limits may only reduce the hard limits",
        ));
    }
    bounded_serialized(snapshot, MAX_SNAPSHOT_BYTES)?;
    if snapshot.target.document_id.is_empty()
        || snapshot.target.revision.is_empty()
        || snapshot.witness.session_id.is_empty()
        || snapshot.witness.design_id.is_empty()
        || snapshot.witness.target != snapshot.target
        || !snapshot.witness.complete
        || !snapshot.witness.read_errors.is_empty()
        || snapshot.witness.calculation_accuracy != CalculationAccuracy::High
    {
        return Err(Error::new(
            "source_incomplete",
            "A complete current-target high-accuracy source witness is required",
        ));
    }
    let config = snapshot.configuration.validate(&snapshot.target)?;
    let graph = Graph::new(snapshot)?;
    for column in snapshot
        .configuration
        .table
        .iter()
        .flat_map(|table| &table.columns)
    {
        if let Some(source) = &column.source {
            graph.validate_configuration_source(source)?;
        }
    }
    let mut run = Run {
        graph: &graph,
        options,
        limits,
        visited: 0,
        physical: PhysicalProperties::default(),
        body_instances: 0,
        parts: Vec::new(),
        part_indexes: BTreeMap::new(),
        instances: Vec::new(),
        excluded: Vec::new(),
        suppressed_sources: &config.suppressed_sources,
        output_budget: MAX_SNAPSHOT_BYTES,
    };
    run.purchasing(None, None, 0, &mut Vec::new())?;
    run.physical_walk(None, 0, &mut Vec::new())?;
    let total_purchasing_instances = run.parts.iter().map(|row| row.quantity).sum();
    let result = Bom {
        target: snapshot.target.clone(), complete: true,
        configuration: config.provenance, configuration_suppression: config.suppression_cells,
        part_count: run.parts.len(), total_purchasing_instances,
        parts: run.parts, instances: run.instances, excluded: run.excluded,
        physical: PhysicalTotals {
            mass: run.physical.mass_kg, volume: run.physical.volume_mm3, body_instances: run.body_instances,
            scope: "all unsuppressed direct-body instances; BOM exclusions do not remove physical mass".into(),
            material_basis: "Fusion assigned/default material; supplier provenance does not certify modeled material density".into(),
        },
        units: Units { mass: "kg".into(), volume: "mm3".into() },
        calculation_accuracy: "high".into(),
        identity_scope: "running add-in session; supplier metadata persists in native archives".into(),
    };
    bounded_serialized(&result, MAX_SNAPSHOT_BYTES)?;
    Ok(result)
}

struct Graph<'a> {
    source: &'a AssemblySnapshot,
    components: BTreeMap<&'a str, &'a Component>,
    occurrences: BTreeMap<&'a str, &'a Occurrence>,
}

impl<'a> Graph<'a> {
    fn new(source: &'a AssemblySnapshot) -> Result<Self> {
        if source.components.is_empty()
            || source.components.len() > MAX_GRAPH_ENTITIES
            || source.occurrences.len() > MAX_GRAPH_ENTITIES
        {
            return Err(Error::new(
                "bom_budget",
                "Assembly entity inventory exceeds budget",
            ));
        }
        let mut graph = Self {
            source,
            components: BTreeMap::new(),
            occurrences: BTreeMap::new(),
        };
        let mut paths = BTreeSet::new();
        let mut aliases = 0;
        for component in &source.components {
            component.metadata.validate()?;
            graph.validate_reference(&component.reference, "adsk::fusion::Component")?;
            if component.direct_bodies.len() > MAX_GRAPH_ENTITIES {
                return Err(Error::new(
                    "bom_budget",
                    "Component body inventory exceeds budget",
                ));
            }
            for properties in &component.direct_bodies {
                properties.validate()?;
            }
            if let Some(properties) = component.subtree_properties {
                properties.validate()?;
            }
            for id in std::iter::once(&component.id).chain(&component.aliases) {
                aliases += 1;
                if aliases > MAX_GRAPH_ENTITIES {
                    return Err(Error::new(
                        "bom_budget",
                        "Identity alias inventory exceeds budget",
                    ));
                }
                if id.is_empty() || graph.components.insert(id, component).is_some() {
                    return Err(Error::new(
                        "ambiguous_identity",
                        "Missing or ambiguous component definition identity",
                    ));
                }
            }
        }
        let root = graph.component(&source.root.component_id)?;
        if root
            .reference
            .root_component_id
            .as_deref()
            .and_then(|id| graph.components.get(id))
            .map(|component| component.id.as_str())
            != Some(root.id.as_str())
        {
            return Err(Error::new(
                "source_incomplete",
                "Root component reference lacks the matching session identity",
            ));
        }
        for occurrence in &source.occurrences {
            if occurrence.id.is_empty()
                || occurrence.path.is_empty()
                || occurrence.path == "<root>"
                || !paths.insert(&occurrence.path)
                || graph
                    .occurrences
                    .insert(&occurrence.id, occurrence)
                    .is_some()
            {
                return Err(Error::new(
                    "ambiguous_identity",
                    "Occurrence identities and full paths must be unique",
                ));
            }
            let component = graph.component(&occurrence.component_id)?;
            graph.validate_reference(&occurrence.reference, "adsk::fusion::Occurrence")?;
            if occurrence.reference.occurrence_path.as_deref() != Some(occurrence.path.as_str()) {
                return Err(Error::new(
                    "source_incomplete",
                    "Occurrence reference path does not match its instance",
                ));
            }
            graph.validate_source(&occurrence.source)?;
            if occurrence.source.object_type != "adsk::fusion::Occurrence"
                || occurrence.source.owner_scope != OwnerScope::Component
                || graph.component(&occurrence.source.owner_id)?.id != component.id
            {
                return Err(Error::new(
                    "configuration_source_mismatch",
                    "Occurrence source owner/type does not match its component",
                ));
            }
            if occurrence.direct_bodies.len() != component.direct_bodies.len() {
                return Err(Error::new(
                    "source_incomplete",
                    "Occurrence direct-body count differs from its definition",
                ));
            }
            for properties in &occurrence.direct_bodies {
                properties.validate()?;
            }
        }
        // Native occurrence instances form a tree, even though definitions are
        // reused. A shared occurrence node would otherwise double-count one body.
        let mut parents = BTreeMap::<&str, usize>::new();
        for child in source
            .root
            .children
            .iter()
            .chain(source.occurrences.iter().flat_map(|o| &o.children))
        {
            graph.occurrence(child)?;
            *parents.entry(child).or_default() += 1;
        }
        if graph
            .occurrences
            .keys()
            .any(|id| parents.get(id) != Some(&1))
        {
            return Err(Error::new(
                "invalid_assembly_graph",
                "Every occurrence must have exactly one containment parent",
            ));
        }
        let mut reached = BTreeSet::new();
        let mut queue: VecDeque<&str> = source.root.children.iter().map(String::as_str).collect();
        while let Some(id) = queue.pop_front() {
            if !reached.insert(id) {
                return Err(Error::new(
                    "cyclic_assembly",
                    "Occurrence containment is cyclic",
                ));
            }
            queue.extend(graph.occurrence(id)?.children.iter().map(String::as_str));
        }
        if reached.len() != graph.occurrences.len() {
            return Err(Error::new(
                "invalid_assembly_graph",
                "Occurrence inventory has unreachable nodes",
            ));
        }
        Ok(graph)
    }

    fn component(&self, id: &str) -> Result<&'a Component> {
        self.components.get(id).copied().ok_or_else(|| {
            Error::new(
                "source_incomplete",
                format!("Unknown component identity: {id}"),
            )
        })
    }
    fn occurrence(&self, id: &str) -> Result<&'a Occurrence> {
        self.occurrences.get(id).copied().ok_or_else(|| {
            Error::new(
                "source_incomplete",
                format!("Unknown occurrence identity: {id}"),
            )
        })
    }
    fn validate_reference(&self, reference: &EntityRef, object_type: &str) -> Result<()> {
        if reference.document_id != self.source.target.document_id
            || reference.revision != self.source.target.revision
            || reference.object_type != object_type
        {
            return Err(Error::new(
                "source_incomplete",
                "Entity reference has a foreign target/revision/type",
            ));
        }
        Ok(())
    }
    fn validate_source(&self, source: &SourceLink) -> Result<()> {
        if source.entity_id.is_empty()
            || source.owner_id.is_empty()
            || source.object_type.is_empty()
            || !(source.object_type.starts_with("adsk::fusion::")
                || source.object_type.starts_with("adsk::core::"))
            || (source.object_type.ends_with("::Occurrence")
                && source.object_type != "adsk::fusion::Occurrence")
            || source.occurrence_id.as_ref().is_some_and(String::is_empty)
        {
            return Err(Error::new(
                "configuration_source_mismatch",
                "Controlled-source identity is incomplete",
            ));
        }
        if source.owner_scope == OwnerScope::Component {
            self.component(&source.owner_id)?;
        } else if source.owner_id != self.source.witness.design_id
            || source.object_type != "adsk::fusion::UserParameter"
        {
            return Err(Error::new(
                "configuration_source_mismatch",
                "Design-owned source is not a user parameter of this Design",
            ));
        }
        Ok(())
    }

    fn validate_configuration_source(&self, source: &SourceLink) -> Result<()> {
        self.validate_source(source)?;
        if source.object_type != "adsk::fusion::Occurrence"
            && self
                .occurrences
                .values()
                .any(|occurrence| occurrence.source.entity_id == source.entity_id)
        {
            return Err(Error::new(
                "configuration_source_mismatch",
                "Controlled source type contradicts an observed occurrence identity",
            ));
        }
        if source.object_type == "adsk::fusion::Occurrence" {
            if source.owner_scope != OwnerScope::Component {
                return Err(Error::new(
                    "configuration_source_mismatch",
                    "Occurrence source must have component ownership",
                ));
            }
            let owner = self.component(&source.owner_id)?;
            let mut found = false;
            for occurrence in self.occurrences.values().filter(|occurrence| {
                occurrence.source.entity_id == source.entity_id
                    && (source.occurrence_id.is_none()
                        || source.occurrence_id == occurrence.source.occurrence_id)
            }) {
                found = true;
                if occurrence.source.object_type != source.object_type
                    || occurrence.source.owner_scope != source.owner_scope
                    || self.component(&occurrence.source.owner_id)?.id != owner.id
                {
                    return Err(Error::new(
                        "configuration_source_mismatch",
                        "Controlled occurrence source contradicts its observed owner/type",
                    ));
                }
            }
            if !found {
                return Err(Error::new(
                    "configuration_source_mismatch",
                    "Controlled occurrence source has no matching observed entity/context",
                ));
            }
        }
        Ok(())
    }
}

struct Run<'a, 'b> {
    graph: &'b Graph<'a>,
    options: Options,
    limits: Limits,
    visited: usize,
    physical: PhysicalProperties,
    body_instances: usize,
    parts: Vec<PartRow>,
    part_indexes: BTreeMap<&'a str, usize>,
    instances: Vec<InstanceRow>,
    excluded: Vec<Exclusion>,
    suppressed_sources: &'b [&'a SourceLink],
    output_budget: usize,
}

impl<'a> Run<'a, '_> {
    fn charge<T: serde::Serialize>(&mut self, value: &T) -> Result<()> {
        // Reserve bytes before cloning repeated metadata into output rows.
        // The fixed allowance covers field names, brackets and numeric values;
        // final exact serialization is checked as well.
        let bytes = serialized_len(value, self.output_budget)?.saturating_add(512);
        self.output_budget = self.output_budget.checked_sub(bytes).ok_or_else(|| {
            Error::new("bom_budget", "BOM materialization exceeds its byte budget")
        })?;
        Ok(())
    }
    fn node(
        &self,
        occurrence: Option<&'a Occurrence>,
    ) -> Result<(&'a Component, &'a [String], &'a str)> {
        match occurrence {
            Some(occurrence) => Ok((
                self.graph.component(&occurrence.component_id)?,
                &occurrence.children,
                &occurrence.path,
            )),
            None => Ok((
                self.graph.component(&self.graph.source.root.component_id)?,
                &self.graph.source.root.children,
                "<root>",
            )),
        }
    }
    fn visit(&mut self, component: &Component, depth: usize, ancestry: &[&str]) -> Result<()> {
        self.visited += 1;
        if depth > self.limits.max_depth || self.visited > self.limits.max_component_visits {
            return Err(Error::new(
                "bom_budget",
                "Assembly exceeds combined purchasing/physical traversal budget",
            ));
        }
        if ancestry.contains(&component.id.as_str()) {
            return Err(Error::new(
                "cyclic_assembly",
                "Assembly contains cyclic component containment",
            ));
        }
        Ok(())
    }
    fn suppression(&self, occurrence: &Occurrence) -> Option<&'static str> {
        if occurrence.timeline_suppressed == Some(true) {
            return Some("suppressed");
        }
        if self.suppressed_sources.iter().any(|source| {
            source.object_type == "adsk::fusion::Occurrence"
                && source.owner_scope == OwnerScope::Component
                && self
                    .graph
                    .components
                    .get(source.owner_id.as_str())
                    .map(|component| &component.id)
                    == self
                        .graph
                        .components
                        .get(occurrence.source.owner_id.as_str())
                        .map(|component| &component.id)
                && source.entity_id == occurrence.source.entity_id
                && (source.occurrence_id.is_none()
                    || source.occurrence_id == occurrence.source.occurrence_id)
        }) {
            return Some("configuration_suppressed");
        }
        None
    }
    fn purchasing(
        &mut self,
        occurrence: Option<&'a Occurrence>,
        parent_path: Option<&str>,
        depth: usize,
        ancestry: &mut Vec<&'a str>,
    ) -> Result<()> {
        let (component, children, path) = self.node(occurrence)?;
        self.visit(component, depth, ancestry)?;
        let reason = occurrence.and_then(|o| self.suppression(o)).or_else(|| {
            (component.metadata.exclude_from_bom == Some(true)).then_some("excludeFromBom")
        });
        if let Some(reason) = reason {
            self.charge(&(path, reason))?;
            self.excluded.push(Exclusion {
                path: path.into(),
                reason: reason.into(),
            });
            return Ok(());
        }
        let purchased =
            component.metadata.as_purchased == Some(true) && !self.options.explode_purchased;
        self.charge(&(
            path,
            parent_path,
            &component.id,
            &component.metadata,
            &component.reference,
            occurrence.map(|o| &o.reference),
        ))?;
        self.instances.push(InstanceRow {
            path: path.into(),
            parent_path: parent_path.map(str::to_owned),
            depth,
            component_id: component.id.clone(),
            metadata: component.metadata.clone(),
            body_count: component.direct_bodies.len(),
            child_count: children.len(),
            as_purchased: purchased,
            component_ref: component.reference.clone(),
            occurrence_ref: occurrence.map(|o| o.reference.clone()),
        });
        if !component.direct_bodies.is_empty() || purchased {
            let index = if let Some(index) = self.part_indexes.get(component.id.as_str()) {
                *index
            } else {
                let mut properties = PhysicalProperties::default();
                if purchased {
                    properties = component.subtree_properties.ok_or_else(|| Error::new("source_incomplete", "Collapsed purchased component lacks native subtree physical properties"))?;
                } else {
                    for body in &component.direct_bodies {
                        properties.add(*body)?;
                    }
                }
                self.charge(&(&component.id, &component.metadata))?;
                let index = self.parts.len();
                self.parts.push(PartRow {
                    component_id: component.id.clone(),
                    metadata: component.metadata.clone(),
                    quantity: 0,
                    occurrence_paths: Vec::new(),
                    scope: if purchased {
                        "component_subtree"
                    } else {
                        "direct_component_bodies"
                    }
                    .into(),
                    unit_mass: properties.mass_kg,
                    unit_volume: properties.volume_mm3,
                });
                self.part_indexes.insert(&component.id, index);
                index
            };
            self.charge(&path)?;
            self.parts[index].quantity += 1;
            self.parts[index].occurrence_paths.push(path.into());
        }
        if !purchased {
            ancestry.push(&component.id);
            for child in children {
                self.purchasing(
                    Some(self.graph.occurrence(child)?),
                    Some(path),
                    depth + 1,
                    ancestry,
                )?;
            }
            ancestry.pop();
        }
        Ok(())
    }
    fn physical_walk(
        &mut self,
        occurrence: Option<&'a Occurrence>,
        depth: usize,
        ancestry: &mut Vec<&'a str>,
    ) -> Result<()> {
        let (component, children, _) = self.node(occurrence)?;
        self.visit(component, depth, ancestry)?;
        let bodies = occurrence
            .map(|o| o.direct_bodies.as_slice())
            .unwrap_or(&component.direct_bodies);
        for body in bodies {
            self.body_instances += 1;
            if self.body_instances > self.limits.max_physical_body_instances {
                return Err(Error::new(
                    "bom_budget",
                    "Physical body-instance budget exceeded",
                ));
            }
            self.physical.add(*body)?;
        }
        ancestry.push(&component.id);
        for child in children {
            let child = self.graph.occurrence(child)?;
            if self.suppression(child).is_none() {
                self.physical_walk(Some(child), depth + 1, ancestry)?;
            }
        }
        ancestry.pop();
        Ok(())
    }
}
