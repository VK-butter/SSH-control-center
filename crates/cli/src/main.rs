use anyhow::{bail, Context, Result};
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
    Targets,
    Doctor {
        target_id: String,
        #[arg(long)]
        session: Option<String>,
    },
    Exec {
        target_id: String,
        #[arg(long)]
        cwd: String,
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
        Action::Targets => print(&call("targets.list", json!({}))?),
        Action::Doctor { target_id, session } => print(&call(
            "targets.doctor",
            json!({"targetId":target_id,"ownerSessionId":session}),
        )?),
        Action::Exec {
            target_id,
            cwd,
            script,
            script_file,
            job_id,
        } => {
            let script = if let Some(path) = script_file {
                std::fs::read_to_string(path)?
            } else {
                script.context("--script or --script-file is required")?
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
