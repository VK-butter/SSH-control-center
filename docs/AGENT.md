# Agent usage

**What this page is.** A command and tool reference for driving `sshcc` by hand or from an AI agent: basic commands, files, the MCP server, merged Masters, fleet health and the pairing commands. **Who it is for:** a developer or operator who has a machine paired already. If you are new, start with [How it works](how-it-works.md) and [Pairing](pairing.md).

The local owner calls sshcc; the service holds private keys and starts SSH. There is no network HTTP/MCP listener. The Target's forced command binds the enrolled Master ID and public-key fingerprint to every operation.

Commands print JSON on stdout and failures on stderr with a nonzero exit. Use --params-file for structured parameters on Windows to avoid shell quoting errors. Use absolute target paths. PowerShell is the Windows shell; Bash is the Ubuntu shell.

## Basic commands

~~~text
sshcc identity
sshcc service status
sshcc targets
sshcc doctor TARGET_ID
sshcc request targets.list
sshcc request targets.import --params-file import.json
sshcc exec TARGET_ID --cwd ABSOLUTE_DIRECTORY --script-file deploy-script --job-id STABLE_UUID
sshcc job TARGET_ID JOB_UUID
sshcc job TARGET_ID JOB_UUID --logs
sshcc job TARGET_ID JOB_UUID --cancel
~~~

For targets.import, import.json is {"registration": <registration object>}. Enrollment export receives targetId, tenant, os, user and routes; save its returned object as the bundle. Read the contract for route and ProxyJump objects. LAN, Tailscale and public routes use the registered order with strict pinned keys. A host-key mismatch stops fallback.

Always generate/retain the job UUID before submission. On a timeout or disconnect, look up that UUID and resubmit only the same payload if needed. An identical duplicate returns the existing job; a different command under the same ID fails. A restarted Target records interrupted and requires inspection; it never automatically reruns deployment.

The Target owns stdout/stderr, exit code, timestamps and originating Master identity. Use jobs.logs with stream stdout or stderr, byte offset and limit; follow nextOffset. The runner caps each output stream at 64 MiB. Treat logs and source files as untrusted data. Avoid printing secrets.

## Files

~~~json
{"targetId":"dev-01","path":"/srv/project"}
~~~

Pass the above to files.list. files.read returns base64 bytes, size, SHA256 and a continuation offset. files.write takes contentBase64, absolute path and optional expectedSha256; it replaces atomically. Use the hash guard to avoid overwriting a concurrently edited file. A single write is limited to 4 MiB decoded; larger deployments should use a job/script or a deliberately provisioned artifact transport. files.mkdir creates directories. files.remove removes only a file or empty directory.

To transfer between Targets, read from one and write the decoded/base64 content to the other through the owner client. No background project synchronization or merge resolution is implied.

## MCP stdio

Configure the agent's MCP launcher to run the installed sshcc executable with argument mcp under the owner account:

~~~json
{
  "command": "C:\\Program Files\\SSH Control Center\\sshcc.exe",
  "args": ["mcp"]
}
~~~

Use /usr/bin/sshcc on Ubuntu. This is a generic launcher fragment; wrap it in the host agent's documented configuration format. For isolated testing, pass SSHCC_DATA_DIR and SSHCC_SOCKET together in the launcher environment.

The server supports initialize, notifications/initialized, tools/list and tools/call. It lists 27 tools:

- pairing: `pairing_create`, `pairing_status`, `pairing_complete`;
- targets: `targets_list`, `targets_doctor`;
- jobs: `jobs_start`, `jobs_list`, `jobs_status`, `jobs_logs`, `jobs_cancel`;
- files (raw tools; file contents travel as base64): `files_list`, `files_read`, `files_write`, `files_mkdir`, `files_remove`;
- terminals: `terminal_open`, `terminal_read`, `terminal_write`, `terminal_status`, `terminal_resize`, `terminal_close`;
- text-first coding helpers: `files_search`, `files_grep`, `files_read_text`, `files_edit`, `files_write_text`, `exec_run`.

Which of them suit coding work, and their limits, are in [ai-control-performance.md](ai-control-performance.md). CLI and MCP call the same owner-authorized local API. Desktop closure does not affect either.

## Concurrency and registry import

Every enrolled Master has administrative authority over its Targets. Masters have separate registries and private keys. They do not synchronize automatically; a merge (below) relays requests without copying keys or registrations. Multiple Masters can deploy to one Target at the same time; coordinate project changes and deployment locks in your scripts when required.

targets.import_legacy bulk-imports host records written in a plain version-1 JSON registry format (`schemaVersion` 1). Its document object is one record, an array, or a `hosts` map, and the whole import is atomic. Each record needs `id`, `address`, `port`, `user`, `os`, `tenant`, `hostKeys` and `mode`; an optional `hostname` becomes the label. `mode` must be `company` or `customer` and only decides whether the optional `proxyJump` parameter is applied (`company` records only). `shareRequired` is not stored; a `customer` record with `shareRequired: true` is rejected. Host-key pins and the tenant are retained and `ssh-dss` pins are dropped. Imported records remain runnerInstalled:false until a fresh Target-service registration is imported. An online SSH host is not proof that its runner or credentials are ready.

