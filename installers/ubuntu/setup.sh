#!/usr/bin/env bash
set -euo pipefail
role=master
owner=""
target_user=sshcc-bridge
label="$(hostname)"
bundle=""
registration=""
package=""
desktop=0
service_env=""
execution_mode=preserve
while (($#)); do
  case "$1" in
    --role) role="$2"; shift 2;;
    --owner) owner="$2"; shift 2;;
    --execution-mode) execution_mode="$2"; shift 2;;
    --target-user) target_user="$2"; shift 2;;
    --label) label="$2"; shift 2;;
    --bundle) bundle="$(realpath "$2")"; shift 2;;
    --registration) registration="$(realpath -m "$2")"; shift 2;;
    --package) package="$(realpath "$2")"; shift 2;;
    --desktop) desktop=1; shift;;
    --service-environment) service_env="$(realpath "$2")"; shift 2;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
done
[[ "$role" =~ ^(master|target|both)$ ]] || { echo "Invalid role" >&2; exit 2; }
[[ "$target_user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || { echo "Invalid bridge username" >&2; exit 2; }
# Never accidentally make the elevating root account the desktop owner.
owner="${owner:-${SUDO_USER:-}}"
[[ -n "$owner" && "$owner" != root ]] || { echo 'Specify --owner <desktop/login-user>, captured before sudo.' >&2; exit 2; }
owner_uid="$(id -u "$owner")"
if [[ -f /var/lib/sshcc/config.json ]]; then
  owner_uid="$(python3 -c 'import json;print(json.load(open("/var/lib/sshcc/config.json"))["owner"])')"
  owner="$(getent passwd "$owner_uid" | cut -d: -f1)"
  [[ -n "$owner" && "$owner" != root ]] || { echo 'Existing Owner account is absent or invalid.' >&2; exit 2; }
fi
owner_home="$(getent passwd "$owner" | cut -d: -f6)"
registration="${registration:-$owner_home/sshcc-registration.json}"
[[ $EUID -eq 0 ]] || { echo 'Run with sudo; pass --owner "$USER".' >&2; exit 2; }
if [[ "$execution_mode" == preserve ]]; then
  execution_mode="$(python3 -c 'import json,pathlib;p=pathlib.Path("/var/lib/sshcc/config.json");print(json.load(p.open()).get("executionMode","service") if p.exists() else "owner-session")')"
fi
[[ "$execution_mode" =~ ^(service|owner-session)$ ]] || { echo 'Invalid execution mode' >&2; exit 2; }
if [[ "$execution_mode" == owner-session ]]; then
  id -nG "$owner" | tr ' ' '\n' | grep -Eq '^(sudo|admin|wheel)$' || { echo 'E_OWNER_NOT_ADMIN: Owner must already belong to an administrator group' >&2; exit 2; }
fi
echo "Execution account: $owner (UID $owner_uid); mode: $execution_mode; admin requests use root."
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 && "$(uname -m)" == x86_64 ]] || {
  echo 'This package targets Ubuntu 24.04 x86_64.' >&2; exit 2;
}
command -v python3 >/dev/null
# Bootstrap passes an inherited locked descriptor; advanced setup takes the same lock.
if [[ "${SSHCC_SETUP_LOCK_FD:-}" == 9 && "$(readlink /proc/$$/fd/9 2>/dev/null)" == /run/sshcc-installer/install.lock ]]; then
  flock -n 9 || { echo 'Installer lock is not held.' >&2; exit 3; }
else
  python3 - <<'PYLOCK'
import pathlib,stat
p=pathlib.Path("/run/sshcc-installer")
try: p.mkdir(mode=0o700)
except FileExistsError: pass
s=p.lstat()
assert stat.S_ISDIR(s.st_mode) and s.st_uid==0 and not(s.st_mode & 0o077), "Untrusted installer lock directory"
PYLOCK
  exec 9>/run/sshcc-installer/install.lock
  flock -n 9 || { echo 'Another SSH Control Center installer is running.' >&2; exit 3; }
