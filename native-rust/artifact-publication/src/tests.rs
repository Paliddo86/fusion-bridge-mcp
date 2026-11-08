use super::*;
use std::sync::{Arc, Barrier};

pub(super) struct Directory(pub(super) PathBuf);
impl Directory {
    pub(super) fn new() -> Self {
        let stage = Staging::create(&std::env::temp_dir()).unwrap();
        Self(stage.directory)
    }
    fn file(&self) -> PathBuf {
        self.0.join("bom.csv")
    }
    fn entries(&self) -> usize {
        fs::read_dir(&self.0).unwrap().count()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn injected() -> io::Error {
    io::Error::other("injected filesystem failure")
}

#[test]
fn new_csv_has_exact_bytes_size_hash_and_no_staging_left() {
    let dir = Directory::new();
    let bytes = b"part,quantity\r\nbolt,4\r\n";
    let result = publish_csv(&dir.file(), bytes, false).unwrap();
    assert!(result.published);
    assert!(!result.overwrite);
    assert_eq!(result.output_path, fs::canonicalize(&dir.file()).unwrap());
    assert_eq!(result.file_size, bytes.len());
    assert_eq!(result.sha256, format!("{:x}", Sha256::digest(bytes)));
    assert!(result.cleanup_warning.is_none());
    assert_eq!(fs::read(dir.file()).unwrap(), bytes);
    assert_eq!(dir.entries(), 1);
}

#[test]
fn existing_destination_requires_explicit_replace() {
    let dir = Directory::new();
    fs::write(dir.file(), b"original").unwrap();
    let err = publish_csv(&dir.file(), b"replacement", false).unwrap_err();
    assert_eq!(err.source.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(dir.file()).unwrap(), b"original");
    assert_eq!(dir.entries(), 1);
    let result = publish_csv(&dir.file(), b"replacement", true).unwrap();
    assert!(result.published && result.overwrite);
    assert_eq!(fs::read(dir.file()).unwrap(), b"replacement");
    assert_eq!(dir.entries(), 1);
}

#[test]
fn replacement_does_not_mutate_another_hardlink_to_old_file() {
    let dir = Directory::new();
    fs::write(dir.file(), b"original").unwrap();
    let link = dir.0.join("old.csv");
    fs::hard_link(dir.file(), &link).unwrap();
    publish_csv(&dir.file(), b"replacement", true).unwrap();
    assert_eq!(fs::read(link).unwrap(), b"original");
    assert_eq!(fs::read(dir.file()).unwrap(), b"replacement");
}

#[test]
fn nonempty_and_size_bounds_apply_before_parent_creation() {
    let dir = Directory::new();
    let output = dir.0.join("absent/bom.csv");
    assert!(publish_csv(&output, b"", false).is_err());
    assert!(publish_csv(&output, &vec![b'x'; MAX_BYTES + 1], false).is_err());
    assert_eq!(dir.entries(), 0);
    publish_csv(&output, &vec![b'x'; MAX_BYTES], false).unwrap();
    assert_eq!(fs::metadata(output).unwrap().len(), MAX_BYTES as u64);
}

#[test]
fn csv_requires_utf8_and_matching_extension_without_filesystem_changes() {
    let dir = Directory::new();
    assert!(publish_csv(&dir.file(), &[0xff], false).is_err());
    assert!(publish_csv(&dir.0.join("part.step"), b"x", false).is_err());
    assert_eq!(dir.entries(), 0);
    publish_csv(
        &dir.0.join("pièce.CSV"),
        "名,数\r\n螺栓,3\r\n".as_bytes(),
        false,
    )
    .unwrap();
    assert_eq!(dir.entries(), 1);
}

#[test]
fn relative_directory_and_parent_traversal_destinations_are_refused() {
    let dir = Directory::new();
    assert!(publish_bytes(Path::new("bom.csv"), b"x", false).is_err());
    assert!(publish_bytes(&dir.0, b"x", true).is_err());
    assert!(publish_bytes(&dir.0.join("inner/../bom.csv"), b"x", true).is_err());
    assert_eq!(dir.entries(), 0);
}

#[test]
fn file_as_parent_is_refused_and_preserved() {
    let dir = Directory::new();
    let parent = dir.0.join("parent");
    fs::write(&parent, b"original").unwrap();
    assert!(publish_csv(&parent.join("bom.csv"), b"x", false).is_err());
    assert_eq!(fs::read(parent).unwrap(), b"original");
    assert_eq!(dir.entries(), 1);
}

#[test]
fn commit_errors_preserve_unknown_outcome_while_preflight_is_not_attempted() {
    let dir = Directory::new();
    let err = publish_with(&dir.file(), b"ours", false, |point, stage| {
        if point == Checkpoint::BeforeCommit {
            fs::remove_file(stage)?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.stage, "commit_exclusive");
    assert_eq!(err.publication, PublicationState::Unknown);
    assert!(!dir.file().exists());
    assert_eq!(dir.entries(), 0);
    assert_eq!(
        publish_bytes(&dir.file(), b"", false)
            .unwrap_err()
            .publication,
        PublicationState::NotAttempted
    );
}

#[test]
fn concurrent_exclusive_writers_have_exactly_one_winner_and_whole_payload() {
    let dir = Directory::new();
    let gate = Arc::new(Barrier::new(8));
    let mut handles = vec![];
    for i in 0..8 {
        let output = dir.file();
        let gate = gate.clone();
        handles.push(std::thread::spawn(move || {
            let bytes = vec![b'0' + i; 65_537];
            let result = publish_with(&output, &bytes, false, |point, _| {
                if point == Checkpoint::BeforeCommit {
                    gate.wait();
                }
                Ok(())
            });
            (i, result)
        }));
    }
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|(_, r)| r.is_ok()).count(), 1);
    let winner = results.iter().find(|(_, r)| r.is_ok()).unwrap().0;
    assert_eq!(fs::read(dir.file()).unwrap(), vec![b'0' + winner; 65_537]);
    assert!(results
        .iter()
        .filter_map(|(_, r)| r.as_ref().err())
        .all(|e| e.source.kind() == io::ErrorKind::AlreadyExists));
    assert_eq!(dir.entries(), 1);
}

#[test]
fn late_destination_creation_cannot_be_overwritten_by_exclusive_publish() {
    let dir = Directory::new();
    let output = dir.file();
    let err = publish_with(&output, b"ours", false, |point, _| {
        if point == Checkpoint::BeforeCommit {
            fs::write(&output, b"other writer")?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.source.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&output).unwrap(), b"other writer");
    assert_eq!(dir.entries(), 1);
}

#[test]
fn failure_before_write_or_after_full_staging_preserves_destination() {
    for fail_at in [Checkpoint::BeforeWrite, Checkpoint::BeforeCommit] {
        let dir = Directory::new();
        fs::write(dir.file(), b"original").unwrap();
        let error = publish_with(&dir.file(), b"ours", true, |point, _| {
            if point == fail_at {
                Err(injected())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.cleanup_warning.is_none());
        assert_eq!(fs::read(dir.file()).unwrap(), b"original");
        assert_eq!(dir.entries(), 1);
    }
}

#[test]
fn directory_created_at_destination_prevents_commit_and_is_not_removed() {
    let dir = Directory::new();
    let output = dir.file();
    assert!(publish_with(&output, b"ours", true, |point, _| {
        if point == Checkpoint::BeforeCommit {
            fs::create_dir(&output)?;
            fs::write(output.join("keep"), b"user")?;
        }
        Ok(())
    })
    .is_err());
    assert_eq!(fs::read(output.join("keep")).unwrap(), b"user");
    assert_eq!(dir.entries(), 1);
}

#[test]
fn post_commit_failure_is_success_with_warning_not_a_retryable_failure() {
    let dir = Directory::new();
    let result = publish_with(&dir.file(), b"committed", false, |point, _| {
        if point == Checkpoint::AfterCommit {
            Err(injected())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert!(result.published);
    assert!(result.cleanup_warning.unwrap().contains("injected"));
    assert_eq!(fs::read(dir.file()).unwrap(), b"committed");
    assert_eq!(dir.entries(), 1);
}

#[test]
fn unexpected_staging_child_is_retained_and_cannot_mask_success() {
    let dir = Directory::new();
    let mut other = None;
    let result = publish_with(&dir.file(), b"committed", false, |point, stage| {
        if point == Checkpoint::AfterCommit {
            let path = stage.parent().unwrap().join("unowned");
            fs::write(&path, b"keep")?;
            other = Some(path);
        }
        Ok(())
    })
    .unwrap();
    assert!(result.published && result.cleanup_warning.is_some());
    assert_eq!(fs::read(other.unwrap()).unwrap(), b"keep");
    assert_eq!(fs::read(dir.file()).unwrap(), b"committed");
}

#[test]
fn cleanup_failure_before_commit_is_reported_with_original_failure() {
    let dir = Directory::new();
    let mut other = None;
    let result = publish_with(&dir.file(), b"not committed", false, |point, stage| {
        if point == Checkpoint::BeforeCommit {
            let path = stage.parent().unwrap().join("unowned");
            fs::write(&path, b"keep")?;
            other = Some(path);
            return Err(injected());
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(result.stage, "before_commit");
    assert!(result.cleanup_warning.is_some());
    assert!(!dir.file().exists());
    assert_eq!(fs::read(other.unwrap()).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn existing_and_dangling_symlinks_are_refused_without_following_them() {
    use std::os::unix::fs::symlink;
    let dir = Directory::new();
    let target = dir.0.join("target.csv");
    fs::write(&target, b"original").unwrap();
    symlink(&target, dir.file()).unwrap();
    assert!(publish_csv(&dir.file(), b"ours", true).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"original");
    fs::remove_file(&target).unwrap();
    assert!(publish_csv(&dir.file(), b"ours", true).is_err());
    assert!(!target.exists());
    assert!(fs::symlink_metadata(dir.file()).unwrap().is_symlink());
}

#[cfg(windows)]
#[test]
fn windows_device_stream_and_ambiguous_names_are_refused_before_io() {
    for path in [
        r"C:\out\CON.csv",
        r"C:\out\NUL.csv",
        r"C:\out\COM1.csv",
        r"C:\out\CON .csv",
        r"C:\out\LPT1 .csv",
        r"C:\out\CONIN$",
        r"C:\out\CONOUT$",
        r"C:\out\LPT².csv",
        r"C:\out\file:stream.csv",
        r"C:\out\bom.csv.",
        r"\\?\C:\out\bom.csv",
        r"\\.\C:\out\bom.csv",
        r"C:\out\file?.csv",
        r"C:\out\file*.csv",
        r"C:\out\file|.csv",
        r"C:\out\file<.csv",
    ] {
        assert_eq!(
            publish_bytes(Path::new(path), b"x", false)
                .unwrap_err()
                .stage,
            "preflight",
            "{path}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_readonly_destination_remains_intact_on_replace_failure() {
    let dir = Directory::new();
    fs::write(dir.file(), b"original").unwrap();
    let mut perms = fs::metadata(dir.file()).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(dir.file(), perms).unwrap();
    let result = publish_csv(&dir.file(), b"ours", true);
    let actual = fs::read(dir.file()).unwrap();
    let mut perms = fs::metadata(dir.file()).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(dir.file(), perms).unwrap();
    assert!(result.is_err());
    assert_eq!(actual, b"original");
    assert_eq!(dir.entries(), 1);
}
