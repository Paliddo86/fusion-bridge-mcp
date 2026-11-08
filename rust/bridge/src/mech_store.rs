//! The journal's SQLite storage (the add-in's journal.py `Journal` I/O), ported: one app-owned
//! directory, a fully initialized database published exclusively, FULL synchronous commits,
//! identity/schema/size checks on every open. Unsupported or corrupt storage is never reset.
//!
//! Parity notes: Python refused any Windows reparse point on the path; std's `is_symlink` covers
//! symlinks and junctions. The hardlink and ownership checks need st_nlink/st_uid, which stable
//! Rust exposes on Unix only, so on Windows they are skipped (the dir is per-user LOCALAPPDATA).

use fusion_core::mech::journal::{JournalError, Row, Table, Tx, APPLICATION_ID, MAX_DATABASE_BYTES, STORAGE_VERSION};
use rusqlite::{params, types::ValueRef, Connection, OpenFlags, OptionalExtension};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub struct SqliteTable { dir: PathBuf, path: PathBuf, max_bytes: u64 }

fn invalid(message: &str) -> JournalError { JournalError::new("operation_storage_invalid_path", message) }
fn failed(e: impl std::fmt::Display) -> JournalError { JournalError::new("operation_storage_failed", format!("sqlite3.Error: {e}")) }
fn io_failed(e: std::io::Error) -> JournalError { JournalError::new("operation_storage_failed", format!("OSError: {e}")) }

