#[cfg(unix)]
use anyhow::Context;
use anyhow::{bail, Result};
use std::path::Path;
pub fn validate_principal(value: &str) -> Result<()> {
    #[cfg(windows)]
    if !value.starts_with("S-1-")
        || !value
            .chars()
            .all(|c| c.is_ascii_digit() || c == 'S' || c == '-')
    {
        bail!("E_ARGUMENT: owner/principal must be a Windows SID");
    }
    #[cfg(unix)]
    {
        value
            .parse::<u32>()
            .context("E_ARGUMENT: owner/principal must be a numeric UID")?;
    }
    Ok(())
}
pub fn current_principal() -> Result<String> {
    #[cfg(unix)]
    {
        Ok(unsafe { libc::geteuid() }.to_string())
    }
    #[cfg(windows)]
    {
        windows::current_sid()
    }
}
pub fn is_system(principal: &str) -> bool {
    #[cfg(windows)]
    {
        principal == "S-1-5-18"
    }
    #[cfg(unix)]
    {
        principal == "0"
    }
}
pub fn protect(path: &Path, is_dir: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if is_dir { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(windows)]
    {
        let owner = current_principal()?;
        let inherit = if is_dir { "OICI" } else { "" };
        windows::set_dacl(
            path,
            &format!("D:P(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)(A;{inherit};FA;;;{owner})"),
        )?;
    }
    Ok(())
}
pub fn replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::rename(from, to)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let a = windows::wide(from.as_os_str());
        let b = windows::wide(to.as_os_str());
        if unsafe {
            MoveFileExW(
                a.as_ptr(),
                b.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
pub fn ensure_absolute(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("E_ARGUMENT: path must be absolute");
    }
    if path.as_os_str().to_string_lossy().contains('\0') {
        bail!("E_ARGUMENT: NUL in path");
    }
    Ok(())
}
#[cfg(windows)]
pub mod windows {
    use super::*;
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SetNamedSecurityInfoW, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
        },
        System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        },
    };
    pub fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    pub fn sid_for_token(token: HANDLE) -> Result<String> {
        let mut len = 0;
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len) };
        let mut bytes = vec![0u8; len as usize];
        if unsafe {
            GetTokenInformation(token, TokenUser, bytes.as_mut_ptr().cast(), len, &mut len)
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let user = unsafe { &*(bytes.as_ptr().cast::<TOKEN_USER>()) };
        let mut out = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut out) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut n = 0;
        unsafe {
            while *out.add(n) != 0 {
                n += 1;
            }
        }
        let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, n) });
        unsafe { LocalFree(out.cast()) };
        Ok(result)
    }
    pub fn current_sid() -> Result<String> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = sid_for_token(token);
        unsafe { CloseHandle(token) };
        result
    }
    pub fn thread_sid() -> Result<String> {
        let mut token = ptr::null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = sid_for_token(token);
        unsafe { CloseHandle(token) };
        result
    }
    pub fn descriptor(sddl: &str) -> Result<*mut std::ffi::c_void> {
        let input = wide(OsStr::new(sddl));
        let mut sd = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                input.as_ptr(),
                1,
                &mut sd,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(sd)
    }
    pub fn set_dacl(path: &Path, sddl: &str) -> Result<()> {
        let sd = descriptor(sddl)?;
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        let ok = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) };
        if ok == 0 {
            unsafe { LocalFree(sd) };
            return Err(std::io::Error::last_os_error().into());
        }
        let name = wide(path.as_os_str());
        let result = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            )
        };
        unsafe { LocalFree(sd) };
        if result != 0 {
            bail!("E_PERMISSION: set ACL failed with Windows error {result}");
        }
        Ok(())
    }
}
