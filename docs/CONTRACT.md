# Implementation contract

**What this page is.** The exact shapes of the local service API, the SSH bridge protocol and the installer and CLI interface. **Who it is for:** developers who write a client or change the code. Operators do not need it. For plain-language pages, read [How it works](how-it-works.md) and [Architecture](architecture.md).

SSH Control Center is a Rust workspace (local service, CLI/MCP bridge, optional desktop tray) that lets an owner or an AI agent run commands, edit files and open terminals on Windows and Ubuntu Targets over OpenSSH. This document is the implementation contract for the local service API, the SSH bridge protocol and the installer/CLI interface. Installation is limited to operator-authorized machines.

## Core public Rust API
- sshcc_core::protocol::Request { method: String, params: serde_json::Value }
- sshcc_core::protocol::Response { ok: bool, result: Option<Value>, error: Option<RpcError> }; RpcError { code: String, message: String }
- sshcc_core::client::call(&Request) -> anyhow::Result<Value>: owner-authorized local IPC, SSHCC_DATA_DIR and SSHCC_SOCKET override for isolated tests.
- sshcc_core::config::Config (serde camelCase): schema_version u32, node_id String, label String, roles Vec<String>, owner String, allowed_principals Vec<String>.
- sshcc_core::config::load(&Path) -> Result<Config>; data_dir() -> PathBuf.
- sshcc_core::fleet::handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>>.
- fleet::authorized_master(state: &Path, master_id: &str) -> Result<bool>: consult enrolled master registry for target bridge authorization.
- fleet::register_master(state: &Path, master: &Value) -> Result<Value>: public master id/label/key only.
- fleet::revoke_master(state: &Path, master_id: &str) -> Result<Value>.
Core calls fleet::handle before its local runtime methods.

## Service API
system.status -> { nodeId, label, roles, version, uptimeSeconds, executionPrincipal, privileged }
targets.list -> array of targets
targets.import { registration: object } -> target
targets.import_legacy { document: object, proxyJump?: object } -> imported summary (version-1 registry import, `schemaVersion` 1: one record, an array, or a `hosts` map; fields are listed in AGENT.md)
targets.remove { targetId }
targets.update { target: object } -> target
targets.doctor { targetId } -> structured stages
enrollment.export { targetId, tenant, label?, routes, os, user } -> public bundle JSON (caller saves file)
enrollment.accept { bundle: object } -> master enrollment (installer only/local owner)
enrollment.registration { targetId, tenant, routes, os, user, hostKeys: string[] } -> registration
masters.list -> enrolled masters
masters.revoke { masterId } -> revoked
jobs.start { targetId, jobId?: UUID, script, cwd, shell?: powershell|bash, env?: object } -> job
jobs.list { targetId } -> array
jobs.status/logs/cancel { targetId, jobId, offset?: integer, limit?: integer, stream?: stdout|stderr }
files.list/read/write { targetId, path, offset?, limit?, contentBase64?, expectedSha256? }
files.mkdir/remove { targetId, path } (remove nonrecursive only)
All client-facing methods return either a Value result or a structured error.
job status fields: id, masterId, script, cwd, shell, status (queued/running/succeeded/failed/cancelled/interrupted), createdAt, startedAt, finishedAt, exitCode.
jobs.logs -> { content, nextOffset, eof }; files.read -> { contentBase64, size, sha256, nextOffset, eof }.
Execution uses the configured mode: logged-in Owner for schema 2 configurations, service mode for schema 1 configurations. Windows Owner is elevated; Ubuntu supports explicit asAdmin. See OWNER-SESSION.md for sessions, executor metadata and terminal APIs. Paths must be absolute, env values strings. File writes are atomic, preserve the existing Windows DACL or Unix owner/group/mode, and support optimistic hash checks; bytes and hash are read from the same open file. Mutating file requests are briefly serialized to enforce the hash precondition. Jobs remain concurrent.

