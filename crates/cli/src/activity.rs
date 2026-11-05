//! `sshcc activity jobs|audit`: this Master's Recent Jobs and audit log, from the same service methods as the
//! window's Activity tab. A table in Thailand time, or the same CSV the window exports (`--csv`).
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// `--since`: `today`, `yesterday`, `Nd` (today and the N-1 days before) or a date (`YYYY-MM-DD`, up to today).
pub fn since(text: Option<&str>) -> Result<Value> {
    Ok(match text.map(str::trim) {
        None | Some("") => json!({"range":"7d"}),
        Some(word @ ("today" | "yesterday")) => json!({"range":word}),
        Some(days) if days.ends_with('d') && days[..days.len() - 1].parse::<u32>().is_ok_and(|n| n >= 1) => json!({"range":days}),
        Some(date) if is_date(date) => json!({"from":date}),
        Some(other) => bail!("--since is today, yesterday, a number of days such as 7d, or a date such as 2026-10-24 (got {other})"),
    })
}
fn is_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 3 && [4, 2, 2].iter().zip(&parts).all(|(n, part)| part.len() == *n && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Host IDs that match the text the way the window's search box does: ID, name, tenant, merged Master, address,
/// folder name or Tag.
pub fn hosts(text: &str, call: &dyn Fn(&str, Value) -> Result<Value>) -> Result<Vec<String>> {
    let search = text.trim().to_lowercase();
    let targets = call("targets.list", json!({}))?;
    let tags = call("tags.list", json!({})).unwrap_or(json!({}));
    let folders = call("folders.list", json!({})).unwrap_or(json!({}));
    let folder_name = |id: &str| -> Option<String> {
        let folder = folders["members"][id].as_str()?;
        folders["folders"].as_array()?.iter().find(|f| f["id"] == folder)?["name"].as_str().map(str::to_owned)
    };
    Ok(targets.as_array().into_iter().flatten().filter_map(|host| {
        let id = host["id"].as_str()?;
        let mut words: Vec<String> = ["id", "label", "tenant", "viaLabel"].iter().filter_map(|k| host[*k].as_str().map(str::to_owned)).collect();
        words.extend(host["routes"].as_array().into_iter().flatten().filter_map(|r| r["address"].as_str().map(str::to_owned)));
        words.extend(folder_name(id));
        words.extend(tags["hosts"][id].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)));
        words.iter().any(|w| w.to_lowercase().contains(&search)).then(|| id.to_owned())
    }).collect())
}

pub fn run(view: &str, host: Option<&str>, since_text: Option<&str>, csv: bool, call: &dyn Fn(&str, Value) -> Result<Value>) -> Result<String> {
    let mut params = since(since_text)?;
    if let Some(text) = host.filter(|t| !t.trim().is_empty()) {
        params["hosts"] = json!(hosts(text, call)?);
        params["text"] = json!(text.trim());
    }
    if csv {
        params["view"] = json!(view);
        return Ok(call("activity.csv", params)?["csv"].as_str().unwrap_or_default().to_owned());
    }
    let mut out = String::new();
    let time = |v: &Value| v.as_str().map(|t| t.get(..16).unwrap_or(t).replace('T', " ")).unwrap_or_default();
    if view == "audit" {
        for line in call("activity.audit", params)?["lines"].as_array().into_iter().flatten() {
            out.push_str(&format!("{}  {:<20} {:<40} {:<14} {}\n", time(&line["time"]), text(&line["action"]), text(&line["object"]), text(&line["result"]), text(&line["channel"])));
        }
    } else {
        for entry in call("activity.jobs", params)?["jobs"].as_array().into_iter().flatten() {
            let (jobs, group) = match entry["kind"].as_str() {
                Some("group") => (entry["hosts"].as_array().cloned().unwrap_or_default(), Some(entry)),
                _ => (vec![entry["job"].clone()], None),
            };
            if let Some(group) = group {
                let selector = &group["selector"];
                let label = selector["tag"].as_str().map(|t| format!("tag {t}")).or_else(|| selector["folder"].as_str().map(|f| format!("folder {f}"))).unwrap_or_default();
                out.push_str(&format!("{}  group {} ({label})\n", time(&group["sentAt"]), text(&group["groupId"])));
            }
            for job in jobs {
                let first = text(&job["script"]).lines().next().unwrap_or_default().to_owned();
                out.push_str(&format!("{}{}  {:<20} {:<12} {:<4} {}\n", if group.is_some() { "  " } else { "" }, time(&job["sentAt"]), text(&job["targetId"]), text(&job["status"]), text(&job["exitCode"]), first));
            }
        }
    }
    if out.is_empty() {
        out.push_str("Nothing in this range\n");
    }
    Ok(out)
}
fn text(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_takes_words_days_and_dates() {
        assert_eq!(since(None).unwrap(), json!({"range":"7d"}));
        assert_eq!(since(Some("yesterday")).unwrap(), json!({"range":"yesterday"}));
        assert_eq!(since(Some("30d")).unwrap(), json!({"range":"30d"}));
        assert_eq!(since(Some("2026-10-24")).unwrap(), json!({"from":"2026-10-24"}));
        for bad in ["0d", "d", "last week", "24/10/2026", "2026-1-4"] {
            assert!(since(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn host_text_matches_like_the_search_box_and_csv_is_the_services() {
        let calls = std::sync::Mutex::new(Vec::new());
        let call = |method: &str, params: Value| -> Result<Value> {
            calls.lock().unwrap().push((method.to_owned(), params.clone()));
            Ok(match method {
                "targets.list" => json!([{"id":"web-1","label":"Web one","routes":[{"address":"10.0.0.1"}]},{"id":"db-1","label":"Database"},{"id":"branch/child","label":"Child","viaLabel":"Branch"}]),
                "tags.list" => json!({"hosts":{"db-1":["Bangkok"]}}),
                "folders.list" => json!({"folders":[{"id":"f","name":"Edge"}],"members":{"web-1":"f"}}),
                "activity.csv" => json!({"csv":"\u{feff}time,host\r\n"}),
                _ => json!({"jobs":[],"lines":[]}),
            })
        };
        assert_eq!(hosts("bangkok", &call).unwrap(), ["db-1"]);
        assert_eq!(hosts("edge", &call).unwrap(), ["web-1"]);
        assert_eq!(hosts("10.0.0", &call).unwrap(), ["web-1"]);
        assert_eq!(hosts("branch", &call).unwrap(), ["branch/child"]);
        assert_eq!(run("jobs", Some("edge"), Some("yesterday"), true, &call).unwrap(), "\u{feff}time,host\r\n");
        let (method, params) = calls.lock().unwrap().last().cloned().unwrap();
        assert_eq!(method, "activity.csv");
        assert_eq!(params, json!({"range":"yesterday","hosts":["web-1"],"text":"edge","view":"jobs"}));
        assert_eq!(run("audit", None, None, false, &call).unwrap(), "Nothing in this range\n");
    }
}
