//! The verb catalog is the contract both the Python bridge and this crate must serve
//! identically. It is frozen in `contracts/describe.json` (regenerated ONLY from
//! describe.py) and loaded here so the Rust router can assert it serves every verb the
//! catalog names, with the same argument names.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub contract: String,
    pub frozen: String,
    pub verbs: serde_json::Value,
}

pub const FROZEN: &str = include_str!("../../../contracts/describe.json");

pub fn load() -> Result<Catalog, serde_json::Error> {
    serde_json::from_str(FROZEN)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_frozen_catalog_parses_and_is_nonempty() {
        let c = super::load().expect("contracts/describe.json must parse");
        let n = c.verbs.as_array().map(|a| a.len())
            .or_else(|| c.verbs.get("commands").and_then(|v| v.as_array()).map(|a| a.len()))
            .unwrap_or(0);
        assert!(n >= 100, "expected the full verb catalog, got {n}");
    }
}