/// Is this component one of macOS's own root-level symlinks into `/private`?
///
/// `/var -> private/var`, `/tmp -> private/tmp`, `/etc -> private/etc` are shipped that way by
/// the OS, so a path check that refuses every symlink refuses almost every real path on a Mac.
/// Two conditions keep the exemption tight:
///   * the component is exactly `/var`, `/tmp` or `/etc` (a direct child of `/`);
///   * what it resolves to actually lands under `/private` — so a replacement link pointing
///     into a user's home does NOT get a free pass.
#[cfg(target_os = "macos")]
fn is_macos_system_symlink(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if !matches!(name, "var" | "tmp" | "etc") {
        return false;
    }
    // one component only: the parent must be the filesystem root
    if path.parent() != Some(Path::new("/")) {
        return false;
    }
    let target = format!("/private/{name}");
    match std::fs::symlink_metadata(&target) {
        Ok(m) => m.is_dir(),
        Err(_) => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn is_macos_system_symlink(_path: &Path) -> bool { false }

pub fn hex32() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl SqliteTable {
    pub fn new(dir: PathBuf) -> Self { let path = dir.join("operations-v1.sqlite3"); SqliteTable { dir, path, max_bytes: MAX_DATABASE_BYTES } }

    fn staged_files(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.dir).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".operations-v1-") && n.ends_with(".tmp"))).collect()
    }

    /// `_initialization_link`: during exclusive publication the database briefly has two links,
    /// the public name and one same-inode staging file in this directory.
    #[cfg(unix)]
    fn initialization_link(&self, info: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        if info.nlink() != 2 { return false; }
        let matches: Vec<PathBuf> = self.staged_files().into_iter().filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.len() == ".operations-v1-.tmp".len() + 32 && name[15..47].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                && std::fs::symlink_metadata(p).is_ok_and(|o| o.is_file() && (o.dev(), o.ino()) == (info.dev(), info.ino()))
        }).collect();
        if matches.is_empty() {
            return std::fs::symlink_metadata(&self.path).is_ok_and(|f| (f.dev(), f.ino()) == (info.dev(), info.ino()) && f.nlink() == 1);
        }
        matches.len() == 1
    }

    fn check_paths(&self, create: bool) -> Result<(), JournalError> {
        if !self.dir.is_absolute() || self.dir.components().any(|c| c == Component::ParentDir) {
            return Err(invalid("Operation storage must be an absolute app-owned path."));
        }
        let mut chain: Vec<&Path> = self.dir.ancestors().collect();
        chain.reverse();
        for item in chain {
            let Ok(info) = std::fs::symlink_metadata(item) else { continue };
            if !info.is_dir() || info.file_type().is_symlink() {
                // macOS FIRMWARE symlinks. `/var`, `/tmp` and `/etc` are symlinks into
                // `/private` by OS design, so EVERY ordinary path on a Mac traverses one:
                // `std::env::temp_dir()` is `/var/folders/...`, and the four mech journal
                // tests failed here with "cannot traverse links" before this exemption.
                //
                // Scoped deliberately narrowly: only these three names, only at the ROOT
                // (one component), and only when the link resolves inside /private. A symlink
                // the user creates anywhere deeper still fails, so the guarantee this check
                // exists for — an app-owned path cannot be redirected elsewhere — holds.
                if !is_macos_system_symlink(item) {
                    return Err(invalid("Operation storage cannot traverse links, junctions or non-directories."));
                }
            }
        }
        if create {
            std::fs::create_dir_all(&self.dir).map_err(io_failed)?;
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700)); }
        }
        #[cfg(unix)]
        if let Ok(info) = std::fs::metadata(&self.dir) {
            use std::os::unix::fs::MetadataExt;
            extern "C" { fn getuid() -> u32; }
            if info.uid() != unsafe { getuid() } || info.mode() & 0o022 != 0 { return Err(invalid("Operation storage must be owned by this user and not writable by others.")); }
        }
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let item = PathBuf::from(format!("{}{suffix}", self.path.display()));
            let Ok(info) = std::fs::symlink_metadata(&item) else { continue };
            #[cfg(unix)]
            let links_safe = { use std::os::unix::fs::MetadataExt; info.nlink() == 1 || (suffix.is_empty() && self.initialization_link(&info)) };
            #[cfg(not(unix))]
            let links_safe = true;
            if !info.is_file() || info.file_type().is_symlink() || !links_safe { return Err(invalid("Operation storage files cannot be links or special files.")); }
        }
        Ok(())
    }

    /// `_initialize`: publish a fully initialized database exclusively, so concurrent first
    /// writers never see a zero-byte file as corrupt or reset one.
    fn initialize(&self) -> Result<(), JournalError> {
        if self.staged_files().len() >= 32 {
            return Err(JournalError::new("operation_storage_unavailable", "Too many unfinished journal initialization files; retained for inspection."));
        }
        let staged = self.dir.join(format!(".operations-v1-{}.tmp", hex32()));
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; open.mode(0o600); }
        drop(open.open(&staged).map_err(io_failed)?);
        let publish = || -> Result<(), JournalError> {
            let conn = Connection::open(&staged).map_err(failed)?;
            conn.execute_batch(&format!("PRAGMA synchronous=FULL; BEGIN IMMEDIATE; CREATE TABLE operations (session_id TEXT NOT NULL, operation_id TEXT NOT NULL, record_json BLOB NOT NULL, record_digest TEXT NOT NULL, PRIMARY KEY(session_id,operation_id)); PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={STORAGE_VERSION}; COMMIT;")).map_err(failed)?;
            drop(conn);
            std::fs::OpenOptions::new().read(true).write(true).open(&staged).and_then(|f| f.sync_all()).map_err(io_failed)?;
            match std::fs::hard_link(&staged, &self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {} // another initializer published first
                Err(e) => return Err(io_failed(e)),
            }
            std::fs::remove_file(&staged).map_err(io_failed)?;
            #[cfg(unix)] { let _ = std::fs::File::open(&self.dir).and_then(|d| d.sync_all()); }
            Ok(())
        };
        let out = publish();
        if staged.exists() { let _ = std::fs::remove_file(&staged); }
        out
    }

    fn connect(&self, create: bool) -> Result<Option<Connection>, JournalError> {
        self.check_paths(create)?;
        let exists = self.path.exists();
        if !exists && !create { return Ok(None); }
        if !exists { self.initialize()?; }
        // Existing-history reads may recover SQLite's hot rollback journal after a crash, so
        // they open read-write too; neither mode creates a missing file here.
        let conn = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(failed)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(failed)?;
        conn.execute_batch("PRAGMA synchronous=FULL").map_err(failed)?;
        let app_id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(failed)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(failed)?;
        if app_id != APPLICATION_ID { return Err(JournalError::new("operation_storage_corrupt", "Storage is not an operation journal; it will not be reset.")); }
        if version != STORAGE_VERSION as i64 { return Err(JournalError::new("operation_storage_version", "Unsupported operation journal version; automatic migration is disabled.")); }
        let columns: Vec<String> = {
            let mut st = conn.prepare("PRAGMA table_info(operations)").map_err(failed)?;
            let rows = st.query_map([], |r| r.get::<_, String>(1)).map_err(failed)?;
            rows.collect::<Result<_, _>>().map_err(failed)?
        };
        if columns != ["session_id", "operation_id", "record_json", "record_digest"] { return Err(JournalError::new("operation_storage_corrupt", "Operation journal schema is invalid.")); }
        if std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) > self.max_bytes { return Err(JournalError::new("operation_storage_full", "Operation journal exceeds its database byte limit.")); }
        if create {
            let page: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0)).map_err(failed)?;
            conn.execute_batch(&format!("PRAGMA max_page_count={}", (self.max_bytes as i64 / page.max(1)).max(1))).map_err(failed)?;
            let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(failed)?;
            if check != "ok" { return Err(JournalError::new("operation_storage_corrupt", "Operation database integrity check failed.")); }
        }
        Ok(Some(conn))
    }
}

