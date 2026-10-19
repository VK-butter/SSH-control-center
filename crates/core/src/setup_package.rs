//! Create operator-owned setup artifacts from installed, verified release templates.
//! No shell, build tool, network service or private key is involved.
use anyhow::{bail, ensure, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const WINDOWS_MAGIC: &[u8; 16] = b"SSHCCPAIRING0001";
const LIMIT: usize = 256 * 1024 * 1024;
const ENVELOPE_PATH: &str = "usr/lib/sshcc-setup/assets/envelope.json";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    templates: Vec<Template>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Template {
    os: String,
    file_name: String,
    sha256: String,
}

pub fn default_templates() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("templates")))
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\SSH Control Center\templates"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/lib/sshcc/templates")
    }
}

fn bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "SETUP_TOO_LARGE: release asset exceeds limit"
    );
    Ok(bytes)
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn build(envelope: &Value, templates: &Path, output: &Path) -> Result<Value> {
    let checked = crate::pairing::inspect(envelope)?;
    let envelope = &checked["envelope"];
    let os = envelope["bundle"]["os"]
        .as_str()
        .context("missing target OS")?;
    let expected = match os {
        "windows" => "sshcc-client-windows-template.exe",
        "linux" => "sshcc-client-ubuntu-template.deb",
        _ => bail!("unsupported target OS"),
    };
    let manifest: Manifest = serde_json::from_slice(
        &bounded(&templates.join("template-manifest.json"), 65536)
            .context("SETUP_TEMPLATES_MISSING: reinstall the complete Master package")?,
    )?;
    ensure!(
        manifest.schema_version == 1,
        "unsupported release template manifest"
    );
    let entry = manifest
        .templates
        .iter()
        .find(|t| t.os == os)
        .context("SETUP_TEMPLATES_MISSING: no release template for this OS")?;
    ensure!(
        entry.file_name == expected,
        "invalid release template filename"
    );
    let template_path = templates.join(expected);
    let template = bounded(&template_path, LIMIT)?;
    ensure!(
        digest(&template) == entry.sha256.to_ascii_lowercase(),
        "SETUP_TEMPLATE_MISMATCH: release template checksum differs"
    );
    let payload = serde_json::to_vec(envelope)?;
    ensure!(payload.len() <= 65536, "pairing envelope exceeds 64 KiB");

    // Do not let the operator accidentally destroy a release template.
    if output.exists() {
        let chosen = output.canonicalize()?;
        for protected in [
            "template-manifest.json",
            "sshcc-client-windows-template.exe",
            "sshcc-client-ubuntu-template.deb",
        ] {
            let path = templates.join(protected);
            if path.exists() {
                ensure!(
                    chosen != path.canonicalize()?,
                    "cannot overwrite release templates or their manifest"
                );
            }
        }
        ensure!(output.is_file(), "output must be a regular file");
    }
    let bytes = if os == "windows" {
        let mut bytes = template;
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(WINDOWS_MAGIC);
        bytes
    } else {
        personalize_deb(&template, &payload, envelope)?
    };
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(".sshcc-setup-{}.tmp", uuid::Uuid::new_v4()));
    let written = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // Rename replaces an existing operator-chosen output on Unix. Windows
        // requires explicitly removing that selected file, after staging succeeds.
        #[cfg(windows)]
        if output.exists() {
            fs::remove_file(output)?;
        }
        fs::rename(&temp, output)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written?;
    Ok(
        json!({"path": output.display().to_string(), "sha256": digest(&bytes), "size": bytes.len(), "requestId": envelope["requestId"], "os": os}),
    )
}

fn ar_members(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    ensure!(bytes.starts_with(b"!<arch>\n"), "invalid Debian archive");
    let mut members = Vec::new();
    let mut pos = 8;
    while pos < bytes.len() {
        ensure!(bytes.len() - pos >= 60, "truncated Debian member");
        let header = &bytes[pos..pos + 60];
        ensure!(&header[58..60] == b"\x60\n", "invalid Debian member header");
        let name = std::str::from_utf8(&header[..16])?
            .trim()
            .trim_end_matches('/')
            .to_string();
        let size: usize = std::str::from_utf8(&header[48..58])?.trim().parse()?;
        ensure!(
            size <= LIMIT && size <= bytes.len() - pos - 60,
            "truncated Debian member content"
        );
        let start = pos + 60;
        members.push((name, bytes[start..start + size].to_vec()));
        pos = start + size + size % 2;
        ensure!(pos <= bytes.len(), "truncated Debian alignment");
    }
    Ok(members)
}
fn ar_write(members: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        ensure!(name.len() <= 15, "Debian member name too long");
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}\x60\n",
            format!("{name}/"),
            "0",
            "0",
            "0",
            "100644",
            data.len()
        );
        ensure!(header.len() == 60, "invalid Debian archive size");
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 != 0 {
            out.push(b'\n');
        }
    }
    Ok(out)
}

