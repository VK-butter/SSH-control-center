use crate::{protocol, search, security};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
};
const CHUNK: u64 = 1024 * 1024;
// Serialize the brief file mutation / precondition check so simultaneous Masters
// cannot both satisfy the same expectedSha256 and silently overwrite one another.
// Jobs and project execution remain independent and never hold this lock.
static MUTATIONS: Mutex<()> = Mutex::new(());

/// Home folder of the account running this call (the Owner in owner-session mode).
fn home_dir() -> Option<String> {
    #[cfg(windows)]
    let from_env = std::env::var("USERPROFILE").ok();
    #[cfg(unix)]
    let from_env = std::env::var("HOME").ok();
    if let Some(home) = from_env.filter(|h| !h.is_empty()) {
        return Some(home);
    }
    #[cfg(unix)]
    unsafe {
        // Services often start without HOME; ask the password database instead.
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut out = std::ptr::null_mut();
        let mut buf = vec![0u8; 16384];
        if libc::getpwuid_r(libc::geteuid(), &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut out) == 0
            && !out.is_null()
            && !pwd.pw_dir.is_null()
        {
            return Some(std::ffi::CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned());
        }
    }
    None
}

/// "~" or "~/rest" (also "~\\rest") becomes the home folder; other paths are unchanged.
fn expand_home(raw: &str) -> Result<Option<String>> {
    expand_home_with(raw, home_dir)
}

fn expand_home_with(raw: &str, home: impl FnOnce() -> Option<String>) -> Result<Option<String>> {
    let rest = match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => rest,
        _ => return Ok(None),
    };
    let home = home().context("E_FILE: home folder is unknown for this account")?;
    Ok(Some(format!("{home}{rest}")))
}

