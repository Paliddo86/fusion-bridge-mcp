use super::*;
fn authority() -> Authority {
    Authority {
        schema_version: 1,
        purpose: "r21-native-placement-observation".into(),
        nonce: "a".repeat(32),
        pid: 42,
        process_creation_time100ns: "100".into(),
        document_creation_id: "973d2018-28d6-453e-bf33-a83279eaf652".into(),
        expected_fixture: "two-definitions-three-occurrences-two-translations-v1".into(),
        allow_read_only_stability_observation: true,
    }
}
fn capture() -> Capture {
    let mut f = AssemblyFacts::default();
    f.root_counts[..4].copy_from_slice(&[3, 3, 5, 5]);
    f.snapshot_count = 2;
    f.snapshot_indexes = [3, 4];
    for c in &mut f.component_counts {
        c[4] = 5;
    }
    f.flags = [1, 1, 0, 1, 0, 0];
    for i in 0..3 {
        f.occurrence_flags[i] = [0, 0, 0, 0, 0, 0, 1];
        for j in [0, 5, 10, 15] {
            f.matrices[i][j] = 1.;
        }
    }
    f.component_slots = [0, 1, 0];
    f.matrices[2][3] = -0.5;
    f.matrices[2][7] = 2.;
    f.matrices[2][11] = 1.;
    f.same_component = [1, 0, 1, 0, 1, 0, 1, 0, 1];
    let metadata = [
        json!({"supplier":"Vendor \"A\" \\ depot","supplierPartNumber":"=TEST,43","sourceUrl":"https://example.invalid/A","asPurchased":false,"excludeFromBom":false}),
        json!({"supplier":"Other vendor","supplierPartNumber":"=TEST,42","sourceUrl":"https://example.invalid/B-revised","asPurchased":true,"excludeFromBom":true}),
    ];
    Capture {
        raw: f,
        texts: [
            "(Unsaved)",
            "Assembly fixture A revised",
            "Assembly fixture B",
            "Assembly fixture A revised:1",
            "Assembly fixture A revised:1",
            "Assembly fixture B:1",
            "Assembly fixture B:1",
            "Assembly fixture A revised:2",
            "Assembly fixture A revised:2",
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| (i as u32, s.to_string()))
        .collect(),
        attributes: std::array::from_fn(|i| {
            metadata[i]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| Attribute {
                    group: "AdomMechanical".into(),
                    name: k.clone(),
                    value: v.to_string(),
                })
                .collect()
        }),
    }
}
fn valid(c: &Capture) -> bool {
    assess(c)["qualified"] == true
}
#[test]
fn exact_authority_rejects_partial_fixture_and_wrong_process() {
    assert!(authority().validate(42, "100").is_ok());
    for i in 0..8 {
        let mut a = authority();
        match i {
            0 => a.schema_version = 2,
            1 => a.purpose = "r20-partial-assembly-observation".into(),
            2 => a.pid = 43,
            3 => a.process_creation_time100ns = "101".into(),
            4 => a.document_creation_id = "Untitled".into(),
            5 => a.expected_fixture = "one-definition-one-occurrence-initial-a-v1".into(),
            6 => a.allow_read_only_stability_observation = false,
            _ => a.nonce = "Z".repeat(32),
        };
        assert!(a.validate(42, "100").is_err());
    }
}
#[test]
fn two_definitions_three_occurrences_current_state_only() {
    let a = assess(&capture());
    assert_eq!(a["qualified"], true);
    assert_eq!(a["definitionQuantities"], json!([2, 1]));
    assert_eq!(a["bomTraversalQualified"], false);
    assert_eq!(a["physicalAssemblyQualified"], false);
    assert_eq!(a["onlineNativeMetadataQualified"], false);
}
#[test]
fn every_resource_and_missing_member_are_refused() {
    for i in 0..18 {
        let mut c = capture();
        c.raw.root_counts[i] += 2;
        assert!(!valid(&c), "root{i}");
    }
    for comp in 0..2 {
        for i in 0..13 {
            let mut c = capture();
            c.raw.component_counts[comp][i] += 1;
            assert!(!valid(&c), "component{comp}/{i}");
        }
    }
    for i in 0..3 {
        let mut c = capture();
        c.raw.children[i] = 1;
        assert!(!valid(&c));
    }
}
#[test]
fn identity_matrix_distinguishes_shared_definition_from_equal_metadata() {
    for i in 0..9 {
        let mut c = capture();
        c.raw.same_component[i] ^= 1;
        assert!(!valid(&c));
    }
    for i in 0..3 {
        let mut c = capture();
        c.raw.component_slots[i] ^= 1;
        assert!(!valid(&c));
    }
    let mut c = capture();
    c.raw.same_component = [1; 9];
    assert!(!valid(&c));
}
#[test]
fn all_three_placements_and_every_matrix_cell_are_checked() {
    for occurrence in 0..3 {
        for i in 0..16 {
            let mut c = capture();
            c.raw.matrices[occurrence][i] += 0.001;
            assert!(!valid(&c));
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut c = capture();
            c.raw.matrices[occurrence][0] = bad;
            assert!(!valid(&c));
        }
    }
}

