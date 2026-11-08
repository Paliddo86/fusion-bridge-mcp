//! Publish already-generated, bounded bytes on the host filesystem.
//!
//! The destination commit is a single rename (replacement) or exclusive hardlink.
//! Publication is independent of model rollback. This is not an SDK exporter,
//! authorization check, journal, or guarantee of durability after power loss.

use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
mod windows;

pub const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct Publication {
    pub output_path: PathBuf,
    pub file_size: usize,
    pub sha256: String,
    /// True means the OS acknowledged the namespace commit, even if cleanup failed.
    pub published: bool,
    pub overwrite: bool,
    pub cleanup_warning: Option<String>,
}

#[derive(Debug)]
pub struct Refusal {
    pub stage: &'static str,
    pub source: io::Error,
    /// A failed attempt can still leave an owned staging directory for diagnosis.
    pub cleanup_warning: Option<String>,
    /// A commit syscall error cannot authorize an automatic retry.
    pub publication: PublicationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationState {
    NotAttempted,
    Unknown,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.stage, self.source)
    }
}
impl std::error::Error for Refusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
fn error(stage: &'static str, source: io::Error) -> Refusal {
    Refusal {
        stage,
        source,
        cleanup_warning: None,
        publication: if stage.starts_with("commit_") {
            PublicationState::Unknown
        } else {
            PublicationState::NotAttempted
        },
    }
}
fn invalid(message: &'static str) -> Refusal {
    error(
        "preflight",
        io::Error::new(io::ErrorKind::InvalidInput, message),
    )
}

/// Publish a UTF-8 CSV produced by the separately verified BOM core.
/// Existing files are preserved unless `overwrite` is explicitly true.
pub fn publish_csv(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<Publication, Refusal> {
    validate_size(bytes)?;
    if destination
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("csv")
    {
        return Err(invalid("BOM output requires a .csv extension"));
    }
    std::str::from_utf8(bytes).map_err(|_| invalid("BOM output must be UTF-8"))?;
    publish_bytes(destination, bytes, overwrite)
}

/// This API accepts bytes, never an arbitrary on-disk staging path.
pub fn publish_bytes(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<Publication, Refusal> {
    publish_with(destination, bytes, overwrite, |_, _| Ok(()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    BeforeWrite,
    BeforeCommit,
    AfterCommit,
}

fn publish_with(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
    mut checkpoint: impl FnMut(Checkpoint, &Path) -> io::Result<()>,
) -> Result<Publication, Refusal> {
    validate_path(destination)?;
    validate_size(bytes)?;
    check_destination(destination, overwrite)?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("output requires a parent"))?;
    fs::create_dir_all(parent).map_err(|e| error("create_parent", e))?;
    // Resolve the selected parent once before acquiring our private staging directory.
    // Ancestor changes by a hostile process are outside this path-based API's scope.
    let parent = fs::canonicalize(parent).map_err(|e| error("resolve_parent", e))?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| invalid("missing filename"))?,
    );
    check_destination(&destination, overwrite)?;
    let mut staging = Staging::create(&parent).map_err(|e| error("create_staging", e))?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let before_commit = (|| -> Result<(), Refusal> {
        checkpoint(Checkpoint::BeforeWrite, &staging.file)
            .map_err(|e| error("write_staging", e))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging.file)
            .map_err(|e| error("open_staging", e))?;
        file.write_all(bytes)
            .map_err(|e| error("write_staging", e))?;
        file.sync_all().map_err(|e| error("sync_staging", e))?;
        // Windows cannot rename/remove an incompatible open file handle.
        drop(file);
        checkpoint(Checkpoint::BeforeCommit, &staging.file)
            .map_err(|e| error("before_commit", e))?;
        check_destination(&destination, overwrite)?;
        if overwrite {
            fs::rename(&staging.file, &destination).map_err(|e| error("commit_replace", e))?;
        } else {
            // An exists() + rename sequence would overwrite a concurrent writer.
            fs::hard_link(&staging.file, &destination).map_err(|e| error("commit_exclusive", e))?;
        }
        Ok(())
    })();
    if let Err(mut failure) = before_commit {
        failure.cleanup_warning = staging.cleanup().err().map(|e| e.to_string());
        return Err(failure);
    }
    // No fallible operation after this point is allowed to report a failed publish.
    let post_commit = checkpoint(Checkpoint::AfterCommit, &staging.file).err();
    let cleanup = staging.cleanup().err();
    let warnings = [post_commit, cleanup]
        .into_iter()
        .flatten()
        .map(|e| e.to_string())
        .collect::<Vec<_>>();
    Ok(Publication {
        output_path: destination,
        file_size: bytes.len(),
        sha256,
        published: true,
        overwrite,
        cleanup_warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
    })
}

fn validate_path(destination: &Path) -> Result<(), Refusal> {
    if !destination.is_absolute()
        || destination.file_name().is_none()
        || destination
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(invalid(
            "output must be an absolute filename without parent traversal",
        ));
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        // Device paths/alternate streams have different publication semantics.
        for component in destination.components() {
            match component {
                Component::Prefix(p)
                    if !matches!(p.kind(), Prefix::Disk(_) | Prefix::UNC(_, _)) =>
                {
                    return Err(invalid("device and verbatim output paths are unsupported"));
                }
                Component::Normal(name) => {
                    let text = name.to_string_lossy();
                    let stem = text
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(' ')
                        .to_ascii_uppercase();
                    if text
                        .chars()
                        .any(|c| matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
                        || text.ends_with([' ', '.'])
                        || text.chars().any(|c| c < ' ')
                        || matches!(
                            stem.as_str(),
                            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                        )
                        || ["COM", "LPT"].iter().any(|p| {
                            stem.strip_prefix(p).is_some_and(|s| {
                                matches!(
                                    s,
                                    "1" | "2"
                                        | "3"
                                        | "4"
                                        | "5"
                                        | "6"
                                        | "7"
                                        | "8"
                                        | "9"
                                        | "¹"
                                        | "²"
                                        | "³"
                                )
                            })
                        })
                    {
                        return Err(invalid("special Windows output names are unsupported"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
fn validate_size(bytes: &[u8]) -> Result<(), Refusal> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        Err(invalid("output must contain between 1 byte and 16 MiB"))
    } else {
        Ok(())
    }
}
fn check_destination(destination: &Path, overwrite: bool) -> Result<(), Refusal> {
    match fs::symlink_metadata(destination) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(invalid(
                    "destination must be a regular file, not a link or directory",
                ));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(invalid("destination reparse points are unsupported"));
                }
            }
            if !overwrite {
                return Err(error(
                    "destination_exists",
                    io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"),
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error("inspect_destination", e)),
    }
}

struct Staging {
    directory: PathBuf,
    file: PathBuf,
}
impl Staging {
    fn create(parent: &Path) -> io::Result<Self> {
        for _ in 0..8 {
            let mut nonce = [0_u8; 16];
            getrandom::getrandom(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
            let suffix = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let directory = parent.join(format!(".adom-export-{suffix}"));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&directory)
            };
            #[cfg(windows)]
            let created = windows::create_private_directory(&directory);
            #[cfg(not(any(unix, windows)))]
            let created = Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private staging is unsupported on this platform",
            ));
            match created {
                Ok(()) => {
                    return Ok(Self {
                        file: directory.join("payload"),
                        directory,
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "staging reservation exhausted",
        ))
    }
    fn cleanup(&mut self) -> io::Result<()> {
        match fs::remove_file(&self.file) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        // Never recursively delete an unexpected file created by another actor.
        match fs::remove_dir(&self.directory) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests;
