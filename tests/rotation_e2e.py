#!/usr/bin/env python3
"""Backup, restore and Change keys over real OpenSSH, in owned Docker containers only.

    docker build -f tests/containers/Dockerfile -t sshcc-e2e:local .
    python3 tests/rotation_e2e.py --image sshcc-e2e:local

Two Ubuntu Targets are set up the way installers/ubuntu/setup.sh does it: the Master's forced-command line in
/etc/ssh/sshcc/sshcc-bridge.authorized_keys with the `sshcc:master:<id>` marker. The old Master is backed up and
removed, a fresh Master restores the file and reaches the Targets with no change on them, then Change keys runs the
elevated job on them while web-2 is offline; web-2 must be moved on the service's own retry when it comes back.
Checks: the new key works, the old key is refused, and the Target's key file holds one line for this Master. Every container and the network are removed at the end.
"""
import argparse
import json
import subprocess
import time
import uuid

PASSPHRASE = "a long passphrase for the e2e test"


def run(*args, stdin=None, check=True, timeout=180):
    result = subprocess.run(list(map(str, args)), input=stdin, text=True, capture_output=True, timeout=timeout, encoding="utf-8")
    if check and result.returncode:
        raise RuntimeError(f"{' '.join(map(str, args[:6]))} exited {result.returncode}: {result.stderr.strip()} {result.stdout.strip()}")
    return result


