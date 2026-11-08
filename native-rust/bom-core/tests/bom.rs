use fusion_bom_core::*;
use std::collections::BTreeMap;

fn target() -> Target {
    Target {
        document_id: "doc-1".into(),
        revision: "rev-1".into(),
    }
}
fn properties(mass: f64) -> PhysicalProperties {
    PhysicalProperties {
        mass_kg: mass,
        volume_mm3: mass * 1000.0,
    }
}
fn metadata(id: &str, purchased: bool, excluded: bool) -> Metadata {
    let mut attributes = BTreeMap::new();
    if purchased {
        attributes.insert("asPurchased".into(), "true".into());
    }
    if excluded {
        attributes.insert("excludeFromBom".into(), "true".into());
    }
    Metadata::from_attributes(
        id.into(),
        format!("PN-{id}"),
        String::new(),
        attributes,
        None,
    )
    .unwrap()
}
fn component(id: &str, masses: &[f64], purchased: bool, excluded: bool) -> Component {
    Component {
        id: id.into(),
        aliases: Vec::new(),
        metadata: metadata(id, purchased, excluded),
        reference: EntityRef {
            document_id: "doc-1".into(),
            revision: "rev-1".into(),
            token: format!("token-{id}"),
            object_type: "adsk::fusion::Component".into(),
            occurrence_token: None,
            occurrence_path: None,
            root_component_id: (id == "root").then(|| id.into()),
        },
        direct_bodies: masses.iter().copied().map(properties).collect(),
        subtree_properties: None,
    }
}
fn occurrence(id: &str, path: &str, component: &Component, children: &[&str]) -> Occurrence {
    Occurrence {
        id: id.into(),
        path: path.into(),
        component_id: component.id.clone(),
        reference: EntityRef {
            document_id: "doc-1".into(),
            revision: "rev-1".into(),
            token: format!("occ-token-{id}"),
            object_type: "adsk::fusion::Occurrence".into(),
            occurrence_token: None,
            occurrence_path: Some(path.into()),
            root_component_id: None,
        },
        source: SourceLink {
            entity_id: format!("source-{id}"),
            object_type: "adsk::fusion::Occurrence".into(),
            owner_id: component.id.clone(),
            owner_scope: OwnerScope::Component,
            occurrence_id: None,
        },
        timeline_suppressed: Some(false),
        visible: true,
        direct_bodies: component.direct_bodies.clone(),
        children: children.iter().map(|id| (*id).into()).collect(),
    }
}
fn snapshot(
    components: Vec<Component>,
    occurrences: Vec<Occurrence>,
    children: &[&str],
) -> AssemblySnapshot {
    AssemblySnapshot {
        target: target(),
        witness: SnapshotWitness {
            session_id: "native-session-1".into(),
            design_id: "design-1".into(),
            target: target(),
            complete: true,
            read_errors: Vec::new(),
            calculation_accuracy: CalculationAccuracy::High,
        },
        configuration: ConfigurationWitness {
            target: target(),
            configured: false,
            is_configuration: false,
            configuration_row_id: None,
            complete: true,
            unsupported: Vec::new(),
            read_errors: Vec::new(),
            configured_occurrences: Vec::new(),
            table: None,
        },
        root: Root {
            component_id: "root".into(),
            children: children.iter().map(|id| (*id).into()).collect(),
        },
        components,
        occurrences,
    }
}
fn simple() -> AssemblySnapshot {
    let root = component("root", &[], false, false);
    let part = component("part", &[1.5], false, false);
    let child = occurrence("part-1", "Part:1", &part, &[]);
    snapshot(vec![root, part], vec![child], &["part-1"])
}
fn purchasing_fixture() -> AssemblySnapshot {
    let root = component("root", &[1.0], false, false);
    let mut kit = component("kit", &[2.0], true, false);
    kit.subtree_properties = Some(properties(5.0));
    let bolt = component("bolt", &[1.5], false, false);
    let housing = component("housing", &[4.0], false, true);
    let mut suppressed = occurrence("kit-3", "Kit:3", &kit, &[]);
    suppressed.timeline_suppressed = Some(true);
    let mut hidden = occurrence("hidden", "Hidden:1", &bolt, &[]);
    hidden.visible = false;
    let occurrences = vec![
        occurrence("kit-1", "Kit:1", &kit, &["a", "b"]),
        occurrence("kit-2", "Kit:2", &kit, &["c", "d"]),
        occurrence("a", "Kit:1+Bolt:1", &bolt, &[]),
        occurrence("b", "Kit:1+Bolt:2", &bolt, &[]),
        occurrence("c", "Kit:2+Bolt:1", &bolt, &[]),
        occurrence("d", "Kit:2+Bolt:2", &bolt, &[]),
        occurrence("housing", "Housing:1", &housing, &["e"]),
        occurrence("e", "Housing:1+Bolt:1", &bolt, &[]),
        suppressed,
        hidden,
    ];
    snapshot(
        vec![root, kit, bolt, housing],
        occurrences,
        &["kit-1", "kit-2", "housing", "kit-3", "hidden"],
    )
}

