//! User-level MCP registration for Cursor, Claude Code, and Codex.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const SERVER_NAME: &str = "ssh_control_center";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientId {
    Cursor,
    Claude,
    Codex,
}

impl ClientId {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "cursor" => Ok(Self::Cursor),
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            _ => bail!("E_ARGUMENT: unknown AI client"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    fn command_name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    MissingClient,
    Disconnected,
    Connected,
    PathMismatch,
}

impl LinkStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingClient => "missingClient",
            Self::Disconnected => "disconnected",
            Self::Connected => "connected",
            Self::PathMismatch => "pathMismatch",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientLink {
    pub id: String,
    pub label: String,
    pub detected: bool,
    pub status: LinkStatus,
    pub config_path: String,
    pub command: Option<String>,
    pub expected_command: String,
    pub snippet: String,
    pub message: String,
}

pub fn list_links(sshcc: &Path, home: &Path) -> Vec<ClientLink> {
    [ClientId::Cursor, ClientId::Claude, ClientId::Codex]
        .into_iter()
        .map(|id| inspect(id, sshcc, home))
        .collect()
}

pub fn connect(client: ClientId, sshcc: &Path, home: &Path) -> Result<ClientLink> {
    let link = inspect(client, sshcc, home);
    if !link.detected {
        bail!("ไม่พบ {} บนเครื่องนี้ สามารถคัดลอก snippet ได้", client.label());
    }
    match link.status {
        LinkStatus::Connected => Ok(link),
        LinkStatus::PathMismatch => bail!(
            "{} มี {} อยู่แล้ว แต่ path ไม่ตรง กด “อัปเดต path” หากต้องการแก้",
            client.label(),
            SERVER_NAME
        ),
        LinkStatus::Disconnected | LinkStatus::MissingClient => {
            write_registration(client, sshcc, home, false)?;
            Ok(inspect(client, sshcc, home))
        }
    }
}

pub fn update_path(client: ClientId, sshcc: &Path, home: &Path) -> Result<ClientLink> {
    let link = inspect(client, sshcc, home);
    if !link.detected {
        bail!("ไม่พบ {} บนเครื่องนี้", client.label());
    }
    if link.status != LinkStatus::PathMismatch {
        bail!("{} ไม่ต้องการอัปเดต path", client.label());
    }
    write_registration(client, sshcc, home, true)?;
    Ok(inspect(client, sshcc, home))
}

pub fn resolve_sshcc(current_exe: &Path) -> Result<PathBuf> {
    let install = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("ไม่พบโฟลเดอร์ที่ติดตั้ง"))?;
    #[cfg(windows)]
    let name = "sshcc.exe";
    #[cfg(not(windows))]
    let name = "sshcc";
    let path = install.join(name);
    if !path.is_file() {
        bail!("ไม่พบ {} ข้างโปรแกรมที่เปิดอยู่", name);
    }
    Ok(path)
}

pub fn user_home() -> Result<PathBuf> {
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    #[cfg(windows)]
    if let Some(home) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    bail!("ไม่พบโฟลเดอร์บ้านของผู้ใช้")
}

fn inspect(client: ClientId, sshcc: &Path, home: &Path) -> ClientLink {
    let expected = sshcc.display().to_string();
    let config_path = config_path(client, home);
    let detected = client_detected(client, home);
    let snippet = snippet(client, &expected);
    let present = registration_present(client, &config_path);
    let registered = read_command(client, &config_path);
    let (status, message) = if !detected {
        (
            LinkStatus::MissingClient,
            format!("ไม่พบ {} บนเครื่องนี้ · คัดลอก snippet ได้", client.label()),
        )
    } else if !present {
        (
            LinkStatus::Disconnected,
            format!("ยังไม่ได้เชื่อม {} กับ local service", client.label()),
        )
    } else if registered
        .as_deref()
        .is_some_and(|command| paths_match(command, sshcc))
    {
        (
            LinkStatus::Connected,
            format!("เชื่อม {} แล้ว", client.label()),
        )
    } else {
        (
            LinkStatus::PathMismatch,
            format!(
                "{} ชี้โปรแกรมอื่นอยู่ กด “อัปเดต path” เพื่อใช้ตัวที่ติดตั้งอยู่",
                client.label()
            ),
        )
    };
    ClientLink {
        id: client.as_str().into(),
        label: client.label().into(),
        detected,
        status,
        config_path: config_path.display().to_string(),
        command: registered,
        expected_command: expected,
        snippet,
        message,
    }
}

