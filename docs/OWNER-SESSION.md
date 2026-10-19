# Owner session execution

**What this page is.** How work runs as the logged-in Owner account on a Target, which is the default for new installs: the account rules on Windows and Ubuntu, the `sshcc shell` terminal, the terminal tools and their limits, and upgrade and rollback notes. **Who it is for:** operators and developers who need to know which account commands run as. If you are new, read [Pairing](pairing.md) first.

Owner is captured before UAC/sudo and remains immutable on upgrade. Config schema 2 uses `executionMode: owner-session`. Config schema 1 uses `service` and keeps that mode unless the installer selects Owner session. The service runs as SYSTEM/root and owns SSH identities, fleet SQLite and job logs.

## Accounts

Windows checks the Owner SID, session ID, authentication generation and elevated administrator token, loads the user profile/environment, and starts an isolated worker in a kill-on-close Job Object. Lock and disconnected RDP sessions remain eligible. Another administrator approving UAC does not become Owner.

Ubuntu requires an active/online logind user login; lingering alone is insufficient. A root worker joins the delegated cgroup before receiving work, then sets supplementary groups, GID, UID and Owner HOME. `asAdmin: true` reports principal `0`, account `root`. No passwords or sudo policy are changed. Builds need `pkg-config`, `libsystemd-dev` and a native compiler; packages depend on `libsystemd0`.

Doctor lists `executor.sessions`, readiness, selected executor, CLI paths/version and sign-in boolean. When `ownerSessionId` is omitted and more than one login exists, the Target auto-picks an active (or newest eligible) session and reports it in `executor.selected`. Pass `ownerSessionId` to pin a specific login. Missing login returns `E_OWNER_NOT_LOGGED_IN`; Windows with no elevated session returns `E_OWNER_NOT_ADMIN`. Work binds to that login generation. Logout stops its process tree and records `interrupted`.

Git/Codex use the Owner PATH and shell profiles. No authentication tokens are displayed or copied between accounts. PowerShell profiles remain subject to the existing execution policy. Workers receive requests over pipes and do not open service state.

## Shell

```cmd
"%ProgramFiles%\SSH Control Center\sshcc.exe" shell win-box
"%ProgramFiles%\SSH Control Center\sshcc.exe" shell win-box --cwd C:\Users\alice\projects --shell cmd
"%ProgramFiles%\SSH Control Center\sshcc.exe" shell win-box --resume <terminalId>
```

```powershell
& "$env:ProgramFiles\SSH Control Center\sshcc.exe" shell win-box
```

```bash
sshcc doctor build-server --session 3
sshcc shell build-server --session 3 --cwd /home/alice/projects
sshcc shell build-server --session 3 --as-admin
```

Names must match one Target; duplicates require Target ID. Windows defaults to PowerShell with profiles, or `--shell cmd`; Ubuntu defaults to the Owner login shell. Cwd defaults to Owner home. Ctrl+] detaches; use the printed UUID with `--resume`. Ctrl+C goes to the remote application. Input, output and resize use one persistent pinned SSH connection.

Codex's own built-in SSH connections (Connections > SSH) are not supported by this tool. To work on a Target from Codex, use the Control Center MCP tools or `sshcc shell`.

## MCP / API

MCP names use underscores; JSON methods use dots. All take `targetId`.

| Tool | Additional parameters |
|---|---|
| `terminal_open` | `cwd?`, `shell?`, `ownerSessionId?`, `asAdmin?`, `rows?`, `cols?` |
| `terminal_read` | `terminalId`, byte `offset?`, `limit?` up to 1 MiB |
| `terminal_write` | `terminalId`, `sequence`, `contentBase64` up to 64 KiB decoded |
| `terminal_status` | `terminalId` |
| `terminal_resize` | `terminalId`, `rows`, `cols` (1–4096) |
| `terminal_close` | `terminalId` |

Jobs start and file methods accept `ownerSessionId` and `asAdmin`, and results report the actual executor. Jobs with the same ID and payload return the durable record without replay, even after logout/restart. Changing payload returns conflict.

Input sequences start at 1 and increase by one for new input. Retry identical bytes with the original sequence after uncertainty; responses include `ack` and `duplicate`. Input is never stored on disk. Output is an in-memory 4 MiB ring; readers receive `truncated` and actual offsets after eviction. Limits: 8 running terminals per Target, 30 minutes detached without access. MCP read/write/status refresh access. Service restart interrupts terminals. Only the authenticated creating Master can use them; revocation interrupts them.

Files keep atomic replacement, DACL/Unix metadata and stale hash checks; the service serializes mutations across workers. Use UNC paths for Windows shares. Test mapped drives and credentials in the actual elevated Owner context.

## Upgrade / rollback

Upgrade Master first; finish/cancel jobs and close terminals. Helpers offer to switch a Target to Owner session; advanced setup uses Windows `Setup.ps1 -ExecutionMode owner-session` or Ubuntu `setup.sh --execution-mode owner-session`. Owner/node/SSH keys remain unchanged.

After stopping the service, snapshots include binaries, config, fleet/jobs/terminals SQLite and sidecars. Rollback restores config with old binaries. Package manager dependency/package records are not a full OS snapshot; inspect diagnostics after failed transactions.