fi
# Before package installation or stopping the service.
python3 - <<'PYJOBS'
import pathlib,sqlite3
p=pathlib.Path("/var/lib/sshcc/jobs.sqlite")
if p.exists():
    with sqlite3.connect(p.as_uri()+"?mode=ro",uri=True) as db:
        count=db.execute("SELECT COUNT(*) FROM jobs WHERE json_extract(record,'$.status') IN ('queued','running')").fetchone()[0]
    assert count==0, "E_BUSY: running jobs must finish or be cancelled before setup"
p=pathlib.Path("/var/lib/sshcc/terminals.sqlite")
if p.exists():
    with sqlite3.connect(p.as_uri()+"?mode=ro",uri=True) as db:
        count=db.execute("SELECT COUNT(*) FROM terminals WHERE json_extract(record,'$.status')='running'").fetchone()[0]
    assert count==0, "E_BUSY: close terminals before setup"
PYJOBS
python3 - <<'PY'
import os,pathlib,stat
p=pathlib.Path("/var/lib/sshcc")
if p.exists() or p.is_symlink():
    todo=[p]
    while todo:
        item=todo.pop(); mode=item.lstat()
        assert not stat.S_ISLNK(mode.st_mode), "Refuse symlink in existing product state"
        assert mode.st_uid==0 and not (mode.st_mode & 0o022), "Refuse untrusted ownership/write access in existing product state"
        if item.is_dir(): todo.extend(item.iterdir())
PY
if [[ -f /var/lib/sshcc/installation.json ]]; then
  python3 - "$owner_uid" "$target_user" <<'PY'
import json,sys
v=json.load(open("/var/lib/sshcc/installation.json"))
assert str(v["owner"])==sys.argv[1] and v["bridgeUser"]==sys.argv[2], "Owner and bridge user are immutable on reinstall"
PY
fi
if ((desktop)); then
  [[ -x /usr/bin/sshcc-desktop ]] || { echo 'Install the desktop package before requesting --desktop.' >&2; exit 2; }
fi
if [[ -n "$bundle" ]]; then
  [[ "$role" != master ]] || { echo 'Enrollment requires target or both role.' >&2; exit 2; }
  python3 - "$bundle" "$target_user" <<'PY'
import json,re,sys,uuid
b=json.load(open(sys.argv[1],encoding="utf-8-sig"))
assert b["schemaVersion"]==2 and b["os"]=="linux" and b["user"]==sys.argv[2], "Bundle OS/user/version mismatch"
uuid.UUID(b["master"]["id"])
assert re.fullmatch(r"ssh-ed25519 [A-Za-z0-9+/=]+(?: [^\r\n]*)?",b["master"]["publicKey"]), "Invalid public key"
PY
fi
was_running=0
systemctl is-active --quiet sshcc && was_running=1
maintenance_cli=/usr/bin/sshcc
if [[ -n "$package" ]]; then
  [[ "$(dpkg-deb -f "$package" Package)" == sshcc ]] || { echo 'Expected sshcc service package.' >&2; exit 2; }
  maintenance_dir="$(mktemp -d /var/tmp/sshcc-maintenance.XXXXXX)"
  dpkg-deb -x "$package" "$maintenance_dir"
  maintenance_cli="$maintenance_dir/usr/bin/sshcc"
fi
guard_started=0
if ((was_running)) && [[ "${SSHCC_SETUP_MAINTENANCE_HELD:-0}" != 1 ]]; then
  if "$maintenance_cli" configure installer-begin >/dev/null 2>"/run/sshcc-installer/install-begin.err"; then
    guard_started=1
  else
    cat "/run/sshcc-installer/install-begin.err" >&2
    exit 1
  fi