fn config_path(client: ClientId, home: &Path) -> PathBuf {
    match client {
        ClientId::Cursor => home.join(".cursor").join("mcp.json"),
        ClientId::Claude => home.join(".claude.json"),
        ClientId::Codex => home.join(".codex").join("config.toml"),
    }
}

fn client_detected(client: ClientId, home: &Path) -> bool {
    if marker_present(client, home) {
        return true;
    }
    // Unit tests supply isolated homes; ignore the host PATH there.
    if cfg!(test) {
        return false;
    }
    command_on_path(client.command_name())
}

fn marker_present(client: ClientId, home: &Path) -> bool {
    match client {
        ClientId::Cursor => home.join(".cursor").exists(),
        ClientId::Claude => home.join(".claude.json").exists() || home.join(".claude").exists(),
        ClientId::Codex => home.join(".codex").exists(),
    }
}

fn command_on_path(name: &str) -> bool {
    #[cfg(windows)]
    let status = Command::new("where").arg(name).output();
    #[cfg(not(windows))]
    let status = Command::new("which").arg(name).output();
    status
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn paths_match(configured: &str, expected: &Path) -> bool {
    let left = PathBuf::from(configured.trim());
    let right = expected.to_path_buf();
    if normalize_path(&left) == normalize_path(&right) {
        return true;
    }
    match (fs::canonicalize(&left), fs::canonicalize(&right)) {
        (Ok(a), Ok(b)) => normalize_path(&a) == normalize_path(&b),
        _ => false,
    }
}

fn normalize_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        text.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        text.into_owned()
    }
}

fn snippet(client: ClientId, command: &str) -> String {
    match client {
        ClientId::Codex => format!(
            "[mcp_servers.{SERVER_NAME}]\ncommand = '{}'\nargs = [\"mcp\"]\nenabled = true",
            command.replace('\'', "")
        ),
        ClientId::Cursor | ClientId::Claude => serde_json::to_string_pretty(&json!({
            "mcpServers": {
                SERVER_NAME: {
                    "command": command,
                    "args": ["mcp"]
                }
            }
        }))
        .unwrap_or_default(),
    }
}

fn registration_present(client: ClientId, path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    match client {
        ClientId::Codex => toml_table(&text).is_some(),
        ClientId::Cursor | ClientId::Claude => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.get("mcpServers")?.get(SERVER_NAME).cloned())
            .is_some(),
    }
}

fn read_command(client: ClientId, path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    match client {
        ClientId::Codex => read_toml_command(&text),
        ClientId::Cursor | ClientId::Claude => read_json_command(&text),
    }
}

fn read_json_command(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value
        .get("mcpServers")?
        .get(SERVER_NAME)?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

fn read_toml_command(text: &str) -> Option<String> {
    let table = toml_table(text)?;
    for line in table.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "command" {
            continue;
        }
        return Some(unquote(value.trim()));
    }
    None
}

fn toml_table(text: &str) -> Option<&str> {
    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let start = text.find(&header)?;
    let after = &text[start + header.len()..];
    let end = after
        .find("\n[")
        .map(|index| {
            // Keep the newline before the next table out of the body.
            if after.as_bytes().get(index.saturating_sub(1)) == Some(&b'\r') {
                index - 1
            } else {
                index
            }
        })
        .unwrap_or(after.len());
    Some(&after[..end])
}

fn unquote(value: &str) -> String {
    let value = value.split('#').next().unwrap_or(value).trim();
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value[1..value.len() - 1].replace("\\\\", "\\").replace("\\\"", "\"")
    } else {
        value.to_string()
    }
}

