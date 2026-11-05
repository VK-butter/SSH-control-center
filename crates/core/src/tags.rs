//! Tags: free-text markers this Master puts on its hosts, merged hosts included (by the `peer/child` ID it uses).
//! Kept in this Master's fleet database only, never shared with other Masters. A Tag is matched without case and
//! shown as it was first typed; a Tag no host has does not exist.
use crate::{config::Config, fleet, protocol::Request};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, path::Path};

pub const MAX_LENGTH: usize = 32;
pub const MAX_PER_HOST: usize = 20;

pub(crate) fn database(state: &Path) -> Result<Connection> {
    let db = crate::folders::database(state)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS tags (target_id TEXT NOT NULL, key TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (target_id, key));",
    )?;
    Ok(db)
}

/// The Tag as typed (trimmed) and its matching key, or the rule it breaks.
fn clean(raw: &str) -> Result<(String, String)> {
    let tag = raw.trim();
    let length = tag.chars().count();
    ensure!((1..=MAX_LENGTH).contains(&length), "E_TAG_LENGTH: a Tag has 1-{MAX_LENGTH} characters");
    ensure!(!tag.contains(',') && !tag.chars().any(char::is_control), "E_TAG_CHARS: a Tag can't contain a comma");
    Ok((tag.to_owned(), tag.to_lowercase()))
}

/// Hosts this Master can tag: its own and the ones reached through a Merge.
fn known_host(state: &Path, config: &Config, id: &str) -> Result<()> {
    let hosts = fleet::handle(state, config, &Request { method: "targets.list".into(), params: json!({}) })?
        .context("targets unavailable")?;
    ensure!(
        hosts.as_array().into_iter().flatten().any(|host| host["id"] == id),
        "E_TARGET: unknown host {id}"
    );
    Ok(())
}

/// Replaces one host's Tags. Each Tag takes the spelling it already has on another host, if any.
pub fn set(state: &Path, config: &Config, id: &str, wanted: &[String]) -> Result<Vec<String>> {
    known_host(state, config, id)?;
    let mut db = database(state)?;
    let tx = db.transaction()?;
    let mut tags: Vec<(String, String)> = Vec::new();
    for raw in wanted {
        let (tag, key) = clean(raw)?;
        if tags.iter().any(|(_, seen)| seen == &key) {
            continue;
        }
        let first: Option<String> = tx
            .query_row("SELECT tag FROM tags WHERE key=?1 AND target_id<>?2 LIMIT 1", params![key, id], |row| row.get(0))
            .optional()?;
        tags.push((first.unwrap_or(tag), key));
    }
    ensure!(tags.len() <= MAX_PER_HOST, "E_TAG_LIMIT: a host has at most {MAX_PER_HOST} Tags");
    tx.execute("DELETE FROM tags WHERE target_id=?1", [id])?;
    for (tag, key) in &tags {
        tx.execute("INSERT INTO tags(target_id,key,tag) VALUES(?1,?2,?3)", params![id, key, tag])?;
    }
    tx.commit()?;
    Ok(tags.into_iter().map(|(tag, _)| tag).collect())
}

/// Every Tag with its hosts, and every tagged host with its Tags (both sorted).
pub fn list(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut by_tag: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    let mut by_host: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut stmt = db.prepare("SELECT target_id, key, tag FROM tags ORDER BY target_id, rowid")?;
    for row in stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))? {
        let (host, key, tag) = row?;
        by_tag.entry(key).or_insert_with(|| (tag.clone(), Vec::new())).1.push(host.clone());
        by_host.entry(host).or_default().push(tag);
    }
    let tags: Vec<Value> = by_tag.into_values().map(|(tag, hosts)| json!({"tag":tag,"hosts":hosts})).collect();
    let hosts: Map<String, Value> = by_host.into_iter().map(|(host, tags)| (host, json!(tags))).collect();
    Ok(json!({"tags":tags,"hosts":hosts}))
}

/// Drops a removed host's Tags.
pub(crate) fn forget(state: &Path, target_id: &str) -> Result<()> {
    database(state)?.execute("DELETE FROM tags WHERE target_id=?1", [target_id])?;
    Ok(())
}

pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !req.method.starts_with("tags.") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    let p = &req.params;
    Ok(Some(match req.method.as_str() {
        "tags.list" => list(state)?,
        "tags.set" => {
            let id = p.get("targetId").and_then(Value::as_str).context("E_ARGUMENT: missing targetId")?;
            let wanted: Vec<String> = serde_json::from_value(p.get("tags").cloned().context("E_ARGUMENT: missing tags")?)
                .context("E_ARGUMENT: tags must be a list of text")?;
            json!({"targetId":id,"tags":set(state, config, id, &wanted)?})
        }
        _ => return Ok(None),
    }))
}
