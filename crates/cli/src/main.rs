use anyhow::{bail, Context, Result};
mod activity;
mod backup;
mod shell;
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use sshcc_core::{
    client, config, fleet, pairing,
    protocol::{self, Request, Response},
    security,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
#[derive(Parser)]
#[command(
    name = "sshcc",
    version,
    about = "SSH Control Center: local agent tools and durable remote jobs"
)]
struct Args {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Init {
        #[arg(long)]
        role: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long, default_value = "SSH Control Center")]
        label: String,
        #[arg(long)]
        allow_principal: Vec<String>,
        #[arg(long)]
        execution_mode: Option<String>,
    },
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Request {
        method: String,
        #[arg(long, conflicts_with = "params_file")]
        params: Option<String>,
        #[arg(long)]
        params_file: Option<PathBuf>,
    },
    Bridge {
        #[arg(long)]
        master_id: String,
        #[arg(long)]
        master_key: String,
    },
    Mcp,
    #[command(hide = true)]
    Worker,
    Shell {
        target: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        shell: Option<String>,
        #[arg(long)]
        resume: Option<String>,
        #[arg(long)]
        as_admin: bool,
    },
    Configure {
        #[command(subcommand)]
        command: ConfigureCommand,
    },
    Pairing {
        #[command(subcommand)]
        command: PairingCommand,
    },
    Identity,
    /// Save this Master (identity, keys and hosts) to one passphrase-encrypted file.
    Backup {
        file: PathBuf,
    },
    /// Make this fresh Master the one saved in a backup file.
    Restore {
        file: PathBuf,
    },
    /// List Tags and their hosts, or replace one host's Tags (`--set web,bangkok`; `--set ""` clears them).
    Tags {
        host: Option<String>,
        #[arg(long, requires = "host")]
        set: Option<String>,
    },
    /// This Master's Recent Jobs or audit log, in Thailand time (`--csv`: the CSV the window exports).
    Activity {
        #[arg(value_parser = ["jobs", "audit"])]
        view: String,
        /// Host name, ID, address, folder or Tag, as in the window's search box.
        #[arg(long)]
        host: Option<String>,
        /// today, yesterday, 7d (the default), 30d, or a date such as 2026-10-24.
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        csv: bool,
    },
    /// List the Masters that merged this one with what each is granted, or change one Master's grant.
    Relays {
        master_id: Option<String>,
        /// full (run, write, Shell) or view (look and download only).
        #[arg(long, requires = "master_id", value_parser = ["full", "view"])]
        role: Option<String>,
        /// Only the hosts with one of these Tags (comma-separated).
        #[arg(long, requires = "master_id", conflicts_with = "all_hosts")]
        tags: Option<String>,
        #[arg(long, requires = "master_id")]
        all_hosts: bool,
        /// Whether that Master may pass on the hosts this one reaches through its own Merges.
        #[arg(long, requires = "master_id", value_parser = ["on", "off"])]
        pass_through: Option<String>,
    },
    /// Active health alarms (red first), or `ack <id>` to acknowledge one.
    Alarms {
        #[arg(value_parser = ["ack"])]
        action: Option<String>,
        #[arg(requires = "action")]
        id: Option<String>,
    },
    /// List hosts, or update older ones from this Master (`upgrade <host>` or `upgrade --all`, one at a time).
    Targets {
        #[command(subcommand)]
        command: Option<TargetsCommand>,
    },
    Doctor {
        target_id: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Run a script on one host, or on every host with a Tag (`--tag`) or in a folder (`--folder`).
    Exec {
        #[arg(required_unless_present_any = ["tag", "folder"], conflicts_with_all = ["tag", "folder"])]
        target_id: Option<String>,
        #[arg(long, conflicts_with = "folder")]
        tag: Option<String>,
        #[arg(long)]
        folder: Option<String>,
        /// For a group with both Linux and Windows hosts: one script each.
        #[arg(long)]
        script_linux: Option<String>,
        #[arg(long)]
        script_windows: Option<String>,
        /// Working folder; defaults to / (or C:\ on a Windows host).
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, conflicts_with = "script_file")]
        script: Option<String>,
        #[arg(long)]
        script_file: Option<PathBuf>,
        #[arg(long)]
        job_id: Option<String>,
    },
    Job {
        target_id: String,
        job_id: String,
        #[arg(long)]
        logs: bool,
        #[arg(long)]
        cancel: bool,
    },
}
#[derive(Subcommand)]
enum ServiceCommand {
    Run,
    Windows,
    Stop,
    Status,
}
#[derive(Subcommand)]
enum TargetsCommand {
    /// Update a host (or every outdated host this Master can update) with this Master's installer.
    Upgrade {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        host: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Which hosts are older than this Master, and whether this Master can update them.
    Check,
}
#[derive(Subcommand)]
enum PairingCommand {
    InspectCode {
        #[arg(long)]
        code_file: PathBuf,
        #[arg(long)]
        request: Option<PathBuf>,
    },
    Inspect {
        #[arg(long)]
        request: PathBuf,
        #[arg(long, alias = "bundle-out")]
        bundle_output: Option<PathBuf>,
    },
    Package {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        templates: Option<PathBuf>,
    },
    Create {
        #[arg(long)]
        label: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        os: String,
        #[arg(long)]
        address: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Status {
        request_id: Option<String>,
        #[arg(long)]
        retry: bool,
        #[arg(long)]
        routes_file: Option<PathBuf>,
    },
    Complete {
        #[arg(long, conflicts_with = "code_file")]
        code: Option<String>,
        #[arg(long)]
        code_file: Option<PathBuf>,
    },
    /// The 6 digits to compare on both screens, for a pairing code (the installers print them after Approve).
    ShortCode {
        #[arg(long)]
        code_file: PathBuf,
    },
    /// Ask the new machine for its pairing code once; prints the 6 digits when it is ready.
    Fetch {
        request_id: String,
    },
    /// After comparing the digits: `--same` finishes the pairing, `--different` cancels it.
    Confirm {
        request_id: String,
        #[arg(long, conflicts_with = "different", required_unless_present = "different")]
        same: bool,
        #[arg(long)]
        different: bool,
    },
}
#[derive(Subcommand)]
enum ConfigureCommand {
    InstallerPreflight,
    InstallerBegin,
    InstallerEnd,
    ExecutionMode {
        mode: String,
    },
    OwnerCheck,
    PairingReceipt {
        #[arg(long, alias = "envelope")]
        request: PathBuf,
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    AcceptBundle {
        file: PathBuf,
    },
    Registration {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    RevokeMaster {
        master_id: String,
    },
}
fn print(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn load_json(path: &PathBuf) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn load_code(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((pairing::MAX_CODE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= pairing::MAX_CODE_BYTES,
        "E_PAIRING_SIZE: code exceeds 64 KiB"
    );
    Ok(String::from_utf8(bytes)?)
}
/// `sshcc exec --tag/--folder`: one job on every host of the group. One `--script` is enough when the group has
/// one OS; a mixed group needs `--script-linux` and `--script-windows`. Waits until every host has finished.
fn group_exec(selector: Value, script: Option<String>, linux: Option<String>, windows: Option<String>, cwd: Option<String>) -> Result<Value> {
    let preview = call("groups.preview", json!({"selector":selector}))?;
    let mut oses: Vec<&str> = preview["hosts"].as_array().into_iter().flatten().filter_map(|h| h["os"].as_str()).collect();
    oses.sort_unstable();
    oses.dedup();
    let mut scripts = json!({"linux":linux,"windows":windows});
    if let (Some(script), [os]) = (script, oses.as_slice()) {
        scripts[*os] = json!(script);
    }
    for skipped in preview["skipped"].as_array().into_iter().flatten() {
        eprintln!("skipped {}: no SSH Control Center service", skipped["label"].as_str().unwrap_or("?"));
    }
    let mut params = json!({"selector":selector,"scripts":scripts});
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    let run = call("groups.run", params)?;
    let id = run["id"].as_str().context("the service returned no run")?.to_owned();
    eprintln!("runId={id}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let run = call("groups.status", json!({"runId":id}))?;
        if run["done"] == true {
            return Ok(run);
        }
    }
}
/// The working folder a job starts in when none is given: the root of the host's drive.
fn default_cwd(target_id: &str) -> Result<String> {
    let hosts = call("targets.list", json!({}))?;
    let host = hosts.as_array().into_iter().flatten().find(|host| host["id"] == target_id || host["label"] == target_id);
    Ok(if host.is_some_and(|host| host["os"] == "windows") { "C:\\" } else { "/" }.into())
}
/// `relays.set` params from `sshcc relays <id> --role … --tags … | --all-hosts --pass-through on|off`.
fn relay_grant(id: &str, role: Option<String>, tags: Option<String>, all_hosts: bool, pass_through: Option<String>) -> Result<Value> {
    let mut params = json!({"masterId":id});
    if let Some(role) = role {
        params["role"] = json!(role);
    }
    if let Some(tags) = tags {
        params["tags"] = json!(tags.split(',').map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>());
    }
    if all_hosts {
        params["tags"] = Value::Null;
    }
    if let Some(on) = pass_through {
        params["passThrough"] = json!(on == "on");
    }
    anyhow::ensure!(params.as_object().is_some_and(|p| p.len() > 1), "nothing to change: give --role, --tags, --all-hosts or --pass-through");
    Ok(params)
}
fn call(method: &str, params: Value) -> Result<Value> {
    client::call(&Request {
        method: method.into(),
        params,
    })
}
fn main() {
    if let Err(error) = run() {
        eprintln!(
            "{}",
            serde_json::to_string(&json!({"ok":false,"error":format!("{error:#}")})).unwrap()
        );
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args = Args::parse();
    sshcc_core::client::set_channel(if matches!(args.command, Action::Mcp) { "mcp" } else { "cli" });
    if let Some(path) = args.data_dir {
        std::env::set_var("SSHCC_DATA_DIR", path);
    }
    if let Some(path) = args.socket {
        std::env::set_var("SSHCC_SOCKET", path);
    }
    match args.command {
        Action::Identity => print(&json!({"principal":security::current_principal()?})),
        Action::Init {
            role,
            owner,
            label,
            allow_principal,
            execution_mode,
        } => {
            let owner = owner.unwrap_or(security::current_principal()?);
            let cfg = config::init(&config::data_dir(), &role, &owner, &label, &allow_principal)?;
            print(&serde_json::to_value(if let Some(mode) = execution_mode {
                config::set_execution_mode(&config::data_dir(), &mode)?
            } else {
                cfg
            })?)
        }
        Action::Service { command } => match command {
            ServiceCommand::Run => {
                let stop = Arc::new(AtomicBool::new(false));
                let signal = stop.clone();
                ctrlc::set_handler(move || {
                    signal.store(true, Ordering::SeqCst);
                    sshcc_core::service::wake();
                })?;
                sshcc_core::service::run(stop)
            }
            ServiceCommand::Windows => sshcc_core::windows_service::run(),
            ServiceCommand::Stop => print(&call("system.stop", json!({}))?),
            ServiceCommand::Status => print(&call("system.status", json!({}))?),
        },
        Action::Request {
            method,
            params,
            params_file,
        } => {
            let params = if let Some(path) = params_file {
                load_json(&path)?
            } else {
                serde_json::from_str(params.as_deref().unwrap_or("{}"))?
            };
            print(&call(&method, params)?)
        }
        Action::Bridge {
            master_id,
            master_key,
        } => {
            uuid::Uuid::parse_str(&master_id).context("invalid forced master identity")?;
            let mut input = std::io::BufReader::new(std::io::stdin());
            let mut request: Request = protocol::read_frame(&mut input)?;
            let result = (|| -> Result<Value> {
                if !matches!(request.method.as_str(), "bridge.request" | "bridge.stream") {
                    bail!("E_PERMISSION: bridge envelope required");
                }
                request
                    .params
                    .as_object_mut()
                    .context("E_ARGUMENT: bridge params required")?
                    .insert("masterId".into(), json!(master_id));
                request
                    .params
                    .as_object_mut()
                    .unwrap()
                    .insert("masterKey".into(), json!(master_key));
                if request.method == "bridge.stream" {
                    return Ok(Value::Null);
                }
                client::call(&request)
            })();
            if request.method == "bridge.stream" && result.is_ok() {
                return sshcc_core::stream::bridge(input, std::io::stdout(), &request);
            }
            protocol::write_frame(
                &Response::from_result(result),
                &mut std::io::stdout().lock(),
            )
        }
        Action::Mcp => sshcc_core::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock()),
        Action::Worker => sshcc_core::worker::run(),
        Action::Shell {
            target,
            cwd,
            session,
            shell,
            resume,
            as_admin,
        } => shell::run(target, cwd, session, shell, resume, as_admin),
        Action::Pairing { command } => match command {
            PairingCommand::ShortCode { code_file } => {
                println!("{}", pairing::short_code_of(std::fs::read_to_string(code_file)?.trim())?);
                Ok(())
            }
            PairingCommand::Fetch { request_id } => print(&call("pairing.fetch", json!({"requestId":request_id}))?),
            PairingCommand::Confirm { request_id, same, .. } => print(&call("pairing.confirm", json!({"requestId":request_id,"same":same}))?),
            PairingCommand::InspectCode { code_file, request } => {
                let envelope = request
                    .map(|path| pairing::load_envelope(&path))
                    .transpose()?;
                print(&pairing::inspect_receipt(
                    &load_code(&code_file)?,
                    envelope.as_ref(),
                )?)
            }
            PairingCommand::Inspect {
                request,
                bundle_output,
            } => {
                let inspection = pairing::inspect(&pairing::load_envelope(&request)?)?;
                if let Some(path) = bundle_output {
                    std::fs::write(path, serde_json::to_vec_pretty(&inspection["bundle"])?)?;
                }
                print(&inspection)
            }
            PairingCommand::Package {
                request,
                output,
                templates,
            } => {
                let envelope = pairing::load_envelope(&request)?;
                print(&sshcc_core::setup_package::build(
                    &envelope,
                    &templates.unwrap_or_else(sshcc_core::setup_package::default_templates),
                    &output,
                )?)
            }
            PairingCommand::Create {
                label,
                tenant,
                os,
                address,
                output,
            } => {
                // "unknown" makes two requests (one per installer); that form is for the UI only.
                anyhow::ensure!(
                    matches!(os.as_str(), "windows" | "linux"),
                    "--os must be windows or linux"
                );
                let value = call(
                    "pairing.create",
                    json!({"label":label,"tenant":tenant,"os":os,"address":address}),
                )?;
                if let Some(path) = output {
                    std::fs::write(path, serde_json::to_vec_pretty(&value["envelope"])?)?;
                }
                print(&value)
            }
            PairingCommand::Status {
                request_id,
                retry,
                routes_file,
            } => {
                let mut params = json!({"retry":retry});
                if let Some(id) = request_id {
                    params["requestId"] = json!(id);
                }
                if let Some(path) = routes_file {
                    params["routes"] = load_json(&path)?;
                }
                print(&call("pairing.status", params)?)
            }
            PairingCommand::Complete { code, code_file } => {
                let code = match code_file {
                    Some(path) => load_code(&path)?,
                    None => code.context("--code or --code-file is required")?,
                };
                print(&call("pairing.complete", json!({"code":code}))?)
            }
        },
        Action::Configure {
            command: ConfigureCommand::InstallerPreflight,
        } => print(&pairing::installer_preflight(&config::data_dir())?),
        Action::Configure { command } => {
            let state = config::data_dir();
            let cfg = config::load(&state)?;
            // Offline configure is for the installer. State permissions prevent transport users from using it.
            match command {
                ConfigureCommand::ExecutionMode { mode } => print(&serde_json::to_value(
                    config::set_execution_mode(&state, &mode)?,
                )?),
                ConfigureCommand::OwnerCheck => {
                    let executor = sshcc_core::executor::Executor::new(cfg.clone());
                    let sessions = executor.sessions()?;
                    anyhow::ensure!(
                        !sessions.is_empty(),
                        "E_OWNER_NOT_LOGGED_IN: Owner must be logged in during installation"
                    );
                    #[cfg(windows)]
                    anyhow::ensure!(
                        sessions.iter().any(|s| s.elevated),
                        "E_OWNER_NOT_ADMIN: Owner has no elevated administrator token"
                    );
                    print(&json!({"owner":cfg.owner,"sessions":sessions}))
                }
                ConfigureCommand::InstallerPreflight => unreachable!(),
                ConfigureCommand::InstallerBegin | ConfigureCommand::InstallerEnd => {
                    let begin = matches!(command, ConfigureCommand::InstallerBegin);
                    let status = call("system.status", json!({}))?;
                    anyhow::ensure!(
                        status["nodeId"].as_str() == Some(&cfg.node_id),
                        "E_IDENTITY: installer connected to a different node"
                    );
                    let method = if begin {
                        "system.install-begin"
                    } else {
                        "system.install-end"
                    };
                    match call(
                        method,
                        json!({"expectedNodeId":cfg.node_id,"dataDir":state}),
                    ) {
                        Ok(value) => print(&value),
                        Err(error)
                            if error.to_string()
                                == format!("E_METHOD: E_METHOD: unsupported method {method}") =>
                        {
                            // A service without the atomic guard rejects this method. Never conceal permission,
                            // busy, transport or identity errors as a missing guard.
                            let preflight = pairing::installer_preflight(&state)?;
                            if begin {
                                anyhow::ensure!(
                                    preflight["runningJobs"].as_u64() == Some(0),
                                    "E_BUSY: queued or running jobs must finish before installing"
                                );
                            }
                            print(
                                &json!({"guarded":false,"legacySnapshot":true,"runningJobs":preflight["runningJobs"],"nodeId":cfg.node_id}),
                            )
                        }
                        Err(error) => Err(error),
                    }
                }
                ConfigureCommand::PairingReceipt {
                    request,
                    registration,
                    output,
                } => {
                    let envelope = pairing::load_envelope(&request)?;
                    let registration = load_json(&registration)?;
                    let status = call("system.status", json!({}))?;
                    let code = pairing::installer_receipt(
                        &state,
                        &cfg,
                        &envelope,
                        &registration,
                        &status,
                    )?;
                    std::fs::write(&output, &code)?;
                    // It contains only public registration data; protect installer result state nonetheless.
                    security::protect(&output, false)?;
                    // Entry 8: its Master fetches it from here and both screens show the same 6 digits.
                    pairing::store_receipt(&state, &code)?;
                    print(
                        &json!({"code":code,"requestId":envelope["requestId"],"targetId":registration["id"]}),
                    )
                }
                ConfigureCommand::AcceptBundle { file } => {
                    let bundle = load_json(&file)?;
                    print(
                        &fleet::handle(
                            &state,
                            &cfg,
                            &Request {
                                method: "enrollment.accept".into(),
                                params: json!({"bundle":bundle}),
                            },
                        )?
                        .context("enrollment handler unavailable")?,
                    )
                }
                ConfigureCommand::Registration { spec, output } => {
                    let value = fleet::handle(
                        &state,
                        &cfg,
                        &Request {
                            method: "enrollment.registration".into(),
                            params: load_json(&spec)?,
                        },
                    )?
                    .context("registration handler unavailable")?;
                    std::fs::write(output, serde_json::to_vec_pretty(&value)?)?;
                    print(&value)
                }
                ConfigureCommand::RevokeMaster { master_id } => {
                    print(&fleet::revoke_master(&state, &master_id)?)
                }
            }
        }
        Action::Backup { file } => print(&backup::backup(&file, call)?),
        Action::Restore { file } => print(&backup::restore(&file, call)?),
        Action::Tags { host: None, .. } => print(&call("tags.list", json!({}))?),
        Action::Tags { host: Some(host), set: None } => {
            let all = call("tags.list", json!({}))?;
            print(&json!({"targetId":host,"tags":all["hosts"].get(&host).cloned().unwrap_or(json!([]))}))
        }
        Action::Tags { host: Some(host), set: Some(list) } => {
            let tags: Vec<&str> = list.split(',').map(str::trim).filter(|tag| !tag.is_empty()).collect();
            print(&call("tags.set", json!({"targetId":host,"tags":tags}))?)
        }
        Action::Activity { view, host, since, csv } => {
            print!("{}", activity::run(&view, host.as_deref(), since.as_deref(), csv, &call)?);
            Ok(())
        }
        Action::Relays { master_id: None, .. } => print(&call("relays.list", json!({}))?),
        Action::Relays { master_id: Some(id), role, tags, all_hosts, pass_through } => {
            print(&call("relays.set", relay_grant(&id, role, tags, all_hosts, pass_through)?)?)
        }
        Action::Alarms { action: None, .. } => {
            let list = call("alarms.list", json!({}))?;
            for alarm in list["alarms"].as_array().into_iter().flatten() {
                println!("{:<6} {:<20} {:<12} {:<8} since {}{}  [{}]", alarm["level"].as_str().unwrap_or(""), alarm["host"].as_str().unwrap_or(""),
                    alarm["condition"].as_str().unwrap_or(""), alarm["value"].to_string().trim_matches('"'), alarm["since"].as_str().unwrap_or(""),
                    if alarm["acknowledged"] == true { "  (acknowledged)" } else { "" }, alarm["id"].as_str().unwrap_or(""));
            }
            if list["alarms"].as_array().is_none_or(|a| a.is_empty()) {
                println!("No alarms");
            }
            Ok(())
        }
        Action::Alarms { id, .. } => {
            let id = id.ok_or_else(|| anyhow::anyhow!("give the alarm ID shown by sshcc alarms"))?;
            print(&call("alarms.ack", json!({"id":id}))?)
        }
        Action::Targets { command: None } => print(&call("targets.list", json!({}))?),
        Action::Targets { command: Some(TargetsCommand::Check) } => print(&call("targets.update_check", json!({}))?),
        Action::Targets { command: Some(TargetsCommand::Upgrade { host: Some(host), .. }) } => {
            eprintln!("Updating {host}…");
            print(&call("targets.upgrade", json!({"targetId":host}))?)
        }
        Action::Targets { command: Some(TargetsCommand::Upgrade { host: None, .. }) } => {
            call("targets.upgrade_all", json!({}))?;
            let mut shown = std::collections::BTreeMap::new();
            loop {
                let status = call("targets.upgrade_status", json!({}))?;
                for row in status["hosts"].as_array().into_iter().flatten() {
                    let line = format!("{} {}{}", row["targetId"].as_str().unwrap_or(""), row["state"].as_str().unwrap_or(""),
                        row["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default());
                    if shown.insert(row["targetId"].to_string(), line.clone()).as_ref() != Some(&line) {
                        eprintln!("{line}");
                    }
                }
                if status["done"] == true {
                    break print(&status);
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
        Action::Doctor { target_id, session } => print(&call(
            "targets.doctor",
            json!({"targetId":target_id,"ownerSessionId":session}),
        )?),
        Action::Exec {
            target_id,
            tag,
            folder,
            script_linux,
            script_windows,
            cwd,
            script,
            script_file,
            job_id,
        } => {
            let script = match script_file {
                Some(path) => Some(std::fs::read_to_string(path)?),
                None => script,
            };
            let Some(target_id) = target_id else {
                let selector = match (tag, folder) {
                    (Some(tag), _) => json!({"tag":tag,"label":format!("Tag {tag}")}),
                    (_, Some(folder)) => json!({"folder":folder,"label":format!("folder {folder}")}),
                    _ => unreachable!("clap requires a host, --tag or --folder"),
                };
                return print(&group_exec(selector, script, script_linux, script_windows, cwd)?);
            };
            let script = script.context("--script or --script-file is required")?;
            let cwd = match cwd {
                Some(cwd) => cwd,
                None => default_cwd(&target_id)?,
            };
            let id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            // Print before submission on stderr so an uncertain SSH result remains reconcilable.
            eprintln!("jobId={id}");
            print(&call(
                "jobs.start",
                json!({"targetId":target_id,"jobId":id,"cwd":cwd,"script":script}),
            )?)
        }
        Action::Job {
            target_id,
            job_id,
            logs,
            cancel,
        } => print(&call(
            if cancel {
                "jobs.cancel"
            } else if logs {
                "jobs.logs"
            } else {
                "jobs.status"
            },
            json!({"targetId":target_id,"jobId":job_id}),
        )?),
    }
}
#[cfg(test)]
mod relay_grant_tests {
    use super::*;
    #[test]
    fn targets_lists_checks_or_upgrades() {
        assert!(Args::try_parse_from(["sshcc", "targets"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "targets", "check"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "targets", "upgrade", "web-1"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "targets", "upgrade", "--all"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "targets", "upgrade"]).is_err(), "a host or --all");
        assert!(Args::try_parse_from(["sshcc", "targets", "upgrade", "web-1", "--all"]).is_err());
    }
    #[test]
    fn alarms_lists_or_acknowledges() {
        assert!(Args::try_parse_from(["sshcc", "alarms"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "alarms", "ack", "default-disk:web-1"]).is_ok());
        assert!(Args::try_parse_from(["sshcc", "alarms", "clear"]).is_err());
    }
    #[test]
    fn flags_become_one_relays_set_request() {
        let full = relay_grant("m", Some("view".into()), Some("web, db,".into()), false, Some("off".into())).unwrap();
        assert_eq!(full, json!({"masterId":"m","role":"view","tags":["web","db"],"passThrough":false}));
        assert_eq!(relay_grant("m", None, None, true, None).unwrap(), json!({"masterId":"m","tags":null}));
        assert!(relay_grant("m", None, None, false, None).is_err());
        let args = Args::try_parse_from(["sshcc", "relays", "m", "--role", "admin"]);
        assert!(args.is_err(), "role is full or view");
        assert!(Args::try_parse_from(["sshcc", "relays", "m", "--tags", "web", "--all-hosts"]).is_err());
    }
}