fi
# Release the gate even if prerequisite installation fails before the full rollback trap.
release_early_guard() {
  local status=$?
  ((guard_started == 0)) || "$maintenance_cli" configure installer-end >/dev/null 2>&1 || true
  exit "$status"
}
trap release_early_guard EXIT
[[ -n "$package" || -x /usr/bin/sshcc ]] || { echo 'Install sshcc .deb first or pass --package ./sshcc_0.4.0_amd64.deb.' >&2; exit 2; }
apt-get update
apt-get install -y openssh-client
if [[ "$role" != master ]]; then
  apt-get install -y openssh-server openssl
  # Fresh server images may not have started ssh.service yet. sshd -t needs this runtime directory.
  install -d -o root -g root -m 0755 /run/sshd
  if ! id "$target_user" >/dev/null 2>&1; then
    password_hash="$(openssl rand -base64 48 | openssl passwd -6 -stdin)"
    useradd --system --create-home --shell /bin/sh --comment "SSH Control Center forced-command bridge" --password "$password_hash" "$target_user"
    unset password_hash
  fi
  [[ "$(getent passwd "$target_user" | cut -d: -f5)" == 'SSH Control Center forced-command bridge' ]] || {
    echo 'Refusing to take over an account not created by SSH Control Center.' >&2; exit 2;
  }
  bridge_uid="$(id -u "$target_user")"
  [[ "$bridge_uid" != 0 && "$bridge_uid" != "$owner_uid" ]] || { echo 'Bridge must be a separate non-root account.' >&2; exit 2; }
  if id -nG "$target_user" | tr ' ' '\n' | grep -Eq '^(sudo|admin|wheel)$'; then
    echo 'Bridge account must not have administrative groups.' >&2; exit 2
  fi
fi
install -d -m 0700 /var/lib/sshcc
install -d -m 0755 /etc/sshcc
install -d -m 0755 /run/sshcc
backup_dir="/var/lib/sshcc/installer-backups/$(date -u +%Y%m%dT%H%M%S)-$$"
install -d -m 0700 "$backup_dir"
systemctl stop sshcc || true
for name in config.json installation.json service-env.json fleet.sqlite fleet.sqlite-wal fleet.sqlite-shm jobs.sqlite jobs.sqlite-wal jobs.sqlite-shm terminals.sqlite terminals.sqlite-wal terminals.sqlite-shm; do
  [[ ! -f "/var/lib/sshcc/$name" ]] || cp -p "/var/lib/sshcc/$name" "$backup_dir/$name"
done
python3 - "$backup_dir" <<'PYBIN'
import json,pathlib,shutil,sys
backup=pathlib.Path(sys.argv[1]);entries=[]
for index,name in enumerate(["/usr/bin/sshcc","/usr/bin/sshcc-desktop","/usr/bin/sshcc-window","/lib/systemd/system/sshcc.service","/usr/lib/sshcc/setup.sh","/usr/lib/sshcc/uninstall.sh"]):
    path=pathlib.Path(name)
    assert not path.is_symlink(),"Refuse symlink product file"
    entry=dict(path=name,existed=path.is_file(),backup="binary-"+str(index))
    if entry["existed"]: shutil.copy2(path,backup/entry["backup"])
    entries.append(entry)
(backup/"binaries.json").write_text(json.dumps(entries))
PYBIN
rollback() {
  local status=$?
  if ((status)); then
    systemctl stop sshcc || true
    python3 - "$backup_dir" <<'PYRESTOREBIN'
import json,pathlib,shutil,sys
backup=pathlib.Path(sys.argv[1])
for entry in json.loads((backup/"binaries.json").read_text()):
    path=pathlib.Path(entry["path"])
    if entry["existed"]: shutil.copy2(backup/entry["backup"],path)
    else: path.unlink(missing_ok=True)
PYRESTOREBIN
    systemctl daemon-reload || true
    for name in config.json installation.json service-env.json fleet.sqlite fleet.sqlite-wal fleet.sqlite-shm jobs.sqlite jobs.sqlite-wal jobs.sqlite-shm terminals.sqlite terminals.sqlite-wal terminals.sqlite-shm; do
      if [[ -f "$backup_dir/$name" ]]; then
        cp -p "$backup_dir/$name" "/var/lib/sshcc/$name"
      else
        rm -f -- "/var/lib/sshcc/$name"
      fi
    done
    if [[ -f "$backup_dir/90-sshcc.conf" ]]; then
      cp -p "$backup_dir/90-sshcc.conf" /etc/ssh/sshd_config.d/90-sshcc.conf
    elif [[ -f "$backup_dir/new-sshd-block" ]]; then
      rm -f -- /etc/ssh/sshd_config.d/90-sshcc.conf
    fi
    if [[ -f "$backup_dir/authorized_keys" ]]; then
      cp -p "$backup_dir/authorized_keys" "/etc/ssh/sshcc/$target_user.authorized_keys"
    elif [[ -f "$backup_dir/new-authorized-keys" ]]; then
      rm -f -- "/etc/ssh/sshcc/$target_user.authorized_keys"
    fi
    /usr/sbin/sshd -t && systemctl reload ssh || true
    ((guard_started == 0)) || "$maintenance_cli" configure installer-end >/dev/null 2>&1 || true
    ((was_running == 0)) || systemctl start sshcc || true
    echo "Setup failed; prior binaries, config and databases restored. Review $backup_dir." >&2
  fi
  exit "$status"
}
trap rollback EXIT
if [[ -n "$package" ]]; then
  export SSHCC_SETUP_INSTALL=1
  apt-get update
  apt-get install -y --reinstall "$package"