## SSH/target protocol
Master invokes installed sshcc bridge over strict OpenSSH. Installer key forced command supplies --master-id UUID and --master-key SHA256:FINGERPRINT, binding the request to the authenticated public key. JSON line request {method,params}; response {ok,result,error}.
Transport wraps operation as method bridge.request, params {expectedTargetId: registration.nodeId, request:{method:"runner.start"|runner.list|runner.status|runner.logs|runner.cancel|fs.list|fs.read|fs.write|fs.mkdir|fs.remove|system.status, params:{...}}}.
CLI bridge adds its forced master identity and key fingerprint to the envelope, then calls local service. Service checks the exact active enrollment key, target identity and method allowlist, and injects authenticated masterId into runner.start. Revocation is checked each request.
Target owns jobs; stable job ID duplicates return same record only if payload matches. No automatic re-run after crash.
Known-host mismatch stops all fallback. Operational transport failures may try next registered route with same job ID.
Configurable ProxyJump must carry its own pinned keys, SSH user and service-owned identity. Never read ambient user SSH config.
Routes JSON: { kind:"lan"|"tailscale"|"public", address, port:22, proxyJump?:{address,port,user,hostKeys,identityFile} }.
Registration v2: {schemaVersion:2,id,nodeId,label,tenant,os,user,hostKeys,routes,bridgePath,runnerInstalled:true}.
Enrollment bundle v2: {schemaVersion:2,targetId,tenant,os,user,routes,master:{id,label,publicKey}}. No private key/credential exports.
Imports through targets.import_legacy are marked runnerInstalled:false until a validated registration replaces them.

## UI
Tauri IPC command rpc(method:String, params:Value) -> core client call (blocking work off UI thread).
No window opens at startup: the tray opens it on demand. Closing the window hides it, and the window process exits after the idle time (see [INSTALL.md](INSTALL.md#packages-and-roles)). Explicit tray Quit closes the tray but not the service.
UI: fleet/group filter, enrollment import/export, target routes + diagnostics, jobs + logs + cancel, masters, service status.
Native file picker: tauri-plugin-dialog.
With no service running, the window shows an error instead of data. Fixture data exists only in the UI test fixture ([UI-FIXTURE.md](../desktop/tests/UI-FIXTURE.md)).

## Installer/CLI interface
sshcc init --role master|target|both --owner OWNER --label LABEL [--allow-principal PRINCIPAL] [--data-dir PATH]
sshcc service run (foreground for dev/Linux); sshcc service windows (SCM)
sshcc request METHOD [--params JSON | --params-file PATH]
sshcc bridge --master-id UUID --master-key SHA256:FINGERPRINT (forced command)
sshcc mcp (stdio)
sshcc configure ... local offline operation for installer while service stopped: accept-bundle FILE; registration --spec FILE --output FILE.
Global --data-dir PATH and --socket PATH supported.
Installer never changes existing SSH settings or key lines except its own marked additions. Windows LocalSystem; Linux root systemd. Owner chosen before privilege elevation.

## Runtime bounds and environment
Local requests are bounded to 64 concurrent handlers and 8 MiB JSON frames. Unix peer credentials are checked before parsing. Job history queries return at most 1,000 records; script previews are truncated to 4,096 characters and stdout/stderr storage is capped at 64 MiB per stream. Protected service-env.json supplies explicitly provisioned environment values at service startup and rejects reserved SSHCC_* overrides. Diagnostics report whether the actual Target service runs as root/SYSTEM.

Windows replacement metadata follows [ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew); arbitrary Windows owner/SACL restoration is not claimed by the file API.

## Operator-approved pairing

`pairing.create` accepts `{label,tenant,os,address}` plus advanced `targetId,user,port,routeKind,routes`. It persists a request and returns `{requestId,targetId,label,tenant,os,state,createdAt,updatedAt,envelope,diagnostics}`. `pairing.status {}` returns requests; `{requestId,retry?,routes?}` returns one and can retry diagnostics with new routes while preserving identity/pins. `pairing.complete {code}` verifies and imports a matching receipt atomically, then runs diagnostics. States are `awaiting_approval`, `needs_attention`, `ready`; ready requires SYSTEM/root. Repeated identical receipts do not overwrite edited routes or create another Target.

The envelope `{schemaVersion:1,requestId,masterId,bundleDigest,targetLabel,bundle}` wraps unchanged enrollment v2. The receipt `{schemaVersion:1,requestId,masterId,bundleDigest,registration}` wraps unchanged registration v2. Code encoding is `SSHCC1.<base64url-no-padding compact UTF-8 JSON>.<SHA-256 hex JSON>` with a 64 KiB input cap and no compression. Checksum is corruption detection; trust begins with Target approval and operator transfer, followed by pinned SSH checks. No private key/token is embedded.

Installer-only local `system.install-begin/end` uses a shared runner admission lock, exact state directory/node identity and owner authorization. Bridge requests cannot acquire this fence. The installer refuses active jobs, then prevents new submission before stopping/replacing the service. A restart releases the in-memory fence.