struct SqliteTx { conn: Connection }

fn blob(v: ValueRef) -> Vec<u8> { match v { ValueRef::Blob(b) | ValueRef::Text(b) => b.to_vec(), _ => vec![] } }

impl Table for SqliteTable {
    fn open(&self, create: bool) -> Result<Option<Box<dyn Tx + '_>>, JournalError> {
        let Some(conn) = self.connect(create)? else { return Ok(None) };
        if create { conn.execute_batch("BEGIN IMMEDIATE").map_err(failed)?; }
        Ok(Some(Box::new(SqliteTx { conn })))
    }
}

impl Tx for SqliteTx {
    fn get(&mut self, session: &str, operation: &str) -> Result<Option<(Vec<u8>, String)>, JournalError> {
        self.conn.query_row("SELECT record_json,record_digest FROM operations WHERE session_id=?1 AND operation_id=?2", params![session, operation],
            |r| Ok((blob(r.get_ref(0)?), r.get::<_, String>(1)?))).optional().map_err(failed)
    }
    fn all(&mut self) -> Result<Vec<Row>, JournalError> {
        let mut st = self.conn.prepare("SELECT session_id,operation_id,record_json,record_digest FROM operations").map_err(failed)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, blob(r.get_ref(2)?), r.get::<_, String>(3)?))).map_err(failed)?;
        rows.collect::<Result<_, _>>().map_err(failed)
    }
    fn count(&mut self) -> Result<u64, JournalError> {
        self.conn.query_row("SELECT COUNT(*) FROM operations", [], |r| r.get::<_, i64>(0)).map(|n| n as u64).map_err(failed)
    }
    fn put(&mut self, row: &Row) -> Result<(), JournalError> {
        self.conn.execute("INSERT OR REPLACE INTO operations VALUES (?1,?2,?3,?4)", params![row.0, row.1, row.2, row.3]).map(|_| ()).map_err(failed)
    }
    fn commit(self: Box<Self>) -> Result<(), JournalError> { self.conn.execute_batch("COMMIT").map_err(failed) }
}

#[cfg(test)]
mod path_check_tests {
    use super::*;

    #[test]
    fn macos_firmware_symlinks_are_exempt_only_at_the_root_and_only_into_private() {
        // `/var`, `/tmp`, `/etc` are shipped as symlinks into `/private` by macOS. The temp dir
        // is under `/var/folders/...`, so refusing every symlink refused every ordinary path on
        // a Mac — that is what failed the four mech journal tests.
        for name in ["var", "tmp", "etc"] {
            let p = PathBuf::from("/").join(name);
            #[cfg(target_os = "macos")]
            assert!(is_macos_system_symlink(&p), "{} must be exempt on macOS", p.display());
            #[cfg(not(target_os = "macos"))]
            assert!(!is_macos_system_symlink(&p), "no exemption off macOS");
        }
    }

    #[test]
    fn a_user_symlink_is_still_refused() {
        // The exemption must not become a hole: any link the exemption does not name is refused,
        // and the same names are refused when they are NOT the root-level component.
        for p in ["/Users/x/var", "/home/x/tmp", "/private/etc", "/var/folders/81/x"] {
            assert!(!is_macos_system_symlink(Path::new(p)), "{p} must not be exempt");
        }
    }

    #[test]
    fn the_temp_dir_passes_the_path_check() {
        // The regression this fixes, end to end: a directory under std::env::temp_dir() must
        // survive check_paths(), which is what every mech journal test does.
        let dir = std::env::temp_dir().join(format!("mech-pathcheck-{}", hex32()));
        let t = SqliteTable::new(dir.clone());
        t.check_paths(true).expect("temp dir must be an acceptable journal path");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