#[test]
fn hand_computed_purchased_fixture_keeps_purchasing_and_physical_totals_distinct() {
    let bom = generate(&purchasing_fixture(), Options::default()).unwrap();
    // Root 1 + two kits (2 + 1.5 + 1.5) + excluded housing (4 + 1.5)
    // + hidden bolt 1.5 = 18 kg and ten direct-body instances.
    assert_eq!(
        (
            bom.physical.mass,
            bom.physical.volume,
            bom.physical.body_instances
        ),
        (18.0, 18_000.0, 10)
    );
    assert_eq!(
        (
            bom.part_count,
            bom.total_purchasing_instances,
            bom.instances.len()
        ),
        (3, 4, 4)
    );
    assert_eq!(
        bom.parts
            .iter()
            .map(|row| (row.component_id.as_str(), row.quantity, row.unit_mass))
            .collect::<Vec<_>>(),
        vec![("root", 1, 1.0), ("kit", 2, 5.0), ("bolt", 1, 1.5)]
    );
    assert_eq!(bom.parts[1].scope, "component_subtree");
    assert_eq!(bom.parts[1].occurrence_paths, vec!["Kit:1", "Kit:2"]);
    assert_eq!(
        bom.excluded,
        vec![
            Exclusion {
                path: "Housing:1".into(),
                reason: "excludeFromBom".into()
            },
            Exclusion {
                path: "Kit:3".into(),
                reason: "suppressed".into()
            }
        ]
    );
    assert_eq!(bom.instances[1].child_count, 2);
    assert_eq!(bom.instances[1].parent_path.as_deref(), Some("<root>"));
    assert!(bom.instances[0].occurrence_ref.is_none());
}

#[test]
fn exploding_purchased_parts_changes_quantity_scope_but_not_physical_mass() {
    let source = purchasing_fixture();
    let collapsed = generate(&source, Options::default()).unwrap();
    let exploded = generate(
        &source,
        Options {
            explode_purchased: true,
        },
    )
    .unwrap();
    assert_eq!(exploded.physical, collapsed.physical);
    assert_eq!(
        (
            exploded.part_count,
            exploded.total_purchasing_instances,
            exploded.instances.len()
        ),
        (3, 8, 8)
    );
    assert_eq!(
        exploded
            .parts
            .iter()
            .map(|row| (row.component_id.as_str(), row.quantity, row.unit_mass))
            .collect::<Vec<_>>(),
        vec![("root", 1, 1.0), ("kit", 2, 2.0), ("bolt", 5, 1.5)]
    );
    assert!(exploded
        .parts
        .iter()
        .all(|row| row.scope == "direct_component_bodies"));
    assert!(exploded.instances.iter().all(|row| !row.as_purchased));
    // Metadata still records the source purchasing flag, while the effective
    // structured-row flag correctly reflects explodePurchased.
    assert_eq!(exploded.instances[1].metadata.as_purchased, Some(true));
    assert!(!exploded.instances[1].as_purchased);
}

