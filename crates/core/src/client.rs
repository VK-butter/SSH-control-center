use crate::{
    ipc,
    protocol::{read_frame, write_frame, Request, Response},
};
use anyhow::Result;
use std::io::BufReader;
static CHANNEL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
/// Names this program in every request it sends (`desktop`, `cli`, `mcp`), for the Master's audit log. A label, not
/// a check.
pub fn set_channel(name: &'static str) {
    let _ = CHANNEL.set(name);
}
pub fn call(request: &Request) -> Result<serde_json::Value> {
    let mut connection = ipc::connect()?;
    let mut frame = serde_json::to_value(request)?;
    if let Some(channel) = CHANNEL.get() {
        frame["channel"] = serde_json::json!(channel);
    }
    write_frame(&frame, &mut connection)?;
    let response: Response = read_frame(&mut BufReader::new(connection))?;
    response.into_result()
}

/// Copies a file from a target host to `dest` in 1 MiB `files.read` chunks.
/// Bytes go to a sibling temp file that is renamed only when the whole file
/// arrived with one unchanged SHA-256, so a file edited mid-download is refused.
pub fn download(target_id: &str, path: &str, dest: &std::path::Path) -> Result<u64> {
    download_with(dest, |offset| {
        call(&Request {
            method: "files.read".into(),
            // The first chunk marks it as a download, so the audit log records it once.
            params: serde_json::json!({"targetId": target_id, "path": path, "offset": offset, "download": offset == 0}),
        })
    })
}

fn download_with(
    dest: &std::path::Path,
    mut read: impl FnMut(u64) -> Result<serde_json::Value>,
) -> Result<u64> {
    use anyhow::{bail, Context};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::io::Write;
    let name = dest.file_name().context("destination has no file name")?;
    let temp = dest.with_file_name(format!(".{}.sshcc-part", name.to_string_lossy()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        let (mut offset, mut digest) = (0u64, None::<String>);
        loop {
            let chunk = read(offset)?;
            let sha = chunk["sha256"].as_str().unwrap_or_default().to_string();
            if digest.get_or_insert_with(|| sha.clone()) != &sha {
                bail!("E_CONFLICT: the file changed during the download; try again");
            }
            let bytes = STANDARD.decode(chunk["contentBase64"].as_str().unwrap_or_default())?;
            file.write_all(&bytes)?;
            offset += bytes.len() as u64;
            if chunk["eof"].as_bool().unwrap_or(true) {
                break;
            }
            if bytes.is_empty() {
                bail!("E_IO: the host stopped sending data before the end of the file");
            }
        }
        file.sync_all()?;
        Ok(offset)
    })();
    match result {
        Ok(size) => {
            std::fs::rename(&temp, dest)?;
            Ok(size)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod download_tests {
    use super::download_with;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::json;

    fn chunk(data: &[u8], offset: u64, sha: &str, total: u64) -> serde_json::Value {
        let next = offset + data.len() as u64;
        json!({"contentBase64": STANDARD.encode(data), "sha256": sha, "eof": next >= total})
    }

    #[test]
    fn joins_chunks_and_renames_at_the_end() {
        let dir = std::env::temp_dir().join(format!("sshcc-dl-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");
        let size = download_with(&dest, |offset| {
            Ok(if offset == 0 { chunk(b"hello ", 0, "a", 11) } else { chunk(b"world", offset, "a", 11) })
        })
        .unwrap();
        assert_eq!(size, 11);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_a_file_that_changes_and_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("sshcc-dl-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");
        let error = download_with(&dest, |offset| {
            Ok(if offset == 0 { chunk(b"hello ", 0, "a", 11) } else { chunk(b"world", offset, "b", 11) })
        })
        .unwrap_err();
        assert!(error.to_string().contains("E_CONFLICT"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
