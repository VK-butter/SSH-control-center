# Teams share Targets through merged Masters, not a login

A small team (2–5 people) shares Targets by each person running their own Master and merging the others' Masters (several hops allowed). They do not share one Master through an app login or OS-account list. A Master has no network door: the service listens only on a local pipe or socket and trusts the OS caller check. A login would add a network-facing attack surface and password storage to a tool whose core promise is that it has neither. So the person's identity is their Master. Rights for relayed access are a per-merge **Merge Role** (Full / View-only, optionally limited to Tags). Each relaying Master enforces it, and the effective right is the lowest along the **Relay Path**. Each relaying Master writes the full path to its own audit log, so Targets need no update.

## Consequences

- A Master paired directly with a Target always has full rights there. Merge Roles govern only relayed access.
- The AI agent on a Master has that Master's full power. The audit log's channel field (desktop / cli / mcp) is a label the client reports, not a security check.
- Every Master on a Relay Path must run a version that understands paths and roles.