#[test]
fn reused_empty_assembly_definitions_repeat_children_without_adding_flat_parts() {
    let root = component("root", &[], false, false);
    let assembly = component("assembly", &[], false, false);
    let bolt = component("bolt", &[2.0], false, false);
    let children = vec![
        occurrence("a", "A:1", &assembly, &["a1", "a2"]),
        occurrence("b", "A:2", &assembly, &["b1", "b2"]),
        occurrence("a1", "A:1+B:1", &bolt, &[]),
        occurrence("a2", "A:1+B:2", &bolt, &[]),
        occurrence("b1", "A:2+B:1", &bolt, &[]),
        occurrence("b2", "A:2+B:2", &bolt, &[]),
    ];
    let bom = generate(
        &snapshot(vec![root, assembly, bolt], children, &["a", "b"]),
        Options::default(),
    )
    .unwrap();
    assert_eq!(
        (
            bom.part_count,
            bom.total_purchasing_instances,
            bom.instances.len()
        ),
        (1, 4, 7)
    );
    assert_eq!(bom.physical.mass, 8.0);
    assert_eq!(
        bom.parts[0].occurrence_paths,
        vec!["A:1+B:1", "A:1+B:2", "A:2+B:1", "A:2+B:2"]
    );
}

#[test]
fn root_bom_exclusion_prunes_all_purchasing_but_not_physical_inventory() {
    let mut source = purchasing_fixture();
    source.components[0].metadata = metadata("root", false, true);
    let bom = generate(&source, Options::default()).unwrap();
    assert!(bom.parts.is_empty() && bom.instances.is_empty());
    assert_eq!(
        bom.excluded,
        vec![Exclusion {
            path: "<root>".into(),
            reason: "excludeFromBom".into()
        }]
    );
    assert_eq!(bom.physical.mass, 18.0);
}

#[test]
fn purchasing_uses_definition_properties_while_physical_uses_each_context() {
    let mut source = simple();
    let mut second = occurrence("part-2", "Part:2", &source.components[1], &[]);
    second.direct_bodies[0] = properties(2.5);
    source.occurrences.push(second);
    source.root.children.push("part-2".into());
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!((bom.parts[0].quantity, bom.parts[0].unit_mass), (2, 1.5));
    assert_eq!(bom.physical.mass, 4.0);
}

#[test]
fn purchased_unit_mass_is_explicit_native_subtree_observation_not_flat_sum() {
    let mut source = purchasing_fixture();
    source.components[1].subtree_properties = Some(properties(6.25));
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!(bom.parts[1].unit_mass, 6.25);
    assert_eq!(bom.physical.mass, 18.0);
    source.components[1].subtree_properties = None;
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "source_incomplete"
    );
    assert!(generate(
        &source,
        Options {
            explode_purchased: true
        }
    )
    .is_ok());
}

#[test]
fn purchased_empty_container_is_a_flat_part() {
    let mut source = simple();
    source.components[0].metadata = metadata("root", true, false);
    source.components[0].subtree_properties = Some(properties(1.5));
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!(
        (
            bom.part_count,
            bom.parts[0].component_id.as_str(),
            bom.parts[0].quantity
        ),
        (1, "root", 1)
    );
    assert_eq!(bom.physical.mass, 1.5);
}

#[test]
fn proven_wrapper_aliases_aggregate_and_names_part_numbers_tokens_do_not() {
    let mut source = simple();
    source.components[1].aliases.push("wrapper-alias".into());
    let mut second = occurrence("part-2", "Part:2", &source.components[1], &[]);
    second.component_id = "wrapper-alias".into();
    second.source.owner_id = "wrapper-alias".into();
    source.occurrences.push(second);
    source.root.children.push("part-2".into());
    let mut different = source.components[1].clone();
    different.id = "different-definition".into();
    different.aliases.clear();
    // Same name, part number and token deliberately remain separate.
    let third = occurrence("part-3", "Part:3", &different, &[]);
    source.components.push(different);
    source.occurrences.push(third);
    source.root.children.push("part-3".into());
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!(
        bom.parts
            .iter()
            .map(|row| (row.component_id.as_str(), row.quantity))
            .collect::<Vec<_>>(),
        vec![("part", 2), ("different-definition", 1)]
    );
    assert_eq!(bom.physical.mass, 4.5);
}

#[test]
fn ambiguous_aliases_duplicate_paths_and_shared_instance_nodes_fail_closed() {
    let mut source = simple();
    source.components[1].aliases.push("root".into());
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "ambiguous_identity"
    );
    let mut source = simple();
    let mut duplicate = source.occurrences[0].clone();
    duplicate.id = "part-2".into();
    source.occurrences.push(duplicate);
    source.root.children.push("part-2".into());
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "ambiguous_identity"
    );
    let mut source = simple();
    source.root.children.push("part-1".into());
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "invalid_assembly_graph"
    );
}

