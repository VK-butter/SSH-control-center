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

pub fn handle(method: &str, p: &Value) -> Result<Value> {
    let path = Path::new(protocol::string(p, "path")?);
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
                entries.push(json!({
                    "name":item.file_name().to_string_lossy(),"path":item.path().to_string_lossy(),
                    "isDirectory":md.is_dir(),"isSymlink":item.file_type()?.is_symlink(),"size":md.len()
                }));
            }
            entries.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
            Ok(json!(entries))
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