class Harness:
    def __init__(self, image):
        self.image, self.run_id = image, uuid.uuid4().hex[:8]
        self.network, self.containers = "sshcc-rotation-" + self.run_id, []

    def exec(self, name, *args, stdin=None, check=True):
        return run("docker", "exec", *(["-i"] if stdin is not None else []), name, *args, stdin=stdin, check=check)

    def request(self, name, method, params=None):
        return json.loads(self.exec(name, "sshcc", "request", method, "--params", json.dumps(params or {})).stdout)

    def container(self, alias, role, label):
        name = f"{self.network}-{alias}"
        run("docker", "run", "-d", "--name", name, "--hostname", alias, "--network", self.network, "--network-alias", alias, self.image)
        self.containers.append(name)
        args = ["sshcc", "init", "--role", role, "--owner", "0", "--label", label]
        if role == "target":
            # Containers have no logind: jobs run as the (root) service, as in the UI preview.
            args += ["--allow-principal", "1100", "--execution-mode", "service"]
        self.exec(name, *args)
        self.start_service(name)
        return name

    def start_service(self, name):
        run("docker", "exec", "-d", name, "sh", "-c", "exec sshcc service run >>/tmp/service.log 2>&1")
        for _ in range(200):
            if self.exec(name, "sshcc", "service", "status", check=False).returncode == 0:
                return
            time.sleep(0.05)
        raise RuntimeError("service did not start in " + name)

    def enroll(self, master, target, alias):
        """As installers/ubuntu/setup.sh: accept the bundle, then the forced-command line in /etc/ssh/sshcc."""
        self.exec(target, "sh", "-c", "rm -f /etc/ssh/ssh_host_* && ssh-keygen -A >/dev/null && usermod -p '*' sshcc-bridge && install -d -m 0755 /etc/ssh/sshcc")
        config = ("Port 22\nHostKey /etc/ssh/ssh_host_ed25519_key\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
                  "PermitRootLogin no\nUsePAM no\nPidFile /run/sshd.pid\nMatch User sshcc-bridge\n"
                  "    AuthorizedKeysFile /etc/ssh/sshcc/sshcc-bridge.authorized_keys\n")
        self.exec(target, "sh", "-c", "mkdir -p /run/sshd && cat > /etc/ssh/sshd_config", stdin=config)
        run("docker", "exec", "-d", target, "/usr/sbin/sshd", "-D", "-e")
        routes = [{"kind": "lan", "address": alias, "port": 22}]
        bundle = self.request(master, "enrollment.export", {"targetId": alias, "tenant": "lab", "os": "linux", "user": "sshcc-bridge", "routes": routes})
        enrolled = self.request(target, "enrollment.accept", {"bundle": bundle})
        line = (f'restrict,command="/usr/bin/sshcc bridge --master-id {enrolled["id"]} --master-key {enrolled["fingerprint"]}" '
                f'{enrolled["publicKey"]} sshcc:master:{enrolled["id"]}\n')
        self.exec(target, "sh", "-c", "cat > /etc/ssh/sshcc/sshcc-bridge.authorized_keys && chmod 0644 /etc/ssh/sshcc/sshcc-bridge.authorized_keys", stdin=line)
        host_key = self.exec(target, "cat", "/etc/ssh/ssh_host_ed25519_key.pub").stdout.strip()
        registration = self.request(target, "enrollment.registration", {"targetId": alias, "tenant": "lab", "os": "linux",
                                                                         "user": "sshcc-bridge", "routes": routes, "hostKeys": [host_key]})
        self.request(master, "targets.import", {"registration": registration})

    def reaches(self, master, alias):
        jobs = self.request(master, "jobs.list", {"targetId": alias})
        return isinstance(jobs, list)

    def ssh_with(self, master, key_file, alias, node_id):
        """A bridge request with one specific private key, outside the Master's own transport."""
        envelope = json.dumps({"method": "bridge.request", "params": {"expectedTargetId": node_id, "request": {"method": "system.status", "params": {}}}})
        return self.exec(master, "sh", "-c", f"ssh -T -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null "
                         f"-o IdentitiesOnly=yes -i {key_file} sshcc-bridge@{alias} 2>&1", stdin=envelope + "\n", check=False)

    def run_checks(self):
        run("docker", "network", "create", "--internal", self.network)
        old = self.container("old-master", "master", "Office Master")
        target = self.container("web-1", "target", "Web 1")
        self.enroll(old, target, "web-1")
        offline = self.container("web-2", "target", "Web 2")
        self.enroll(old, offline, "web-2")
        assert self.reaches(old, "web-1"), "the old Master reaches the Target"
        node_id = self.request(target, "system.status")["nodeId"]
        print("ok enrolled like the Ubuntu installer")

        self.exec(old, "sh", "-c", "sshcc backup /tmp/office.sshccbak", stdin=PASSPHRASE + "\n" + PASSPHRASE + "\n")
        run("docker", "cp", f"{old}:/tmp/office.sshccbak", f"/tmp/{self.run_id}.sshccbak")
        old_key = self.exec(old, "cat", "/var/lib/sshcc/keys/lab.ed25519").stdout
        run("docker", "rm", "-f", old)
        self.containers.remove(old)
        print("ok backed up and turned the old Master off")

        new = self.container("new-master", "master", "Fresh install")
        run("docker", "cp", f"/tmp/{self.run_id}.sshccbak", f"{new}:/tmp/office.sshccbak")
        restored = json.loads(self.exec(new, "sh", "-c", "sshcc restore /tmp/office.sshccbak", stdin=PASSPHRASE + "\ny\n").stdout)
        assert restored["hosts"] == 2, restored
        time.sleep(1)  # the service exits to load the restored identity; start it again as systemd would
        self.start_service(new)
        assert self.request(new, "system.status")["label"] == "Office Master"
        assert self.reaches(new, "web-1"), "the restored Master reaches the Target with no change on it"
        print("ok restored on a fresh Master, the Target answers")

        self.exec(new, "sh", "-c", "cat > /tmp/old.key && chmod 600 /tmp/old.key", stdin=old_key)
        assert '"ok":true' in self.ssh_with(new, "/tmp/old.key", "web-1", node_id).stdout.replace(" ", ""), "the old key works before the change"
        self.exec(offline, "pkill", "-x", "sshd")  # web-2 is offline while the keys change
        self.request(new, "keys.rotate")
        for _ in range(120):
            status = self.request(new, "keys.rotation_status")
            if status["moved"] == 1 and status["oldKey"] == 1 and status["lastPassAt"]:
                break
            time.sleep(1)
        waiting = [host for host in status["hosts"] if host["targetId"] == "web-2"]
        assert status["active"] and waiting[0]["state"] == "pending" and waiting[0]["error"], json.dumps(status, indent=2)
        print("ok web-1 moved, offline web-2 still on the old key")
        run("docker", "exec", "-d", offline, "/usr/sbin/sshd", "-D", "-e")  # back online: moved on the service's own retry
        for _ in range(150):
            status = self.request(new, "keys.rotation_status")
            if not status["active"]:
                break
            time.sleep(1)
        assert not status["active"] and status["moved"] == 2, json.dumps(status, indent=2)
        print("ok web-2 moved when it came back, with no action")
        assert self.reaches(new, "web-1"), "the Master reaches the Target with the new key"
        refused = self.ssh_with(new, "/tmp/old.key", "web-1", node_id)
        assert '"ok":true' not in refused.stdout.replace(" ", ""), "the old key is refused: " + refused.stdout
        lines = [line for line in self.exec(target, "cat", "/etc/ssh/sshcc/sshcc-bridge.authorized_keys").stdout.splitlines() if line.strip()]
        new_public = self.exec(new, "cat", "/var/lib/sshcc/keys/lab.ed25519.pub").stdout.split()[1]
        assert len(lines) == 1 and new_public in lines[0], lines
        assert self.request(new, "backup.status")["backupBeforeKeys"] is True
        print("ok Change keys: new key works, old key refused, one line on the Target")

    def stop(self):
        for name in self.containers:
            run("docker", "rm", "-f", name, check=False)
        run("docker", "network", "rm", self.network, check=False)
        run("rm", "-f", f"/tmp/{self.run_id}.sshccbak", check=False)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--keep", action="store_true", help="keep the containers after a failure, to look at them")
    args = parser.parse_args()
    harness = Harness(args.image)
    ok = False
    try:
        harness.run_checks()
        ok = True
        print(json.dumps({"ok": True}))
    finally:
        if ok or not args.keep:
            harness.stop()
        else:
            print("kept: " + " ".join(harness.containers) + " on network " + harness.network)


if __name__ == "__main__":
    main()
