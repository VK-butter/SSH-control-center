#!/usr/bin/env bash
# Invoked only by the setup helper after explicit Approve.
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
assets="$root/assets"
caller_uid=""
desktop=0
execution_mode=preserve
while (($#)); do
  case "$1" in
    --caller-uid) caller_uid="$2"; shift 2;;
    --desktop) desktop=1; shift;;
    --execution-mode) execution_mode="$2"; shift 2;;
    *) echo "Unknown setup option: $1" >&2; exit 2;;
  esac
done
[[ $EUID -eq 0 && "$caller_uid" =~ ^[1-9][0-9]*$ ]] || { echo 'ต้องอนุญาตสิทธิ์ผู้ดูแลระบบเพื่อทำขั้นตอนนี้' >&2; exit 2; }
elevated_uid="${PKEXEC_UID:-${SUDO_UID:-$caller_uid}}"
[[ "$elevated_uid" == "$caller_uid" ]] || { echo 'บัญชีเจ้าของไม่ตรงกับบัญชีที่เริ่มติดตั้ง' >&2; exit 2; }
caller_name="$(getent passwd "$caller_uid" | cut -d: -f1)"
caller_home="$(getent passwd "$caller_uid" | cut -d: -f6)"
[[ -n "$caller_name" && "$caller_home" == /* ]] || { echo 'ไม่พบบัญชีเจ้าของเครื่อง' >&2; exit 2; }
echo 'SSHCC_PROGRESS:ตรวจไฟล์ติดตั้งและระบบปฏิบัติการ'
python3 "$root/setup-util.py" verify-assets "$assets" >/dev/null
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 && "$(uname -m)" == x86_64 ]] || {
  echo 'รองรับ Ubuntu 24.04 x64 เท่านั้น' >&2; exit 2;
}
# A root-only directory avoids lock-file symlinks planted by another local user.
python3 - <<'PY'
import os,pathlib,stat
p=pathlib.Path("/run/sshcc-installer")
try: p.mkdir(mode=0o700)
except FileExistsError: pass
s=p.lstat()
assert stat.S_ISDIR(s.st_mode) and s.st_uid==0 and not(s.st_mode & 0o077), "Untrusted installer lock directory"
PY
exec 9>/run/sshcc-installer/install.lock
flock -n 9 || { echo 'มีการติดตั้งอีกหน้าต่างกำลังทำงานอยู่ กรุณารอแล้วลองใหม่' >&2; exit 3; }
export SSHCC_SETUP_LOCK_FD=9
# Refuse BEFORE apt touches a configured service or its executable.
preflight="$("$assets/sshcc" configure installer-preflight)"
python3 -c 'import json,sys;v=json.loads(sys.argv[1]);assert v["runningJobs"]==0 and v.get("runningTerminals",0)==0, "เครื่องนี้มี jobs หรือ terminals กำลังทำงาน กรุณาปิดก่อนติดตั้ง"' "$preflight"

scratch="$(mktemp -d /var/tmp/sshcc-setup.XXXXXX)"
chmod 0700 "$scratch"
backup_dir=""
was_running=0
guard_started=0
cleanup() {
  local status=$?
  if ((status)); then
    if [[ -n "$backup_dir" && -f "$backup_dir/files.json" ]]; then
      systemctl stop sshcc || true
      python3 - "$backup_dir" <<'PYRESTORE'
import json,pathlib,shutil,sys
backup=pathlib.Path(sys.argv[1])
for entry in json.loads((backup/"files.json").read_text()):
    path=pathlib.Path(entry["path"])
    if entry["existed"]:
        shutil.copy2(backup/entry["backup"],path)
    elif str(path).startswith(("/var/lib/sshcc/","/etc/ssh/","/etc/sshcc/","/usr/lib/sshcc/templates/")):
        path.unlink(missing_ok=True)
PYRESTORE
      systemctl daemon-reload || true
      if [[ -x /usr/sbin/sshd ]]; then /usr/sbin/sshd -t && systemctl reload ssh || true; fi
      echo "ติดตั้งไม่สำเร็จ คืนค่าที่ระบบนี้เปลี่ยนไว้แล้ว สำรองที่ $backup_dir" >&2
    fi
    if ((guard_started)); then "$assets/sshcc" configure installer-end >/dev/null 2>&1 || true; fi
    ((was_running==0)) || systemctl start sshcc || true
  fi
  rm -f -- "$scratch/inspection.json" "$scratch/bundle.json" "$scratch/registration.json" "$scratch/status.json" "$scratch/install-begin.err"
  rmdir -- "$scratch" 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT
owner_uid="$caller_uid"
# Reinstallation must not transfer ownership or rename an existing node.
if [[ -f /var/lib/sshcc/config.json ]]; then
  owner_uid="$(python3 -c 'import json;print(json.load(open("/var/lib/sshcc/config.json"))["owner"])')"
fi
owner_name="$(getent passwd "$owner_uid" | cut -d: -f1)"
owner_home="$(getent passwd "$owner_uid" | cut -d: -f6)"
[[ -n "$owner_name" && "$owner_uid" != 0 ]] || { echo 'ไม่พบบัญชีเจ้าของเดิม กรุณาตรวจการตั้งค่า' >&2; exit 2; }
role=master
label="$(hostname)"
request_id=""
if [[ -f "$assets/envelope.json" ]]; then
  "$assets/sshcc" pairing inspect --request "$assets/envelope.json" --bundle-output "$scratch/bundle.json" > "$scratch/inspection.json"
  request_id="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["envelope"]["requestId"])' "$scratch/inspection.json")"
  label="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["envelope"]["targetLabel"])' "$scratch/inspection.json")"
  role=target
  desktop=0
fi
# Capture only product-owned files before an upgrade; restored if later setup/receipt fails.
python3 - <<'PYSTATE'
import pathlib,stat
p=pathlib.Path("/var/lib/sshcc")
if p.exists() or p.is_symlink():
    for item in [p,*p.rglob("*")]:
        mode=item.lstat()
        assert not stat.S_ISLNK(mode.st_mode) and mode.st_uid==0 and not(mode.st_mode & 0o022), "Untrusted existing product state"
PYSTATE
systemctl is-active --quiet sshcc && was_running=1
if ((was_running)); then
  if "$assets/sshcc" configure installer-begin >/dev/null 2>"$scratch/install-begin.err"; then
    guard_started=1
  else
    cat "$scratch/install-begin.err" >&2
    exit 1
  fi
fi
systemctl stop sshcc || true
install -d -m 0700 /var/lib/sshcc /var/lib/sshcc/installer-backups
backup_dir="/var/lib/sshcc/installer-backups/bootstrap-$(python3 -c 'import uuid;print(uuid.uuid4())')"
install -d -m 0700 "$backup_dir"
bridge_user=sshcc-bridge
if [[ -f /var/lib/sshcc/installation.json ]]; then
  bridge_user="$(python3 -c 'import json;print(json.load(open("/var/lib/sshcc/installation.json"))["bridgeUser"])')"
elif [[ -n "$request_id" ]]; then
  bridge_user="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["bundle"]["user"])' "$scratch/inspection.json")"
fi
[[ "$bridge_user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || { echo 'Invalid bridge account' >&2; exit 2; }
python3 - "$backup_dir" "$bridge_user" <<'PYSNAPSHOT'
import json,pathlib,shutil,sys
backup=pathlib.Path(sys.argv[1]);user=sys.argv[2]
paths=["/usr/bin/sshcc","/usr/bin/sshcc-desktop","/usr/bin/sshcc-window",
       "/usr/lib/sshcc/setup.sh","/usr/lib/sshcc/uninstall.sh","/lib/systemd/system/sshcc.service",
       "/usr/lib/sshcc/templates/template-manifest.json","/usr/lib/sshcc/templates/sshcc-client-windows-template.exe","/usr/lib/sshcc/templates/sshcc-client-ubuntu-template.deb",
       "/etc/ssh/sshd_config.d/90-sshcc.conf","/etc/ssh/sshcc/"+user+".authorized_keys",
       "/etc/sshcc/service.env"]
paths += ["/var/lib/sshcc/"+name for name in ["config.json","service-env.json","fleet.sqlite","fleet.sqlite-wal","fleet.sqlite-shm","installation.json","jobs.sqlite","jobs.sqlite-wal","jobs.sqlite-shm","terminals.sqlite","terminals.sqlite-wal","terminals.sqlite-shm"]]
entries=[]
for index,name in enumerate(paths):
    path=pathlib.Path(name)
    assert not path.is_symlink(),"Refuse symlink product file"
    entry={"path":name,"existed":path.is_file(),"backup":str(index)}
    if entry["existed"]: shutil.copy2(path,backup/entry["backup"])
    entries.append(entry)
(backup/"files.json").write_text(json.dumps(entries),encoding="utf-8")
PYSNAPSHOT
export SSHCC_SETUP_INSTALL=1
export SSHCC_SETUP_MAINTENANCE_HELD=1
echo 'SSHCC_PROGRESS:ตรวจ prerequisites และติดตั้ง service'
packages=("$assets/sshcc-service.deb")
if ((desktop)); then
  [[ -f "$assets/sshcc-desktop.deb" ]] || { echo 'ชุดติดตั้งนี้ไม่มี desktop package' >&2; exit 2; }
  packages+=("$assets/sshcc-desktop.deb")
fi
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall --no-install-recommends "${packages[@]}"
echo 'SSHCC_PROGRESS:ตั้งค่าบัญชีระบบและ SSH'
setup_args=(--role "$role" --owner "$owner_name" --label "$label" --target-user "$bridge_user" --execution-mode "$execution_mode")
if [[ "$role" == target ]]; then
  setup_args+=(--bundle "$scratch/bundle.json" --registration "$scratch/registration.json")
fi
((!desktop)) || setup_args+=(--desktop)
/usr/lib/sshcc/setup.sh "${setup_args[@]}"
echo 'SSHCC_PROGRESS:ตรวจ service และสิทธิ์ root'
ready=0
for attempt in {1..20}; do
  if /usr/bin/sshcc request system.status > "$scratch/status.json"; then ready=1; break; fi
  sleep 0.5
done
((ready)) || { echo 'Service ไม่ตอบกลับ กรุณาตรวจรายละเอียดแล้วลองใหม่' >&2; exit 1; }
python3 - "$scratch/status.json" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
assert v.get("privileged") is True and str(v.get("executionPrincipal"))=="0", "Service ไม่ได้รันด้วยสิทธิ์ root"
PY
if [[ "$role" == target ]]; then
  install -d -m 0700 /var/lib/sshcc/pairing-codes
  /usr/bin/sshcc configure pairing-receipt --request "$assets/envelope.json" --registration "$scratch/registration.json" --output "/var/lib/sshcc/pairing-codes/$request_id.txt"
  chmod 0600 "/var/lib/sshcc/pairing-codes/$request_id.txt"
  # The public receipt is written with owner privileges, never root privileges in a user-controlled home.
  runuser -u "$caller_name" -- python3 -c 'import os,pathlib,sys;p=pathlib.Path(sys.argv[1])/".local/share/sshcc/setup"/sys.argv[2];p.mkdir(parents=True,exist_ok=True,mode=0o700);p.chmod(0o700);f=p/"pairing-code.txt";fd=os.open(f,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,0o600);os.write(fd,sys.stdin.buffer.read());os.close(fd);f.chmod(0o600)' "$caller_home" "$request_id" < "/var/lib/sshcc/pairing-codes/$request_id.txt"
else
  if [[ -d "$assets/release-templates" ]]; then
    install -d -m 0755 /usr/lib/sshcc/templates
    for name in sshcc-client-windows-template.exe sshcc-client-ubuntu-template.deb template-manifest.json; do
      [[ ! -f "$assets/release-templates/$name" ]] || install -m 0644 "$assets/release-templates/$name" "/usr/lib/sshcc/templates/$name"
    done
  fi
  echo 'SSHCC_PROGRESS:เชื่อม Codex หากมีการติดตั้งไว้'
  runuser -u "$owner_name" -- python3 "$root/setup-util.py" codex-mcp "$owner_home" || {
    echo 'Service พร้อมแล้ว แต่ตั้งค่า Codex อัตโนมัติไม่สำเร็จ ตรวจรายละเอียดด้านบน' >&2
  }
fi
echo 'SSHCC_PROGRESS:ติดตั้งสำเร็จ'