fi
if [[ -f /var/lib/sshcc/config.json ]]; then
  label="$(python3 -c 'import json;print(json.load(open("/var/lib/sshcc/config.json"))["label"])')"
fi
init_args=(--data-dir /var/lib/sshcc init --role "$role" --owner "$owner_uid" --label "$label" --execution-mode "$execution_mode")
[[ "$role" == master ]] || init_args+=(--allow-principal "$bridge_uid")
/usr/bin/sshcc "${init_args[@]}"
if [[ -n "$bundle" ]]; then
  python3 - "$bundle" /var/lib/sshcc/enrollment-bundle.json <<'PY'
import json,pathlib,sys
pathlib.Path(sys.argv[2]).write_text(json.dumps(json.load(open(sys.argv[1],encoding="utf-8-sig"))),encoding="utf-8")
PY
  chmod 0600 /var/lib/sshcc/enrollment-bundle.json
  /usr/bin/sshcc --data-dir /var/lib/sshcc configure accept-bundle /var/lib/sshcc/enrollment-bundle.json
  install -d -m 0755 /etc/ssh/sshcc
  key_file="/etc/ssh/sshcc/$target_user.authorized_keys"
  if [[ -f "$key_file" ]]; then cp -p "$key_file" "$backup_dir/authorized_keys"; else touch "$backup_dir/new-authorized-keys"; fi
  python3 - "$bundle" "$key_file" <<'PY'
import base64,hashlib,json,pathlib,sys
b=json.load(open(sys.argv[1],encoding="utf-8-sig"))
p=pathlib.Path(sys.argv[2]); marker="sshcc:master:"+b["master"]["id"]
lines=[s for s in p.read_text().splitlines() if not s.endswith(" "+marker)] if p.exists() else []
key=" ".join(b["master"]["publicKey"].split()[:2])
fingerprint="SHA256:"+base64.b64encode(hashlib.sha256(base64.b64decode(key.split()[1])).digest()).decode().rstrip("=")
lines.append('restrict,command="/usr/bin/sshcc bridge --master-id '+b["master"]["id"]+' --master-key '+fingerprint+'" '+key+" "+marker)
p.write_text("\n".join(lines)+"\n")
PY
  chmod 0644 "$key_file"; chown root:root "$key_file"
  if [[ -f /etc/ssh/sshd_config.d/90-sshcc.conf ]]; then
    grep -q '^# SSHCC managed bridge configuration$' /etc/ssh/sshd_config.d/90-sshcc.conf || {
      echo 'Refusing to overwrite unowned SSH config.' >&2; exit 1;
    }
    cp -p /etc/ssh/sshd_config.d/90-sshcc.conf "$backup_dir/90-sshcc.conf"
  else
    touch "$backup_dir/new-sshd-block"
  fi
  cat > /etc/ssh/sshd_config.d/90-sshcc.conf <<EOF
# SSHCC managed bridge configuration
Match User $target_user
    AuthorizedKeysFile $key_file
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    AllowTcpForwarding no
    AllowAgentForwarding no
    PermitTTY no
    X11Forwarding no