#[test]
fn cyclic_component_containment_is_detected_before_suppression_return() {
    let mut root = component("root", &[], false, false);
    root.aliases.push("root-wrapper".into());
    let mut child = occurrence("cycle", "Cycle:1", &root, &[]);
    child.component_id = "root-wrapper".into();
    child.timeline_suppressed = Some(true);
    let source = snapshot(vec![root], vec![child], &["cycle"]);
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "cyclic_assembly"
    );
}

#[test]
fn combined_purchasing_physical_budget_is_not_reset_between_traversals() {
    let source = simple();
    let limits = Limits {
        max_component_visits: 4,
        ..Limits::default()
    };
    assert!(generate_with_limits(&source, Options::default(), limits).is_ok());
    let limits = Limits {
        max_component_visits: 3,
        ..limits
    };
    assert_eq!(
        generate_with_limits(&source, Options::default(), limits)
            .unwrap_err()
            .code,
        "bom_budget"
    );
    let limits = Limits {
        max_physical_body_instances: 0,
        ..Limits::default()
    };
    assert_eq!(
        generate_with_limits(&source, Options::default(), limits)
            .unwrap_err()
            .code,
        "bom_budget"
    );
    let limits = Limits {
        max_depth: 101,
        ..Limits::default()
    };
    assert_eq!(
        generate_with_limits(&source, Options::default(), limits)
            .unwrap_err()
            .code,
        "invalid_limits"
    );
}

#[test]
fn depth_100_is_allowed_and_101_is_refused() {
    fn chain(depth: usize) -> AssemblySnapshot {
        let mut components = vec![component("root", &[], false, false)];
        let mut occurrences = Vec::new();
        for level in 1..=depth {
            let comp = component(&format!("c{level}"), &[], false, false);
            let next = format!("o{}", level + 1);
            let children = if level < depth {
                vec![next.as_str()]
            } else {
                vec![]
            };
            occurrences.push(occurrence(
                &format!("o{level}"),
                &format!("level-{level}"),
                &comp,
                &children,
            ));
            components.push(comp);
        }
        snapshot(components, occurrences, &["o1"])
    }
    assert!(generate(&chain(100), Options::default()).is_ok());
    assert_eq!(
        generate(&chain(101), Options::default()).unwrap_err().code,
        "bom_budget"
    );
}

#[test]
fn invalid_nonfinite_negative_and_overflowing_physical_data_is_refused() {
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        let mut source = simple();
        source.occurrences[0].direct_bodies[0].mass_kg = value;
        assert_eq!(
            generate(&source, Options::default()).unwrap_err().code,
            "invalid_physical_properties"
        );
    }
    let mut source = simple();
    source.components[1].direct_bodies = vec![
        PhysicalProperties {
            mass_kg: f64::MAX,
            volume_mm3: 1.0
        };
        2
    ];
    source.occurrences[0].direct_bodies = source.components[1].direct_bodies.clone();
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "invalid_physical_properties"
    );
    assert_eq!(
        PhysicalProperties::from_fusion_units(2.0, 3.0).unwrap(),
        PhysicalProperties {
            mass_kg: 2.0,
            volume_mm3: 3000.0
        }
    );
    assert!(PhysicalProperties::from_fusion_units(1.0, f64::MAX).is_err());
}

#[test]
fn source_witness_and_reference_completeness_are_not_inferred_from_partial_data() {
    let base = simple();
    let mut cases = Vec::new();
    let mut source = base.clone();
    source.witness.complete = false;
    cases.push(source);
    let mut source = base.clone();
    source
        .witness
        .read_errors
        .push("failed body properties".into());
    cases.push(source);
    let mut source = base.clone();
    source.witness.target.revision = "stale".into();
    cases.push(source);
    let mut source = base.clone();
    source.witness.session_id.clear();
    cases.push(source);
    let mut source = base.clone();
    source.witness.calculation_accuracy = CalculationAccuracy::Low;
    cases.push(source);
    let mut source = base.clone();
    source.components[1].reference.document_id = "foreign".into();
    cases.push(source);
    let mut source = base.clone();
    source.occurrences[0].direct_bodies.clear();
    cases.push(source);
    let mut source = base.clone();
    source.occurrences[0].reference.occurrence_path = Some("wrong".into());
    cases.push(source);
    let mut source = base.clone();
    source.root.children.push("missing".into());
    cases.push(source);
    for source in cases {
        assert_eq!(
            generate(&source, Options::default()).unwrap_err().code,
            "source_incomplete"
        );
    }
    let mut source = base;
    source.root.children.clear();
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "invalid_assembly_graph"
    );
}

