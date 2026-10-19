# Testing and validation

This page inventories the automated and scripted checks in this repository, how to run each one, and what it does not cover. It contains no recorded results: run a check to produce your own evidence, and keep the command, binary hash, OS build and output with it.

Passing source-level or container tests does not establish a working SYSTEM/root installation on real machines, reachable remote Targets, or desktop resource use on a real desktop session. Each section below says what its check cannot show.

## Quick start

No Docker and no installation are needed for these two:

~~~text
cargo test --workspace --exclude sshcc-desktop
cargo build --release -p sshcc
python scripts/smoke.py --sshcc target/release/sshcc
~~~

On Windows use `target\release\sshcc.exe`. Building needs Rust 1.90 or later (`rust-version` in `Cargo.toml`) and a native compiler.

## 1. Rust tests

| Location | What it exercises |
|---|---|
| `crates/core/tests/runtime.rs` | Job deduplication, persisted exit codes and logs, cancellation, interrupted state on service shutdown and restart recovery, binary file round trip with hash guard, local principal/role checks, MCP negotiation and tool listing |
| `crates/core/tests/fleet.rs` | Target registry limits and idempotent re-import, identity/tenant pinning, route order, atomic version-1 registry import, ProxyJump pins, independent Masters (revoke and re-enroll), job-ID conflict detection, enrollment export/accept, key rotation, merged-Master relay, composite IDs and routine health checks |
| `crates/core/tests/pairing.rs` | Pairing request persistence across restart, receipt code encoding (Thai text, damaged and oversized codes), binding of the receipt to the approved settings, retry of unreachable Targets, installer privilege and approval rules, installer admission guard, MCP pairing tools |
| `crates/core/tests/execution_policy.rs` | Schema 1 configs stay in service mode, terminal metadata blocks the installer, an absent Owner never falls back to root (the last test is Unix only) |
| `crates/core/tests/duplex.rs` | Windows only: named-pipe reads and writes do not block each other |
| `crates/core/src/*.rs` unit tests | `agent_tools`, `client_links`, `executor`, `files`, `fleet_merge`, `fleet_ssh`, `search`, `setup_package` |
| `desktop/src-tauri/src/windows_window.rs` | Windows only: command-line argument quoting |

`crates/cli` has no tests of its own. The desktop crate is excluded from the default workspace members; run its tests on Windows with `cargo test -p sshcc-desktop` once the Tauri prerequisites from [desktop/README.md](../desktop/README.md) are installed. `desktop/src-tauri/src/tauri_app.rs` also holds one `#[ignore]` test (`launch_under_management_job`) that needs an installed Master and a live Target selected through `SSHCC_LIVE_SHELL_TARGET`; it never runs by default.

`tests/containers/Dockerfile` runs `cargo test --locked -p sshcc-core -- --test-threads=1` during its build.

## 2. Isolated CLI, service and MCP smoke

`python scripts/smoke.py --sshcc PATH_TO_BUILT_BINARY` creates a fresh temporary state directory and a unique local endpoint (`SSHCC_DATA_DIR`, `SSHCC_SOCKET`), starts `sshcc service run`, and prints a JSON report that includes the artifact folder with the service log. It never installs a service and never contacts a Target.

It checks: idempotent `init`; five concurrent local jobs carrying two Master IDs; duplicate submission and payload-conflict rejection; jobs continuing after the submitting client exits; stdout/stderr capture; a binary file round trip with hash guard; non-recursive directory removal; cancellation; a service crash leading to `interrupted` with no replay; an unenrolled bridge request being denied; and real MCP `initialize`, `tools/list` and `tools/call` over stdio.

The script's own report lists what it does not cover: real SSH, remote machines, SYSTEM/root installation, IPC from another OS identity, the tray, the resource budget, reboot/logout, and installer upgrade/rollback.

## 3. Real-SSH container harness

`tests/ssh_e2e.py` with `tests/containers/Dockerfile` (needs Docker). See [tests/containers/README.md](../tests/containers/README.md) for the commands. It starts two independent Master containers and five Target containers on an internal Docker network with real OpenSSH, and writes `ssh-e2e-report.json`. The checks, by name in the script:

