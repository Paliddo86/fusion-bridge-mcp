#![allow(non_snake_case)] // DLL file name must match the Fusion manifest folder.
use serde::Serialize;
pub mod assembly;
pub mod bom;
pub mod bom_export;
pub mod execution;
pub mod http;
pub mod identity;
pub mod materials;
pub mod mechanics;
pub mod operation_status;
pub mod physical;
pub mod purchased;
pub mod service;
pub mod sketches;

#[cfg(any(
    test,
    all(
        windows,
        any(feature = "native-sdk", feature = "native-bindings-check")
    )
))]
mod bytes;
#[cfg(any(
    test,
    all(
        windows,
        any(feature = "native-sdk", feature = "native-bindings-check")
    )
))]
mod lifecycle;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub name: String,
    pub creation_id: String,
    pub active: bool,
    pub saved: bool,
    pub modified: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub implementation: &'static str,
    pub scope: &'static str,
    pub fusion_version: String,
    pub thread_id: u32,
    pub creation_ids_unique: bool,
    pub identity_scope: &'static str,
    pub documents: Vec<Document>,
}

// Every document SDK interaction stays behind this thread-bound interface.
// The native listener permits read-only snapshots, with no legacy dispatch.
pub trait ReadSdk {
    fn version(&self) -> Result<String, String>;
    fn thread_id(&self) -> u32;
    fn count(&self) -> Result<usize, String>;
    fn document(&self, index: usize) -> Result<Document, String>;
}

pub fn snapshot(sdk: &impl ReadSdk) -> Result<Snapshot, String> {
    let count = sdk.count()?;
    if count > 256 {
        return Err("document enumeration exceeds bounded probe limit".into());
    }
    let mut documents = Vec::with_capacity(count);
    let mut string_bytes = 0usize;
    let mut creation_ids_unique = true;
    for index in 0..count {
        let doc = sdk.document(index)?;
        string_bytes = string_bytes
            .checked_add(doc.name.len())
            .and_then(|n| n.checked_add(doc.creation_id.len()))
            .ok_or("document snapshot size overflow")?;
        if string_bytes > 1024 * 1024 {
            return Err("document snapshot byte limit exceeded".into());
        }
        if doc.creation_id.is_empty() {
            return Err("missing document creation ID in probe".into());
        }
        // Autodesk preserves creationId when a document is copied. Keep every
        // inventory row; this metadata is not a unique native-object selector.
        if documents
            .iter()
            .any(|prior: &Document| prior.creation_id == doc.creation_id)
        {
            creation_ids_unique = false;
        }
        documents.push(doc);
    }
    if sdk.count()? != count {
        return Err("document count changed while reading".into());
    }
    let fusion_version = sdk.version()?;
    assemble_snapshot(documents, fusion_version, sdk.thread_id(), creation_ids_unique,
        "Read-only inventory metadata: copied documents can share creation IDs. Entry order, name and creation ID are not durable unique document selectors; mutation targeting requires an SDK object identity registry.")
}

pub(crate) fn assemble_snapshot(
    documents: Vec<Document>,
    fusion_version: String,
    thread_id: u32,
    creation_ids_unique: bool,
    identity_scope: &'static str,
) -> Result<Snapshot, String> {
    if documents.iter().filter(|doc| doc.active).count() > 1 {
        return Err("SDK returned multiple active documents".into());
    }
    if fusion_version.is_empty() {
        return Err("empty Fusion version".into());
    }
    Ok(Snapshot {
        implementation: "Rust with Autodesk C++ ABI adapter; no Python",
        scope:
            "read-only document inventory; discover mechanical support through native capabilities",
        fusion_version,
        thread_id,
        creation_ids_unique,
        identity_scope,
        documents,
    })
}

#[cfg(all(
    windows,
    any(feature = "native-sdk", feature = "native-bindings-check")
))]
mod native;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Fake {
        counts: Cell<usize>,
        first: usize,
        second: usize,
        fail: bool,
    }
    struct IdentifiedDocuments<'a>(&'a [&'a str]);
    impl ReadSdk for IdentifiedDocuments<'_> {
        fn version(&self) -> Result<String, String> {
            Ok("sdk-test".into())
        }
        fn thread_id(&self) -> u32 {
            42
        }
        fn count(&self) -> Result<usize, String> {
            Ok(self.0.len())
        }
        fn document(&self, index: usize) -> Result<Document, String> {
            Ok(Document {
                document_id: None,
                name: format!("document {index}"),
                creation_id: self.0[index].into(),
                active: index == 0,
                saved: true,
                modified: false,
            })
        }
    }
    impl ReadSdk for Fake {
        fn version(&self) -> Result<String, String> {
            Ok("sdk-test".into())
        }
        fn thread_id(&self) -> u32 {
            42
        }
        fn count(&self) -> Result<usize, String> {
            let n = self.counts.get();
            self.counts.set(n + 1);
            Ok(if n == 0 { self.first } else { self.second })
        }
        fn document(&self, index: usize) -> Result<Document, String> {
            if self.fail {
                return Err("invalid native handle".into());
            }
            Ok(Document {
                document_id: None,
                name: format!("document {index}"),
                creation_id: format!("id-{index}"),
                active: index == 0,
                saved: true,
                modified: false,
            })
        }
    }
    #[test]
    fn preserves_order_flags_and_native_identity() {
        let result = snapshot(&Fake {
            counts: Cell::new(0),
            first: 3,
            second: 3,
            fail: false,
        })
        .unwrap();
        assert_eq!(result.documents.len(), 3);
        assert_eq!(result.documents[2].creation_id, "id-2");
        assert!(result.documents[0].active);
        assert!(!result.documents[1].modified);
        assert!(result.creation_ids_unique);
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["creationIdsUnique"], true);
        assert!(encoded["identityScope"]
            .as_str()
            .unwrap()
            .contains("not durable unique document selectors"));
    }
    #[test]
    fn copied_creation_ids_retain_both_documents_and_report_nonunique_metadata() {
        let result = snapshot(&IdentifiedDocuments(&["original-id", "original-id"])).unwrap();
        assert_eq!(result.documents.len(), 2);
        assert_eq!(result.documents[0].name, "document 0");
        assert_eq!(result.documents[1].name, "document 1");
        assert_eq!(
            result.documents[0].creation_id,
            result.documents[1].creation_id
        );
        assert!(result.documents[0].active);
        assert!(!result.documents[1].active);
        assert!(!result.creation_ids_unique);
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["creationIdsUnique"], false);
        assert_eq!(encoded["documents"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn empty_creation_id_still_refuses_the_entire_inventory() {
        assert_eq!(
            snapshot(&IdentifiedDocuments(&["valid", ""])).unwrap_err(),
            "missing document creation ID in probe"
        );
    }
    #[test]
    fn refuses_unbounded_native_collections_before_read() {
        assert!(snapshot(&Fake {
            counts: Cell::new(0),
            first: 257,
            second: 257,
            fail: false
        })
        .unwrap_err()
        .contains("bounded"));
    }
    #[test]
    fn refuses_changed_collection() {
        assert!(snapshot(&Fake {
            counts: Cell::new(0),
            first: 2,
            second: 3,
            fail: false
        })
        .unwrap_err()
        .contains("changed"));
    }
    #[test]
    fn native_errors_propagate_without_partial_success() {
        assert_eq!(
            snapshot(&Fake {
                counts: Cell::new(0),
                first: 2,
                second: 2,
                fail: true
            })
            .unwrap_err(),
            "invalid native handle"
        );
    }
}