#[test]
fn metadata_decodes_json_attributes_strictly_and_preserves_supplier_provenance() {
    let attrs = BTreeMap::from([
        ("supplier".into(), "\"McMaster-Carr\"".into()),
        ("supplierPartNumber".into(), "\"91290A115\"".into()),
        ("sourceFile".into(), "\"C:\\\\CAD\\\\part.step\"".into()),
        ("sha256".into(), "\"digest\"".into()),
        ("asPurchased".into(), "true".into()),
        ("unknownLegacyAttribute".into(), "not JSON".into()),
    ]);
    let data = Metadata::from_attributes(
        "Bolt".into(),
        "PN".into(),
        String::new(),
        attrs.clone(),
        None,
    )
    .unwrap();
    assert_eq!(data.supplier.as_deref(), Some("McMaster-Carr"));
    assert_eq!(data.supplier_part_number.as_deref(), Some("91290A115"));
    assert_eq!(data.source_file.as_deref(), Some("C:\\CAD\\part.step"));
    assert_eq!(data.attributes, attrs);
    for invalid in ["1", "\"true\"", "null", "{", "[]"] {
        let attrs = BTreeMap::from([("asPurchased".into(), invalid.into())]);
        assert_eq!(
            Metadata::from_attributes("Bolt".into(), "PN".into(), String::new(), attrs, None)
                .unwrap_err()
                .code,
            "invalid_metadata"
        );
    }
    let mut source = simple();
    source.components[1].metadata.as_purchased = Some(true);
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "invalid_metadata"
    );
}

fn configured_source() -> AssemblySnapshot {
    let mut source = simple();
    source.occurrences[0].source.entity_id = "same-native-occurrence".into();
    source.occurrences[0].source.occurrence_id = Some("context-a".into());
    let mut second = occurrence("part-2", "Part:2", &source.components[1], &[]);
    second.source.entity_id = "same-native-occurrence".into();
    second.source.occurrence_id = Some("context-b".into());
    second.visible = false;
    second.timeline_suppressed = None;
    source.occurrences.push(second);
    source.root.children.push("part-2".into());
    let suppress = source.occurrences[0].source.clone();
    let visibility = source.occurrences[1].source.clone();
    let rows = vec![
        ConfigurationRow {
            id: "active".into(),
            name: "Working".into(),
            scope: RowScope::Ordinary,
            parent_id: None,
            child_ids: vec![],
        },
        ConfigurationRow {
            id: "other".into(),
            name: "Other".into(),
            scope: RowScope::Ordinary,
            parent_id: None,
            child_ids: vec![],
        },
        ConfigurationRow {
            id: "test".into(),
            name: "Reserved".into(),
            scope: RowScope::Test,
            parent_id: None,
            child_ids: vec![],
        },
    ];
    let mut cells = Vec::new();
    for row in &rows {
        cells.push(ConfigurationCell {
            row_id: row.id.clone(),
            column_id: "suppress".into(),
            scope: row.scope,
            value: CellValue::ConfigurationSuppressCell {
                is_suppressed: false,
                is_effectively_suppressed: row.id != "other",
            },
        });
        cells.push(ConfigurationCell {
            row_id: row.id.clone(),
            column_id: "visible".into(),
            scope: row.scope,
            value: CellValue::ConfigurationVisibilityCell { is_visible: false },
        });
    }
    source.configuration.configured = true;
    source.configuration.table = Some(ConfigurationTable {
        id: "table-1".into(),
        active_row_id: "active".into(),
        active_row_name: "Working".into(),
        has_custom_columns: false,
        theme_table_ids: vec![],
        rows,
        columns: vec![
            ConfigurationColumn {
                id: "suppress".into(),
                kind: ColumnKind::Suppress,
                source: Some(suppress),
            },
            ConfigurationColumn {
                id: "visible".into(),
                kind: ColumnKind::Visibility,
                source: Some(visibility),
            },
        ],
        cells,
    });
    source
}

