//! Text-first MCP tools for agents. Text files and exec_run are composed from
//! existing Master requests, so they work against already-enrolled Targets;
//! search and grep need the Target-side fs.search / fs.grep methods.
use crate::protocol;
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const TEXT_LIMIT: u64 = 4 * 1024 * 1024;
const READ_CHUNK: u64 = 1024 * 1024;
const READ_LINES: u64 = 200;
const READ_MAX_LINES: u64 = 500;
const READ_MAX_BYTES: usize = 256 * 1024;
const RUN_TIMEOUT: u64 = 30;
const RUN_MAX_TIMEOUT: u64 = 120;
const LOG_HEAD: u64 = 32 * 1024;

pub type Call<'a> = dyn FnMut(&str, Value) -> Result<Value> + 'a;

pub fn definitions() -> Vec<Value> {
    let scope = |mut properties: Map<String, Value>| {
        properties.insert("targetId".into(), json!({"type":"string"}));
        properties.insert("ownerSessionId".into(), json!({"type":"string"}));
        properties.insert("asAdmin".into(), json!({"type":"boolean","default":false}));
        properties
    };
    let tool = |name: &str, description: &str, properties: Value, required: &[&str], readonly: bool| {
        let properties = scope(properties.as_object().cloned().unwrap_or_default());
        let mut required: Vec<&str> = required.to_vec();
        required.insert(0, "targetId");
        json!({"name":name,"description":description,
            "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
            "annotations":{"readOnlyHint":readonly,"destructiveHint":!readonly,"openWorldHint":true}})
    };
    vec![
        tool(
            "files_search",
            "Find files under an absolute directory by glob (for example **/*.rs, src/**/*.{ts,tsx}, Cargo.toml). A glob without / matches file names at any depth. Skips .git, node_modules, target, venvs and caches unless includeIgnored. Returns absolute paths.",
            json!({"path":{"type":"string"},"pattern":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":1000,"default":100},"includeIgnored":{"type":"boolean","default":false}}),
            &["path", "pattern"],
            true,
        ),
        tool(
            "files_grep",
            "Search text files under an absolute file or directory with a regex. Returns path:line: text. Optional glob limits which files are scanned. Skips binary files, files over 4 MiB, and the same folders as files_search.",
            json!({"path":{"type":"string"},"pattern":{"type":"string"},"glob":{"type":"string"},"ignoreCase":{"type":"boolean","default":false},"maxMatches":{"type":"integer","minimum":1,"maximum":500,"default":50},"includeIgnored":{"type":"boolean","default":false}}),
            &["path", "pattern"],
            true,
        ),
        tool(
            "files_read_text",
            "Read a UTF-8 text file by line range (offset is the 1-based first line). Returns the lines, total line count and the file sha256 to pass to files_edit. Rejects binary files; use files_read for those.",
            json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":1,"default":1},"limit":{"type":"integer","minimum":1,"maximum":READ_MAX_LINES,"default":READ_LINES}}),
            &["path"],
            true,
        ),
        tool(
            "files_edit",
            "Replace one exact occurrence of oldText with newText in a text file, atomically. Fails if oldText is missing or appears more than once; include enough surrounding lines to be unique. Pass expectedSha256 from files_read_text to refuse edits to a file that changed since.",
            json!({"path":{"type":"string"},"oldText":{"type":"string"},"newText":{"type":"string"},"expectedSha256":{"type":"string"}}),
            &["path", "oldText", "newText"],
            false,
        ),
        tool(
            "files_write_text",
            "Create or fully replace a UTF-8 text file (up to 4 MiB). The parent directory must exist. Use createOnly for new files, or expectedSha256 to replace only an unchanged file. Prefer files_edit for changes to existing files.",
            json!({"path":{"type":"string"},"content":{"type":"string"},"createOnly":{"type":"boolean","default":false},"expectedSha256":{"type":"string"}}),
            &["path", "content"],
            false,
        ),
        tool(
            "exec_run",
            "Run a script on the Target in an absolute cwd and wait up to timeoutSeconds for it to finish. Returns status, exit code and the first 32 KiB of stdout and stderr. jobId must be a new UUID per command. If it is still running, call exec_run again with the same jobId and arguments to keep waiting, or use jobs_status / jobs_logs.",
            json!({"cwd":{"type":"string"},"script":{"type":"string"},"jobId":{"type":"string"},"timeoutSeconds":{"type":"integer","minimum":1,"maximum":RUN_MAX_TIMEOUT,"default":RUN_TIMEOUT},"shell":{"type":"string","enum":["powershell","bash"]},"env":{"type":"object","additionalProperties":{"type":"string"}}}),
            &["cwd", "script", "jobId"],
            false,
        ),
    ]
}

/// Returns None for tools that are not agent tools.
pub fn handle(name: &str, p: &Value, call: &mut Call) -> Result<Option<Value>> {
    let value = match name {
        "files_search" => search(p, call)?,
        "files_grep" => grep(p, call)?,
        "files_read_text" => read_text(p, call)?,
        "files_edit" => edit(p, call)?,
        "files_write_text" => write_text(p, call)?,
        "exec_run" => exec_run(p, call)?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn scope(p: &Value) -> Result<Map<String, Value>> {
    let mut base = Map::new();
    base.insert("targetId".into(), json!(protocol::string(p, "targetId")?));
    for key in ["ownerSessionId", "asAdmin"] {
        if let Some(value) = p.get(key).filter(|v| !v.is_null()) {
            base.insert(key.into(), value.clone());
        }
    }
    Ok(base)
}
fn with(base: &Map<String, Value>, extra: Value) -> Value {
    let mut params = base.clone();
    if let Value::Object(extra) = extra {
        params.extend(extra);
    }
    Value::Object(params)
}

struct Snapshot {
    bytes: Vec<u8>,
    sha256: String,
}
fn read_all(base: &Map<String, Value>, path: &str, call: &mut Call) -> Result<Snapshot> {
    let mut bytes = Vec::new();
    let mut digest: Option<String> = None;
    loop {
        let chunk = call(
            "files.read",
            with(base, json!({"path":path,"offset":bytes.len(),"limit":READ_CHUNK})),
        )?;
        let size = chunk["size"].as_u64().context("E_PROTOCOL: file size missing")?;
        ensure!(
            size <= TEXT_LIMIT,
            "E_LIMIT: text tools handle files up to 4 MiB; use files_read for larger files"
        );
        let sha = chunk["sha256"].as_str().context("E_PROTOCOL: sha256 missing")?;
        match &digest {
            Some(previous) if previous != sha => {
                bail!("E_CONFLICT: file changed while it was being read; retry")
            }
            Some(_) => {}
            None => digest = Some(sha.to_owned()),
        }
        let data = STANDARD.decode(
            chunk["contentBase64"]
                .as_str()
                .context("E_PROTOCOL: contentBase64 missing")?,
        )?;
        let empty = data.is_empty();
        bytes.extend(data);
        if chunk["eof"] == true {
            break;
        }
        ensure!(!empty, "E_PROTOCOL: read made no progress");
    }
    let sha256 = digest.context("E_PROTOCOL: sha256 missing")?;
    ensure!(
        hex::encode(Sha256::digest(&bytes)) == sha256,
        "E_CONFLICT: file changed while it was being read; retry"
    );
    Ok(Snapshot { bytes, sha256 })
}
fn text(bytes: &[u8]) -> Result<&str> {
    ensure!(
        !bytes.contains(&0),
        "E_BINARY: file is not UTF-8 text; use files_read"
    );
    std::str::from_utf8(bytes).map_err(|_| anyhow!("E_BINARY: file is not UTF-8 text; use files_read"))
}

fn read_text(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let path = protocol::string(p, "path")?;
    let start = p["offset"].as_u64().unwrap_or(1).max(1);
    let limit = p["limit"].as_u64().unwrap_or(READ_LINES).clamp(1, READ_MAX_LINES);
    let snapshot = read_all(&base, path, call)?;
    let content = text(&snapshot.bytes)?;
    let crlf = content.contains("\r\n");
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as u64;
    ensure!(
        start <= total.max(1),
        "E_ARGUMENT: offset {start} is past the end of the file ({total} lines)"
    );
    let mut body = String::new();
    let mut end = start - 1;
    let mut clipped = false;
    for line in lines.iter().skip((start - 1) as usize).take(limit as usize) {
        if body.len() + line.len() + 1 > READ_MAX_BYTES {
            if body.is_empty() {
                let mut cut = READ_MAX_BYTES - 1;
                while !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.push_str(&line[..cut]);
                body.push('\n');
                end += 1;
                clipped = true;
            }
            break;
        }
        body.push_str(line);
        body.push('\n');
        end += 1;
    }
    let truncated = end < total;
    let mut header = format!(
        "{path} lines {start}-{end} of {total} sha256={}",
        snapshot.sha256
    );
    if crlf {
        header.push_str(" (CRLF)");
    }
    if clipped {
        header.push_str(" (line clipped at 256 KiB)");
    }
    if truncated {
        header.push_str(&format!(" (more: offset={})", end + 1));
    }
    Ok(json!({"path":path,"startLine":start,"endLine":end,"totalLines":total,"truncated":truncated,
        "sha256":snapshot.sha256,"size":snapshot.bytes.len(),"lineEnding":if crlf {"crlf"} else {"lf"},
        "text":format!("{header}\n{body}")}))
}

fn edit(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let path = protocol::string(p, "path")?;
    let old = protocol::string(p, "oldText")?;
    let new = p["newText"]
        .as_str()
        .context("E_ARGUMENT: newText is required")?;
    ensure!(old != new, "E_ARGUMENT: oldText and newText are identical");
    let snapshot = read_all(&base, path, call)?;
    if let Some(expected) = p["expectedSha256"].as_str().filter(|s| !s.is_empty()) {
        ensure!(
            expected.eq_ignore_ascii_case(&snapshot.sha256),
            "E_CONFLICT: {path} changed since it was read; read it again with files_read_text"
        );
    }
    let content = text(&snapshot.bytes)?;
    let (old, new) = match_line_endings(content, old, new);
    let count = content.matches(old.as_str()).count();
    ensure!(
        count != 0,
        "E_NOT_FOUND: oldText was not found in {path}; read the file again and copy the exact text"
    );
    ensure!(
        count == 1,
        "E_AMBIGUOUS: oldText matches {count} times in {path}; include more surrounding lines"
    );
    let at = content.find(old.as_str()).unwrap();
    let updated = format!("{}{}{}", &content[..at], new, &content[at + old.len()..]);
    ensure!(
        updated.len() as u64 <= TEXT_LIMIT,
        "E_LIMIT: edited file would exceed 4 MiB"
    );
    let written = call(
        "files.write",
        with(
            &base,
            json!({"path":path,"contentBase64":STANDARD.encode(&updated),"expectedSha256":snapshot.sha256}),
        ),
    )?;
    let sha = written["sha256"].as_str().unwrap_or_default().to_owned();
    let line = content[..at].matches('\n').count() + 1;
    Ok(json!({"path":path,"line":line,"sha256":sha,"size":updated.len(),
        "text":format!("edited {path} at line {line}; sha256={sha}")}))
}
/// Agents usually send LF text; without this a CRLF file would never match.
fn match_line_endings(content: &str, old: &str, new: &str) -> (String, String) {
    if content.contains("\r\n")
        && old.contains('\n')
        && !old.contains("\r\n")
        && !content.contains(old)
    {
        (
            old.replace('\n', "\r\n"),
            new.replace("\r\n", "\n").replace('\n', "\r\n"),
        )
    } else {
        (old.to_owned(), new.to_owned())
    }
}

fn write_text(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let path = protocol::string(p, "path")?;
    let content = p["content"]
        .as_str()
        .context("E_ARGUMENT: content is required")?;
    let create_only = p["createOnly"] == true;
    let expected = p["expectedSha256"].as_str().filter(|s| !s.is_empty());
    ensure!(
        !(create_only && expected.is_some()),
        "E_ARGUMENT: use createOnly or expectedSha256, not both"
    );
    ensure!(
        content.len() as u64 <= TEXT_LIMIT,
        "E_LIMIT: text writes are limited to 4 MiB"
    );
    let mut request = json!({"path":path,"contentBase64":STANDARD.encode(content)});
    if create_only {
        // fs.write treats an empty expected hash as "the file must not exist".
        request["expectedSha256"] = json!("");
    } else if let Some(expected) = expected {
        request["expectedSha256"] = json!(expected.to_ascii_lowercase());
    }
    let written = call("files.write", with(&base, request)).map_err(|error| {
        if create_only && format!("{error:#}").contains("E_CONFLICT") {
            anyhow!("E_EXISTS: {path} already exists; read it and use files_edit")
        } else {
            error
        }
    })?;
    let sha = written["sha256"].as_str().unwrap_or_default().to_owned();
    Ok(json!({"path":path,"size":content.len(),"sha256":sha,
        "text":format!("wrote {path} ({} bytes); sha256={sha}", content.len())}))
}

fn native(result: Result<Value>) -> Result<Value> {
    result.map_err(|error| {
        let message = format!("{error:#}");
        if message.contains("method is not available over bridge")
            || message.contains("unknown filesystem method")
            || message.contains("unsupported method files.")
        {
            anyhow!("E_TARGET_OUTDATED: update SSH Control Center on this Master and Target to use files_search and files_grep")
        } else {
            error
        }
    })
}
fn stopped(result: &Value) -> String {
    match result["stoppedBy"].as_str() {
        Some("maxResults") => " (stopped at the result limit)".into(),
        Some("timeLimit") => " (stopped after 20 seconds; narrow the path or pattern)".into(),
        Some("entryLimit") => " (stopped after 200000 entries; narrow the path)".into(),
        _ => String::new(),
    }
}
fn search(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let path = protocol::string(p, "path")?;
    let mut request = json!({"path":path,"pattern":protocol::string(p, "pattern")?,
        "maxResults":p["maxResults"].as_u64().unwrap_or(100)});
    if p["includeIgnored"] == true {
        request["includeIgnored"] = json!(true);
    }
    let mut result = native(call("files.search", with(&base, request)))?;
    let matches: Vec<&str> = result["matches"]
        .as_array()
        .context("E_PROTOCOL: matches missing")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut text = format!("{} files under {path}{}", matches.len(), stopped(&result));
    for found in &matches {
        text.push('\n');
        text.push_str(found);
    }
    result["text"] = json!(text);
    Ok(result)
}
fn grep(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let path = protocol::string(p, "path")?;
    let mut request = json!({"path":path,"pattern":protocol::string(p, "pattern")?,
        "maxMatches":p["maxMatches"].as_u64().unwrap_or(50)});
    for key in ["glob", "ignoreCase", "includeIgnored"] {
        if let Some(value) = p.get(key).filter(|v| !v.is_null()) {
            request[key] = value.clone();
        }
    }
    let mut result = native(call("files.grep", with(&base, request)))?;
    let matches = result["matches"]
        .as_array()
        .context("E_PROTOCOL: matches missing")?;
    let mut text = format!(
        "{} matches in {} files scanned under {path}{}",
        matches.len(),
        result["filesScanned"].as_u64().unwrap_or(0),
        stopped(&result)
    );
    for found in matches {
        text.push_str(&format!(
            "\n{}:{}: {}",
            found["path"].as_str().unwrap_or_default(),
            found["line"],
            found["text"].as_str().unwrap_or_default()
        ));
    }
    result["text"] = json!(text);
    Ok(result)
}

fn exec_run(p: &Value, call: &mut Call) -> Result<Value> {
    let base = scope(p)?;
    let job_id = protocol::string(p, "jobId")?;
    uuid::Uuid::parse_str(job_id).context("E_ARGUMENT: jobId must be a UUID")?;
    let timeout = p["timeoutSeconds"]
        .as_u64()
        .unwrap_or(RUN_TIMEOUT)
        .clamp(1, RUN_MAX_TIMEOUT);
    let mut start = json!({"jobId":job_id,"cwd":protocol::string(p, "cwd")?,"script":protocol::string(p, "script")?});
    for key in ["shell", "env"] {
        if let Some(value) = p.get(key).filter(|v| !v.is_null()) {
            start[key] = value.clone();
        }
    }
    let began = Instant::now();
    let deadline = began + Duration::from_secs(timeout);
    let mut record = call("jobs.start", with(&base, start))?;
    let mut delay = Duration::from_millis(250);
    while matches!(record["status"].as_str(), Some("queued" | "running")) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        std::thread::sleep(delay.min(deadline - now));
        delay = (delay * 2).min(Duration::from_secs(2));
        record = call("jobs.status", with(&base, json!({"jobId":job_id})))?;
    }
    let status = record["status"].as_str().unwrap_or("unknown").to_owned();
    let stdout = log_head(&base, job_id, "stdout", call)?;
    let stderr = log_head(&base, job_id, "stderr", call)?;
    let mut text = if matches!(status.as_str(), "queued" | "running") {
        format!("job {job_id} is still {status} after {timeout}s; call exec_run again with the same jobId and arguments to keep waiting, or use jobs_status / jobs_logs")
    } else {
        format!(
            "job {job_id} {status} exitCode={} ({}s)",
            record["exitCode"],
            began.elapsed().as_secs()
        )
    };
    if let Some(message) = record["message"].as_str() {
        text.push_str(&format!("\nmessage: {message}"));
    }
    for (name, log) in [("stdout", &stdout), ("stderr", &stderr)] {
        if log.content.is_empty() {
            continue;
        }
        let note = if log.complete {
            String::new()
        } else {
            format!(
                " (first {} KiB only; read the rest with jobs_logs stream={name} offset={})",
                LOG_HEAD / 1024,
                log.next_offset
            )
        };
        text.push_str(&format!("\n--- {name}{note} ---\n{}", log.content));
    }
    Ok(json!({"jobId":job_id,"status":status,"exitCode":record["exitCode"],
        "stdout":stdout.content,"stderr":stderr.content,
        "stdoutComplete":stdout.complete,"stderrComplete":stderr.complete,"text":text}))
}
struct Log {
    content: String,
    complete: bool,
    next_offset: u64,
}
fn log_head(
    base: &Map<String, Value>,
    job_id: &str,
    stream: &str,
    call: &mut Call,
) -> Result<Log> {
    let chunk = call(
        "jobs.logs",
        with(
            base,
            json!({"jobId":job_id,"stream":stream,"offset":0,"limit":LOG_HEAD}),
        ),
    )?;
    Ok(Log {
        content: chunk["content"].as_str().unwrap_or_default().to_owned(),
        complete: chunk["eof"] == true,
        next_offset: chunk["nextOffset"].as_u64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files;
    use std::path::Path;

    fn local(method: &str, params: Value) -> Result<Value> {
        files::handle(&method.replacen("files.", "fs.", 1), &params)
    }
    fn run(name: &str, params: Value) -> Result<Value> {
        let mut call = local;
        Ok(handle(name, &params, &mut call)?.expect("agent tool"))
    }
    fn args(path: &Path, extra: Value) -> Value {
        let mut value = json!({"targetId":"t","path":path});
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        value
    }

    #[test]
    fn read_text_pages_by_line_and_reports_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();
        let page = run("files_read_text", args(&path, json!({"offset":2,"limit":1}))).unwrap();
        assert_eq!(page["totalLines"], 3);
        assert_eq!(page["endLine"], 2);
        assert_eq!(page["truncated"], true);
        assert_eq!(page["lineEnding"], "crlf");
        let text = page["text"].as_str().unwrap();
        assert!(text.ends_with("\ntwo\n"));
        assert!(text.contains("(more: offset=3)"));
        assert_eq!(
            page["sha256"],
            hex::encode(Sha256::digest(std::fs::read(&path).unwrap()))
        );
    }
    #[test]
    fn read_text_rejects_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.bin");
        std::fs::write(&path, [0u8, 1, 2]).unwrap();
        let error = run("files_read_text", args(&path, json!({}))).unwrap_err();
        assert!(error.to_string().contains("E_BINARY"));
    }
    #[test]
    fn edit_requires_one_exact_match_and_keeps_crlf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.rs");
        std::fs::write(&path, "fn a() {}\r\nfn b() {}\r\nfn a() {}\r\n").unwrap();
        let ambiguous = run(
            "files_edit",
            args(&path, json!({"oldText":"fn a() {}","newText":"fn z() {}"})),
        )
        .unwrap_err();
        assert!(ambiguous.to_string().contains("E_AMBIGUOUS"));
        let missing = run(
            "files_edit",
            args(&path, json!({"oldText":"fn q() {}","newText":"x"})),
        )
        .unwrap_err();
        assert!(missing.to_string().contains("E_NOT_FOUND"));
        let edited = run(
            "files_edit",
            args(&path, json!({"oldText":"fn b() {}\nfn a","newText":"fn b() { 1 }\nfn c"})),
        )
        .unwrap();
        assert_eq!(edited["line"], 2);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn a() {}\r\nfn b() { 1 }\r\nfn c() {}\r\n"
        );
    }
    #[test]
    fn edit_refuses_a_stale_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.txt");
        std::fs::write(&path, "alpha").unwrap();
        let error = run(
            "files_edit",
            args(&path, json!({"oldText":"alpha","newText":"beta","expectedSha256":"00"})),
        )
        .unwrap_err();
        assert!(error.to_string().contains("E_CONFLICT"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
    }
    #[test]
    fn write_text_create_only_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        run(
            "files_write_text",
            args(&path, json!({"content":"first","createOnly":true})),
        )
        .unwrap();
        let error = run(
            "files_write_text",
            args(&path, json!({"content":"second","createOnly":true})),
        )
        .unwrap_err();
        assert!(error.to_string().contains("E_EXISTS"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        run("files_write_text", args(&path, json!({"content":""}))).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }
    #[test]
    fn search_reports_an_outdated_target() {
        let mut call = |_: &str, _: Value| -> Result<Value> {
            bail!("E_PERMISSION: method is not available over bridge")
        };
        let error = handle(
            "files_search",
            &json!({"targetId":"t","path":"/srv","pattern":"*.rs"}),
            &mut call,
        )
        .unwrap_err();
        assert!(error.to_string().contains("E_TARGET_OUTDATED"));
    }
    #[test]
    fn exec_run_waits_for_the_job_and_returns_logs() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut polls = 0;
        let mut call = |method: &str, params: Value| -> Result<Value> {
            assert_eq!(params["targetId"], "t");
            Ok(match method {
                "jobs.start" => {
                    assert_eq!(params["script"], "cargo test");
                    json!({"status":"running"})
                }
                "jobs.status" => {
                    polls += 1;
                    json!({"status":"failed","exitCode":101})
                }
                "jobs.logs" if params["stream"] == "stdout" => {
                    json!({"content":"test result: FAILED","nextOffset":19,"eof":true})
                }
                "jobs.logs" => json!({"content":"","nextOffset":0,"eof":true}),
                other => panic!("unexpected {other}"),
            })
        };
        let result = handle(
            "exec_run",
            &json!({"targetId":"t","jobId":id,"cwd":"/srv/app","script":"cargo test"}),
            &mut call,
        )
        .unwrap()
        .unwrap();
        assert_eq!(polls, 1);
        assert_eq!(result["status"], "failed");
        assert_eq!(result["exitCode"], 101);
        assert!(result["text"].as_str().unwrap().contains("test result: FAILED"));
    }
    #[test]
    fn exec_run_labels_a_long_log_as_its_first_part() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut log_calls = 0;
        let mut call = |method: &str, params: Value| -> Result<Value> {
            Ok(match method {
                "jobs.logs" => {
                    log_calls += 1;
                    assert_eq!(params["offset"], 0);
                    assert_eq!(params["limit"], LOG_HEAD);
                    if params["stream"] == "stdout" {
                        json!({"content":"compiling...","nextOffset":LOG_HEAD,"eof":false})
                    } else {
                        json!({"content":"","nextOffset":0,"eof":true})
                    }
                }
                _ => json!({"status":"succeeded","exitCode":0}),
            })
        };
        let result = handle(
            "exec_run",
            &json!({"targetId":"t","jobId":id,"cwd":"/srv","script":"make"}),
            &mut call,
        )
        .unwrap()
        .unwrap();
        assert_eq!(log_calls, 2);
        assert_eq!(result["stdoutComplete"], false);
        let text = result["text"].as_str().unwrap();
        assert!(text.contains("first 32 KiB only"));
        assert!(text.contains("jobs_logs stream=stdout offset=32768"));
        assert!(!text.contains("last"));
    }
    #[test]
    fn exec_run_returns_running_at_the_timeout() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut call = |method: &str, _: Value| -> Result<Value> {
            Ok(match method {
                "jobs.logs" => json!({"content":"","nextOffset":0,"eof":true}),
                _ => json!({"status":"running"}),
            })
        };
        let result = handle(
            "exec_run",
            &json!({"targetId":"t","jobId":id,"cwd":"/srv","script":"sleep 9","timeoutSeconds":1}),
            &mut call,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result["status"], "running");
        assert!(result["text"].as_str().unwrap().contains("same jobId"));
    }
}
