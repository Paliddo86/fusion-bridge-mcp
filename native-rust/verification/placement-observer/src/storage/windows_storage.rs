//! Windows isolation: inspect metadata/security without reading file contents,
//! create protected private objects atomically, and never repair existing ACLs.
use super::{error, io_at, storage_policy, Error, FileIdentity, Result};
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::mem::{size_of, zeroed};
use std::os::windows::{
    ffi::OsStrExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::path::{Component, Path, Prefix};
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        SystemServices::{FILE_PERSISTENT_ACLS, FILE_SUPPORTS_HARD_LINKS},
        Threading::*,
        WindowsProgramming::DRIVE_FIXED,
    },
};

fn invalid(message: &str) -> Error {
    error("operation_storage_invalid_path", message)
}
fn unavailable(message: &str) -> Error {
    error("operation_storage_unavailable", message)
}
fn last() -> Error {
    std::io::Error::last_os_error().into()
}
fn wide(path: &Path) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(invalid("Storage path contains NUL."));
    }
    value.push(0);
    Ok(value)
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
                    "Impersonating threads are unsupported for journal storage.",
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
            // SQLite creates rollback sidecars with the process default owner.
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

pub(super) fn validate_path(path: &Path) -> Result<()> {
    if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        return Err(invalid("Windows journal requires an absolute local drive path; UNC and device paths are unsupported."));
    }
    for part in path.components() {
        if let Component::Normal(value) = part {
            if value
                .encode_wide()
                .any(|value| value == b':' as u16 || value == 0)
            {
                return Err(invalid(
                    "Alternate data streams and NUL are unsupported in journal paths.",
                ));
            }
        }
    }
    Ok(())
}

fn metadata_handle(path: &Path) -> Result<File> {
    io_at(
        OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path),
        "storage.windows.metadata_handle",
        path,
    )
}

fn filesystem(handle: HANDLE) -> Result<()> {
    let mut name = [0u16; 64];
    let mut flags = 0;
    unsafe {
        if GetVolumeInformationByHandleW(
            handle,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            name.as_mut_ptr(),
            name.len() as u32,
        ) == 0
        {
            return Err(last());
        }
        // A GUID volume path is a local-volume identity. SMB and mapped network
        // shares cannot satisfy this check; no path-based drive inference.
        let mut final_path = vec![0u16; 32768];
        let count = GetFinalPathNameByHandleW(
            handle,
            final_path.as_mut_ptr(),
            final_path.len() as u32,
            VOLUME_NAME_GUID,
        );
        if count == 0 {
            return Err(last());
        }
        if count as usize >= final_path.len() {
            return Err(unavailable("Journal requires a local fixed NTFS volume."));
        }
        let final_path = String::from_utf16(&final_path[..count as usize])
            .map_err(|_| unavailable("Volume path encoding is unsupported."))?;
        let root_end = final_path
            .find("}\\")
            .map(|i| i + 2)
            .ok_or_else(|| unavailable("Journal requires a local volume GUID."))?;
        if !final_path.starts_with("\\\\?\\Volume{") {
            return Err(unavailable("Journal requires a local volume GUID."));
        }
        let root: Vec<u16> = final_path[..root_end].encode_utf16().chain([0]).collect();
        let fs_name =
            String::from_utf16(&name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())])
                .map_err(|_| unavailable("Filesystem encoding is unsupported."))?;
        if !storage_policy::supported_filesystem(
            &fs_name,
            GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED,
            flags & FILE_PERSISTENT_ACLS != 0,
            flags & FILE_SUPPORTS_HARD_LINKS != 0,
        ) {
            return Err(unavailable("Only local fixed NTFS volumes with persistent ACLs and hardlinks are qualified; ReFS and remote/removable storage are unsupported."));
        }
    }
    Ok(())
}

fn acl(handle: HANDLE, directory: bool) -> Result<()> {
    let user = User::current()?;
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut raw_descriptor = null_mut();
    unsafe {
        let status = GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut raw_descriptor,
        );
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        let descriptor = Local(raw_descriptor);
        validate_descriptor(&descriptor, owner, dacl, &user, directory)
    }
}

