# SSH Control Center

**English** | [ไทย](README.th.md)

> **Personal showcase project, version 0.4.0. Not audited and not production-ready. Try it on disposable virtual machines only.**

A remote control for a small fleet of PCs and servers: say what you want done and it happens on the right machine, even if the network blinks.

*An everyday picture.* Think of a building manager with a ring of keys. You say "check room 12". The manager walks over, does it, and writes the result in that room's logbook. If you hang up the phone, the work still gets done and the logbook is still there. In this tool the manager is the **Master** (the machine you work from), the rooms are **Targets** (the machines you control), and the logbook is the job log kept on the Target.

SSH Control Center lets you, or your AI agent, run commands, edit files and open terminals on all your Windows and Ubuntu machines over SSH, from one place. It is built for 64-bit (x64) Windows and Ubuntu 24.04 machines, with up to 50 Targets per Master.

![Fleet view of the desktop app, with six example machines](docs/images/fleet-mockup.png)

*Illustrative mockup with fake data. The real app UI is in Thai.*

## What it is and why

- **What.** A small Rust program called `sshcc` plus an optional tray app. SSH is the standard, encrypted way for one computer to control another. The machine you work from is the **Master**. The machines you control are **Targets**. A **job** is one command you ask a Target to run.
- **Why.** Looking after many machines usually means many logins, keys and terminal windows. Here one place knows all of them. An AI agent can be given the same reach through **MCP** (a common way for AI assistants to use outside tools).
- **Designed for flaky connections.** A job has an ID. If the connection drops, the job keeps running on the Target and you can read its result later. Sending the same job again does not run it twice. This is tested with simulated disconnects (container and process restarts). Real WAN, reboot and logout cases are not tested.
- **Who it is for.** People who look after a small group of their own Windows or Ubuntu machines and want to drive them from one place, by hand or through an AI agent. It suits trying it out, learning from it and reading the code. Not a fit today: Mac machines, or anyone who needs a signed, independently audited product ([FAQ](docs/faq.md#2-who-is-it-for)).

## How it works

```mermaid
flowchart LR
  A["You or your AI agent"] --> B["Master service"]
  B -->|"SSH"| C["Target machine"]
  C --> D["Job logs kept on the Target"]
```

*In plain words: you ask the Master, the Master reaches the Target over SSH, and the Target keeps the record.*

Step by step: [How it works](docs/how-it-works.md). For developers: [Architecture](docs/architecture.md).

<details>
<summary>For developers: where things live</summary>

- `crates/cli`: the `sshcc` command (`init`, `service`, `mcp`, `shell`, `pairing`, `doctor`, `exec`, `job` and more).
- `crates/core`: the service, SSH handling, jobs, files, terminals, pairing and the MCP tools.
- `desktop`: the tray (Tauri 2 on Windows, GTK/AppIndicator on Ubuntu) and the on-demand Tauri 2 management window. The UI is plain HTML, CSS and JS with no frontend build step ([desktop/README.md](desktop/README.md)).
- `installers`, `scripts`, `tests`, `docs`: Windows and Ubuntu installers, build and smoke scripts, tests, documentation.

</details>

## Features

- **Durable jobs.** Run a script on a Target in the background. Jobs keep running if SSH drops. If a Target's service restarts, unfinished jobs are marked `interrupted` and are never re-run automatically.
- **Careful file access.** List, read, search, edit and write files. Writes replace the whole file at once, refuse symlinks, and can carry a SHA-256 check so you do not overwrite someone else's change. One write is limited to 4 MiB.
- **Interactive terminals.** `sshcc shell <Target>` or the terminal tools (they need Owner-session mode, see below). Detach with Ctrl+] and resume by terminal ID within 30 minutes of the last use. A service restart or an Owner logout ends the terminal, and at most 8 run per Target. Output is kept in a 4 MiB memory buffer and typed input is never stored on disk.
- **MCP server for AI agents.** `sshcc mcp` speaks MCP over stdio. Tools cover targets, jobs, files, terminals, pairing and coding helpers such as `exec_run`, `files_grep` and `files_edit`.
- **AI client setup.** The desktop "connect" button adds an `ssh_control_center` entry for Cursor, Claude Code or Codex, and only when none exists. If an entry exists but its stored path differs, a separate "update path" button rewrites that entry's command and arguments, after a timestamped backup of the config file ([ADR 0001](docs/adr/0001-mcp-registration-no-overwrite.md)).
- **Pairing in five steps.** Enter the new machine's details on the Master, save the setup file, open it on the Target and approve, copy the pairing code the Target shows, paste the code back on the Master. No SSH keys or JSON files to handle ([Pairing](docs/pairing.md)).
- **Routes and pinned host keys.** Each Target can have LAN, Tailscale (a private network service) and public routes, tried in order, with optional ProxyJump (going through a "jump" machine). Host keys are pinned, and a host-key mismatch stops all fallback.
- **Many Masters.** Several independent Masters can control one Target, each with its own keys. A Master can be merged to reach another Master's Targets through it, one hop, also in both directions. A Master holds up to 50 Targets.
- **Light health checks.** For a direct route, a routine check only reads the SSH banner and does not log in.
- **Desktop app.** Tray icon plus a management window (Thai UI): fleet graph and table, add machine, jobs and logs, file editor, an "open shell" button. The window process starts on demand and the service keeps running without it.
- **Runs as the logged-in Owner.** New Targets run work as the Owner account that installed them (Windows: the Owner's elevated token; Ubuntu: the Owner's UID/GID, or root if `asAdmin` is requested). Targets still in the earlier service mode run jobs and files as SYSTEM or root, and answer `E_EXECUTION_MODE` for terminals and `sshcc shell` ([Owner session](docs/OWNER-SESSION.md)).
- **No network listener of its own.** The CLI, MCP server and tray talk to the service over a Windows named pipe or a Unix socket on the same machine, and the caller is identity-checked. The tool opens no network port, but each Target's SSH port must still be reachable from the Master.

Intended Target baseline: Windows 10/11 x64, Windows Server 2019/2022/2025, Ubuntu 24.04 x64. The tests do not cover every edition ([INSTALL.md](docs/INSTALL.md), [VALIDATION.md](docs/VALIDATION.md)).

## Quick start

The installers are unsigned, so build from source. Install on a disposable VM first: the installers set up a privileged service.

**What one machine is enough for.** The build, the tests and the smoke test below all run on a single Windows or Ubuntu machine. They install no service and contact no remote machine. Building the installable packages is different: see "Installable packages" below, because a full Master package needs two build machines. To watch real OpenSSH between two Masters and five Targets without installing the product, use the Docker harness ([tests/containers/README.md](tests/containers/README.md) and [VALIDATION.md, section 3](docs/VALIDATION.md#3-real-ssh-container-harness)). It needs Docker.

**Prerequisites.** Git, to get the code. Rust 1.90 or newer (the version declared in `Cargo.toml`): install it with [rustup](https://rustup.rs) and check `rustc --version`, because the `cargo` package in a Linux distribution's default repositories can be older than 1.90. A native C/C++ compiler (the bundled SQLite is compiled during the build), the OpenSSH client (`ssh` and `ssh-keygen` on your PATH, which `sshcc` calls to reach Targets and to make the Master key, and which the tests use), and Python 3 for the smoke test and build scripts (3.11 or newer for `scripts/build-deb.sh`, which uses `tomllib`).

- **Ubuntu 24.04:** `sudo apt install git curl build-essential pkg-config libsystemd-dev openssh-client`. The core links `libsystemd`, and `curl` is for installing rustup. For the desktop app also install `libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`.
- **Windows:** Git for Windows, the C/C++ compiler above (with Rust's default toolchain this is normally the Visual Studio Build Tools C++ workload) and the OpenSSH Client, an optional Windows feature (check with `ssh -V`). No other libraries for the command-line tool. The desktop app needs WebView2 and the Tauri 2 Windows toolchain. Windows installers need NSIS `makensis` ([docs/BUILD-TOOLS.md](docs/BUILD-TOOLS.md)).

**Get the code.** Run every command below from this folder.

```bash
git clone https://github.com/VK-butter/SSH-control-center
cd SSH-control-center
```

**Build, test and try it.** The smoke test uses a private temporary state, installs no service and contacts no remote machine.

```powershell
# Windows (PowerShell)
cargo test --workspace --exclude sshcc-desktop
cargo build --release -p sshcc
python scripts/smoke.py --sshcc target\release\sshcc.exe
```

```bash
# Ubuntu 24.04
cargo test --workspace --exclude sshcc-desktop
cargo build --release -p sshcc
python3 scripts/smoke.py --sshcc target/release/sshcc
```

The first build compiles every dependency, including the bundled SQLite, so it takes a while. On Windows, if `python` is not found, use `py` instead. A passing smoke test ends by printing a JSON report that contains `"passed": true`, and exits with code 0. If a check fails, it stops with an error and a non-zero exit code.

**Desktop app (optional).** `cargo build --release -p sshcc-desktop`

**Installable packages.** A Windows setup EXE and Ubuntu `.deb` files are built in several steps (thin templates first, then the full Master installer). A full Master package for either OS needs both templates. So you need one Windows host (with NSIS) and one Ubuntu 24.04 x86_64 host (a VM is fine), and you copy files between them. `scripts/build-windows.ps1` stops a full Master build when either template is missing, and `scripts/build-deb.sh --setup` stops without the frozen Windows template and its manifest. With only one machine, stop at the smoke test above. Follow [Build packages](docs/INSTALL.md#build-packages). `scripts/build-deb.sh` only runs on Ubuntu 24.04 x86_64. Then add machines with [Pairing](docs/pairing.md) (Thai screens: [QUICKSTART.th.md](docs/QUICKSTART.th.md)).

On Ubuntu Targets in Owner mode, the service needs cgroup v2 and `Delegate=yes`. The shipped `installers/ubuntu/sshcc.service` sets it.

## Documentation

| Topic | English | ไทย |
|---|---|---|
| How it works, in plain words | [how-it-works](docs/how-it-works.md) | [how-it-works](docs/th/how-it-works.md) |
| Architecture | [architecture](docs/architecture.md) | [architecture](docs/th/architecture.md), [ARCHITECTURE.th](docs/ARCHITECTURE.th.md) |
| Pairing a machine | [pairing](docs/pairing.md) | [pairing](docs/th/pairing.md), [QUICKSTART.th](docs/QUICKSTART.th.md) |
| Security model | [security-model](docs/security-model.md) | [security-model](docs/th/security-model.md) |
| Reporting a vulnerability | [SECURITY](SECURITY.md) | English only |
| FAQ | [faq](docs/faq.md) | [faq](docs/th/faq.md) |
| Install, upgrade, uninstall | [INSTALL](docs/INSTALL.md) | English only |
| CLI and MCP usage | [AGENT](docs/AGENT.md) | English only |
| Owner session and terminals | [OWNER-SESSION](docs/OWNER-SESSION.md) | English only |
| API contract | [CONTRACT](docs/CONTRACT.md) | English only |
| Build tools | [BUILD-TOOLS](docs/BUILD-TOOLS.md) | English only |
| Tests and what they do not cover | [VALIDATION](docs/VALIDATION.md) | English only |
| MCP tool limits and tuning notes (static review, nothing measured) | [ai-control-performance](docs/ai-control-performance.md) | English only |
| Terms used in the code and the ADR | [GLOSSARY](docs/GLOSSARY.md) | English only |
| Design decision | [ADR 0001](docs/adr/0001-mcp-registration-no-overwrite.md) | English only |
| Desktop app, developer notes | [desktop/README](desktop/README.md) | English only |
| Desktop UI test fixture | [UI-FIXTURE](desktop/tests/UI-FIXTURE.md) | English only |
| Docker test harness with real OpenSSH | [tests/containers/README](tests/containers/README.md) | English only |
| Shell button in the app | Thai only | [SHELL-BUTTON](docs/SHELL-BUTTON.md) |

## Status and known limitations

Version 0.4.0, a personal open-source project by VK-butter. It is not production-ready. Known gaps:

- **Platforms.** Windows and Ubuntu 24.04 x64 only. macOS is not supported: there is no installer for it and the Unix executor uses Linux-only systemd and cgroup features.
- **Unsigned installers.** `SHA256SUMS` checks file integrity, not who made the file.
- **Thai desktop UI and installer screens** (with English technical terms). There is no English translation of the UI yet. The CLI and MCP tools are English.
- **Broad power.** Every enrolled Master has administrative authority over its Targets. `asAdmin` is a per-request option, not a separate approval step. Any process running as the local Owner can drive these commands on all Targets.
- **Master key has no passphrase.** The Master's private SSH key is created without a passphrase, so file permissions are its only protection. Anyone who can read it can reach every Target of that group until you revoke the Master.
- **Prompt injection.** The tool runs whatever script it is given. An AI agent that reads untrusted text (logs, files, web pages) can be tricked into sending harmful commands. Review what your agent does.
- **Pairing code.** Its checksum catches copy and paste damage; it is not a signature. Send it over a trusted channel.
- **Jobs keep their script and output** on the Target, and the Master keeps a copy of each job request (script, working folder, environment) in its fleet database. Do not put passwords in scripts or environment values.
- **Owner mode needs** an active logged-in Owner (Windows: an administrator), and on Ubuntu cgroup v2 with `Delegate=yes`.
- **Codex-first helpers.** Master setup adds the `ssh_control_center` entry to Codex's config (with a backup) when it detects Codex. Cursor and Claude Code (and Codex, if setup skipped it) are connected from the desktop app.
- **Testing gaps.** No test runs the Windows installer end to end, and real WAN, Tailscale, reboot and logout cases are not tested ([VALIDATION.md](docs/VALIDATION.md#not-covered-by-anything-in-this-tree)). This tree has no CI configuration, and no third-party security audit is claimed.

## Security

Private keys stay on the Master and are never exported (the Master's key has no passphrase, see the limits above). SSH host keys are pinned, and each Target's forced command is bound to the enrolled Master's key. Read the [security model](docs/security-model.md) and [SECURITY.md](SECURITY.md) before using it on machines that matter.

## Feedback

Issues are welcome. This is a personal project maintained on a best-effort basis, so replies may be slow and there is no support promise.

## License

[MIT](LICENSE). Copyright (c) 2026 VK-butter.