fn rewrite_tar(bytes: &[u8], control: bool, payload: &[u8], version: &str) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut control_seen = false;
    let mut total = 0;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            !path.is_absolute()
                && !path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_))),
            "unsafe release archive path"
        );
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();
        if !control && normalized == ENVELOPE_PATH {
            continue;
        }
        // The template has no generated md5sums. A personalized envelope is
        // intentionally outside the immutable asset manifest, validated by inspect.
        if control && normalized == "md5sums" {
            continue;
        }
        let mut header = entry.header().clone();
        let mut data = Vec::new();
        let size = header.size()? as usize;
        total += size;
        ensure!(
            size <= LIMIT && total <= LIMIT,
            "expanded release archive exceeds limit"
        );
        entry.take(size as u64 + 1).read_to_end(&mut data)?;
        ensure!(data.len() == size, "truncated release archive");
        if control && normalized == "control" {
            let text = std::str::from_utf8(&data)?;
            ensure!(
                text.lines().any(|line| line == "Package: sshcc-setup"),
                "unexpected bootstrap package"
            );
            let mut has_version = false;
            let mut new_text = String::new();
            for line in text.lines() {
                if line.starts_with("Version: ") {
                    new_text.push_str(&format!("Version: {version}\n"));
                    has_version = true;
                } else {
                    new_text.push_str(line);
                    new_text.push('\n');
                }
            }
            ensure!(has_version, "bootstrap version missing");
            data = new_text.into_bytes();
            header.set_size(data.len() as u64);
            header.set_cksum();
            control_seen = true;
        }
        builder.append_data(&mut header, path, Cursor::new(data))?;
    }
    if control {
        ensure!(control_seen, "bootstrap control missing");
    } else {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(payload.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, ENVELOPE_PATH, Cursor::new(payload))?;
    }
    let encoder = builder.into_inner()?;
    Ok(encoder.finish()?)
}
fn personalize_deb(bytes: &[u8], payload: &[u8], envelope: &Value) -> Result<Vec<u8>> {
    let mut members = ar_members(bytes)?;
    ensure!(
        members.len() == 3
            && members[0].0 == "debian-binary"
            && members[0].1 == b"2.0\n"
            && members[1].0 == "control.tar.gz"
            && members[2].0 == "data.tar.gz",
        "release template must use the standard gzip Debian format"
    );
    let request = envelope["requestId"]
        .as_str()
        .context("missing request ID")?;
    let version = format!(
        "{}+pairing.{}.{}",
        env!("CARGO_PKG_VERSION"),
        chrono::Utc::now().format("%Y%m%d%H%M%S"),
        request.replace('-', "")
    );
    members[1].1 = rewrite_tar(&members[1].1, true, payload, &version)?;
    members[2].1 = rewrite_tar(&members[2].1, false, payload, &version)?;
    ar_write(&members)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tar_gz(path: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(content.len() as u64);
        h.set_cksum();
        builder.append_data(&mut h, path, content).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn deb_payload_roundtrip_and_version() {
        let template = ar_write(&[
            ("debian-binary".into(), b"2.0\n".to_vec()),
            (
                "control.tar.gz".into(),
                tar_gz(
                    "./control",
                    b"Package: sshcc-setup\nVersion: 0.1.0\nArchitecture: amd64\n",
                ),
            ),
            (
                "data.tar.gz".into(),
                tar_gz("./usr/bin/sshcc-setup", b"launcher"),
            ),
        ])
        .unwrap();
        let env = json!({"requestId":uuid::Uuid::new_v4().to_string()});
        let payload = serde_json::to_vec(&env).unwrap();
        let personalized = personalize_deb(&template, &payload, &env).unwrap();
        let members = ar_members(&personalized).unwrap();
        let mut tar = tar::Archive::new(GzDecoder::new(Cursor::new(&members[2].1)));
        let mut seen = false;
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == ENVELOPE_PATH {
                let mut actual = Vec::new();
                entry.read_to_end(&mut actual).unwrap();
                assert_eq!(actual, payload);
                seen = true;
            }
        }
        assert!(seen);
        let mut control = tar::Archive::new(GzDecoder::new(Cursor::new(&members[1].1)));
        let mut text = String::new();
        control
            .entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains(&format!("Version: {}+pairing.", env!("CARGO_PKG_VERSION"))));
    }
    #[test]
    fn malformed_archive_is_rejected() {
        assert!(ar_members(b"!<arch>\ntruncated").is_err());
        assert!(ar_members(b"not an archive").is_err());
    }
}