pub fn handle(method: &str, p: &Value) -> Result<Value> {
    let raw = protocol::string(p, "path")?;
    let expanded = expand_home(raw)?;
    let path = Path::new(expanded.as_deref().unwrap_or(raw));
    security::ensure_absolute(path)?;
    match method {
        "fs.list" => {
            let mut entries = Vec::new();
            for item in fs::read_dir(path)? {
                if entries.len() >= 10000 {
                    bail!("E_LIMIT: directory has over 10000 entries");
                }
                let item = item?;
                let md = item.metadata()?;
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                entries.push(json!({
                    "name":item.file_name().to_string_lossy(),"path":item.path().to_string_lossy(),
                    "isDirectory":md.is_dir(),"isSymlink":item.file_type()?.is_symlink(),"size":md.len(),
                    "modified":modified
                }));
            }
            entries.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
            // A "~" request also returns the folder it resolved to, so the client can show and climb it.
            Ok(match &expanded {
                Some(resolved) => json!({"path":resolved,"entries":entries}),
                None => json!(entries),
            })
        }
        "fs.read" => {
            let mut file = fs::File::open(path)?;
            read_open_file(&mut file, p)
        }
        "fs.search" => search::search(path, p),
        "fs.grep" => search::grep(path, p),
        "fs.write" => {
            let bytes = STANDARD.decode(protocol::string(p, "contentBase64").or_else(|_| {
                if p["contentBase64"].as_str() == Some("") {
                    Ok("")
                } else {
                    bail!("E_ARGUMENT: contentBase64 required")
                }
            })?)?;
            if bytes.len() > 4 * 1024 * 1024 {
                bail!("E_LIMIT: atomic writes are limited to 4 MiB; use command transfer for larger files");
            }
            let _guard = MUTATIONS
                .lock()
                .map_err(|_| anyhow::anyhow!("E_FILE: mutation lock poisoned"))?;
            let existing = match fs::symlink_metadata(path) {
                Ok(metadata) => {
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        bail!("E_ARGUMENT: write to a regular file, not a symlink");
                    }
                    Some(metadata)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(expected) = p["expectedSha256"].as_str() {
                let actual = if existing.is_some() {
                    hash_file(&mut fs::File::open(path)?)?
                } else {
                    String::new()
                };
                if actual != expected {
                    bail!("E_CONFLICT: file hash changed");
                }
            }
            let parent = path.parent().context("E_ARGUMENT: parent required")?;
            let tmp = parent.join(format!(".sshcc-{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| -> Result<Value> {
                let mut file = private_temp(&tmp)?;
                // Restrict the file before any content reaches disk, including on
                // Windows where the surrounding directory may have a broad DACL.

                file.write_all(&bytes)?;
                file.sync_all()?;
                if let Some(metadata) = &existing {
                    preserve_unix_metadata(&file, metadata)?;
                }
                drop(file);
                commit_file(&tmp, path, existing.is_some())?;
                Ok(
                    json!({"path":path.to_string_lossy(),"size":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}),
                )
            })();
            let _ = fs::remove_file(&tmp);
            result
        }
        "fs.upload" => upload(path, p),
        "fs.mkdir" => {
            let _guard = MUTATIONS
                .lock()
                .map_err(|_| anyhow::anyhow!("E_FILE: mutation lock poisoned"))?;
            fs::create_dir_all(path)?;
            Ok(json!({"path":path.to_string_lossy()}))
        }
        "fs.remove" => {
            let _guard = MUTATIONS
                .lock()
                .map_err(|_| anyhow::anyhow!("E_FILE: mutation lock poisoned"))?;
            let md = fs::symlink_metadata(path)?;
            if md.is_dir() && !md.file_type().is_symlink() {
                fs::remove_dir(path)?;
            } else {
                fs::remove_file(path)?;
            }
            Ok(json!({"removed":true}))
        }
        _ => bail!("E_METHOD: unknown filesystem method"),
    }
}
/// Upper bound for one chunked upload, so a runaway client cannot fill the disk.
const UPLOAD_MAX: u64 = 2 * 1024 * 1024 * 1024;

/// Chunked upload: chunks are appended in order to a private `.sshcc-upload-<id>.part`
/// next to the destination, which replaces the destination only on `done`, with the
/// same expectedSha256 guard and atomic commit as fs.write. `abort` deletes the part.
fn upload(path: &Path, p: &Value) -> Result<Value> {
    let id = protocol::string(p, "uploadId")?;
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("E_ARGUMENT: uploadId must be 32 hex characters");
    }
    let parent = path.parent().context("E_ARGUMENT: parent required")?;
    let part = parent.join(format!(".sshcc-upload-{id}.part"));
    if p["abort"].as_bool() == Some(true) {
        match fs::remove_file(&part) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(json!({"aborted":true}));
    }
    let offset = p["offset"].as_u64().unwrap_or(0);
    let bytes = STANDARD.decode(p["contentBase64"].as_str().unwrap_or_default())?;
    if bytes.len() > 4 * 1024 * 1024 {
        bail!("E_LIMIT: an upload chunk is limited to 4 MiB");
    }
    if offset + bytes.len() as u64 > UPLOAD_MAX {
        bail!("E_LIMIT: uploads are limited to 2 GiB");
    }
    let mut file = if offset == 0 {
        sweep_stale_parts(parent);
        private_temp(&part)?
    } else {
        let metadata = fs::symlink_metadata(&part).context("E_CONFLICT: upload not started or already finished")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("E_ARGUMENT: upload part is not a regular file");
        }
        if metadata.len() != offset {
            bail!("E_CONFLICT: upload offset {offset} does not match {} bytes received", metadata.len());
        }
        let mut options = fs::OpenOptions::new();
        options.append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        options.open(&part)?
    };
    let result = (|| -> Result<Value> {
        file.write_all(&bytes)?;
        let received = offset + bytes.len() as u64;
        if p["done"].as_bool() != Some(true) {
            return Ok(json!({"received":received}));
        }
        file.sync_all()?;
        let _guard = MUTATIONS
            .lock()
            .map_err(|_| anyhow::anyhow!("E_FILE: mutation lock poisoned"))?;
        let existing = match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    bail!("E_ARGUMENT: write to a regular file, not a symlink");
                }
                Some(metadata)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(expected) = p["expectedSha256"].as_str() {
            let actual = if existing.is_some() { hash_file(&mut fs::File::open(path)?)? } else { String::new() };
            if actual != expected {
                bail!("E_CONFLICT: file hash changed");
            }
        }
        if let Some(metadata) = &existing {
            preserve_unix_metadata(&file, metadata)?;
        }
        drop(file);
        let digest = hash_file(&mut fs::File::open(&part)?)?;
        commit_file(&part, path, existing.is_some())?;
        Ok(json!({"path":path.to_string_lossy(),"size":received,"sha256":digest}))
    })();
    // A failed commit (conflict) keeps the part so the client can retry the last
    // chunk with another guard; it deletes it with `abort`. Other errors drop it.
    if let Err(error) = &result {
        if !error.to_string().contains("E_CONFLICT") {
            let _ = fs::remove_file(&part);
        }
    }
    result
}
/// Parts left by an app that closed mid-upload are removed after a day, when the
/// next upload into the same folder starts.
fn sweep_stale_parts(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(".sshcc-upload-") && name.ends_with(".part")) {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 24 * 3600);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}
fn read_open_file(file: &mut fs::File, p: &Value) -> Result<Value> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("E_ARGUMENT: regular file required");
    }
    let size = metadata.len();
    let offset = p["offset"].as_u64().unwrap_or(0);
    let limit = p["limit"].as_u64().unwrap_or(CHUNK).min(CHUNK);
    // Hash and content use one open inode/file handle. A concurrent atomic rename
    // must not produce old bytes paired with the replacement file's hash.
    let digest = hash_file(file)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    Read::by_ref(file).take(limit).read_to_end(&mut bytes)?;
    Ok(json!({
        "contentBase64":STANDARD.encode(&bytes),"size":size,"sha256":digest,
        "nextOffset":offset+bytes.len() as u64,"eof":offset+bytes.len() as u64>=size
    }))
}
fn hash_file(file: &mut fs::File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hasher.update(&bytes[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
#[cfg(unix)]
fn private_temp(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
#[cfg(windows)]
fn private_temp(path: &Path) -> Result<fs::File> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL},
    };
    let owner = security::current_principal()?;
    let descriptor = security::windows::descriptor(&format!("D:P(A;;FA;;;SY)(A;;FA;;;{owner})"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = security::windows::wide(path.as_os_str());
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    let error = std::io::Error::last_os_error();
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error.into());
    }
    Ok(unsafe { fs::File::from_raw_handle(handle) })
}
#[cfg(unix)]
fn preserve_unix_metadata(file: &fs::File, metadata: &fs::Metadata) -> Result<()> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    // chown can clear setuid/setgid; restore the existing mode only afterwards.
    if unsafe { libc::fchown(file.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("E_FILE: cannot preserve file ownership");
    }
    file.set_permissions(metadata.permissions())?;
    file.sync_all()?;
    Ok(())
}
#[cfg(not(unix))]
fn preserve_unix_metadata(_file: &fs::File, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn commit_file(temp: &Path, path: &Path, existing: bool) -> Result<()> {
    if existing {
        fs::rename(temp, path)?;
    } else {
        // Link fails if another writer created the path; it never overwrites a
        // newly created file that was absent at the precondition check.
        fs::hard_link(temp, path)?;
        fs::remove_file(temp)?;
    }
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
#[cfg(windows)]
fn commit_file(temp: &Path, path: &Path, existing: bool) -> Result<()> {
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH,
    };
    let destination = security::windows::wide(path.as_os_str());
    let replacement = security::windows::wide(temp.as_os_str());
    if !existing {
        // No REPLACE_EXISTING: do not overwrite a file created after our check.
        if unsafe {
            MoveFileExW(
                replacement.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("E_FILE: atomic file creation failed");
        }
        return Ok(());
    }
    let backup = path
        .parent()
        .context("E_FILE: missing parent")?
        .join(format!(".sshcc-backup-{}.tmp", uuid::Uuid::new_v4()));
    let backup_name = security::windows::wide(backup.as_os_str());
    // ReplaceFileW preserves the original DACL, creation attributes and alternate
    // streams. Never request IGNORE_ACL_ERRORS / IGNORE_MERGE_ERRORS.
    if unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            replacement.as_ptr(),
            backup_name.as_ptr(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        if backup.exists() && !path.exists() {
            security::replace(&backup, path)
                .context("E_FILE: replacement failed and original requires recovery from backup")?;
        } else if backup.exists() {
            bail!(
                "E_FILE: replacement failed ({error}); original backup retained at {}",
                backup.display()
            );
        }
        return Err(error).context("E_FILE: replacement failed; original preserved");
    }
    fs::remove_file(&backup)
        .context("E_FILE: file replaced but old backup could not be removed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(path: &Path, id: &str, offset: u64, data: &[u8], extra: Value) -> Result<Value> {
        let mut params = json!({"path":path.to_string_lossy(),"uploadId":id,"offset":offset,"contentBase64":STANDARD.encode(data)});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        handle("fs.upload", &params)
    }
    #[test]
    fn chunked_upload_appears_only_when_done() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("big.bin");
        let id = "0123456789abcdef0123456789abcdef";
        chunk(&path, id, 0, b"hello ", json!({})).unwrap();
        assert!(!path.exists());
        assert!(chunk(&path, id, 3, b"x", json!({})).unwrap_err().to_string().contains("E_CONFLICT"));
        let done = chunk(&path, id, 6, b"world", json!({"done":true,"expectedSha256":""})).unwrap();
        assert_eq!(done["size"], 11);
        assert_eq!(fs::read(&path).unwrap(), b"hello world");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn chunked_upload_guard_keeps_part_until_replace_or_abort() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("taken.txt");
        fs::write(&path, b"old").unwrap();
        let id = "fedcba9876543210fedcba9876543210";
        let error = chunk(&path, id, 0, b"new", json!({"done":true,"expectedSha256":""})).unwrap_err();
        assert!(error.to_string().contains("E_CONFLICT"));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        chunk(&path, id, 3, b"", json!({"done":true})).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let other = "00000000000000000000000000000001";
        chunk(&path, other, 0, b"partial", json!({})).unwrap();
        chunk(&path, other, 0, b"", json!({"abort":true})).unwrap();
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(chunk(&path, "../../etc", 0, b"", json!({})).unwrap_err().to_string().contains("uploadId"));
    }
    #[test]
    fn opened_file_read_retains_matching_bytes_and_hash_after_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.txt");
        fs::write(&path, b"original").unwrap();
        let mut original = fs::File::open(&path).unwrap();
        handle(
            "fs.write",
            &json!({"path":path,"contentBase64":STANDARD.encode(b"replacement")}),
        )
        .unwrap();
        let result = read_open_file(&mut original, &json!({})).unwrap();
        assert_eq!(
            STANDARD
                .decode(result["contentBase64"].as_str().unwrap())
                .unwrap(),
            b"original"
        );
        assert_eq!(result["sha256"], hex::encode(Sha256::digest(b"original")));
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }
    #[test]
    fn simultaneous_hash_guarded_writes_only_have_one_winner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conflict.txt");
        fs::write(&path, b"original").unwrap();
        let expected = hex::encode(Sha256::digest(b"original"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers:Vec<_> = [b"first".as_slice(),b"second".as_slice()].into_iter().map(|content| {
            let path=path.clone();let expected=expected.clone();let barrier=barrier.clone();
            std::thread::spawn(move|| {
                barrier.wait();
                handle("fs.write",&json!({"path":path,"contentBase64":STANDARD.encode(content),"expectedSha256":expected})).is_ok()
            })
        }).collect();
        barrier.wait();
        assert_eq!(
            workers
                .into_iter()
                .filter_map(|worker| worker.join().ok())
                .filter(|success| *success)
                .count(),
            1
        );
    }
    #[cfg(unix)]
    #[test]
    fn replacing_file_preserves_owner_group_and_mode() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.txt");
        fs::write(&path, b"original").unwrap();
        // Container acceptance runs as root; exercise changing an actual distinct
        // owner/group in that environment. Unprivileged development keeps own UID.
        if unsafe { libc::geteuid() } == 0 {
            let file = fs::File::open(&path).unwrap();
            assert_eq!(unsafe { libc::fchown(file.as_raw_fd(), 1100, 1100) }, 0);
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&path).unwrap();
        handle(
            "fs.write",
            &json!({"path":path,"contentBase64":STANDARD.encode(b"replacement")}),
        )
        .unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (after.uid(), after.gid(), after.mode()),
            (before.uid(), before.gid(), before.mode())
        );
    }
    #[cfg(windows)]
    #[test]
    fn replacing_file_preserves_existing_dacl() {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                    SE_FILE_OBJECT,
                },
                DACL_SECURITY_INFORMATION,
            },
        };
        fn dacl(path: &Path) -> String {
            let name = security::windows::wide(path.as_os_str());
            let mut descriptor = std::ptr::null_mut();
            let code = unsafe {
                GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut descriptor,
                )
            };
            assert_eq!(code, 0);
            let mut text = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    ConvertSecurityDescriptorToStringSecurityDescriptorW(
                        descriptor,
                        1,
                        DACL_SECURITY_INFORMATION,
                        &mut text,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let mut len = 0;
            unsafe {
                while *text.add(len) != 0 {
                    len += 1;
                }
            }
            let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
            unsafe {
                LocalFree(text.cast());
                LocalFree(descriptor);
            }
            value
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("restricted.txt");
        fs::write(&path, b"original").unwrap();
        let owner = security::current_principal().unwrap();
        security::windows::set_dacl(&path, &format!("D:P(A;;FA;;;{owner})(A;;FR;;;SY)")).unwrap();
        let before = dacl(&path);
        handle(
            "fs.write",
            &json!({"path":path,"contentBase64":STANDARD.encode(b"replacement")}),
        )
        .unwrap();
        assert_eq!(dacl(&path), before);
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    #[test]
    fn tilde_expands_only_as_a_whole_prefix() {
        let home = || Some("/home/owner".to_string());
        assert_eq!(expand_home_with("~", home).unwrap().as_deref(), Some("/home/owner"));
        assert_eq!(expand_home_with("~/src", home).unwrap().as_deref(), Some("/home/owner/src"));
        assert_eq!(expand_home_with("~bob/x", home).unwrap(), None);
        assert_eq!(expand_home_with("/etc", home).unwrap(), None);
        assert!(expand_home_with("~", || None).is_err());
    }
}
