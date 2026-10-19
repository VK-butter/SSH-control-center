# MCP tool limits and tuning notes (static review, nothing measured)

**In plain words.**

- Nothing on this page was timed. It is a reading of the code, not a benchmark.
- The tools suit reading, searching and editing files by absolute path, and running background scripts, on machines on the same LAN. They are a poor fit for fast back-and-forth, because every request opens a new SSH connection.
- The rest of the page is for developers: sizes, limits and `file:line` references that will drift as the code changes.

This is a static reading of the Rust under `crates/core/src` and the Codex installer script. No wall-clock time was measured. Byte counts below are arithmetic from constants in the code. Poll counts are the sleep schedule with instant status calls, not a LAN timing. The `file:line` references describe the tree as written and will drift as the code changes; search for the named function if a line no longer matches.

## Facts from the source

### What the server exposes

`mcp::tools` builds 21 tools, then appends the six agent tools. That is 27 names on every `tools/list`.

The 21 are `pairing_create`, `pairing_status`, `pairing_complete`, `targets_list`, `targets_doctor`, `jobs_start`, `jobs_list`, `jobs_status`, `jobs_logs`, `jobs_cancel`, `files_list`, `files_read`, `files_write`, `files_mkdir`, `files_remove`, `terminal_open`, `terminal_read`, `terminal_write`, `terminal_status`, `terminal_resize`, `terminal_close` (`crates/core/src/mcp.rs:9-31`). The six are `files_search`, `files_grep`, `files_read_text`, `files_edit`, `files_write_text`, `exec_run` (`crates/core/src/agent_tools.rs:38-81`). They are chained at `crates/core/src/mcp.rs:60`.

`initialize` advertises `tools.listChanged: false` (`crates/core/src/mcp.rs:103`). The server returns the full list only when the client calls `tools/list` (`crates/core/src/mcp.rs:111`). It does not attach schemas to `tools/call` results. `tools/call` does rebuild the 27 schemas in-process to check the name (`crates/core/src/mcp.rs:114`). This repository does not record whether Codex, Cursor, or Claude Code then place those 27 schemas in every model turn.

Agent-tool results send only the `text` field (`crates/core/src/mcp.rs:128-131`). The other 21 tools send the whole JSON both as a text blob and as `structuredContent` (`crates/core/src/mcp.rs:134-135`). A JSON-RPC line over 8 MiB is rejected (`crates/core/src/protocol.rs:5`, `crates/core/src/protocol.rs:74-76`, `crates/core/src/mcp.rs:75-76`). There is no second cap inside `mcp.rs` on the `text` string beyond that frame and whatever the tool already put in `text`.

The server handles one stdio request at a time (`crates/core/src/mcp.rs:63-151`). `exec_run` sleeps inside that call (`crates/core/src/agent_tools.rs:408-419`), so the same MCP process does not accept another tool call until the wait returns.

The initialize instructions tell the model to doctor once, then search, grep, read text, edit, and `exec_run` (`crates/core/src/mcp.rs:62`). They do not say to copy `ownerSessionId` onto later calls. On the six agent tools the field exists and is optional: `required` starts with `targetId` only (`crates/core/src/agent_tools.rs:24-36`).

### 1. Known limitations

These are properties of the current code, not a wish list.

**Search and grep do not exist on an old Target.** `files_search` and `files_grep` forward `files.search` / `files.grep` (`crates/core/src/agent_tools.rs:347`, `crates/core/src/agent_tools.rs:372`). The Master maps those to `fs.search` / `fs.grep` (`crates/core/src/fleet.rs:749-750`). This tree's bridge allow-list includes them (`crates/core/src/service.rs:213-214`) and `files::handle` runs them (`crates/core/src/files.rs:42-43`). An older bridge that answers `method is not available over bridge`, `unknown filesystem method`, or `unsupported method files.` is rewritten to `E_TARGET_OUTDATED` (`crates/core/src/agent_tools.rs:318-326`). The header comment states the split: text and `exec_run` are composed from existing Master requests; search and grep need the Target-side methods (`crates/core/src/agent_tools.rs:1-3`).

**A line read still moves the whole file.** `files_read_text` calls `read_all` before it slices lines (`crates/core/src/agent_tools.rs:173-177`). `read_all` loops `files.read` in 1 MiB chunks until `eof` (`crates/core/src/agent_tools.rs:12`, `crates/core/src/agent_tools.rs:120-152`). The default window is 200 lines and the maximum is 500 (`crates/core/src/agent_tools.rs:13-14`, `crates/core/src/agent_tools.rs:172`). The model then sees at most 256 KiB of line text (`crates/core/src/agent_tools.rs:15`, `crates/core/src/agent_tools.rs:187-198`). Files over 4 MiB are rejected, but only after the first chunk has already been fetched (`crates/core/src/agent_tools.rs:11`, `crates/core/src/agent_tools.rs:128-132`). Each Target `fs.read` hashes the entire file, then returns the requested window (`crates/core/src/files.rs:121-138`). Chunk size on the Target is also 1 MiB (`crates/core/src/files.rs:12`, `crates/core/src/files.rs:128`).

