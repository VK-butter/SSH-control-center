//! Operator grouping for the Inventory tree: Datacenters hold Masters, folders hold machines.
//! Purely organizational: neither changes keys, tenants or routing. Both are optional and kept
//! in this Master's fleet database. Masters are "local" or a merged Master's target ID; folders
//! are kept per Master branch, and a machine sits in at most one folder or none.
use crate::{config::Config, fleet, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::path::Path;
use uuid::Uuid;

const MAX_FOLDERS: i64 = 200;

pub(crate) fn database(state: &Path) -> Result<Connection> {
    let db = fleet::database(state)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS folders (id TEXT PRIMARY KEY, master TEXT NOT NULL, name TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS folder_members (target_id TEXT PRIMARY KEY, folder_id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS datacenters (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS datacenter_members (master TEXT PRIMARY KEY, datacenter_id TEXT NOT NULL);",
    )?;
    Ok(db)
}

fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key).and_then(Value::as_str).with_context(|| format!("E_ARGUMENT: missing string parameter: {key}"))
}

fn name(p: &Value) -> Result<String> {
    let name = text(p, "name")?.trim();
    ensure!(
        !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control),
        "E_ARGUMENT: folder names are 1-64 characters without control characters"
    );
    Ok(name.into())
}

fn ensure_unique(db: &Connection, master: &str, name: &str, except: Option<&str>) -> Result<()> {
    let clash: Option<String> = db
        .query_row(
            "SELECT id FROM folders WHERE master=?1 AND lower(name)=lower(?2) AND id<>?3",
            params![master, name, except.unwrap_or("")],
            |row| row.get(0),
        )
        .optional()?;
    ensure!(clash.is_none(), "E_EXISTS: a folder with this name already exists here");
    Ok(())
}

fn folder_master(db: &Connection, id: &str) -> Result<String> {
    db.query_row("SELECT master FROM folders WHERE id=?1", [id], |row| row.get(0))
        .optional()?
        .context("E_NOT_FOUND: unknown folder")
}

fn list(db: &Connection) -> Result<Value> {
    let mut folders = Vec::new();
    let mut stmt = db.prepare("SELECT id, master, name FROM folders ORDER BY lower(name)")?;
    for row in stmt.query_map([], |row| Ok(json!({"id": row.get::<_, String>(0)?, "master": row.get::<_, String>(1)?, "name": row.get::<_, String>(2)?})))? {
        folders.push(row?);
    }
    let mut members = Map::new();
    let mut stmt = db.prepare("SELECT target_id, folder_id FROM folder_members")?;
    for row in stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (target, folder) = row?;
        members.insert(target, json!(folder));
    }
    Ok(json!({"folders": folders, "members": members}))
}

/// Drop a removed machine from its folder.
pub(crate) fn forget(state: &Path, target_id: &str) -> Result<()> {
    database(state)?.execute("DELETE FROM folder_members WHERE target_id=?1", [target_id])?;
    Ok(())
}

fn datacenters(db: &Connection) -> Result<Value> {
    let mut rows = Vec::new();
    let mut stmt = db.prepare("SELECT id, name FROM datacenters ORDER BY lower(name)")?;
    for row in stmt.query_map([], |row| Ok(json!({"id": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?})))? {
        rows.push(row?);
    }
    let mut members = Map::new();
    let mut stmt = db.prepare("SELECT master, datacenter_id FROM datacenter_members")?;
    for row in stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (master, datacenter) = row?;
        members.insert(master, json!(datacenter));
    }
    Ok(json!({"datacenters": rows, "members": members}))
}

fn datacenter_exists(db: &Connection, id: &str) -> Result<()> {
    let found: Option<String> = db.query_row("SELECT id FROM datacenters WHERE id=?1", [id], |row| row.get(0)).optional()?;
    ensure!(found.is_some(), "E_NOT_FOUND: unknown datacenter");
    Ok(())
}

fn unique_datacenter(db: &Connection, name: &str, except: Option<&str>) -> Result<()> {
    let clash: Option<String> = db
        .query_row("SELECT id FROM datacenters WHERE lower(name)=lower(?1) AND id<>?2", params![name, except.unwrap_or("")], |row| row.get(0))
        .optional()?;
    ensure!(clash.is_none(), "E_EXISTS: a datacenter with this name already exists");
    Ok(())
}

pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !req.method.starts_with("folders.") && !req.method.starts_with("datacenters.") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master" || r == "both"), "E_ROLE: master role is required");
    let p = &req.params;
    let db = database(state)?;
    let value = match req.method.as_str() {
        "folders.list" => list(&db)?,
        "folders.create" => {
            let name = name(p)?;
            let master = p.get("master").and_then(Value::as_str).unwrap_or("local");
            ensure!(!master.is_empty() && master.len() <= 160, "E_ARGUMENT: invalid master");
            let count: i64 = db.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;
            ensure!(count < MAX_FOLDERS, "E_LIMIT: at most {MAX_FOLDERS} folders");
            ensure_unique(&db, master, &name, None)?;
            let id = Uuid::new_v4().to_string();
            db.execute(
                "INSERT INTO folders(id, master, name, created_at) VALUES(?1, ?2, ?3, ?4)",
                params![id, master, name, chrono::Utc::now().to_rfc3339()],
            )?;
            json!({"id": id, "master": master, "name": name})
        }
        "folders.rename" => {
            let id = text(p, "id")?;
            let name = name(p)?;
            ensure_unique(&db, &folder_master(&db, id)?, &name, Some(id))?;
            db.execute("UPDATE folders SET name=?2 WHERE id=?1", params![id, name])?;
            json!({"id": id, "name": name})
        }
        "folders.delete" => {
            let id = text(p, "id")?;
            folder_master(&db, id)?;
            // Machines stay registered; they just become unfiled.
            db.execute("DELETE FROM folder_members WHERE folder_id=?1", [id])?;
            db.execute("DELETE FROM folders WHERE id=?1", [id])?;
            json!({"id": id, "deleted": true})
        }
        "folders.assign" => {
            let target = text(p, "targetId")?;
            ensure!(!target.is_empty() && target.len() <= 160, "E_ARGUMENT: invalid targetId");
            match p.get("folderId").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                Some(folder) => {
                    let master = folder_master(&db, folder)?;
                    // A machine relayed by a merged Master ("peer/child") belongs to that Master's branch.
                    let branch = target.split_once('/').map(|(peer, _)| peer).unwrap_or("local");
                    ensure!(master == branch, "E_ARGUMENT: a machine can only go into a folder of its own Master");
                    db.execute(
                        "INSERT INTO folder_members(target_id, folder_id) VALUES(?1, ?2) ON CONFLICT(target_id) DO UPDATE SET folder_id=excluded.folder_id",
                        params![target, folder],
                    )?;
                    json!({"targetId": target, "folderId": folder})
                }
                None => {
                    db.execute("DELETE FROM folder_members WHERE target_id=?1", [target])?;
                    json!({"targetId": target, "folderId": null})
                }
            }
        }
        "datacenters.list" => datacenters(&db)?,
        "datacenters.create" => {
            let name = name(p)?;
            let count: i64 = db.query_row("SELECT COUNT(*) FROM datacenters", [], |row| row.get(0))?;
            ensure!(count < MAX_FOLDERS, "E_LIMIT: at most {MAX_FOLDERS} datacenters");
            unique_datacenter(&db, &name, None)?;
            let id = Uuid::new_v4().to_string();
            db.execute("INSERT INTO datacenters(id, name, created_at) VALUES(?1, ?2, ?3)", params![id, name, chrono::Utc::now().to_rfc3339()])?;
            json!({"id": id, "name": name})
        }
        "datacenters.rename" => {
            let id = text(p, "id")?;
            let name = name(p)?;
            datacenter_exists(&db, id)?;
            unique_datacenter(&db, &name, Some(id))?;
            db.execute("UPDATE datacenters SET name=?2 WHERE id=?1", params![id, name])?;
            json!({"id": id, "name": name})
        }
        "datacenters.delete" => {
            let id = text(p, "id")?;
            datacenter_exists(&db, id)?;
            // Masters stay; they move back under Inventory.
            db.execute("DELETE FROM datacenter_members WHERE datacenter_id=?1", [id])?;
            db.execute("DELETE FROM datacenters WHERE id=?1", [id])?;
            json!({"id": id, "deleted": true})
        }
        "datacenters.assign" => {
            let master = text(p, "master")?;
            ensure!(!master.is_empty() && master.len() <= 160 && !master.contains('/'), "E_ARGUMENT: invalid master");
            match p.get("datacenterId").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                Some(datacenter) => {
                    datacenter_exists(&db, datacenter)?;
                    db.execute(
                        "INSERT INTO datacenter_members(master, datacenter_id) VALUES(?1, ?2) ON CONFLICT(master) DO UPDATE SET datacenter_id=excluded.datacenter_id",
                        params![master, datacenter],
                    )?;
                    json!({"master": master, "datacenterId": datacenter})
                }
                None => {
                    db.execute("DELETE FROM datacenter_members WHERE master=?1", [master])?;
                    json!({"master": master, "datacenterId": null})
                }
            }
        }
        _ => bail!("E_METHOD: unsupported folders method"),
    };
    Ok(Some(value))
}