`two_masters_five_targets`, `five_concurrent_root_build_deploy_jobs`, `atomic_file_transfer_and_hash_conflict`, `ordered_route_fallback`, `client_disconnect_retry_and_master_restart`, `concurrent_master_attribution`, `revoke_and_stale_key_fingerprint`, `pinned_proxy_jump_and_jump_mismatch`, `lost_ssh_reply_fallback_executes_once`, `host_key_mismatch_stops_fallback`, `target_identity_binding`, `target_service_restart_interrupted`, `local_principal_rejection`.

The LAN, Tailscale and public route kinds are labels on a single internal network, so this harness does not test real network policy. It also does not claim systemd boot, Windows SSH, real WAN/Tailscale, desktop performance, signed installers or a physical reboot.

## 4. Package personalization smoke

`python scripts/package-smoke.py --sshcc PATH --templates DIRECTORY --report REPORT.json` starts an isolated Master (no installation, no SSH), creates a pairing request for a Windows and a Linux Target, builds a personalized client package for each from the thin templates in `DIRECTORY`, and checks that the embedded public envelope matches the request and contains no private key or token. Build the templates first as described in [INSTALL.md](INSTALL.md#build-packages).

## 5. Installer tests

**Windows.** `installers/windows/Onboarding.Tests.ps1 -CliPath PATH_TO_sshcc.exe [-NativeTemplate PATH]` runs the pairing bootstrap and Codex configuration helpers in a temporary directory, with no install, scheduled task or service change. It checks template and trailer handling (missing, corrupted or truncated pairing metadata is rejected; Thai labels survive), that denying approval leaves the service untouched, rejection of envelopes over 64 KiB and of tampered bundles, Codex configuration (idempotent, preserves other MCP entries, one backup), and, when a native template is supplied, inspection of an executable with an appended envelope.

**Ubuntu.** `test-setup.py`, `test-setup-gui.py` and `isolated-build.sh` run inside a disposable Docker container and refuse to run elsewhere. The two `isolated-*.py` drivers run on a host that has Docker and create their own container.

- `installers/ubuntu/test-setup.py`: unit tests, run as root, for the setup helper (Codex configuration merge and backup, cached pairing code revalidation, asset hash/symlink/traversal rejection, piped input cannot approve, terminal cancel never calls the privileged worker).
- `installers/ubuntu/test-setup-gui.py`: native GTK/Zenity approve, cancel and code dialogs with clipboard paste, under `xvfb-run` (needs `zenity`, `xdotool` and `xclip`).
- `installers/ubuntu/isolated-build.sh` with `installers/ubuntu/Dockerfile.build`: a build helper that runs the Rust tests, builds the release binaries and Debian packages, then runs `test-setup.py`. It expects the source at `/source` and writable `/cache` and `/dist` mounts.
- `installers/ubuntu/isolated-onboarding.py` and `installers/ubuntu/isolated-master-setup.py`: end-to-end runs in a disposable Ubuntu 24.04 container with real systemd, covering pairing with two Masters, the root service, stale-hash rejection, the installer refusing to stop the service while a job is active, preservation of owner/roles/node identity and Masters, headless installs without a tray, Codex MCP wiring that preserves other entries, and Windows and Ubuntu package exports from a Linux Master. They hard-code `0.4.0` image and package names and expect built packages under `dist/`, so adjust them to your build before use.

## 6. Desktop checks

- **UI fixture (browser):** `node desktop/tests/fixture.cjs` and the Playwright script `desktop/tests/open-shell.cjs`; see [desktop/tests/UI-FIXTURE.md](../desktop/tests/UI-FIXTURE.md) and [desktop/README.md](../desktop/README.md). `scripts/onboarding-ui-fixture.py` is a loopback-only fixture for the add-machine flow with a stubbed IPC bridge. These validate UI behavior only.
- **Windows lifecycle:** `python tests/windows_lifecycle.py --release RELEASE_DIR --output REPORT.json` checks normal open, duplicate activation, normal close reaping the whole window Job Object, reopening, and tray-crash cleanup with the service surviving. It needs Windows and `psutil`; it installs no service and changes no SSH configuration.
- **Ubuntu lifecycle:** `desktop/tests/lifecycle-ubuntu.py OUTPUT_DIR_UNDER_/tmp` checks normal close and duplicate activation for the GTK/WebKit window. It requires a Docker container (`/.dockerenv`) holding the release binaries under `/src/target/release`, as built by `desktop/tests/Dockerfile.ubuntu`.

### Ten-minute resource procedure

`scripts/benchmark.py` samples the service, the tray and all descendants. It requires exactly 50 registered Targets and at least 600 seconds of idle sampling after the tray has completed ten window cycles. Use a fresh private state directory, and set `SSHCC_DATA_DIR` and `SSHCC_SOCKET` consistently for the service, CLI and tray.

On Windows, `python tests/run_native_benchmark.py --release RELEASE_DIR --output-dir NEW_DIR` performs the whole sequence in temporary state. On Ubuntu, build the image from `desktop/tests/Dockerfile.ubuntu`, start a container from it, and run `desktop/tests/benchmark-ubuntu.sh OUTPUT_DIR_UNDER_/tmp` inside; it uses Xvfb, a private D-Bus session and tint2 as the tray host. To do it by hand:

1. Initialize role master with the actual owner principal and a label starting with `benchmark:`, then start `sshcc service run`.
2. `python scripts/seed-benchmark.py --sshcc PATH --isolated-state ABSOLUTE_STATE` refuses a non-empty registry or a label without the `benchmark:` prefix, and adds exactly 50 offline fixtures from the documentation address range. It does not SSH to them.
3. Launch a fresh release tray with `--benchmark-cycles 10 --benchmark-report ABSOLUTE_NEW_REPORT_PATH`, and immediately start the collector. The hook keeps the window open for five seconds and closed for five seconds per cycle, and an existing report file is rejected so observation starts before the cycles finish.
4. With no build or deploy jobs running: `python scripts/benchmark.py --sshcc PATH --service-pid PID --tray-pid PID --seconds 600 --window-report ABSOLUTE_NEW_REPORT_PATH --output benchmark.json`

The collector reports every sampled process, summed resident memory (shared memory counted per process), CPU as a share of one logical core, target count, duration, OS/version, release binary SHA256 hashes and the service execution principal. A run passes only if the summed resident memory stays at or below 100,000,000 bytes and mean CPU is below 1%; the process exit code reflects this. Its own stated limitations: descendants that live shorter than the sampling interval may be missed, and every observed descendant must be readable. Install `psutil` into a test-only Python environment first. Treat a failed or missing measurement as a failure, not a pass.

The Ubuntu harness disables WebKit's sandbox inside the container because it runs as root; production startup does not. It does not represent a GNOME/Wayland session, a real GPU, login/logout, or an installed boot service.

## Optional syntax checks

~~~text
bash -n installers/ubuntu/*.sh scripts/build-deb.sh installers/ubuntu/postinst installers/ubuntu/postrm installers/ubuntu/preinst installers/ubuntu/prerm
python -m py_compile scripts/*.py tests/*.py installers/ubuntu/*.py desktop/tests/lifecycle-ubuntu.py
~~~

For the PowerShell files (`installers/windows/*.ps1`, `scripts/*.ps1`), parse them with `[System.Management.Automation.Language.Parser]::ParseFile`.

## Not covered by anything in this tree

- No script under `tests/`, `scripts/` or the installer test files runs `Setup.ps1`, `Uninstall.ps1` or the NSIS-built Windows installer, installs the Windows service, or tests Windows upgrade, uninstall or rollback. The Windows installer helpers are tested in isolation (section 5).
- The Ubuntu installer is exercised only inside disposable containers.
- No test covers Git or SMB credentials inside the job execution context; [INSTALL.md](INSTALL.md) says to test them with a real job.
- Physical reboot, logout, a real network outage and public-IP routing are not tested; the harnesses simulate disconnects with container and process restarts.
- Release signing is not configured; the generated `SHA256SUMS` only verify integrity (see [BUILD-TOOLS.md](BUILD-TOOLS.md)).
- Resource budgets are measured only by the procedure above, on whatever machine you run it on.
