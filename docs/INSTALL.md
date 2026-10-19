# Install and pair machines

**What this page is.** The install, pairing, upgrade and uninstall reference for operators. It says what the installers put on a machine, how to build the packages, and how to install, upgrade and remove them. **Who it is for:** someone who has read the [README](../README.md) and is ready to try the tool on a disposable VM. For a guided walk through adding a machine, read [pairing.md](pairing.md) (English) or [QUICKSTART.th.md](QUICKSTART.th.md) (Thai). Who commands run as (the Owner account) is explained in [Which account runs the work](#which-account-runs-the-work) below and in [OWNER-SESSION.md](OWNER-SESSION.md).

Install into a disposable VM first. These scripts deliberately configure a privileged service; merely building or parsing them does not verify installation.

## Packages and roles

Role master stores the fleet and starts outgoing SSH sessions; target accepts enrolled Masters; both combines them. The owner is the OS account whose agent, CLI and tray may use the local service. Select it before elevation. The dedicated sshcc-bridge account is separate, non-administrative, and may use only the authenticated bridge interface.

Windows binaries are installed in C:\Program Files\SSH Control Center: sshcc.exe, and for desktop packages sshcc-desktop.exe plus sshcc-window.exe. Closing the management window hides it so the next tray click reopens it instantly; after 10 idle minutes (SSHCC_WINDOW_IDLE_SECS, 0 = exit on close) the separate window process exits and releases its WebView runtime; the resident tray remains. Private state is C:\ProgramData\SSHControlCenter. The SSHControlCenter service starts as LocalSystem at boot. Target setup creates a dedicated local bridge account when missing; its generated password is not exported. Public authorization lives under C:\ProgramData\ssh\sshcc and an owned Match User block in sshd_config.

Ubuntu binaries are /usr/bin/sshcc and, in the optional desktop package, /usr/bin/sshcc-desktop plus /usr/bin/sshcc-window. The small GTK/AppIndicator tray launches the Tauri window process on demand; closing management hides the window, and the window process exits after the same idle period so its WebKit runtime can be reclaimed. Private state is /var/lib/sshcc. Root systemd service sshcc uses /run/sshcc/control.sock. Public bridge keys live under /etc/ssh/sshcc and the managed block is /etc/ssh/sshd_config.d/90-sshcc.conf. Owner authorization uses a numeric UID; Windows uses a SID.

Intended Target baseline: Windows 10/11 x64 (Windows 10 needs OpenSSH Optional Features), Windows Server 2019/2022/2025, Ubuntu 24.04 x64. The repository's automated tests do not cover every edition listed here; see [VALIDATION.md](VALIDATION.md). Server Core uses service/CLI. Desktop dependencies are optional on Ubuntu. Tailscale is standard IP transport: install/sign in using its ordinary workflow, keep your own ACL/firewall policy, then add the Tailscale address as a route.

## Build packages

A machine that only runs an installed package does not need Rust, NSIS or Linux build tools. Whoever builds the packages needs Rust 1.90+, native compilers, NSIS on Windows and Tauri desktop dependencies on Ubuntu 24.04. A full Master package needs both a Windows build host and an Ubuntu 24.04 x86_64 build host, because both full Master installers embed both thin templates (steps 3 and 5 copy files between the two hosts). This repository ships no prebuilt installers: you build them with the steps below, and they are written to `dist/`. Build thin templates first to avoid recursively embedding installers:

1. Windows: `scripts/build-windows.ps1 -Desktop -ThinTemplateOnly -NsisPath PATH_TO_MAKENSIS`.
2. Ubuntu: `bash scripts/build-deb.sh --desktop`; this emits service/desktop DEBs and `sshcc-client-ubuntu-template.deb`.
3. Put both `sshcc-client-windows-template.exe` and `sshcc-client-ubuntu-template.deb` in one release template directory. Run `python scripts/build-templates.py --directory DIRECTORY` to create `template-manifest.json`.
4. Windows full Master: `scripts/build-windows.ps1 -Desktop -TemplatesDir DIRECTORY -NsisPath PATH_TO_MAKENSIS`.
5. Copy the same two frozen thin files and manifest into the `dist/` directory that the Ubuntu build writes to. Ubuntu full Master helper: `bash scripts/build-deb.sh --desktop --setup --skip-build`.

A published thin template must stay byte-identical after generating its manifest. Both native full Master installers include the two thin templates; the Rust package builder personalizes either OS without invoking native build tools. Check SHA256SUMS and package contents. Current binaries are unsigned; Windows prerequisites use online Optional Features and the signed Microsoft WebView2 installer. No Tailscale login or firewall policy is changed.

## Normal installation and pairing

The generic installers named below, `sshcc-0.4.0-windows-x64-setup.exe` and `sshcc-0.4.0-ubuntu-setup.deb`, are the files that the "Build packages" steps write to `dist/`. No prebuilt downloads are provided. For a guided walkthrough, follow [pairing.md](pairing.md) (English) or [QUICKSTART.th.md](QUICKSTART.th.md) (Thai).

On Windows open the generic `sshcc-0.4.0-windows-x64-setup.exe` in the intended Owner's session and choose “ใช้เครื่องนี้เป็นเครื่องหลัก” (Use this machine as the Master). The window has a box “ใช้ Owner session (administrator) สำหรับ jobs, files และ terminal” that is ticked on a fresh install. With it ticked (the default Owner-session mode), the account that opens the file must itself be an administrator, because the Owner needs an elevated token. A different administrator can supply the UAC password only when you clear that box (service mode). Either way, the account that opened the file is the one recorded as Owner. Reinstallation retains the existing owner and node identity.

On Ubuntu install the generic `sshcc-0.4.0-ubuntu-setup.deb`, then open **SSH Control Center Setup** from the application menu. Installing the helper alone does not install/start the actual service or add Master SSH authority. Approval runs the privileged worker. Desktop dependencies/tray are optional; a Server uses the same helper in a terminal:

~~~bash
sudo apt install --no-install-recommends ./sshcc-0.4.0-ubuntu-setup.deb
sshcc-setup --terminal
~~~

On the Master choose **เพิ่มเครื่อง**, enter name/group/OS/address and save the single tailored EXE/DEB. Open it on the Target, verify the controlling Master and **Approve**, then copy the whole `SSHCC1...` code. Paste it on the Master. If diagnostics fail, the installed Target stays “ติดตั้งแล้ว—รอตรวจการเชื่อมต่อ”; edit its address and retry without reinstallation.

Windows Server without a desktop launches the same tailored executable using its terminal helper:

~~~powershell
.\client.exe /CONSOLE
~~~

On Ubuntu Server install the tailored helper DEB with `--no-install-recommends`, then run `sshcc-setup --terminal` from a real terminal. Type `APPROVE` when requested; piped approval is rejected. Reopening the same helper validates the saved receipt against the live service before allowing another copy.

Approval explicitly grants agents on the named Master command execution and file access on the Target: as the Owner account for owner-session installations (Windows uses the Owner's elevated token; Ubuntu uses the Owner UID/GID, or root when `asAdmin` is requested), or as SYSTEM/root for installations still in service mode. See [OWNER-SESSION.md](OWNER-SESSION.md). The returned code contains public registration and request bindings; its checksum detects damaged copy/paste and is not a signature. Transfer it through a trusted channel. Mismatched Master/request, identity or existing host-key pins are rejected. Multiple Masters have independent keys and remain authorized separately.

Master setup adds `ssh_control_center` to detected Codex configuration under the owner account, with a backup and preservation of other MCP entries. Start a new Codex session to load the tools.

## Which account runs the work

The short version, in the order a newcomer needs it:

- New jobs, files and terminals run as the logged-in installing Owner. A Target configured in service mode (config schema 1) stays in it until the installer selects Owner session.
- On Windows the Owner must already be an administrator. On Ubuntu, normal work uses the Owner's UID and GID, and an explicit admin request uses root. Details: [OWNER-SESSION.md](OWNER-SESSION.md).
- `sshcc shell <Target>` opens an interactive terminal. `sshcc doctor` lists the Owner sessions and the CLI readiness.
- When several Owner logins exist, the Target auto-picks an active or newest eligible session unless `ownerSessionId` (or `--session` on the CLI) pins one.
- Finish jobs and close terminals before upgrades. Snapshots restore binaries, config and databases together.

## Advanced file enrollment

The public-bundle/registration file flow remains available for advanced use. Targets enrolled this way need no new pairing:

~~~powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Setup.ps1 -Role target -EnrollmentBundle C:\Setup\bundle.json -RegistrationOut C:\Setup\registration.json
~~~

~~~bash
sudo apt install ./sshcc_0.4.0_amd64.deb
sudo /usr/lib/sshcc/setup.sh --role target --owner "$USER" --bundle ./bundle.json --registration ./registration.json
~~~

Import registration on the Master and run doctor before the first job. Re-running setup preserves owner, node identity, prior roles and allowed Masters. New enrollment changes only that Master's owned key line. Existing accepted jobs must finish or be explicitly cancelled before setup: an atomic maintenance gate refuses an active-job upgrade and blocks new jobs until the installer stops the service.

SSH must be reachable via existing network/firewall/NAT policy. The installer validates SSH syntax and the effective bridge authorization path, retaining unrelated settings. Existing AllowUsers/AllowGroups restrictions may require operator adjustment. Revoking a Master uses `masters.revoke`; to remove its obsolete key line on disk remove only the line marked `sshcc:master:<UUID>`.

## Service environment and credentials

Outbound SSH from a Master always runs in the service context, and so do jobs on installations still in service mode; jobs on owner-session installations run in the Owner's context (see [OWNER-SESSION.md](OWNER-SESSION.md)). A mapped drive, interactive SSH agent, user Git credential store, or desktop Tailscale login success does not prove access under SYSTEM/root, so test credentials in the context where the job actually runs.

Both installers support a protected JSON object of string environment values:

~~~json
{
  "GIT_TERMINAL_PROMPT": "0",
  "GIT_CONFIG_GLOBAL": "C:\\ProgramData\\SSHControlCenter\\gitconfig",
  "DEPLOY_REGION": "example"
}
~~~

Windows: pass -ServiceEnvironmentFile C:\Secure\service-environment.json. Ubuntu: pass --service-environment /secure/service-environment.json. Setup copies it into protected service state as service-env.json; restart the service after changing it. Ubuntu additionally supports the root-owned 0600 /etc/sshcc/service.env systemd EnvironmentFile. The JSON environment takes precedence over inherited systemd values.

Use a dedicated deployment identity and provision its credentials explicitly in protected service-owned files or the service account's credential manager/helper. Do not copy the logged-in user's profile wholesale. Git SSH keys require pinned known-host entries and noninteractive authentication. The service's fleet keys are transport credentials and are not exported as Git credentials.

For SMB on Windows in service mode, SYSTEM does not carry any user's credentials to the share. Grant the intended machine/deployment identity access, or establish the required SMB credential/session in the job's service logon context. Test an actual read/write of the intended UNC path with a job; do not infer it from Explorer. Avoid passwords in scripts, command lines and logs because jobs retain their script and output. On Ubuntu use a deliberately provisioned CIFS mount/credentials file protected to root. The product does not manage these credentials automatically.

## Upgrade, uninstall and rollback

Stop new submissions and inspect running jobs before upgrading. A service restart interrupts active work; the next start marks unfinished jobs interrupted and never replays them.

Windows reruns Setup.ps1 against the same installation. It stops the product service/tray, backs up binaries/config and the stopped fleet database with WAL/SHM under installer-backups, and keeps identity, databases, keys and logs. On a setup failure it restores saved binaries/config/sshd configuration where available. Authorization database and environment are restored too, including removal of files created by a failed first install. Online prerequisites and newly created OS accounts can remain after failure; inspect the installer record. These installers do not claim an atomic OS transaction.

Ubuntu apt upgrades preserve /var/lib/sshcc. Rerun setup for changed role, owner-approved environment or additional enrollment. Setup keeps configuration backups; retain the preceding .deb separately for binary downgrade. Do not downgrade across a database schema change without that version's release notes.

Windows removal:

~~~powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 'C:\Program Files\SSH Control Center\Uninstall.ps1'
~~~

Ubuntu removal:

~~~bash
sudo /usr/lib/sshcc/uninstall.sh
~~~

Removal stops/deletes only this service/package and removes the owned SSH block and owner tray entry. Private state, logs, public key files, bridge account and prerequisites remain for explicit recovery. Keys, services and Tailscale grants that this product did not create are not touched. State removal and bridge account deletion are separate operator decisions.

Rollback installs the preceding verified binaries/package, restores its compatible saved config if necessary, and starts the service. Review interrupted jobs before resuming deployment.

References: [Windows OpenSSH configuration](https://learn.microsoft.com/windows-server/administration/openssh/openssh_server_configuration), [WebView2 runtime distribution](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution), [Ubuntu SSH server configuration](https://manpages.ubuntu.com/manpages/jammy/man5/sshd_config.5.html).