#[test]
fn captured_positions_must_be_two_distinct_healthy_rows_at_exact_indexes() {
    for count in [0, 1, 3, u64::MAX] {
        let mut c = capture();
        c.raw.snapshot_count = count;
        assert!(!valid(&c));
    }
    for i in 0..2 {
        for index in [-1, 0, 2, 5, i64::MAX] {
            let mut c = capture();
            c.raw.snapshot_indexes[i] = index;
            assert!(!valid(&c));
        }
        for health in [1, 2, u32::MAX] {
            let mut c = capture();
            c.raw.snapshot_health[i] = health;
            assert!(!valid(&c));
        }
        for flag in 0..2 {
            let mut c = capture();
            c.raw.snapshot_flags[i][flag] = 1;
            assert!(!valid(&c));
        }
    }
    let mut c = capture();
    c.raw.snapshot_indexes = [3, 3];
    assert!(!valid(&c));
}

#[test]
fn an_identity_or_accumulated_pose_does_not_prove_the_second_absolute_move() {
    for xyz in [[0., 0., 0.], [2., 1., 1.5], [2.5, -1., 0.5]] {
        let mut c = capture();
        for (cell, n) in [3, 7, 11].into_iter().zip(xyz) {
            c.raw.matrices[2][cell] = n;
        }
        assert!(!valid(&c));
    }
    let mut a = authority();
    a.purpose = "r20-native-assembly-observation".into();
    assert!(a.validate(42, "100").is_err());
}
#[test]
fn forbidden_flags_and_grounded_occurrences_are_refused() {
    for i in 0..5 {
        let mut c = capture();
        c.raw.flags[i] ^= 1;
        assert!(!valid(&c));
    }
    let mut c = capture();
    c.raw.flags[5] = 1;
    assert!(valid(&c));
    c.raw.flags[5] = 2;
    assert!(!valid(&c));
    for o in 0..3 {
        for i in 2..7 {
            let mut c = capture();
            c.raw.occurrence_flags[o][i] ^= 1;
            assert!(!valid(&c));
        }
        for i in 0..2 {
            let mut c = capture();
            c.raw.occurrence_flags[o][i] = 1;
            assert!(!valid(&c));
            c.raw.occurrence_flags[o][i] = 2;
            assert!(!valid(&c));
        }
    }
}
#[test]
fn exact_final_metadata_on_each_definition_is_required() {
    assert!(valid(&capture()));
    for comp in 0..2 {
        for i in 0..5 {
            let mut c = capture();
            c.attributes[comp][i].value = "null".into();
            assert!(!valid(&c));
        }
        for bad in ["not-json", "{", "\"wrong\"", "1"] {
            let mut c = capture();
            c.attributes[comp][0].value = bad.into();
            assert!(!valid(&c));
        }
    }
}
#[test]
fn attribute_missing_duplicate_truncated_and_wrong_group_refused() {
    for comp in 0..2 {
        let mut c = capture();
        c.attributes[comp].push(c.attributes[comp][0].clone());
        c.raw.component_counts[comp][4] += 1;
        assert!(!valid(&c));
        let mut c = capture();
        c.attributes[comp].remove(0);
        c.raw.component_counts[comp][4] -= 1;
        assert!(!valid(&c));
        let mut c = capture();
        c.attributes[comp][0].value = "x".repeat(4097);
        assert!(!valid(&c));
        let mut c = capture();
        c.attributes[comp][0].group = "Other".into();
        assert!(!valid(&c));
    }
}
#[test]
fn definition_names_and_text_shape_are_exact() {
    for i in 0..9 {
        let mut c = capture();
        c.texts[i].0 = 100;
        assert!(!valid(&c));
        let mut c = capture();
        c.texts[i].1 = "".into();
        assert!(!valid(&c));
    }
    let mut c = capture();
    c.texts[1].1 = "Assembly fixture B".into();
    assert!(!valid(&c));
    let mut c = capture();
    c.texts[2].1 = "Assembly fixture A revised".into();
    assert!(!valid(&c));
    let mut c = capture();
    c.texts.remove(8);
    assert!(!valid(&c));
}
struct Fake {
    calls: usize,
    fail: usize,
}
impl Backend for Fake {
    fn observe(&mut self, kind: Kind) -> Observation {
        self.calls += 1;
        let c = capture();
        let (bits, bad) = float_bits(c.raw.numbers());
        Observation {
            kind,
            abi_status: if self.calls == self.fail { 7 } else { 0 },
            data: if kind == Kind::Assembly {
                json!(c)
            } else {
                json!({"values":[[0,"root-rev"],[1,"a-rev"],[2,"b-rev"]]})
            },
            float_bits: if kind == Kind::Assembly {
                bits
            } else {
                BTreeMap::new()
            },
            non_finite: bad,
            diagnostic: json!({"stage":self.calls}),
            elapsed_micros: 0,
        }
    }
}
#[test]
fn every_failed_sample_stops_without_retry_and_retains_failure() {
    for fail in 1..=5 {
        let mut b = Fake { calls: 0, fail };
        let v = collect(&mut b);
        assert_eq!(v.len(), fail);
        assert_eq!(b.calls, fail);
        assert_eq!(v.last().unwrap().abi_status, 7);
        assert!(!qualified(&v));
    }
}
#[test]
fn full_current_state_qualification_never_reconciles_journal() {
    let v = collect(&mut Fake { calls: 0, fail: 0 });
    assert!(qualified(&v));
    let c = comparisons(&v);
    assert_eq!(c["finitePlacementTruthQualified"], true);
    assert_eq!(c["operationJournalReconciled"], false);
    assert_eq!(c["fullBridgeParity"], false);
}
#[test]
fn all_bracket_revisions_raw_properties_and_float_bits_must_match() {
    let v = collect(&mut Fake { calls: 0, fail: 0 });
    for i in 0..5 {
        let mut changed = v.clone();
        changed[i].data = json!({"changed":true});
        assert!(!qualified(&changed));
    }
    let mut changed = v.clone();
    changed[3]
        .float_bits
        .insert("matrix/0/0".into(), "0000000000000000".into());
    assert!(!qualified(&changed));
    let mut wrong = v.clone();
    wrong.swap(0, 1);
    assert!(!qualified(&wrong));
    assert!(!qualified(&v[..4]));
}
#[test]
fn missing_or_empty_revision_fields_refuse_even_if_all_captures_agree() {
    for bad in [
        json!({"values":[[0,"r"],[1,"a"]]}),
        json!({"values":[[0,"r"],[1,"a"],[2,""]]}),
        json!({"values":[[0,"r"],[1,"a"],[1,"b"]]}),
    ] {
        let mut v = collect(&mut Fake { calls: 0, fail: 0 });
        for i in [0, 2, 4] {
            v[i].data = bad.clone();
        }
        assert!(!qualified(&v));
    }
}
#[test]
fn document_correlation_never_uses_name_or_active() {
    let d = Document {
        name: "Untitled".into(),
        creation_id: authority().document_creation_id,
        saved: false,
        modified: true,
        active: false,
        has_data_file: false,
    };
    assert_eq!(unique_document(&[d.clone()], &d.creation_id).unwrap(), 0);
    assert!(unique_document(&[d.clone(), d.clone()], &d.creation_id).is_err());
    assert!(unique_document(&[d], "missing").is_err());
}
#[test]
fn abi_and_raw_nonfinite_bits() {
    assert_eq!(std::mem::size_of::<AssemblyFacts>(), 968);
    assert_eq!(std::mem::offset_of!(AssemblyFacts, snapshot_count), 920);
    assert_eq!(std::mem::offset_of!(AssemblyFacts, snapshot_indexes), 928);
    assert_eq!(std::mem::offset_of!(AssemblyFacts, snapshot_health), 944);
    assert_eq!(std::mem::offset_of!(AssemblyFacts, snapshot_flags), 952);
    assert_eq!(std::mem::offset_of!(AssemblyFacts, matrices), 536);
    let (bits, bad) = float_bits(vec![("n".into(), f64::NAN)]);
    assert!(bad);
    assert_eq!(bits["n"], format!("{:016x}", f64::NAN.to_bits()));
}