#[test]
fn active_configuration_effective_suppression_respects_exact_occurrence_context() {
    let source = configured_source();
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!((bom.parts[0].quantity, bom.physical.mass), (1, 1.5));
    assert_eq!(bom.parts[0].occurrence_paths, vec!["Part:2"]);
    assert_eq!(bom.excluded[0].reason, "configuration_suppressed");
    assert_eq!(bom.configuration_suppression.len(), 1);
    assert_eq!(bom.configuration_suppression[0].row_id, "active");
    assert_eq!(bom.configuration.row_id.as_deref(), Some("active"));
    assert_eq!(bom.configuration.table_id.as_deref(), Some("table-1"));
}

#[test]
fn configuration_without_context_suppresses_all_reused_native_instances() {
    let mut source = configured_source();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .occurrence_id = None;
    let bom = generate(&source, Options::default()).unwrap();
    assert!(bom.parts.is_empty());
    assert_eq!(
        (
            bom.physical.mass,
            bom.physical.body_instances,
            bom.excluded.len()
        ),
        (0.0, 0, 2)
    );
}

#[test]
fn inactive_test_rows_and_visibility_never_suppress_a_bom_instance() {
    let mut source = configured_source();
    let table = source.configuration.table.as_mut().unwrap();
    table.active_row_id = "other".into();
    table.active_row_name = "Other".into();
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!((bom.parts[0].quantity, bom.physical.mass), (2, 3.0));
    assert!(bom.excluded.is_empty());
    assert_eq!(bom.configuration_suppression[0].row_id, "other");
}

#[test]
fn timeline_suppression_takes_precedence_and_non_occurrence_sources_are_not_instance_filters() {
    let mut source = configured_source();
    source.occurrences[0].timeline_suppressed = Some(true);
    assert_eq!(
        generate(&source, Options::default()).unwrap().excluded[0].reason,
        "suppressed"
    );
    let mut source = configured_source();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .object_type = "adsk::fusion::ExtrudeFeature".into();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .entity_id = "observed-feature-source".into();
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!(bom.parts[0].quantity, 2);
    assert_eq!(bom.physical.mass, 3.0);
}

#[test]
fn incomplete_unsupported_generated_and_external_configuration_scopes_are_refused() {
    let base = configured_source();
    let mut cases = Vec::new();
    let mut source = base.clone();
    source.configuration.complete = false;
    cases.push(source);
    let mut source = base.clone();
    source
        .configuration
        .unsupported
        .push("custom column".into());
    cases.push(source);
    let mut source = base.clone();
    source.configuration.read_errors.push("read failed".into());
    cases.push(source);
    let mut source = base.clone();
    source.configuration.is_configuration = true;
    cases.push(source);
    let mut source = base.clone();
    source
        .configuration
        .table
        .as_mut()
        .unwrap()
        .has_custom_columns = true;
    cases.push(source);
    let mut source = base.clone();
    source
        .configuration
        .table
        .as_mut()
        .unwrap()
        .theme_table_ids
        .push("materials".into());
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().rows[0]
        .child_ids
        .push("other".into());
    cases.push(source);
    let mut source = base;
    source
        .configuration
        .configured_occurrences
        .push(ConfiguredOccurrence {
            occurrence_id: "part-1".into(),
            path: "Part:1".into(),
            table_id: "foreign-table".into(),
            row_id: "row".into(),
            row_name: "Row".into(),
            source_file_id: "cloud-id".into(),
            source_version: 3,
        });
    cases.push(source);
    for source in cases {
        assert_eq!(
            generate(&source, Options::default()).unwrap_err().code,
            "configuration_scope_unsupported"
        );
    }
}

#[test]
fn configuration_matrix_addresses_types_flags_and_provenance_are_checked() {
    let base = configured_source();
    let mut cases = Vec::new();
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().cells.pop();
    cases.push(source);
    let mut source = base.clone();
    let table = source.configuration.table.as_mut().unwrap();
    table.cells[1] = table.cells[0].clone();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().cells[0].scope = RowScope::Test;
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().cells[0].value =
        CellValue::ConfigurationVisibilityCell { is_visible: false };
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0].source = None;
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().active_row_name = "stale-name".into();
    cases.push(source);
    let mut source = base.clone();
    let table = source.configuration.table.as_mut().unwrap();
    table.active_row_id = "test".into();
    table.active_row_name = "Reserved".into();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.configured = false;
    cases.push(source);
    for source in cases {
        assert_eq!(
            generate(&source, Options::default()).unwrap_err().code,
            "configuration_read_failed"
        );
    }
    let mut source = base;
    source.configuration.target.revision = "foreign-rev".into();
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "configuration_source_mismatch"
    );
}