fn write_registration(client: ClientId, sshcc: &Path, home: &Path, update_existing: bool) -> Result<()> {
    let path = config_path(client, home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("สร้างโฟลเดอร์ {} ไม่ได้", parent.display()))?;
    }
    match client {
        ClientId::Codex => write_codex(&path, sshcc, update_existing),
        ClientId::Cursor | ClientId::Claude => write_json_client(&path, sshcc, update_existing),
    }
}

fn write_json_client(path: &Path, sshcc: &Path, update_existing: bool) -> Result<()> {
    let before = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("อ่าน {} ไม่ได้", path.display()))?
    } else {
        String::new()
    };
    let mut root: Value = if before.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&before).with_context(|| format!("อ่าน {} ไม่ได้", path.display()))?
    };
    let servers = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("รูปแบบ mcpServers ไม่รองรับ"))?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let map = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("รูปแบบ mcpServers ไม่รองรับ"))?;
    if let Some(existing) = map.get(SERVER_NAME) {
        if !update_existing {
            bail!("{} มีอยู่แล้วใน config", SERVER_NAME);
        }
        let mut entry = existing.clone();
        if let Some(object) = entry.as_object_mut() {
            object.insert("command".into(), json!(sshcc.display().to_string()));
            object.insert("args".into(), json!(["mcp"]));
        } else {
            entry = json!({
                "command": sshcc.display().to_string(),
                "args": ["mcp"]
            });
        }
        map.insert(SERVER_NAME.into(), entry);
    } else {
        map.insert(
            SERVER_NAME.into(),
            json!({
                "command": sshcc.display().to_string(),
                "args": ["mcp"]
            }),
        );
    }
    let after = serde_json::to_string_pretty(&root)? + "\n";
    atomic_write(path, &before, &after)
}

fn write_codex(path: &Path, sshcc: &Path, update_existing: bool) -> Result<()> {
    let before = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("อ่าน {} ไม่ได้", path.display()))?
    } else {
        String::new()
    };
    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let managed = format!(
        "command = '{}'\nargs = [\"mcp\"]\nstartup_timeout_sec = 20\ntool_timeout_sec = 150\nenabled = true\n",
        sshcc.display().to_string().replace('\'', "")
    );
    let after = if let Some(table_start) = before.find(&header) {
        if !update_existing {
            bail!("{} มีอยู่แล้วใน config", SERVER_NAME);
        }
        let after_header = table_start + header.len();
        let rest = &before[after_header..];
        let next = rest
            .find("\n[")
            .map(|index| after_header + index)
            .unwrap_or(before.len());
        let body = &before[after_header..next];
        let preserved = preserve_codex_body(body);
        let mut replacement = String::new();
        replacement.push_str(&header);
        replacement.push('\n');
        replacement.push_str(&managed);
        if !preserved.is_empty() {
            replacement.push_str(&preserved);
            if !preserved.ends_with('\n') {
                replacement.push('\n');
            }
        }
        let mut out = String::new();
        out.push_str(&before[..table_start]);
        out.push_str(&replacement);
        out.push_str(&before[next..]);
        out
    } else {
        if before.to_ascii_lowercase().contains("ssh_control_center") {
            bail!("พบการตั้งค่า ssh_control_center ที่อ่านไม่ได้ กรุณาตรวจ config.toml ก่อนลองใหม่");
        }
        let mut out = before.trim_end().to_string();
        if !out.is_empty() {
            out.push('\n');
            out.push('\n');
        }
        out.push_str(&header);
        out.push('\n');
        out.push_str(&managed);
        out
    };
    atomic_write(path, &before, &after)
}

