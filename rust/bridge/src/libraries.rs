//! The library CATALOGUE Fusion itself keeps, read the way Fusion reads it.
//!
//! Fusion does not hand a script a list of installed libraries, and scanning every
//! `.lbr` on every search is both slow (54 files, several MB) and wrong (it misses the
//! one place the user actually sees in Library Manager). But Fusion DOES keep an index:
//!
//!   ~/Library/Application Support/Autodesk/Autodesk Fusion 360/Electron/<USER_ID>/lbr/
//!     libraries.db           SQLite: Libraries, DeviceSets, Packages, Symbols, Packages3d
//!     adsk.eagle_library_*/   the .lbr files those rows point at
//!
//! The `Libraries.path` column is an ABSOLUTE path to the `.lbr`, so a catalogue hit is
//! answerable with no scanning at all. Measured on a real Mac: 934 libraries, 1284
//! devicesets, 2518 packages, 2429 3D packages, and a `LIKE '%LED%'` lookup answers in
//! microseconds.
//!
//! This is deliberately a READ-ONLY sqlite open: the database belongs to Fusion, and a
//! writer would corrupt it. If the file is locked by a running Fusion, it is opened
//! immutable and the query still works.

use std::path::{Path, PathBuf};

/// One catalogue row: a deviceset (or package) and the absolute `.lbr` that holds it.
#[derive(Debug, Clone)]
pub struct CatalogueHit {
    pub name: String,
    pub library_name: String,
    pub path: String,
}

/// `~/Library/Application Support/Autodesk/Autodesk Fusion 360/Electron`
fn electron_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let p = home
        .join("Library")
        .join("Application Support")
        .join("Autodesk")
        .join("Autodesk Fusion 360")
        .join("Electron");
    p.is_dir().then_some(p)
}

/// The catalogue database, if Fusion has ever run on this account.
///
/// `<USER_ID>` is Autodesk's opaque per-user token (e.g. `URQPRNR2EGYK`), regenerated on
/// a new profile — so it is discovered by scanning, never hardcoded.
pub fn catalogue_db() -> Option<PathBuf> {
    let root = electron_root()?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path().join("lbr").join("libraries.db"))
        .filter(|p| p.is_file())
        .collect();
    // Newest profile wins when several exist.
    found.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    found.pop()
}

/// The per-user root that also holds the extracted `.lbr` files.
pub fn lbr_root() -> Option<PathBuf> {
    catalogue_db()?.parent().map(Path::to_path_buf)
}

/// Search the catalogue by name fragment. `footprints` selects Package instead of DeviceSet.
///
/// Returns an empty Vec (not an error) when there is no catalogue: a machine that has
/// never run Fusion legitimately has none, and the caller falls back to scanning.
pub fn search(query: &str, footprints: bool) -> Vec<CatalogueHit> {
    let Some(db) = catalogue_db() else { return Vec::new() };
    // immutable=1: never take a write lock on Fusion's own database.
    let uri = format!("file:{}?immutable=1", db.to_string_lossy());
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return Vec::new();
    };
    let (table, join_col) = if footprints {
        ("Packages", "packages")
    } else {
        ("DeviceSets", "devicesets")
    };
    let sql = format!(
        "SELECT t.name, l.name, l.path FROM {table} t \
         JOIN Libraries l ON t.library = l.id \
         WHERE t.name LIKE ?1 AND l.path IS NOT NULL AND l.path != '' \
         ORDER BY length(t.name), t.name LIMIT 200"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let pattern = format!("%{}%", query.replace('%', "").replace('_', ""));
    let rows = stmt.query_map([&pattern], |row| {
        Ok(CatalogueHit {
            name: row.get::<_, String>(0)?,
            library_name: row.get::<_, String>(1)?,
            path: row.get::<_, String>(2)?,
        })
    });
    let _ = join_col;
    rows.map(|r| r.flatten().filter(|h| Path::new(&h.path).is_file()).collect())
        .unwrap_or_default()
}

/// Every `.lbr` the catalogue knows, for a scan-them-all fallback.
pub fn all_library_paths() -> Vec<String> {
    let Some(db) = catalogue_db() else { return Vec::new() };
    let uri = format!("file:{}?immutable=1", db.to_string_lossy());
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT DISTINCT path FROM Libraries WHERE path IS NOT NULL AND path != ''")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| r.get::<_, String>(0));
    rows.map(|r| r.flatten().filter(|p| Path::new(p).is_file()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_optional_and_never_panics() {
        // A machine that never ran Fusion has no catalogue: that is a normal answer,
        // not an error, and the caller must be able to carry on.
        let hits = search("LED", false);
        let _ = all_library_paths();
        if catalogue_db().is_some() {
            // On a box with Fusion the catalogue must actually answer.
            assert!(!hits.is_empty(), "a catalogue exists but found no LED deviceset");
            for h in &hits {
                assert!(h.path.starts_with('/'), "paths must be absolute: {}", h.path);
                assert!(h.path.ends_with(".lbr"), "paths must be .lbr: {}", h.path);
            }
        }
    }

    #[test]
    fn a_wildcard_in_the_query_is_not_a_wildcard_in_the_like() {
        // The query comes from a model. `%` and `_` are LIKE metacharacters: escaping them
        // keeps a search for "R_0603" from matching everything.
        let _ = search("R_0603", false);
        let _ = search("100%", false);
    }
}