#[test]
fn json_roundtrip_keeps_exact_float_observations_and_typed_booleans() {
    let mut source = simple();
    source.components[1].direct_bodies[0].mass_kg = f64::from_bits(0x402d_ffff_ffff_fffc);
    source.occurrences[0].direct_bodies = source.components[1].direct_bodies.clone();
    let bytes = serde_json::to_vec(&source).unwrap();
    let parsed: AssemblySnapshot = serde_json::from_slice(&bytes).unwrap();
    let bom = generate(&parsed, Options::default()).unwrap();
    assert_eq!(bom.physical.mass.to_bits(), 0x402d_ffff_ffff_fffc);
    let mut json = serde_json::to_value(source).unwrap();
    json["occurrences"][0]["timelineSuppressed"] = serde_json::json!(1);
    assert!(serde_json::from_value::<AssemblySnapshot>(json).is_err());
}

#[test]
fn flat_csv_has_exact_columns_context_and_basic_numeric_spelling() {
    let bom = generate(&simple(), Options::default()).unwrap();
    let csv = String::from_utf8(csv_bytes(&bom, CsvMode::Flat).unwrap()).unwrap();
    assert_eq!(csv, concat!(
        "componentId,partNumber,name,description,quantity,supplier,supplierPartNumber,sourceUrl,sha256,unitMass,unitVolume,scope,documentId,revision,configurationTableId,configurationRowId,configurationRowName\r\n",
        "part,PN-part,part,,1,,,,,1.5,1500.0,direct_component_bodies,doc-1,rev-1,,,\r\n"));
}

#[test]
fn structured_csv_has_containers_quantity_one_effective_purchase_flag_and_context() {
    let bom = generate(&configured_source(), Options::default()).unwrap();
    let csv = String::from_utf8(csv_bytes(&bom, CsvMode::Structured).unwrap()).unwrap();
    assert_eq!(csv, concat!(
        "path,parentPath,depth,componentId,name,partNumber,description,quantity,bodyCount,childCount,asPurchased,supplier,supplierPartNumber,sourceUrl,sha256,documentId,revision,configurationTableId,configurationRowId,configurationRowName\r\n",
        "<root>,,0,root,root,PN-root,,1,0,2,False,,,,,doc-1,rev-1,table-1,active,Working\r\n",
        "Part:2,<root>,1,part,part,PN-part,,1,1,0,False,,,,,doc-1,rev-1,table-1,active,Working\r\n"));
    let bom = generate(&purchasing_fixture(), Options::default()).unwrap();
    let collapsed = String::from_utf8(csv_bytes(&bom, CsvMode::Structured).unwrap()).unwrap();
    assert!(collapsed.contains("Kit:1,<root>,1,kit,kit,PN-kit,,1,1,2,True,"));
    let bom = generate(
        &purchasing_fixture(),
        Options {
            explode_purchased: true,
        },
    )
    .unwrap();
    let exploded = String::from_utf8(csv_bytes(&bom, CsvMode::Structured).unwrap()).unwrap();
    assert!(exploded.contains("Kit:1,<root>,1,kit,kit,PN-kit,,1,1,2,False,"));
}

#[test]
fn csv_escapes_quotes_commas_multiline_unicode_and_neutralizes_formulas() {
    let mut source = simple();
    let metadata = &mut source.components[1].metadata;
    metadata.name = "=HYPERLINK(\"bad\",\"Ö\")".into();
    metadata.part_number = "PN,7".into();
    metadata.description = "First\r\nSecond \"quoted\" 🦀".into();
    let bom = generate(&source, Options::default()).unwrap();
    let csv = String::from_utf8(csv_bytes(&bom, CsvMode::Flat).unwrap()).unwrap();
    assert!(csv.contains("part,\"PN,7\",\"'=HYPERLINK(\"\"bad\"\",\"\"Ö\"\")\",\"First\r\nSecond \"\"quoted\"\" 🦀\",1,"));
    for value in [
        "=1+1",
        "+1",
        "-123",
        "@SUM(A1)",
        "  =SUM(A1)",
        "\t=1",
        "\n@x",
        "\u{feff}=1",
    ] {
        let mut bom = bom.clone();
        bom.parts[0].metadata.name = value.into();
        let csv = String::from_utf8(csv_bytes(&bom, CsvMode::Flat).unwrap()).unwrap();
        assert!(
            csv.contains(&format!("'{value}")),
            "not neutralized: {value:?}"
        );
    }
}

