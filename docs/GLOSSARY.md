# Glossary

**What this page is.** The few terms that the code and the [ADR](adr/0001-mcp-registration-no-overwrite.md) use with one fixed meaning. **Who it is for:** developers and contributors. The everyday words (Master, Target, job, Owner and so on) are explained in the "Words used on this page" tables of [How it works](how-it-works.md), [Pairing](pairing.md) and the [security model](security-model.md).

SSH Control Center: independent Masters control Windows and Ubuntu Targets over OpenSSH, with a local service, CLI/MCP bridge, and optional desktop tray.

## Language

**MCP Registration**:
A user-level config entry named `ssh_control_center` that launches `sshcc mcp` over stdio for one AI client.
_Avoid_: plugin, connector, MCP library

**AI Client**:
An installed agent host that can load MCP servers: Cursor, Claude Code, or Codex.
_Avoid_: IDE, editor, tool (when meaning one of these three)

**Local Service**:
The machine-local SSH Control Center service that owns keys, jobs, and Target sessions on this host.
_Avoid_: desktop window, tray (those are separate UI processes)
