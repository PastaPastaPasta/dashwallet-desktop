use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::{MaybeUninit, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{BOOL, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION,
    EqualSid, GetAce, GetSecurityDescriptorControl, GetTokenInformation, INHERITED_ACE,
    InitializeAcl, InitializeSecurityDescriptor, OWNER_SECURITY_INFORMATION, PSID,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SECURITY_MAX_SID_SIZE,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn checked(ok: BOOL) -> io::Result<()> {
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn with_user_sid<T>(f: impl FnOnce(PSID) -> io::Result<T>) -> io::Result<T> {
    let mut token = null_mut();
    // SAFETY: The output is writable and the returned token is owned by this call.
    unsafe {
        checked(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        ))?
    };
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    const USER_BYTES: usize = size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize;
    let mut user = [0usize; USER_BYTES.div_ceil(size_of::<usize>())];
    let mut written = 0;
    // SAFETY: The aligned buffer is large enough for TOKEN_USER and its SID.
    // The SID points into this buffer, which lives through the callback.
    unsafe {
        checked(GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            size_of_val(&user) as u32,
            &mut written,
        ))?;
        f((*user.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}

pub(super) fn create(path: &Path) -> io::Result<File> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let part = std::fs::canonicalize(parent)?.join(name);
    let mut wide: Vec<_> = part.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in file name",
        ));
    }
    wide.push(0);

    with_user_sid(|sid| {
        const ACL_BYTES: usize = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>()
            - size_of::<u32>()
            + SECURITY_MAX_SID_SIZE as usize;
        let mut acl = [0u32; ACL_BYTES.div_ceil(size_of::<u32>())];
        let acl_len = size_of_val(&acl) as u32;
        let acl = acl.as_mut_ptr().cast();
        let mut descriptor = MaybeUninit::<SECURITY_DESCRIPTOR>::uninit();
        let descriptor = descriptor.as_mut_ptr().cast::<c_void>();
        // SAFETY: The ACL, SID and descriptor live until CreateFileW returns.
        // Each initialization is checked before passing the object on.
        let file = unsafe {
            checked(InitializeAcl(acl, acl_len, ACL_REVISION))?;
            checked(AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid))?;
            checked(InitializeSecurityDescriptor(
                descriptor,
                SECURITY_DESCRIPTOR_REVISION,
            ))?;
            checked(SetSecurityDescriptorOwner(descriptor, sid, 0))?;
            checked(SetSecurityDescriptorDacl(descriptor, 1, acl, 0))?;
            checked(SetSecurityDescriptorControl(
                descriptor,
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            ))?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let file = CreateFileW(
                wide.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );
            if file == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            File::from_raw_handle(file)
        };
        if let Err(error) = verify(&file, sid) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
        Ok(file)
    })
}

struct LocalDescriptor(*mut c_void);

impl Drop for LocalDescriptor {
    fn drop(&mut self) {
        // SAFETY: GetSecurityInfo returns a LocalAlloc allocation, or NULL.
        unsafe { LocalFree(self.0) };
    }
}

fn verify(file: &File, sid: PSID) -> io::Result<()> {
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the destination did not retain the owner-only file permissions",
        )
    };
    let (mut owner, mut dacl) = (null_mut(), null_mut());
    let mut descriptor = LocalDescriptor(null_mut());
    // SAFETY: The handle and token SID are live. The output descriptor owns
    // the returned SID and ACL and stays live until all checks have finished.
    unsafe {
        let status = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor.0,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        if descriptor.0.is_null() || owner.is_null() || dacl.is_null() {
            return Err(denied());
        }
        let (mut control, mut revision) = (0, 0);
        checked(GetSecurityDescriptorControl(
            descriptor.0,
            &mut control,
            &mut revision,
        ))?;
        if control & SE_DACL_PROTECTED == 0 || EqualSid(owner, sid) == 0 || (*dacl).AceCount != 1 {
            return Err(denied());
        }
        let mut ace = null_mut();
        checked(GetAce(dacl, 0, &mut ace))?;
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        if u32::from((*ace).Header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || u32::from((*ace).Header.AceFlags) & INHERITED_ACE != 0
            || (*ace).Mask != FILE_ALL_ACCESS
            || EqualSid((&raw mut (*ace).SidStart).cast(), sid) == 0
        {
            return Err(denied());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn renamed_export_has_only_the_users_explicit_access() {
        let dir = dw_testutil::private_tempdir();
        let part = dir.path().join("export.part");
        let output = dir.path().join("export.dat");
        let mut file = crate::create_new_owner_only(&part).unwrap();
        file.write_all(b"fixture").unwrap();
        file.sync_all().unwrap();
        drop(file);
        std::fs::rename(part, &output).unwrap();
        let file = File::open(output).unwrap();
        with_user_sid(|sid| verify(&file, sid)).unwrap();
    }
}
