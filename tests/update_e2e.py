#!/usr/bin/env python3
"""Updating a Target from the Master over real OpenSSH, in owned Docker containers only.

    python3 tests/update_e2e.py --image sshcc-e2e:local --old-binary path/to/sshcc-0.4.0

The image carries the new `sshcc`; the Target is switched to an older build (`--old-binary`) before it starts, so
it reports the older version. The Master gets release templates in /usr/lib/sshcc/templates: a Windows placeholder
and an Ubuntu thin template whose `assets/sshcc-service.deb` is a real Debian package of the new binary, built here
with dpkg-deb, whose postinst restarts the service as the real one does. The Master then runs `targets.upgrade`: it
uploads that package over SSH, starts `dpkg -i` detached, and waits for the new version.

Containers have no systemd, so `systemd-run` on the Target is a small shim that starts its command in a new session
(what a transient unit gives the real one: it outlives the job and the service restart). Everything else is real:
the package, dpkg, SSH, the upload, the job and the restarted service. Containers and the network are removed.
"""
import argparse
import json
import time

from rotation_e2e import Harness, run

SHIM = """#!/bin/sh
# systemd-run stand-in for containers: drop the options, run the command in its own session.
while [ $# -gt 0 ]; do case "$1" in --unit) shift 2;; --*) shift;; *) break;; esac; done
setsid "$@" >/tmp/update.log 2>&1 < /dev/null &
"""
POSTINST = """#!/bin/sh
set -e
if [ "$1" = configure ] && [ -f /var/lib/sshcc/config.json ]; then
  pkill -x sshcc || true
  sleep 1
  setsid /usr/bin/sshcc service run >>/tmp/service.log 2>&1 < /dev/null &
fi
"""
TEMPLATES = r"""
set -e
v=$(sshcc --version 2>/dev/null | awk '{print $2}'); v=${v:-0.5.0}
w=$(mktemp -d); mkdir -p $w/pkg/DEBIAN $w/pkg/usr/bin $w/thin/DEBIAN $w/thin/usr/lib/sshcc-setup/assets
cp /usr/bin/sshcc $w/pkg/usr/bin/sshcc
printf 'Package: sshcc\nVersion: %s\nArchitecture: all\nMaintainer: e2e\nDescription: e2e service package\n' "$v" > $w/pkg/DEBIAN/control
cat > $w/pkg/DEBIAN/postinst; chmod 0755 $w/pkg/DEBIAN/postinst
dpkg-deb -Zgzip --root-owner-group --build $w/pkg $w/thin/usr/lib/sshcc-setup/assets/sshcc-service.deb >/dev/null
printf 'Package: sshcc-setup\nVersion: %s\nArchitecture: all\nMaintainer: e2e\nDescription: e2e thin template\n' "$v" > $w/thin/DEBIAN/control
install -d /usr/lib/sshcc/templates
dpkg-deb -Zgzip --root-owner-group --build $w/thin /usr/lib/sshcc/templates/sshcc-client-ubuntu-template.deb >/dev/null
printf 'MZ windows placeholder' > /usr/lib/sshcc/templates/sshcc-client-windows-template.exe
cd /usr/lib/sshcc/templates
python3 - <<'PY'
import hashlib, json, pathlib
sha = lambda n: hashlib.sha256(pathlib.Path(n).read_bytes()).hexdigest()
json.dump({"schemaVersion": 1, "templates": [
  {"os": "linux", "fileName": "sshcc-client-ubuntu-template.deb", "sha256": sha("sshcc-client-ubuntu-template.deb")},
  {"os": "windows", "fileName": "sshcc-client-windows-template.exe", "sha256": sha("sshcc-client-windows-template.exe")}]},
  open("template-manifest.json", "w"))
PY
"""


class Update(Harness):
    def __init__(self, image, old_binary):
        super().__init__(image)
        self.old_binary = old_binary

    def version(self, master, alias):
        self.request(master, "fleet.health", {"refresh": True})
        rows = self.request(master, "targets.update_check")["hosts"]
        return next(r for r in rows if r["targetId"] == alias)

    def run_checks(self):
        run("docker", "network", "create", "--internal", self.network)
        master = self.container("master", "master", "Office")
        self.exec(master, "sh", "-c", TEMPLATES, stdin=POSTINST)
        # The Target starts on the older build.
        name = f"{self.network}-web-1"
        run("docker", "run", "-d", "--name", name, "--hostname", "web-1", "--network", self.network, "--network-alias", "web-1", self.image)
        self.containers.append(name)
        run("docker", "cp", self.old_binary, f"{name}:/usr/bin/sshcc")
        self.exec(name, "sh", "-c", "chmod 0755 /usr/bin/sshcc && cat > /usr/local/bin/systemd-run && chmod 0755 /usr/local/bin/systemd-run", stdin=SHIM)
        self.exec(name, "sshcc", "init", "--role", "target", "--owner", "0", "--label", "web-1", "--allow-principal", "1100", "--execution-mode", "service")
        self.start_service(name)
        self.enroll(master, name, "web-1")
        before = self.version(master, "web-1")
        assert before["version"] == "0.4.0" and before["outdated"] and before["canUpdate"], before
        started = time.time()
        done = self.request(master, "targets.upgrade", {"targetId": "web-1"})
        took = time.time() - started
        latest = self.request(master, "targets.update_check")["latest"]
        assert done["version"] == latest and done["from"] == "0.4.0", done
        after = self.version(master, "web-1")
        assert after["version"] == latest and not after["outdated"], after
        assert self.exec(name, "sshcc", "service", "status", check=False).returncode == 0, "the service runs again"
        assert self.exec(name, "sh", "-c", "ls /tmp/sshcc-update-* 2>/dev/null || true").stdout.strip() == "", "the package was removed"
        assert isinstance(self.request(master, "jobs.list", {"targetId": "web-1"}), list), "the updated host still answers jobs"
        # Partitions as a real container reports them (overlay root: falls back to /).
        stats = self.request(master, "monitor.stats", {"targetId": "web-1"})
        print(json.dumps({"ok": True, "updateSeconds": round(took, 1), "from": "0.4.0", "to": latest, "disks": stats.get("disks")}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--old-binary", required=True, help="an sshcc build of an older version, for the Target")
    parser.add_argument("--keep", action="store_true", help="keep the containers after a failure, to look at them")
    args = parser.parse_args()
    harness = Update(args.image, args.old_binary)
    ok = False
    try:
        harness.run_checks()
        ok = True
    finally:
        if ok or not args.keep:
            harness.stop()
        else:
            print("kept: " + " ".join(harness.containers))


if __name__ == "__main__":
    main()