fn preserve_codex_body(body: &str) -> String {
    let mut kept = String::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("command")
            || trimmed.starts_with("args")
            || trimmed.starts_with("startup_timeout_sec")
            || trimmed.starts_with("tool_timeout_sec")
            || trimmed.starts_with("enabled")
        {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

fn atomic_write(path: &Path, before: &str, after: &str) -> Result<()> {
    if after == before {
        return Ok(());
    }
    if path.is_file() {
        let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%3f");
        let backup = path.with_file_name(format!(
            "{}.sshcc-backup-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config"),
            stamp
        ));
        fs::copy(path, &backup).with_context(|| format!("สำรอง {} ไม่ได้", path.display()))?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(".sshcc-config-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, after).with_context(|| format!("เขียน {} ไม่ได้", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("บันทึก {} ไม่ได้", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn touch_dir(path: &Path) {
        fs::create_dir_all(path).unwrap();
    }

    fn fake_sshcc(root: &Path) -> PathBuf {
        let path = root.join(if cfg!(windows) { "sshcc.exe" } else { "sshcc" });
        fs::write(&path, b"sshcc").unwrap();
        path
    }

    #[test]
    fn connect_creates_cursor_json_without_clobbering_others() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        touch_dir(&home.join(".cursor"));
        let config = home.join(".cursor").join("mcp.json");
        fs::write(
            &config,
            r#"{"mcpServers":{"other":{"command":"other","args":[]}}}"#,
        )
        .unwrap();
        let sshcc = fake_sshcc(dir.path());
        let link = connect(ClientId::Cursor, &sshcc, &home).unwrap();
        assert_eq!(link.status, LinkStatus::Connected);
        let text = fs::read_to_string(config).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["mcpServers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(
            value["mcpServers"][SERVER_NAME]["command"].as_str(),
            Some(sshcc.to_str().unwrap())
        );
        assert_eq!(
            value["mcpServers"][SERVER_NAME]["args"],
            json!(["mcp"])
        );
    }

    #[test]
    fn connect_skips_existing_registration() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        touch_dir(&home.join(".cursor"));
        let sshcc = fake_sshcc(dir.path());
        let config = home.join(".cursor").join("mcp.json");
        fs::write(
            &config,
            format!(
                r#"{{"mcpServers":{{"{SERVER_NAME}":{{"command":"{}","args":["mcp"]}}}}}}"#,
                sshcc.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let before = fs::read_to_string(&config).unwrap();
        let link = connect(ClientId::Cursor, &sshcc, &home).unwrap();
        assert_eq!(link.status, LinkStatus::Connected);
        assert_eq!(fs::read_to_string(config).unwrap(), before);
    }

    #[test]
    fn path_mismatch_requires_explicit_update() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        touch_dir(&home.join(".codex"));
        let config = home.join(".codex").join("config.toml");
        fs::write(
            &config,
            "[mcp_servers.other]\ncommand = 'x'\n\n[mcp_servers.ssh_control_center]\ncommand = '/old/sshcc'\nargs = [\"mcp\"]\nenv.FOO = 'bar'\n",
        )
        .unwrap();
        let sshcc = fake_sshcc(dir.path());
        let link = inspect(ClientId::Codex, &sshcc, &home);
        assert_eq!(link.status, LinkStatus::PathMismatch);
        assert!(connect(ClientId::Codex, &sshcc, &home).is_err());
        let updated = update_path(ClientId::Codex, &sshcc, &home).unwrap();
        assert_eq!(updated.status, LinkStatus::Connected);
        let text = fs::read_to_string(config).unwrap();
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains(&format!("command = '{}'", sshcc.display())));
        assert!(text.contains("env.FOO = 'bar'"));
    }

    #[test]
    fn missing_client_stays_disconnected() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("empty-home");
        touch_dir(&home);
        let sshcc = fake_sshcc(dir.path());
        let link = inspect(ClientId::Claude, &sshcc, &home);
        assert!(!link.detected);
        assert_eq!(link.status, LinkStatus::MissingClient);
        assert!(connect(ClientId::Claude, &sshcc, &home).is_err());
    }

    #[test]
    fn list_links_covers_three_clients() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        touch_dir(&home);
        let sshcc = fake_sshcc(dir.path());
        let links = list_links(&sshcc, &home);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].id, "cursor");
        assert_eq!(links[1].id, "claude");
        assert_eq!(links[2].id, "codex");
    }
}
