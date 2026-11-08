//! Read-only finite assembly witness using independently acquired public SDK objects.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
pub type Result<T> = std::result::Result<T, String>;
#[cfg(all(
    windows,
    any(feature = "native-sdk", feature = "native-bindings-check")
))]
mod native;
#[cfg_attr(not(windows), allow(dead_code))]
mod storage;
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Authority {
    pub schema_version: u32,
    pub purpose: String,
    pub nonce: String,
    pub pid: u32,
    pub process_creation_time100ns: String,
    pub document_creation_id: String,
    pub expected_fixture: String,
    pub allow_read_only_stability_observation: bool,
}
impl Authority {
    pub fn validate(&self, pid: u32, creation: &str) -> Result<()> {
        if self.schema_version != 1
            || self.purpose != "r21-native-placement-observation"
            || pid == 0
            || self.pid != pid
            || self.process_creation_time100ns != creation
            || creation.parse::<u64>().ok().filter(|v| *v > 0).is_none()
            || self.expected_fixture != "two-definitions-three-occurrences-two-translations-v1"
            || !self.allow_read_only_stability_observation
            || self.nonce.len() != 32
            || !self
                .nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.document_creation_id.len() != 36
            || !self.document_creation_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                }
            })
        {
            return Err("observer_authority_refused".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub name: String,
    pub creation_id: String,
    pub saved: bool,
    pub modified: bool,
    pub active: bool,
    pub has_data_file: bool,
}
pub fn unique_document(rows: &[Document], creation: &str) -> Result<usize> {
    if rows.is_empty() || rows.len() > 16 {
        return Err("bounded_document_roster_required".into());
    }
    let m: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.creation_id == creation)
        .map(|(i, _)| i)
        .collect();
    if m.len() != 1 {
        return Err("creation_correlation_ambiguous_or_missing".into());
    }
    Ok(m[0])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Kind {
    Revisions,
    Assembly,
}
pub const SEQUENCE: [Kind; 5] = [
    Kind::Revisions,
    Kind::Assembly,
    Kind::Revisions,
    Kind::Assembly,
    Kind::Revisions,
];
#[repr(C)]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssemblyFacts {
    /// components,occurrences,timeline,marker,bodies,sketches,meshes,joints,as-built,
    /// rigid,assembly-constraints,motion-links,joint-origins,features,graphics,all/user/model parameters.
    pub root_counts: [u64; 18],
    /// bodies,sketches,occurrences,meshes,attributes,joints,as-built,rigid,
    /// assembly-constraints,motion-links,joint-origins,features,model-parameters.
    pub component_counts: [[u64; 13]; 2],
    /// parametric,hybrid,configured,active,pending-position-snapshot,offline.
    pub flags: [u32; 6],
    /// grounded,grounded-to-parent,external,configured,suppressed,rolled-back,native-context.
    pub occurrence_flags: [[u32; 7]; 3],
    pub children: [u64; 3],
    pub component_slots: [u32; 3],
    pub same_component: [u32; 9],
    /// Explicit getCell(row,column) order, no flattened-storage convention assumption.
    pub matrices: [[f64; 16]; 3],
    pub snapshot_count: u64,
    pub snapshot_indexes: [i64; 2],
    pub snapshot_health: [u32; 2],
    /// Suppressed, rolled back. Parent timeline and exact row identity are checked in ABI.
    pub snapshot_flags: [[u32; 2]; 2],
}
impl AssemblyFacts {
    pub fn numbers(&self) -> Vec<(String, f64)> {
        self.matrices
            .iter()
            .enumerate()
            .flat_map(|(i, a)| {
                a.iter()
                    .enumerate()
                    .map(move |(j, v)| (format!("matrix/{i}/{j}"), *v))
            })
            .collect()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub group: String,
    pub name: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub raw: AssemblyFacts,
    pub texts: Vec<(u32, String)>,
    pub attributes: [Vec<Attribute>; 2],
}
fn text_ok(s: &str) -> bool {
    s.len() <= 4096 && !s.contains('\0')
}
pub fn assess(c: &Capture) -> Value {
    let f = &c.raw;
    let counts = f.root_counts[0] == 3
        && f.root_counts[1] == 3
        && f.root_counts[2] == 5
        && f.root_counts[2] == f.root_counts[3]
        && f.root_counts[4..].iter().all(|n| *n == 0)
        && f.component_counts.iter().all(|a| {
            a.iter()
                .enumerate()
                .all(|(i, n)| if i == 4 { *n <= 128 } else { *n == 0 })
        });
    let flags = f.flags[..5] == [1, 1, 0, 1, 0]
        && f.flags[5] <= 1
        && f.occurrence_flags
            .iter()
            .all(|a| a[..] == [0, 0, 0, 0, 0, 0, 1]);
    let identities = f.component_slots == [0, 1, 0]
        && f.same_component == [1, 0, 1, 0, 1, 0, 1, 0, 1]
        && f.children == [0; 3];
    let matrices = f.matrices.iter().enumerate().all(|(i, a)| {
        a.iter().enumerate().all(|(j, x)| {
            let wanted = match (i, j) {
                (2, 3) => -0.5,
                (2, 7) => 2.,
                (2, 11) => 1.,
                _ => {
                    if j % 5 == 0 {
                        1.
                    } else {
                        0.
                    }
                }
            };
            x.is_finite() && (*x - wanted).abs() <= 1e-10
        })
    });
    let snapshots = f.snapshot_count == 2
        && f.snapshot_indexes == [3, 4]
        && f.snapshot_health == [0, 0]
        && f.snapshot_flags == [[0, 0], [0, 0]];
    let texts = c.texts.len() == 9
        && c.texts
            .iter()
            .enumerate()
            .all(|(i, (field, s))| *field == i as u32 && !s.is_empty() && text_ok(s))
        && c.texts
            .get(1)
            .is_some_and(|(_, s)| s == "Assembly fixture A revised")
        && c.texts
            .get(2)
            .is_some_and(|(_, s)| s == "Assembly fixture B");
    let expected = [
        json!({"supplier":"Vendor \"A\" \\ depot","supplierPartNumber":"=TEST,43","sourceUrl":"https://example.invalid/A","asPurchased":false,"excludeFromBom":false}),
        json!({"supplier":"Other vendor","supplierPartNumber":"=TEST,42","sourceUrl":"https://example.invalid/B-revised","asPurchased":true,"excludeFromBom":true}),
    ];
    let metadata = c.attributes.iter().enumerate().all(|(i, attrs)| {
        if attrs.len() > 128 || attrs.len() as u64 != f.component_counts[i][4] {
            return false;
        }
        let mut all = std::collections::BTreeSet::new();
        let mut observed = serde_json::Map::new();
        for a in attrs {
            if !text_ok(&a.group)
                || !text_ok(&a.name)
                || !text_ok(&a.value)
                || a.group.is_empty()
                || a.name.is_empty()
                || !all.insert((&a.group, &a.name))
            {
                return false;
            }
            if a.group == "AdomMechanical" {
                let Ok(value) = serde_json::from_str::<Value>(&a.value) else {
                    return false;
                };
                observed.insert(a.name.clone(), value);
            }
        }
        Value::Object(observed) == expected[i]
    });
    json!({"qualified":counts&&flags&&identities&&matrices&&snapshots&&texts&&metadata,"emptyFiniteGraph":counts,"localNativeState":flags,"sharedDefinitionIdentityAndQuantities":identities,"exactFinalOccurrencePoses":matrices,"twoHealthyCapturedPositions":snapshots,"namesAndBoundedTexts":texts,"localMetadata":metadata,"definitionQuantities":[2,1],"selectedOccurrence":2,"selectedPositionMm":[-5,20,10],"onlineNativeMetadataQualified":false,"bomTraversalQualified":false,"physicalAssemblyQualified":false,"historicalIntermediatePoseQualified":false,"archivePersistenceQualified":false})
}
#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub kind: Kind,
    pub abi_status: i32,
    pub data: Value,
    pub float_bits: BTreeMap<String, String>,
    pub non_finite: bool,
    pub diagnostic: Value,
    pub elapsed_micros: u128,
}
impl Observation {
    pub fn succeeded(&self) -> bool {
        self.abi_status == 0 && !self.non_finite
    }
}
pub fn float_bits(
    values: impl IntoIterator<Item = (String, f64)>,
) -> (BTreeMap<String, String>, bool) {
    let mut bad = false;
    let v = values
        .into_iter()
        .map(|(k, v)| {
            bad |= !v.is_finite();
            (k, format!("{:016x}", v.to_bits()))
        })
        .collect();
    (v, bad)
}
pub trait Backend {
    fn observe(&mut self, kind: Kind) -> Observation;
}
pub fn collect(b: &mut impl Backend) -> Vec<Observation> {
    let mut out = vec![];
    for kind in SEQUENCE {
        let s = b.observe(kind);
        let ok = s.kind == kind && s.succeeded();
        out.push(s);
        if !ok {
            break;
        }
    }
    out
}