**An edit rewrites the whole file.** `files_edit` reads the whole file, requires exactly one match, and `files.write`s the full new bytes as base64 with the snapshot hash (`crates/core/src/agent_tools.rs:231-260`). The Target write is atomic and capped at 4 MiB decoded (`crates/core/src/files.rs:44-94`). There is no hunk or range write.

**`exec_run` polls over a new SSH connection each time.** See the transport facts below. After the wait it still reads logs from offset 0, up to 2 MiB per stream, in 1 MiB chunks, then keeps the last 32 KiB of what it got (`crates/core/src/agent_tools.rs:18-19`, `crates/core/src/agent_tools.rs:422-423`, `crates/core/src/agent_tools.rs:455-488`). For a log longer than 2 MiB that is the last 32 KiB of the first 2 MiB, while the text says `last 32 KiB` (`crates/core/src/agent_tools.rs:443`). `jobs.logs` has no file size, only `nextOffset` and `eof` (`crates/core/src/jobs.rs:442-443`), so this path cannot seek to the real end. The job record's `stdoutTruncated` / `stderrTruncated` flags (`crates/core/src/jobs.rs:300-301`, `crates/core/src/jobs.rs:367-371`) are not copied into the `exec_run` text.

**Owner session can be omitted.** When execution mode is `owner-session` and more than one session is logged in, `select` auto-picks an active or newest eligible session unless `ownerSessionId` is passed (`crates/core/src/executor.rs`). That selection runs for owner-mode filesystem calls (`crates/core/src/service.rs:345-346`), for `jobs.start` (`crates/core/src/jobs.rs:157-158`), for opening a terminal (`crates/core/src/terminal.rs:76`), and inside doctor (`crates/core/src/executor.rs`). `jobs.status` and `jobs.logs` only read SQLite (`crates/core/src/service.rs:326-336`) and do not select a session. On an old Target, `fs.search` fails the allow-list before selection (`crates/core/src/service.rs:227-229`), so search can return `E_TARGET_OUTDATED` even when a session id was passed.

**Doctor's top-level `ok` ignores executor readiness.** After SSH, identity, and SYSTEM/root checks succeed, doctor returns `ok: true` even when the executor stage is not ready (`crates/core/src/fleet_ssh.rs:456-462`). Zero logins or (on Windows) no elevated session set `executor.ready` false on that stage.

**A job id is the whole command, including the session.** The stored payload includes script, cwd, shell, env, `ownerSessionId`, and `asAdmin` (`crates/core/src/jobs.rs:129-155`). The same id with a different payload is `E_CONFLICT`. The Master stores the same document and rejects a mismatch before treating it as a retry (`crates/core/src/fleet.rs:762-785`). `exec_run` always calls `jobs.start` first (`crates/core/src/agent_tools.rs:402-411`). A follow-up wait has to repeat every one of those fields.

**The coding tool list still carries pairing, terminals, and base64 file IO.** `files_read` is base64 (`crates/core/src/mcp.rs:21`). `files_list` returns up to 10,000 entries in one JSON array with no page token (`crates/core/src/files.rs:22-36`). `jobs_list` returns up to 1,000 full records (`crates/core/src/jobs.rs:91-95`). `terminal_read` defaults to 64 KiB of base64 and allows 1 MiB (`crates/core/src/terminal.rs:231`). Terminal bytes live in a 4 MiB memory buffer (`crates/core/src/terminal.rs:1`, `crates/core/src/terminal.rs:20-24`, `crates/core/src/terminal.rs:131-133`). `files_remove` deletes one file or an empty directory (`crates/core/src/mcp.rs:24`, `crates/core/src/files.rs:106-116`). There is no rename. The shell on a Target is only bash on Unix and only PowerShell on Windows (`crates/core/src/jobs.rs:110-116`). Scripts over 1 MiB are rejected (`crates/core/src/jobs.rs:104-106`). The visible job record keeps 4,096 characters of the script (`crates/core/src/jobs.rs:137`).