Match all
EOF
  /usr/sbin/sshd -t
  effective_keys="$(/usr/sbin/sshd -T -C "user=$target_user,host=localhost,addr=127.0.0.1" | awk '$1=="authorizedkeysfile" {$1=""; sub(/^ /,""); print}')"
  [[ "$effective_keys" == "$key_file" ]] || { echo 'Existing SSH policy overrides bridge AuthorizedKeysFile; reconcile manually.' >&2; exit 1; }
  systemctl enable --now ssh
  systemctl reload ssh
  # Existing socket-activated installations can retain a second listener.
  # Reload only ssh.service listeners; preserve established sessions and other daemons.
  python3 - <<'PY'
import pathlib,os,signal
for proc in pathlib.Path('/proc').iterdir():
    if not proc.name.isdigit():
        continue
    try:
        cgroup=(proc/'cgroup').read_text()
        command=(proc/'cmdline').read_bytes()
        if '/system.slice/ssh.service' in cgroup and b'[listener]' in command and (proc/'exe').resolve()==pathlib.Path('/usr/sbin/sshd'):
            os.kill(int(proc.name),signal.SIGHUP)
    except (FileNotFoundError,ProcessLookupError,PermissionError):
        pass
PY
  python3 - "$bundle" /var/lib/sshcc/registration-spec.json <<'PY'
import json,pathlib,subprocess,sys
b=json.load(open(sys.argv[1],encoding="utf-8-sig"))
settings=subprocess.check_output(["/usr/sbin/sshd","-T"],text=True)
paths=[pathlib.Path(line.split(" ",1)[1]+".pub") for line in settings.splitlines() if line.startswith("hostkey ")]
keys=[p.read_text().strip() for p in paths if p.is_file()]
assert keys,"No public host keys"
spec={k:b[k] for k in ["targetId","tenant","routes","os","user"]}
spec.update(hostKeys=keys,bridgePath="/usr/bin/sshcc")
pathlib.Path(sys.argv[2]).write_text(json.dumps(spec))
PY
  /usr/bin/sshcc --data-dir /var/lib/sshcc configure registration --spec /var/lib/sshcc/registration-spec.json --output "$registration"
  chown "$owner_uid:$(id -g "$owner")" "$registration"
  chmod 0600 "$registration"
fi
if [[ -n "$service_env" ]]; then
  python3 - "$service_env" /var/lib/sshcc/service-env.json <<'PY'
import json,pathlib,sys
v=json.load(open(sys.argv[1],encoding="utf-8-sig"))
assert isinstance(v,dict) and all(isinstance(k,str) and k and "=" not in k and chr(0) not in k and isinstance(x,str) and chr(0) not in x for k,x in v.items()), "Service environment must contain string values"
pathlib.Path(sys.argv[2]).write_text(json.dumps(v),encoding="utf-8")
PY
  chmod 0600 /var/lib/sshcc/service-env.json
  chown root:root /var/lib/sshcc/service-env.json
fi
if [[ ! -e /etc/sshcc/service.env ]]; then install -m 0600 /dev/null /etc/sshcc/service.env; fi
systemctl daemon-reload
if ((desktop)); then
  [[ -x /usr/bin/sshcc-desktop ]] || { echo 'Install the separate sshcc-desktop .deb before --desktop.' >&2; exit 1; }
  install -d -o "$owner_uid" -g "$(id -g "$owner")" -m 0755 "$owner_home/.config/autostart"
  install -o "$owner_uid" -g "$(id -g "$owner")" -m 0644 /usr/share/applications/sshcc-tray.desktop "$owner_home/.config/autostart/sshcc-tray.desktop"
fi
python3 - /var/lib/sshcc/installation.json "$owner_uid" "$target_user" "$role" "$backup_dir" <<'PY'
import json,pathlib,sys
cfg=json.load(open("/var/lib/sshcc/config.json"))
actual_role="both" if len(cfg["roles"])>1 else cfg["roles"][0]
pathlib.Path(sys.argv[1]).write_text(json.dumps(dict(schemaVersion=2,owner=sys.argv[2],executionMode=cfg.get("executionMode","service"),bridgeUser=sys.argv[3],role=actual_role,backupDir=sys.argv[5])))
PY
systemctl enable --now sshcc
echo "Installed SSH Control Center ($role); private state /var/lib/sshcc."
[[ -z "$bundle" ]] || echo "Import public registration on the Master: $registration"
echo 'Tailscale login, firewall and network policy remain operator-managed.'