pub fn comparisons(v: &[Observation]) -> Value {
    let cs: Vec<_> = v.iter().filter(|s| s.kind == Kind::Assembly).collect();
    let rs: Vec<_> = v.iter().filter(|s| s.kind == Kind::Revisions).collect();
    let assessments: Vec<_> = cs
        .iter()
        .map(|s| {
            serde_json::from_value::<Capture>(s.data.clone())
                .map(|c| assess(&c))
                .unwrap_or_else(|_| json!({"qualified":false,"error":"capture_shape"}))
        })
        .collect();
    let revision_shape = rs.iter().all(|s| {
        s.data
            .get("values")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                a.len() == 3
                    && a.iter().enumerate().all(|(i, v)| {
                        v.as_array().is_some_and(|row| {
                            row.len() == 2
                                && row[0].as_u64() == Some(i as u64)
                                && row[1].as_str().is_some_and(|s| !s.is_empty() && text_ok(s))
                        })
                    })
            })
    });
    let stable = cs.len() == 2
        && cs
            .iter()
            .all(|s| s.succeeded() && s.data == cs[0].data && s.float_bits == cs[0].float_bits)
        && rs.len() == 3
        && rs.iter().all(|s| s.succeeded() && s.data == rs[0].data)
        && revision_shape;
    let sequence = v.len() == SEQUENCE.len() && v.iter().zip(SEQUENCE).all(|(a, b)| a.kind == b);
    json!({"twoAssemblySamplesEqualAndRevisionsStable":stable,"sequenceComplete":sequence,"assessments":assessments,"finitePlacementTruthQualified":stable&&sequence&&assessments.len()==2&&assessments.iter().all(|a|a["qualified"]==true),"operationJournalReconciled":false,"fullBridgeParity":false})
}
pub fn qualified(v: &[Observation]) -> bool {
    comparisons(v)["finitePlacementTruthQualified"] == true
}
#[cfg(test)]
mod tests;