**Search, once the Target has it, is still a bounded walk.** One request, 20 second budget, 200,000 entries, skip list of eight directory names, symlinks not followed, default 100 paths / 50 grep hits, caps 1,000 and 500 (`crates/core/src/search.rs:12-26`, `crates/core/src/search.rs:28-43`, `crates/core/src/search.rs:89-142`). Grep reads each file up to 4 MiB, skips a NUL in the first 8 KiB, and clips lines at 400 characters (`crates/core/src/search.rs:67-75`, `crates/core/src/search.rs:144-156`). The regex size limit is 1 MiB (`crates/core/src/search.rs:46-49`). Brace globs expand one group (`crates/core/src/search.rs:159-179`). The owner worker is killed at 30 seconds (`crates/core/src/executor.rs:165-178`), which is why the walk stops at 20 (`crates/core/src/search.rs:12-13`).

### 2. Transport and stability

**One SSH connection per request.** The module header says one JSON request per SSH connection, and the Target service owns long-running work (`crates/core/src/fleet_ssh.rs:1`). Every fleet method ends in `transport::request` (`crates/core/src/fleet.rs:786-791`). Each attempt writes a new config and `known_hosts`, runs `ssh`, and deletes both files (`crates/core/src/fleet_ssh.rs:19-28`, `crates/core/src/fleet_ssh.rs:61-104`, `crates/core/src/fleet_ssh.rs:339-355`). The generated config sets `ControlMaster no`, `ControlPath none`, `ControlPersist no`, `ConnectTimeout 8`, `ConnectionAttempts 1`, `ServerAliveInterval 5`, `ServerAliveCountMax 2` (`crates/core/src/fleet_ssh.rs:97`). The client kills the SSH process after 45 seconds (`crates/core/src/fleet_ssh.rs:17`, `crates/core/src/fleet_ssh.rs:184-207`). A failed connect can try the next route; a host-key failure stops (`crates/core/src/fleet_ssh.rs:359-360`, `crates/core/src/fleet_ssh.rs:394-412`). No elapsed time for a handshake was measured.

**Host keys are pinned.** Enrollment requires 1 to 16 keys, accepts Ed25519, ECDSA NIST P-256/384/521, and RSA of at least 2048 bits, and rejects other algorithms (`crates/core/src/fleet.rs:156-202`). Each connection writes those keys into that call's `known_hosts` under a host alias (`crates/core/src/fleet_ssh.rs:70-77`, `crates/core/src/fleet_ssh.rs:100`). The config sets `StrictHostKeyChecking yes`, `UpdateHostKeys no`, `GlobalKnownHostsFile none`, and `CheckHostIP no` (`crates/core/src/fleet_ssh.rs:97`). A mismatch string becomes `HOST_KEY_MISMATCH` and does not fall through to another route (`crates/core/src/fleet_ssh.rs:231-244`, `crates/core/src/fleet_ssh.rs:359-360`). Updating a registration with different keys is refused until the old registration is removed (`crates/core/src/fleet.rs:364`). `CheckHostIP no` means the pin is the key material, not an extra IP binding in `known_hosts`.

**Jobs survive SSH disconnect and do not survive a Target restart or a dead owner session.** Status is SQLite with WAL and `synchronous=FULL` (`crates/core/src/jobs.rs:37-41`). The same `jobId` and the same payload return the existing record (`crates/core/src/jobs.rs:151-155`). The process runs on the Target after the SSH request returns (`crates/core/src/fleet_ssh.rs:1`, `crates/core/src/jobs.rs:171-189`). On service start, queued and running rows are marked `interrupted` and are not resumed (`crates/core/src/jobs.rs:50-66`). In owner mode, if the bound session ends or the service stops, the job is `interrupted` with message `Bound Owner session ended or service stopped` (`crates/core/src/jobs.rs:341-407`). Cancel sets a flag on an in-memory map (`crates/core/src/jobs.rs:413-417`). Each log stream stops being written at 64 MiB (`crates/core/src/jobs.rs:22`, `crates/core/src/jobs.rs:491-508`). The instructions say no project locks are enforced (`crates/core/src/mcp.rs:62`).

**Linux lists every eligible logind session.** Sessions for the configured owner UID that are `active` or `online` and class `user` or `user-early` are enumerated (`crates/core/src/executor_linux.rs:33-72`). Zero sessions is `E_OWNER_NOT_LOGGED_IN`. One or more sessions without an id auto-pick active-then-newest (`crates/core/src/executor.rs`). The Windows elevated-token filter is `cfg(windows)` only. On Unix, `asAdmin: true` runs the worker as uid 0. Each owner-mode filesystem call spawns a worker, waits up to 15 seconds for attestation, then kills the worker if the work exceeds 30 seconds.