fn validate_descriptor(
    descriptor: &Local,
    owner: PSID,
    dacl: *mut ACL,
    user: &User,
    directory: bool,
) -> Result<()> {
    unsafe {
        let mut control = 0;
        let mut revision = 0;
        if IsValidSecurityDescriptor(descriptor.0) == 0
            || GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) == 0
            || owner.is_null()
            || IsValidSid(owner) == 0
            || control & SE_DACL_PRESENT == 0
            || dacl.is_null()
            || IsValidAcl(dacl) == 0
        {
            return Err(invalid(
                "Journal ownership and a non-null valid DACL are required.",
            ));
        }
        let mut aces = Vec::new();
        for i in 0..(*dacl).AceCount as u32 {
            let mut pointer = null_mut();
            if GetAce(dacl, i, &mut pointer) == 0 {
                return Err(invalid("Journal DACL ACE cannot be inspected."));
            }
            let header = &*pointer.cast::<ACE_HEADER>();
            // Only the simple allow layout is read; all variable/conditional
            // layouts and deny semantics fail closed before dereferencing SID.
            if header.AceType != 0 || (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>() {
                return Err(invalid("Journal DACL contains an unsupported ACE."));
            }
            let allowed = &*pointer.cast::<ACCESS_ALLOWED_ACE>();
            let sid: PSID = (&allowed.SidStart as *const u32).cast_mut().cast();
            // IsValidAcl validates ACE layout but bound SID explicitly before
            // invoking SID APIs (8-byte header followed by subauthorities).
            let sid_bytes = header.AceSize as usize - 8;
            if sid_bytes < 8
                || 8 + 4 * (*sid.cast::<u8>().add(1) as usize) > sid_bytes
                || IsValidSid(sid) == 0
            {
                return Err(invalid("Journal DACL contains an invalid SID."));
            }
            let principal = if EqualSid(sid, user.sid()) != 0 {
                storage_policy::Principal::User
            } else if IsWellKnownSid(sid, WinLocalSystemSid) != 0 {
                storage_policy::Principal::System
            } else if IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0 {
                storage_policy::Principal::Administrators
            } else {
                storage_policy::Principal::Other
            };
            aces.push(storage_policy::Ace {
                principal,
                allow: true,
                flags: header.AceFlags,
                mask: allowed.Mask,
            });
        }
        if !storage_policy::private_acl(
            EqualSid(owner, user.sid()) != 0,
            control & SE_DACL_PROTECTED != 0,
            directory,
            Some(&aces),
        ) {
            return Err(invalid("Journal storage must be current-user owned, private to that user and trusted SYSTEM/Administrators, with full user access; the leaf requires a protected inheritable DACL."));
        }
    }
    Ok(())
}

pub(super) fn inspect(file: &File, directory: bool) -> Result<FileIdentity> {
    let handle = file.as_raw_handle();
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(last());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(invalid(
            "Journal object changed type or is a reparse point.",
        ));
    }
    filesystem(handle)?;
    acl(handle, directory)?;
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber as u64,
        index: (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
        links: info.nNumberOfLinks as u64,
    })
}

pub(super) fn validate_directory(path: &Path) -> Result<()> {
    inspect(&metadata_handle(path)?, true)
        .map(|_| ())
        .map_err(|failure| failure.at_path("storage.windows.private_directory", path))
}
pub(super) fn identity(path: &Path) -> Result<FileIdentity> {
    inspect(&metadata_handle(path)?, false)
        .map_err(|failure| failure.at_path("storage.windows.private_file", path))
}
#[allow(dead_code)] // Preserved qualified helper; utility uses stricter create-new below.
pub(super) fn create_directories(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    for ancestor in path.ancestors() {
        if super::path_exists(ancestor)? {
            filesystem(metadata_handle(ancestor)?.as_raw_handle())?;
            break;
        }
        missing.push(ancestor);
    }
    let descriptor = private_descriptor(true)?;
    let security = attributes(&descriptor);
    for directory in missing.into_iter().rev() {
        let name = wide(directory)?;
        if unsafe { CreateDirectoryW(name.as_ptr(), &security) } == 0 {
            let failure = std::io::Error::last_os_error();
            if failure.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(failure.into());
            }
        }
        // Concurrent creation is acceptable only if the winner is private too.
        validate_directory(directory)?;
    }
    Ok(())
}
// Utility delta: output roots must not accept a concurrent pre-existing winner.
pub(super) fn create_directory_new(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Private output parent missing."))?;
    filesystem(metadata_handle(parent)?.as_raw_handle())?;
    let descriptor = private_descriptor(true)?;
    let security = attributes(&descriptor);
    let name = wide(path)?;
    if unsafe { CreateDirectoryW(name.as_ptr(), &security) } == 0 {
        return Err(last());
    }
    validate_directory(path)
}
pub(super) fn create_file(path: &Path) -> Result<File> {
    let descriptor = private_descriptor(false)?;
    let security = attributes(&descriptor);
    let name = wide(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(last());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    inspect(&file, false)?;
    Ok(file)
}
