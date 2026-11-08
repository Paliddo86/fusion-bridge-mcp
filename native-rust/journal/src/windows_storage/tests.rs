use super::*;
use crate::{Journal, Limits, DATABASE_NAME};
use std::fs;

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "fusion-private-acl-{}-{}-{}",
            std::process::id(),
            crate::now_ms().unwrap(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn check_sddl(sddl: &str, directory: bool) -> Result<()> {
    let descriptor = descriptor(sddl)?;
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut defaulted = 0;
    let mut present = 0;
    unsafe {
        assert_ne!(
            GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted),
            0
        );
        assert_ne!(
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted),
            0
        );
    }
    validate_descriptor(&descriptor, owner, dacl, &User::current()?, directory)
}

#[test]
fn windows_descriptor_parser_accepts_only_private_current_user_grammar() {
    let sid = User::current().unwrap().text().unwrap();
    for tail in [
        format!("D:P(A;OICI;FA;;;{sid})"),
        format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
    ] {
        check_sddl(&format!("O:{sid}{tail}"), true).unwrap();
    }
    check_sddl(&format!("O:{sid}D:(A;ID;FA;;;{sid})"), false).unwrap();
    for text in [
        format!("O:{sid}"),
        format!("O:{sid}D:NO_ACCESS_CONTROL"),
        format!("O:{sid}D:P"),
        format!("O:WDD:P(A;OICI;FA;;;{sid})"),
        format!("O:{sid}D:(A;OICI;FA;;;{sid})"),
        format!("O:{sid}D:P(A;;FA;;;{sid})"),
        format!("O:{sid}D:P(A;OICIIO;FA;;;{sid})"),
        format!("O:{sid}D:P(A;OICINP;FA;;;{sid})"),
        format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;;FR;;;WD)"),
        format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;CO)"),
        format!("O:{sid}D:P(D;;FW;;;WD)(A;OICI;FA;;;{sid})"),
        format!("O:{sid}D:P(OA;;FR;00000000-0000-0000-0000-000000000001;;WD)(A;OICI;FA;;;{sid})"),
    ] {
        let failure = check_sddl(&text, true).unwrap_err();
        assert_eq!(
            failure.code, "operation_storage_invalid_path",
            "{text}: {failure}"
        );
    }
}

#[test]
fn windows_private_creation_and_inherited_files_are_valid_before_journal_open() {
    let temporary = Temp::new();
    let directory = temporary.0.join("nested").join("journal");
    crate::validate_directory(&directory, true).unwrap();
    validate_directory(&temporary.0).unwrap();
    validate_directory(&directory).unwrap();
    let file = directory.join("private.tmp");
    drop(create_file(&file).unwrap());
    identity(&file).unwrap();
    let inherited = directory.join("inherited.tmp");
    fs::write(&inherited, b"unchanged").unwrap();
    identity(&inherited).unwrap();
    let journal = Journal::open(&directory, "session", "test", Limits::default()).unwrap();
    identity(journal.database_path()).unwrap();
    assert_eq!(journal.database_path(), directory.join(DATABASE_NAME));
}

#[test]
fn windows_rejects_remote_device_and_alternate_stream_paths() {
    for text in [
        r"\\server\share\journal",
        r"\\?\UNC\server\share\journal",
        r"\\.\C:\journal",
        r"C:\journal:stream",
        r"C:\a:stream\journal",
    ] {
        assert!(validate_path(Path::new(text)).is_err(), "{text}");
    }
    for text in [r"C:\journal", r"\\?\C:\journal"] {
        validate_path(Path::new(text)).unwrap();
    }
}