**Illustrative scenario (not a recorded run).** Take a Linux Target called `build-server`, in owner-session mode, with more than one login session. Omitting `ownerSessionId` auto-picks; passing one pins it. If its installed binary predates `fs.search` / `fs.grep`, write, read, edit, and `exec_run` still work through the composed tools, while search and grep return `E_TARGET_OUTDATED`, which this code produces only when the Target rejects those methods (`crates/core/src/agent_tools.rs:318-326`).

**Summary.** The tool set can control an enrolled machine on the same LAN for absolute-path reads, exact-text edits, and background scripts. Pass `ownerSessionId` when a stable login binding is required; otherwise the Target auto-picks. Treat a lost Target service or a logged-out owner as an interrupted job. It is a poor fit for unattended search, for low-latency back-and-forth, and for any client that drops the MCP call while `exec_run` is still inside its wait. Host-key pinning is the stable part of the transport. Connection setup is the unstable part: every chunk and every status poll pays for a new SSH session with multiplexing turned off.

### 3. Performance and token bounds

No round trip was timed. The quantities below are in the source.

| Bound | Value | Where |
| --- | --- | --- |
| Read chunk | 1 MiB raw | `agent_tools.rs:12`, `files.rs:12` |
| Base64 of one full chunk | 1,398,104 characters (`4 * ceil(1048576 / 3)`) | size arithmetic, not a captured payload |
| Text tools | 4 MiB decoded | `agent_tools.rs:11`, `files.rs:52-53` |
| Line window returned to the model | default 200, max 500 lines, and 256 KiB | `agent_tools.rs:13-15`, `172`, `187` |
| `exec_run` wait | default 30 s, max 120 s | `agent_tools.rs:16-17`, `398-401` |
| Status sleep | 250 ms, then double, cap 2 s | `agent_tools.rs:411-418` |
| SSH kill | 45 s | `fleet_ssh.rs:17`, `189-206` |
| Connect timeout in ssh config | 8 s, 1 attempt | `fleet_ssh.rs:97` |
| Log fetch before the 32 KiB tail | 2 MiB per stream | `agent_tools.rs:18-19`, `464-488` |
| Log stored on the Target | 64 MiB per stream | `jobs.rs:22` |
| Grep line in the result | 400 characters | `search.rs:16`, `151-156` |
| MCP / IPC frame | 8 MiB | `protocol.rs:5` |
| SSH request body | 8 MiB minus 1 KiB | `fleet_ssh.rs:16`, `133-136` |
| Codex installer in this tree | `tool_timeout_sec = 150`, `startup_timeout_sec = 20` | `installers/windows/Configure-Codex.ps1:24-25` |

`files_read_text` of a file of size `S` (`S <= 4 MiB`) issues `ceil(S / 1 MiB)` `files.read` calls. Each call is its own SSH session and, in owner mode, its own worker, and each one re-hashes the whole file (`crates/core/src/files.rs:129-137`, `crates/core/src/service.rs:345-346`, `crates/core/src/executor.rs:153-155`). A 200-line request does not reduce that. A file under 1 MiB is one full-file transfer. A 4 MiB file is four.

`files_edit` and `files_write_text` add one `files.write` of the entire contents as base64 (`crates/core/src/agent_tools.rs:255-260`, `crates/core/src/agent_tools.rs:299-306`). The model sends `files_write_text` content as raw text up to 4 MiB (`crates/core/src/agent_tools.rs:295-298`). The model sends `files_edit` only `oldText` and `newText`; the Master still uploads the rewritten file.

`exec_run` sets the deadline before `jobs.start`, then sleeps and calls `jobs.status` (`crates/core/src/agent_tools.rs:408-419`). With instant status calls, the 30 second default produces 18 status calls: sleeps of 0.25 + 0.5 + 1 + (2 × 13) + 0.25 seconds. The same schedule over 120 seconds is 63 status calls: 0.25 + 0.5 + 1 + (2 × 59) + 0.25. Real SSH time reduces how many polls fit. The status call is started after a sleep that was allowed to run, and that call is not cancelled when `timeoutSeconds` expires; the SSH client itself dies at 45 seconds (`crates/core/src/fleet_ssh.rs:184-207`). Log reads run after the deadline (`crates/core/src/agent_tools.rs:421-423`). So the MCP call can outlive `timeoutSeconds` by one in-flight SSH (at most 45 seconds in this code) plus the log reads. That is a bound, not a measured overrun.

The installer writes `tool_timeout_sec = 150` and `startup_timeout_sec = 20` only when `Configure-Codex.ps1` from this tree is applied (`installers/windows/Configure-Codex.ps1:21-36`); a Codex `config.toml` that script has not touched keeps whatever it already says. The MCP process does not open SSH during `initialize` (`crates/core/src/mcp.rs:101-103`); SSH starts on the first forwarded call (`crates/core/src/mcp.rs:121-126`, `crates/core/src/client.rs:7-11`).

