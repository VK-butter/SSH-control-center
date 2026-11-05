//! Saving the installers of an unknown-OS pair: one folder, the Windows `.exe` and the Ubuntu `.deb`.
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sshcc_core::{config::Config, pairing, protocol::Request, setup_package};
use std::{fs, path::Path};
use uuid::Uuid;

fn key(seed: u8) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(wire))
}
fn tar_gz(path: &str, content: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(content.len() as u64);
    header.set_cksum();
    builder.append_data(&mut header, path, content).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}
fn ar(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}\x60\n",
            format!("{name}/"),
            "0",
            "0",
            "0",
            "100644",
            data.len()
        );
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 != 0 {
            out.push(b'\n');
        }
    }
    out
}
/// Release templates the way an installed Master has them.
fn templates(dir: &Path) {
    let exe = b"MZ windows template".to_vec();
    let deb = ar(&[
        ("debian-binary", b"2.0\n".to_vec()),
        (
            "control.tar.gz",
            tar_gz(
                "./control",
                b"Package: sshcc-setup\nVersion: 0.1.0\nArchitecture: amd64\n",
            ),
        ),
        ("data.tar.gz", tar_gz("./usr/bin/sshcc-setup", b"launcher")),
    ]);
    fs::write(dir.join("sshcc-client-windows-template.exe"), &exe).unwrap();
    fs::write(dir.join("sshcc-client-ubuntu-template.deb"), &deb).unwrap();
    let sha = |bytes: &[u8]| hex::encode(Sha256::digest(bytes));
    fs::write(
        dir.join("template-manifest.json"),
        json!({"schemaVersion":1,"templates":[
            {"os":"windows","fileName":"sshcc-client-windows-template.exe","sha256":sha(&exe)},
            {"os":"linux","fileName":"sshcc-client-ubuntu-template.deb","sha256":sha(&deb)}]})
        .to_string(),
    )
    .unwrap();
}
/// Requests from a Master, as `pairing.create` returns them.
fn requests(state: &Path, os: &str) -> Vec<Value> {
    fs::create_dir_all(state.join("keys")).unwrap();
    fs::write(state.join("keys/company.ed25519"), "placeholder").unwrap();
    fs::write(state.join("keys/company.ed25519.pub"), key(1)).unwrap();
    let config = Config {
        schema_version: 1,
        node_id: Uuid::new_v4().to_string(),
        label: "Master".into(),
        roles: vec!["master".into()],
        owner: "test".into(),
        execution_mode: "service".into(),
        allowed_principals: vec![],
    };
    let created = pairing::handle(
        state,
        &config,
        &Request {
            method: "pairing.create".into(),
            params: json!({"label":"New PC","tenant":"company","os":os,"address":"192.0.2.31"}),
        },
    )
    .unwrap()
    .unwrap();
    match created {
        Value::Array(rows) => rows,
        single => vec![single],
    }
}

#[test]
fn a_pair_saves_both_installers_into_one_folder() {
    let (state, release, folder) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    templates(release.path());
    let pair = requests(state.path(), "unknown");
    let envelopes: Vec<Value> = pair.iter().map(|row| row["envelope"].clone()).collect();
    let saved = setup_package::build_into(&envelopes, release.path(), folder.path()).unwrap();
    let paths: Vec<&str> = saved["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    let target = pair[0]["targetId"].as_str().unwrap();
    let names: Vec<String> = paths
        .iter()
        .map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into())
        .collect();
    assert_eq!(
        names,
        [
            format!("sshcc-client-{target}.exe"),
            format!("sshcc-client-{target}.deb")
        ]
    );
    for path in &paths {
        assert_eq!(Path::new(path).parent().unwrap(), folder.path());
    }
    let exe = fs::read(paths[0]).unwrap();
    assert!(exe.starts_with(b"MZ windows template"));
    assert!(exe.ends_with(setup_package::WINDOWS_MAGIC));
    assert!(fs::read(paths[1]).unwrap().starts_with(b"!<arch>\n"));
    // Each file carries its own request.
    assert_eq!(saved["files"][0]["requestId"], pair[0]["requestId"]);
    assert_eq!(saved["files"][1]["requestId"], pair[1]["requestId"]);
    assert_eq!(saved["files"][0]["os"], "windows");
    assert_eq!(saved["files"][1]["os"], "linux");
}

#[test]
fn a_single_request_saves_one_file_named_for_its_host() {
    let (state, release, folder) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    templates(release.path());
    let single = &requests(state.path(), "windows")[0];
    let name = setup_package::file_name(&single["envelope"]).unwrap();
    assert_eq!(
        name,
        format!("sshcc-client-{}.exe", single["targetId"].as_str().unwrap())
    );
    let saved = setup_package::build(
        &single["envelope"],
        release.path(),
        &folder.path().join(&name),
    )
    .unwrap();
    assert_eq!(saved["os"], "windows");
}

#[test]
fn a_missing_folder_is_refused() {
    let (state, release) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    templates(release.path());
    let envelopes: Vec<Value> = requests(state.path(), "unknown")
        .iter()
        .map(|row| row["envelope"].clone())
        .collect();
    let missing = release.path().join("nowhere");
    assert!(setup_package::build_into(&envelopes, release.path(), &missing).is_err());
}
