use crate::security;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub schema_version: u32,
    pub node_id: String,
    pub label: String,
    pub roles: Vec<String>,
    pub owner: String,
    #[serde(default = "legacy_mode")]
    pub execution_mode: String,
    #[serde(default)]
    pub allowed_principals: Vec<String>,
}
fn legacy_mode() -> String {
    "service".into()
}
pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("SSHCC_DATA_DIR") {
        return path.into();
    }
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("SSHControlCenter")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/var/lib/sshcc")
    }
}
pub fn endpoint() -> PathBuf {
    if let Some(path) = std::env::var_os("SSHCC_SOCKET") {
        return path.into();
    }
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        let digest = hex::encode(Sha256::digest(
            data_dir().to_string_lossy().to_lowercase().as_bytes(),
        ));
        PathBuf::from(format!(r"\\.\pipe\sshcc-{}", &digest[..16]))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/run/sshcc/control.sock")
    }
}
pub fn load(state: &Path) -> Result<Config> {
    let cfg: Config = serde_json::from_slice(
        &std::fs::read(state.join("config.json"))
            .context("E_NOT_INITIALIZED: run sshcc init through installer")?,
    )?;
    if !matches!(cfg.schema_version, 1 | 2)
        || cfg.roles.is_empty()
        || !matches!(cfg.execution_mode.as_str(), "service" | "owner-session")
        || (cfg.schema_version == 1 && cfg.execution_mode != "service")
    {
        bail!("E_CONFIG: unsupported config");
    }
    Ok(cfg)
}
pub fn init(
    state: &Path,
    role: &str,
    owner: &str,
    label: &str,
    principals: &[String],
) -> Result<Config> {
    if !state.is_absolute() {
        bail!("E_ARGUMENT: state directory must be absolute");
    }
    let roles = match role {
        "master" => vec!["master"],
        "target" => vec!["target"],
        "both" => vec!["master", "target"],
        _ => bail!("E_ARGUMENT: role must be master, target, or both"),
    };
    security::validate_principal(owner)?;
    for p in principals {
        security::validate_principal(p)?;
    }
    std::fs::create_dir_all(state)?;
    security::protect(state, true)?;
    let mut cfg = if state.join("config.json").exists() {
        load(state)?
    } else {
        Config {
            schema_version: 2,
            node_id: uuid::Uuid::new_v4().to_string(),
            label: label.into(),
            roles: Vec::new(),
            owner: owner.into(),
            execution_mode: "owner-session".into(),
            allowed_principals: Vec::new(),
        }
    };
    if cfg.owner != owner {
        bail!("E_CONFIG: existing owner differs; owner transfer requires explicit reconfiguration");
    }
    for role in roles {
        if !cfg.roles.iter().any(|r| r == role) {
            cfg.roles.push(role.into());
        }
    }
    for principal in principals {
        if !cfg.allowed_principals.contains(principal) {
            cfg.allowed_principals.push(principal.clone());
        }
    }
    if !label.is_empty() {
        cfg.label = label.into();
    }
    let path = state.join("config.json");
    let tmp = state.join(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&cfg)?)?;
    security::protect(&tmp, false)?;
    security::replace(&tmp, &path)?;
    security::protect(&path, false)?;
    Ok(cfg)
}
/// Installer only: preserves the original Owner when selecting a new execution policy.
pub fn set_execution_mode(state: &Path, mode: &str) -> Result<Config> {
    if !matches!(mode, "service" | "owner-session") {
        bail!("E_ARGUMENT: invalid execution mode");
    }
    let mut cfg = load(state)?;
    cfg.schema_version = 2;
    cfg.execution_mode = mode.into();
    let path = state.join("config.json");
    let tmp = state.join(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&cfg)?)?;
    security::protect(&tmp, false)?;
    security::replace(&tmp, &path)?;
    Ok(cfg)
}
