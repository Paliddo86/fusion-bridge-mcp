//! Atomically private Windows staging, adapted from the frozen r7 journal policy.
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};
type Result<T> = io::Result<T>;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn unavailable(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}
fn last() -> io::Error {
    io::Error::last_os_error()
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// TOKEN_USER contains an embedded SID pointer. usize storage gives its alignment
// and remains alive until every operation using that SID has finished.
struct User(Vec<usize>);
impl User {
    fn current() -> Result<Self> {
        unsafe {
            let mut thread_token = null_mut();
            if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut thread_token) != 0 {
                let _token = Handle(thread_token);
                return Err(unavailable(
                    "Impersonating threads are unsupported for artifact staging.",
                ));
            }
            if GetLastError() != ERROR_NO_TOKEN {
                return Err(last());
            }
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(last());
            }
            let token = Handle(token);
            let mut needed = 0;
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed);
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER
                || needed < size_of::<TOKEN_USER>() as u32
            {
                return Err(last());
            }
            let mut data = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
            if GetTokenInformation(
                token.0,
                TokenUser,
                data.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) == 0
            {
                return Err(last());
            }
            let value = Self(data);
            if IsValidSid(value.sid()) == 0 {
                return Err(invalid("Process token user SID is invalid."));
            }
            // Rust creates the payload with the process default owner.
            // Reject alternate token owners (common in elevated tokens) rather
            // than changing the token, taking ownership, or repairing sidecars.
            let mut owner_needed = 0;
            GetTokenInformation(token.0, TokenOwner, null_mut(), 0, &mut owner_needed);
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER
                || owner_needed < size_of::<TOKEN_OWNER>() as u32
            {
                return Err(last());
            }
            let mut owner_data = vec![0usize; (owner_needed as usize).div_ceil(size_of::<usize>())];
            if GetTokenInformation(
                token.0,
                TokenOwner,
                owner_data.as_mut_ptr().cast(),
                owner_needed,
                &mut owner_needed,
            ) == 0
            {
                return Err(last());
            }
            let owner = (*(owner_data.as_ptr().cast::<TOKEN_OWNER>())).Owner;
            if owner.is_null() || IsValidSid(owner) == 0 || EqualSid(owner, value.sid()) == 0 {
                return Err(unavailable("The process default object owner must be its current user; elevated alternate-owner tokens are unsupported."));
            }
            Ok(value)
        }
    }
    fn sid(&self) -> PSID {
        unsafe { (*(self.0.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }
    fn text(&self) -> Result<String> {
        let mut string = null_mut();
        unsafe {
            if ConvertSidToStringSidW(self.sid(), &mut string) == 0 {
                return Err(last());
            }
            let owned = Local(string.cast());
            let mut len = 0;
            while *string.add(len) != 0 {
                len += 1;
            }
            let result = String::from_utf16(std::slice::from_raw_parts(string, len))
                .map_err(|_| invalid("Process SID encoding is invalid."));
            drop(owned);
            result
        }
    }
}

fn descriptor(sddl: &str) -> Result<Local> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor = null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(last());
        }
    }
    Ok(Local(descriptor))
}
fn private_descriptor(directory: bool) -> Result<Local> {
    let user = User::current()?.text()?;
    descriptor(&format!(
        "O:{user}D:P(A;{};FA;;;{user})",
        if directory { "OICI" } else { "" }
    ))
}
fn attributes(descriptor: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

pub(super) fn create_private_directory(path: &Path) -> Result<()> {
    let descriptor = private_descriptor(true)?;
    create_directory_with_descriptor(path, &descriptor)?;
    if let Err(e) = verify_private_directory(path) {
        // Only the just-created empty directory can be removed; never repair ACLs.
        return match std::fs::remove_dir(path) {
            Ok(()) => Err(e),
            Err(cleanup) => Err(io::Error::other(format!(
                "{e}; owned empty staging cleanup failed at {}: {cleanup}",
                path.display()
            ))),
        };
    }
    Ok(())
}
fn create_directory_with_descriptor(path: &Path, descriptor: &Local) -> Result<()> {
    let mut name = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if name.contains(&0) {
        return Err(invalid("NUL in staging path"));
    }
    name.push(0);
    let security = attributes(descriptor);
    if unsafe { CreateDirectoryW(name.as_ptr(), &security) } == 0 {
        return Err(last());
    }
    Ok(())
}

pub(super) fn verify_private_directory(path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(last());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(invalid("staging must be a plain directory"));
    }
    let user = User::current()?;
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut raw = null_mut();
    unsafe {
        let status = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut raw,
        );
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let descriptor = Local(raw);
        let mut control = 0;
        let mut revision = 0;
        if IsValidSecurityDescriptor(descriptor.0) == 0
            || GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) == 0
            || owner.is_null()
            || IsValidSid(owner) == 0
            || EqualSid(owner, user.sid()) == 0
            || control & SE_DACL_PRESENT == 0
            || control & SE_DACL_PROTECTED == 0
            || dacl.is_null()
            || IsValidAcl(dacl) == 0
            || (*dacl).AceCount != 1
        {
            return Err(invalid(
                "staging requires a protected current-user-only DACL",
            ));
        }
        let mut pointer = null_mut();
        if GetAce(dacl, 0, &mut pointer) == 0 {
            return Err(last());
        }
        let header = &*pointer.cast::<ACE_HEADER>();
        if header.AceType != 0 || (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(invalid("unsupported staging ACE"));
        }
        let allow = &*pointer.cast::<ACCESS_ALLOWED_ACE>();
        let sid: PSID = (&allow.SidStart as *const u32).cast_mut().cast();
        let sid_bytes = header.AceSize as usize - 8;
        if sid_bytes < 8
            || 8 + 4 * (*sid.cast::<u8>().add(1) as usize) > sid_bytes
            || IsValidSid(sid) == 0
            || EqualSid(sid, user.sid()) == 0
            || header.AceFlags != (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8
            || allow.Mask != FILE_ALL_ACCESS
        {
            return Err(invalid(
                "staging ACE must give only the current user inheritable full control",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn protected_staging_does_not_inherit_a_shared_parent_dacl() {
    let root = super::tests::Directory::new();
    let user = User::current().unwrap().text().unwrap();
    let shared = root.0.join("shared");
    let descriptor =
        descriptor(&format!("O:{user}D:P(A;OICI;FA;;;WD)(A;OICI;FA;;;{user})")).unwrap();
    create_directory_with_descriptor(&shared, &descriptor).unwrap();
    assert!(verify_private_directory(&shared).is_err());
    let mut private = super::Staging::create(&shared).unwrap();
    verify_private_directory(&private.directory).unwrap();
    std::fs::write(&private.file, b"private").unwrap();
    private.cleanup().unwrap();
    let output = shared.join("bom.csv");
    assert!(
        super::publish_csv(&output, b"part,quantity\r\nbolt,4\r\n", false)
            .unwrap()
            .published
    );
    assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 1);
}