Token shape, from what is actually returned:

- A text read the model keeps is at most 256 KiB plus a one-line header (`crates/core/src/agent_tools.rs:205-220`). The bytes discarded on the Master still crossed SSH as base64.
- An edit result the model keeps is one line (`crates/core/src/agent_tools.rs:264-265`).
- `exec_run` keeps at most 32 KiB of stdout and 32 KiB of stderr (`crates/core/src/agent_tools.rs:481-488`).
- Grep keeps at most 500 lines of 400 characters (`crates/core/src/search.rs:16`, `crates/core/src/search.rs:55`).
- The non-agent `files_read`, `files_list`, `jobs_list`, and `terminal_read` put the full JSON in the text channel and again in `structuredContent` (`crates/core/src/mcp.rs:134-135`).
- All 27 schemas are one `tools/list` payload (`crates/core/src/mcp.rs:111`). Pairing and terminal schemas are in that payload even for a coding turn. Serialized token size was not measured.

## Tuning recommendations

These are changes in how agents call the tools, or possible code changes. They are not extra measurements.

Use the six text tools and skip the base64 ones. Prefer the auto-picked `executor.selected.ownerSessionId` from doctor when you need a stable binding across `files_*` and `exec_run`, and repeat that same id when waiting on a job. Keep `timeoutSeconds` at 30 unless the client tool timeout is known to be 150. Apply `installers/windows/Configure-Codex.ps1` from this tree before relying on 150. Do not call `files_search` or `files_grep` on a Target whose binary predates them (it answers `E_TARGET_OUTDATED`) until that binary is replaced. Read `executor.ready` and the session list, not only doctor `ok`.

After the Target binary includes `fs.search` and `fs.grep`, one search or grep replaces a tree of `files_list` plus whole-file reads. The 20 second walk is inside the 30 second worker limit (`crates/core/src/search.rs:12-13`, `crates/core/src/executor.rs:165-178`).

## Tuning checklist

**Using the tools well**

1. On an owner-session Target such as `build-server`, omit `ownerSessionId` to auto-pick, or pass a pinned id from `executor.selected` / the session list on `files_*`, `exec_run`, and `jobs_start`. Read `executor.ready` in the doctor result (`crates/core/src/fleet_ssh.rs:456-462`, `crates/core/src/executor.rs`).
2. Stop calling `files_search` and `files_grep` on a Target that returns `E_TARGET_OUTDATED` until it is upgraded. Use known absolute paths with `files_read_text` and `files_edit`.
3. Run `installers/windows/Configure-Codex.ps1` from this tree so Codex `tool_timeout_sec` becomes 150. Leave `exec_run` at the 30 second default. Resume with the same `jobId`, script, cwd, and `ownerSessionId` (`crates/core/src/jobs.rs:151-155`).
4. Do not use `files_read`, `files_list` on large directories, `jobs_list`, or `terminal_*` for ordinary coding. They return base64 or unbounded JSON, and the 21 non-agent tools duplicate that payload (`crates/core/src/mcp.rs:134-135`).

**When deploying Targets**

5. Install the Target binary that allow-lists and implements `fs.search` and `fs.grep` (`crates/core/src/service.rs:213-214`, `crates/core/src/files.rs:42-43`). Until that binary is on the machine, those two tools stay `E_TARGET_OUTDATED`.

**Possible code changes** (ideas for contributors; nothing here is promised)

6. Reuse one SSH session. `ControlMaster no` plus one request per connection makes every 1 MiB chunk and every status poll pay a handshake (`crates/core/src/fleet_ssh.rs:1`, `crates/core/src/fleet_ssh.rs:97`).
7. Read and replace on the Target by range. `files_read_text` should stop after the requested lines, and `files_edit` should not upload the whole file. `fs.read` should not hash the entire file once per chunk (`crates/core/src/files.rs:129-137`).
8. Tail logs from the end, and cap `exec_run` wall time so a 120 second wait plus a 45 second SSH plus log reads cannot exceed the client tool timeout.
9. Prefer the auto-picked `executor.selected.ownerSessionId` when a stable binding is needed, document that in the initialize instructions (`crates/core/src/mcp.rs:62`), and return doctor `ok: false` when `executor.ready` is false (`crates/core/src/fleet_ssh.rs:462`).
10. Offer a coding tool list without `pairing_*` and `terminal_*`, and return the non-agent tools' results once, not as both text and `structuredContent`.
