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

**Tag**:
A free-text marker an operator attaches to a Target; one Target can carry many Tags, and they are used to filter lists and to choose a group of Targets for one action.
_Avoid_: label (that is a Target's or Master's display name), key-value label, selector

**Merge**:
One Master reaching another Master's Targets through it, possibly across several Masters in a row; on a team, each person is identified by their own Master.
_Avoid_: link, chain, user account (a person has no account of their own; their Master is their identity)

**Relay Path**:
The ordered list of Masters a request passes through, from the Master that issued it to the last Master before the Target.
_Avoid_: route (that is how one Master reaches a Target directly), hop list

**Merge Role**:
The rights one Master grants a Master that merged it: Full or View-only, optionally limited to chosen Tags; across a Relay Path the lowest role applies.
_Avoid_: permission, user role

**Audit Log**:
A Master's own JSON-lines record of each action that changes something, with its result and Channel; a record for the owner, not proof.
_Avoid_: history, event log (the job history is Recent Jobs)

**Recent Jobs**:
The list of jobs a Master started itself (one host or a group run), kept on that Master and shown on its Activity tab.
_Avoid_: job list (that is one Target's own list, read from the Target)

**Channel**:
The label a client reports for a request (`desktop`, `cli`, `mcp`), or `merge: <Master>` for a relayed one; recorded, never checked.
_Avoid_: source, origin, caller (the caller is the OS account the service checks)