#[test]
fn csv_does_not_rewrite_regular_text_and_refuses_invalid_or_oversized_output() {
    let bom = generate(&simple(), Options::default()).unwrap();
    let mut regular = bom.clone();
    regular.parts[0].metadata.name = "A-1 @ workshop".into();
    let csv = String::from_utf8(csv_bytes(&regular, CsvMode::Flat).unwrap()).unwrap();
    assert!(csv.contains(",A-1 @ workshop,"));
    let mut invalid = bom.clone();
    invalid.parts[0].unit_mass = f64::NAN;
    assert_eq!(
        csv_bytes(&invalid, CsvMode::Flat).unwrap_err().code,
        "invalid_physical_properties"
    );
    let mut incomplete = bom.clone();
    incomplete.complete = false;
    assert_eq!(
        csv_bytes(&incomplete, CsvMode::Flat).unwrap_err().code,
        "source_incomplete"
    );
    let mut large = bom;
    large.parts[0].metadata.name = "\"".repeat(MAX_CSV_BYTES / 2);
    assert_eq!(
        csv_bytes(&large, CsvMode::Flat).unwrap_err().code,
        "bom_budget"
    );
}

#[test]
fn repeated_large_metadata_is_bounded_before_result_materialization() {
    let mut source = simple();
    // The input contains this string once. A part plus its structured instance
    // would duplicate it past the 32 MiB output budget without the early charge.
    source.components[1].metadata.name = "x".repeat(17 * 1024 * 1024);
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "bom_budget"
    );
}

#[test]
fn configuration_parameter_sources_must_belong_to_the_observed_design() {
    let mut source = configured_source();
    let table = source.configuration.table.as_mut().unwrap();
    table.columns.push(ConfigurationColumn {
        id: "parameter".into(),
        kind: ColumnKind::Parameter,
        source: Some(SourceLink {
            entity_id: "parameter-id".into(),
            object_type: "adsk::fusion::UserParameter".into(),
            owner_id: "design-1".into(),
            owner_scope: OwnerScope::Design,
            occurrence_id: None,
        }),
    });
    for row in &table.rows {
        table.cells.push(ConfigurationCell {
            row_id: row.id.clone(),
            column_id: "parameter".into(),
            scope: row.scope,
            value: CellValue::ConfigurationParameterCell {
                expression: "12 mm".into(),
            },
        });
    }
    assert!(generate(&source, Options::default()).is_ok());
    source.configuration.table.as_mut().unwrap().columns[2]
        .source
        .as_mut()
        .unwrap()
        .owner_id = "foreign-design".into();
    assert_eq!(
        generate(&source, Options::default()).unwrap_err().code,
        "configuration_source_mismatch"
    );
}

#[test]
fn suppression_sources_reconcile_exact_type_and_canonical_owner_with_observed_instances() {
    let base = configured_source();
    let mut source = base.clone();
    source.components[1]
        .aliases
        .push("proven-owner-alias".into());
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .owner_id = "proven-owner-alias".into();
    let bom = generate(&source, Options::default()).unwrap();
    assert_eq!(bom.parts[0].occurrence_paths, vec!["Part:2"]);

    let mut cases = Vec::new();
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .owner_id = "root".into();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .object_type = "fake::Occurrence".into();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .object_type = "adsk::core::Occurrence".into();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .object_type = "adsk::fusion::ExtrudeFeature".into();
    cases.push(source);
    let mut source = base.clone();
    let controlled = source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap();
    controlled.owner_scope = OwnerScope::Design;
    controlled.owner_id = "design-1".into();
    cases.push(source);
    let mut source = base.clone();
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .entity_id = "unobserved-entity".into();
    cases.push(source);
    let mut source = base;
    source.configuration.table.as_mut().unwrap().columns[0]
        .source
        .as_mut()
        .unwrap()
        .occurrence_id = Some("unobserved-context".into());
    cases.push(source);
    for source in cases {
        assert_eq!(
            generate(&source, Options::default()).unwrap_err().code,
            "configuration_source_mismatch"
        );
    }
}
