# Desktop connect does not overwrite existing MCP registrations

The Local service “เชื่อม” button creates an `ssh_control_center` MCP Registration only when that entry is absent. If the entry already exists, connect is a no-op; a separate “อัปเดต path” action is required when the stored command does not point at the installed `sshcc`. The Windows installer script `Configure-Codex.ps1` still updates Codex during setup so fresh installs keep a working path without asking the operator to open the desktop UI.
