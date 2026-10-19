use crate::{
    agent_tools, client,
    protocol::{self, Request},
};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
pub fn tools() -> Vec<Value> {
    let definitions=[
        ("pairing_create","Create a public setup request for a machine; requires explicit operator approval on the machine before pairing completes.",vec!["label","tenant","os","address"]),
        ("pairing_status","Read pending setup requests; retry diagnostics or update approved connection routes.",vec![]),
        ("pairing_complete","Import the public pairing code approved on a Target and verify its pinned SSH identity and SYSTEM/root service.",vec!["code"]),
        ("targets_list","List enrolled targets, routes and readiness.",vec![]),
        ("targets_doctor","Check target transport, identity and runner readiness.",vec!["targetId"]),
        ("jobs_start","Start a background script using the Target's configured executor. Use a stable UUID jobId for retries. Work continues after disconnection.",vec!["targetId","jobId","script","cwd"]),
        ("jobs_list","List target-owned jobs from every Master.",vec!["targetId"]),
        ("jobs_status","Read durable job status.",vec!["targetId","jobId"]),
        ("jobs_logs","Read a bounded stdout or stderr chunk using byte offset.",vec!["targetId","jobId"]),
        ("jobs_cancel","Cancel a job and its process tree.",vec!["targetId","jobId"]),
        ("files_list","List a target directory. Absolute path required.",vec!["targetId","path"]),
        ("files_read","Read a file chunk as base64 with a SHA256 digest.",vec!["targetId","path"]),
        ("files_write","Atomically replace a file, optionally checking expectedSha256. Up to 4 MiB decoded.",vec!["targetId","path","contentBase64"]),
        ("files_mkdir","Create a target directory.",vec!["targetId","path"]),
        ("files_remove","Remove one file or an empty directory. Nonrecursive.",vec!["targetId","path"]),
        ("terminal_open","Open an interactive terminal in the logged-in Owner session.",vec!["targetId"]),
        ("terminal_read","Read terminal output as base64 using a byte offset; output is buffered in memory.",vec!["targetId","terminalId"]),
        ("terminal_write","Send base64 input with an increasing sequence (first is 1). Retry with the same sequence and bytes.",vec!["targetId","terminalId","contentBase64","sequence"]),
        ("terminal_status","Read terminal status, executor and last acknowledged input sequence.",vec!["targetId","terminalId"]),
        ("terminal_resize","Resize a terminal in rows and columns.",vec!["targetId","terminalId","rows","cols"]),
        ("terminal_close","Close a terminal and its process tree.",vec!["targetId","terminalId"]),
    ];
    definitions.into_iter().map(|(name,description,required)|{
        let mut properties=serde_json::Map::new();
        for field in &required{properties.insert(field.to_string(),json!({"type":"string"}));}
        if name=="jobs_start" || name.starts_with("files_") || name=="terminal_open" {
            properties.insert("ownerSessionId".into(),json!({"type":"string"}));
            properties.insert("asAdmin".into(),json!({"type":"boolean","default":false}));
        }
        if name=="terminal_open" { for field in ["cwd","shell"] {properties.insert(field.into(),json!({"type":"string"}));} }
        if name=="targets_doctor" {properties.insert("ownerSessionId".into(),json!({"type":"string"}));}
        if name=="terminal_open" || name=="terminal_resize" { for field in ["rows","cols"] {properties.insert(field.into(),json!({"type":"integer","minimum":1,"maximum":4096}));} }
        if name=="terminal_write" {properties.insert("sequence".into(),json!({"type":"integer","minimum":1}));}
        if name=="terminal_read" { for field in ["offset","limit"] {properties.insert(field.into(),json!({"type":"integer","minimum":0}));} }
        if name=="jobs_start"{properties.insert("shell".into(),json!({"type":"string","enum":["powershell","bash"]}));properties.insert("env".into(),json!({"type":"object","additionalProperties":{"type":"string"}}));}
        if name=="jobs_logs"||name=="files_read"{for field in ["offset","limit"]{properties.insert(field.into(),json!({"type":"integer","minimum":0}));}}
        if name=="jobs_logs"{properties.insert("stream".into(),json!({"type":"string","enum":["stdout","stderr"]}));}
        if name=="files_write"{properties.insert("expectedSha256".into(),json!({"type":"string"}));}
        if name=="pairing_create" {
            for field in ["targetId","user","routeKind"] { properties.insert(field.into(),json!({"type":"string"})); }
            properties.insert("port".into(),json!({"type":"integer","minimum":1,"maximum":65535}));
            properties.insert("routes".into(),json!({"type":"array","items":{"type":"object"}}));
        }
        if name=="pairing_status" {
            properties.insert("requestId".into(),json!({"type":"string"}));
            properties.insert("retry".into(),json!({"type":"boolean"}));
            properties.insert("routes".into(),json!({"type":"array","items":{"type":"object"}}));
        }
        let readonly=matches!(name,"targets_list"|"targets_doctor"|"jobs_list"|"jobs_status"|"jobs_logs"|"files_list"|"files_read"|"terminal_read"|"terminal_status");
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":readonly,"destructiveHint":!readonly,"openWorldHint":true}})
    }).chain(agent_tools::definitions()).collect()
}
const INSTRUCTIONS: &str = "Work on a Target like a local project: targets_list first, then targets_doctor once before the first action. Doctor ok only means SSH and identity passed; work can run only when ready is true. When ownerSessionId is omitted, the Target auto-picks an active or newest eligible Owner session and reports it in result.executor.selected; pass ownerSessionId to pin a login and reuse that same id on every files_*, jobs_start and exec_run call to that Target when you need a stable binding. Find files with files_search and files_grep, read them with files_read_text, change them with files_edit (or files_write_text for new files), and build or test with exec_run. Use jobs_status / jobs_logs only after exec_run times out, files_read / files_write only for binary files, and terminal_* only when a person needs an interactive session. Paths are absolute on the Target. Leave asAdmin off unless the user asked for administrator rights. Use a new UUID jobId per command and reuse it only to reconcile or keep waiting; never blindly repeat a deployment. No project locks are enforced.";
pub fn serve(input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut input = input;
    let mut initialized = false;
    let mut ready = false;
    loop {
        let mut line = String::new();
        let count = (&mut input)
            .take((protocol::MAX_FRAME + 1) as u64)
            .read_line(&mut line)?;
        if count == 0 {
            break;
        }
        if count > protocol::MAX_FRAME {
            anyhow::bail!("E_PROTOCOL: oversized MCP frame");
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                protocol::write_frame(
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
                    &mut output,
                )?;
                continue;
            }
        };
        let method = message["method"].as_str().unwrap_or("");
        let id = message.get("id").cloned();
        if id.is_none() {
            if method == "notifications/initialized" && initialized {
                ready = true;
            }
            continue;
        }
        let id = id.unwrap();
        let response = if message["jsonrpc"] != "2.0" {
            error(id, -32600, "Invalid Request")
        } else {
            match method {
                "initialize" if !initialized => {
                    initialized = true;
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"ssh-control-center","version":env!("CARGO_PKG_VERSION")},"instructions":INSTRUCTIONS}})
                }
                "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
                _ if !ready => error(
                    id,
                    -32002,
                    "Initialize and send notifications/initialized first",
                ),
                "tools/list" => json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools()}}),
                "tools/call" => {
                    let name = message["params"]["name"].as_str().unwrap_or("");
                    if !tools().iter().any(|t| t["name"] == name) {
                        error(id, -32602, "Unknown tool")
                    } else {
                        let args = message["params"]
                            .get("arguments")
                            .cloned()
                            .unwrap_or(json!({}));
                        let mut forward = |method: &str, params: Value| {
                            client::call(&Request {
                                method: method.into(),
                                params,
                            })
                        };
                        match agent_tools::handle(name, &args, &mut forward) {
                            // Agent tools answer in plain text so every client shows file content as-is.
                            Ok(Some(value)) => {
                                let text = value["text"].as_str().unwrap_or_default();
                                json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":false}})
                            }
                            Ok(None) => match forward(&name.replacen('_', ".", 1), args) {
                                Ok(value) => {
                                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":serde_json::to_string(&value)?}],"structuredContent":{"result":value},"isError":false}})
                                }
                                Err(err) => {
                                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":format!("{err:#}")}],"isError":true}})
                                }
                            },
                            Err(err) => {
                                json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":format!("{err:#}")}],"isError":true}})
                            }
                        }
                    }
                }
                _ => error(id, -32601, "Method not found"),
            }
        };
        protocol::write_frame(&response, &mut output)?;
    }
    Ok(())
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
