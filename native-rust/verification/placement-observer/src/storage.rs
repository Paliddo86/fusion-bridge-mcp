//! Private create-new output. Existing objects are inspected, never repaired.
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
#[cfg(any(windows, test))]
mod storage_policy;
#[cfg(windows)]
mod windows_storage;
#[derive(Debug)]
struct Error(String);
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}
#[cfg(windows)]
impl Error {
    fn at_path(self, stage: &str, path: &Path) -> Self {
        Self(format!("{stage} [{}]: {}", path.display(), self.0))
    }
}
type Result<T> = std::result::Result<T, Error>;
fn error(code: &str, message: &str) -> Error {
    Error(format!("{code}: {message}"))
}
#[cfg(windows)]
fn io_at<T>(result: std::io::Result<T>, stage: &str, path: &Path) -> Result<T> {
    result.map_err(|e| Error::from(e).at_path(stage, path))
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    index: u64,
    links: u64,
}
fn path_exists(p: &Path) -> Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn identity(p: &Path) -> Result<FileIdentity> {
    #[cfg(windows)]
    {
        windows_storage::validate_path(p)?;
        let value = windows_storage::identity(p)?;
        if value.links != 1 {
            return Err(error("linked_file", "exactly one link required"));
        }
        Ok(value)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = fs::symlink_metadata(p)?;
        if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 {
            return Err(error(
                "invalid_file",
                "private regular single-link file required",
            ));
        }
        Ok(FileIdentity {
            volume: m.dev(),
            index: m.ino(),
            links: m.nlink(),
        })
    }
}
fn opened_identity(f: &File) -> Result<FileIdentity> {
    #[cfg(windows)]
    {
        windows_storage::inspect(f, false)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = f.metadata()?;
        Ok(FileIdentity {
            volume: m.dev(),
            index: m.ino(),
            links: m.nlink(),
        })
    }
}
fn directory(p: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        windows_storage::validate_path(p)?;
        windows_storage::validate_directory(p)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = fs::symlink_metadata(p)?;
        if !m.is_dir() || m.file_type().is_symlink() || m.mode() & 0o077 != 0 {
            return Err(error(
                "invalid_directory",
                "private real directory required",
            ));
        }
        Ok(())
    }
}
fn new_directory(p: &Path) -> Result<()> {
    if path_exists(p)? {
        return Err(error("existing_directory", "never reuse output directory"));
    }
    #[cfg(windows)]
    {
        windows_storage::validate_path(p)?;
        windows_storage::create_directory_new(p)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(p)?;
    }
    directory(p)
}
fn new_file(p: &Path) -> Result<File> {
    #[cfg(windows)]
    {
        windows_storage::create_file(p)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(p)?)
    }
}
fn locked_read(p: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    Ok(options.open(p)?)
}
pub fn read_private(p: &Path, max: u64) -> crate::Result<Vec<u8>> {
    (function_read(p, max)).map_err(|e| e.0)
}
fn function_read(p: &Path, max: u64) -> Result<Vec<u8>> {
    let before = identity(p)?;
    let mut f = locked_read(p)?;
    if opened_identity(&f)? != before {
        return Err(error("file_changed", "opened file identity differs"));
    }
    let mut data = Vec::new();
    (&mut f).take(max + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max || identity(p)? != before {
        return Err(error(
            "bounded_read",
            "file exceeded bound or changed identity",
        ));
    }
    Ok(data)
}
pub fn immutable_receipt(p: &Path, value: &Value) -> crate::Result<()> {
    (function_receipt(p, value)).map_err(|e| e.0)
}
fn function_receipt(p: &Path, value: &Value) -> Result<()> {
    directory(p.parent().ok_or_else(|| error("no_parent", "receipt"))?)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| Error(e.to_string()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(error("receipt_size", "one MiB bound"));
    }
    let mut f = new_file(p)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}
pub fn create_output(p: &Path) -> crate::Result<()> {
    new_directory(p).map_err(|e| e.0)
}