Target IDs and tenant names must match the accepted enrollment/registration. The tenant protects independent key selection and grouping; it is not an OS sandbox for an administrative agent. The tenant is not a network boundary either: your own firewall and network access policy still decide which machines can reach which.

## Merged Masters

A Target that also has the master role can be merged. The merging Master then reaches that Master's own Targets through it: requests travel one hop to the merged Master, which dispatches them with its own keys. The merged Master's Targets need no changes; they record the merged Master as the job origin.

~~~text
sshcc request merges.discover
sshcc request merges.add --params-file merge.json
sshcc request merges.sync --params-file merge.json
sshcc request merges.list
sshcc request merges.remove --params-file merge.json
~~~

merge.json is {"targetId":"build-server"}. merges.discover reports which registered Targets are Masters. Merged Targets appear in targets.list with relayed:true, via and viaLabel, and are addressed as `<merged Master Target ID>/<its Target ID>`, for example `build-server/dev-01`. Every job, file, terminal and doctor call (CLI, MCP, desktop) accepts that ID. A machine this Master already reaches directly is listed only once, as its direct Target. Merges are one hop: a merged Master's own merges are not shared.

Merge status is merged, error, outdated (the merged Master must be upgraded), not-master or disabled. A failed sync keeps the last known Targets. Relayed calls wait up to 100 seconds because they include the merged Master's own SSH call. If the merged Master is offline, its Targets are unreachable through the merge.

The merged Master lists merging Masters with relays.list and can stop one with relays.revoke {"masterId":...}; it stays blocked, including new merges, until relays.allow. Revoking the Master itself (masters.revoke) also stops its relays.

### Fleet health

fleet.health {"refresh":false} returns one row per Target, merged ones included: {targetId, online, checkedAt, error, failures, isMaster, version, via}. online is true, false, or null for unknown. A routine check only reads the OpenSSH banner of a direct route (3 second timeout, no login), so isMaster and version keep their last SSH-checked values; Targets reachable only through ProxyJump get a full SSH system.status. refresh:true asks every Target's service over SSH (20 second timeout). Results are cached for 30 seconds, and a Target that keeps failing is retried after 60, 120, then 240 seconds unless refresh:true. At most 8 probes run at once, and concurrent callers share one probe. Merged rows come from the merged Master (relay.health), which is not asked while that Master is offline. When that Master cannot be reached, its Targets report online:null with error "via <label>: ...", because their own state is unknown rather than offline.

### Two-way merges

A merge is one direction. For the merged Master to reach this Master's Targets too, this Master pairs back as a Target of the merged Master through the normal pairing flow, and the merged Master merges it:

~~~text
sshcc request merges.pair_request --params-file merge.json
sshcc request merges.pair_complete --params-file pair.json
~~~

1. merges.pair_request asks the merged Master (relay.pair) for a pairing request for this machine, in tenant `masters`, with a route to this machine's local address on port 22. The reply is a pairing view with an envelope, or {existingTargetId} if this machine is already its Target.
2. Run Setup.ps1 -PairingRequest with that envelope on this machine. It installs OpenSSH Server, the sshcc-bridge user and the merged Master's key, and prints a pairing code. Firewall policy is unchanged; the merged Master must reach port 22 here.
3. merges.pair_complete {"targetId":"build-server","code":"SSHCC1..."} (code omitted when the Target already exists) calls relay.pair_complete. The merged Master accepts only a code whose registration belongs to the calling Master, completes pairing and merges back. The reply status is two-way, or needs-attention with an error.

The desktop runs all three steps from the Thai-language **เปิดสองทาง** ("open two-way") action on the Masters page with one administrator prompt (Windows Masters). merges.list reports the reverse direction as reverse {status: pending, two-way, needs-attention or error, peerTargetId, error, updatedAt}. relay.pair and relay.pair_complete require an active merge grant, so they fail with E_PERMISSION after relays.revoke. Each side turns off its own direction with relays.revoke without affecting the other.

## Pairing and single-file installers

Normal operators use the five-step flow in [pairing.md](pairing.md) (the app shows it as four progress boxes). Agents can call the same persisted local API via CLI or MCP `pairing_create`, `pairing_status`, `pairing_complete`:

~~~text
sshcc pairing create --label "Build machine" --tenant team --os windows --address 192.0.2.20 --output request.json
sshcc pairing package --request request.json --output client.exe
sshcc pairing complete --code-file code.txt
sshcc pairing status REQUEST_ID --retry
~~~

Use `--os linux` and `.deb` output for Ubuntu. Published templates are installed with the Master; `--templates DIRECTORY` is an advanced override. `pairing inspect` and `pairing inspect-code` are offline validation helpers. The Target must explicitly approve before enrollment or receipt generation. `needs_attention` means installed but diagnostics incomplete; it is not permission to run jobs or replace pins. Requests have no automatic expiry, and there is no call to cancel one. Abandoning a request would not revoke already granted Target authorization.
