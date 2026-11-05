#!/usr/bin/env python3
"""The 6-digit compare over real OpenSSH, in owned Docker containers only.

    python3 tests/pairing_e2e.py --image sshcc-e2e:local

A Master creates a pairing request. On the new machine, Approve is done the way the installers do it: accept the
Master's bundle, put its forced-command key line in place, and write the pairing code with `configure
pairing-receipt` (which also keeps it for the Master to fetch). The Master then fetches the code itself
(`pairing fetch`: ssh-keyscan, then the bridge's `pairing.receipt`), and its 6 digits must equal the ones the new
machine prints (`pairing short-code`). "Yes, same number" registers the host, which then answers jobs. A second
request is fetched before Approve ("waiting") and refused with "No, it's different". Containers are removed.
"""
import argparse
import json
import time

from rotation_e2e import Harness, run


class Pairing(Harness):
    def approve(self, master, target, alias, request):
        """What the installer does after the person clicks Approve."""
        envelope = request["envelope"]
        self.exec(target, "sh", "-c", "rm -f /etc/ssh/ssh_host_* && ssh-keygen -A >/dev/null && usermod -p '*' sshcc-bridge && install -d -m 0755 /etc/ssh/sshcc")
        config = ("Port 22\nHostKey /etc/ssh/ssh_host_ed25519_key\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
                  "PermitRootLogin no\nUsePAM no\nPidFile /run/sshd.pid\nMatch User sshcc-bridge\n"
                  "    AuthorizedKeysFile /etc/ssh/sshcc/sshcc-bridge.authorized_keys\n")
        self.exec(target, "sh", "-c", "mkdir -p /run/sshd && cat > /etc/ssh/sshd_config", stdin=config)
        run("docker", "exec", "-d", target, "/usr/sbin/sshd", "-D", "-e")
        enrolled = self.request(target, "enrollment.accept", {"bundle": envelope["bundle"]})
        line = (f'restrict,command="/usr/bin/sshcc bridge --master-id {enrolled["id"]} --master-key {enrolled["fingerprint"]}" '
                f'{enrolled["publicKey"]} sshcc:master:{enrolled["id"]}\n')
        self.exec(target, "sh", "-c", "cat > /etc/ssh/sshcc/sshcc-bridge.authorized_keys && chmod 0644 /etc/ssh/sshcc/sshcc-bridge.authorized_keys", stdin=line)
        host_key = self.exec(target, "cat", "/etc/ssh/ssh_host_ed25519_key.pub").stdout.strip()
        bundle = envelope["bundle"]
        registration = self.request(target, "enrollment.registration", {"targetId": bundle["targetId"], "tenant": bundle["tenant"], "os": "linux",
                                                                         "user": bundle["user"], "routes": bundle["routes"], "hostKeys": [host_key]})
        self.exec(target, "sh", "-c", "cat > /tmp/envelope.json", stdin=json.dumps(envelope))
        self.exec(target, "sh", "-c", "cat > /tmp/registration.json", stdin=json.dumps(registration))
        self.exec(target, "sshcc", "configure", "pairing-receipt", "--request", "/tmp/envelope.json", "--registration", "/tmp/registration.json", "--output", "/tmp/pairing-code.txt")
        return self.exec(target, "sshcc", "pairing", "short-code", "--code-file", "/tmp/pairing-code.txt").stdout.strip()

    def fetch_until(self, master, request_id, wanted, seconds=60):
        for _ in range(seconds):
            reply = json.loads(self.exec(master, "sshcc", "pairing", "fetch", request_id).stdout)
            if reply["state"] == wanted:
                return reply
            time.sleep(1)
        raise AssertionError(f"pairing fetch never became {wanted}: {reply}")

    def run_checks(self):
        run("docker", "network", "create", "--internal", self.network)
        master = self.container("master", "master", "Office")
        target = self.container("web-1", "target", "web-1")
        request = self.request(master, "pairing.create", {"label": "web-1", "tenant": "lab", "os": "linux", "address": "web-1", "targetId": "web-1"})
        request_id = request["requestId"]
        # Before Approve the new machine answers nothing yet (no SSH server, or no code): keep waiting.
        assert json.loads(self.exec(master, "sshcc", "pairing", "fetch", request_id).stdout)["state"] == "waiting"
        on_target = self.approve(master, target, "web-1", request)
        ready = self.fetch_until(master, request_id, "ready")
        assert ready["digits"] == on_target, (ready["digits"], on_target)
        assert ready["preview"]["targetId"] == "web-1", ready
        self.exec(master, "sshcc", "pairing", "confirm", request_id, "--same")
        hosts = self.request(master, "targets.list")
        assert any(h["id"] == "web-1" for h in hosts), hosts
        assert isinstance(self.request(master, "jobs.list", {"targetId": "web-1"}), list), "the paired host answers"
        # A second request on another machine: "No, it's different" cancels it.
        other = self.container("web-2", "target", "web-2")
        second = self.request(master, "pairing.create", {"label": "web-2", "tenant": "lab", "os": "linux", "address": "web-2", "targetId": "web-2"})
        self.approve(master, other, "web-2", second)
        self.fetch_until(master, second["requestId"], "ready")
        refused = self.exec(master, "sshcc", "pairing", "confirm", second["requestId"], "--different", check=False)
        assert refused.returncode != 0 and "E_PAIRING_MISMATCH" in refused.stderr + refused.stdout, refused.stderr
        assert not any(h["id"] == "web-2" for h in self.request(master, "targets.list"))
        print(json.dumps({"ok": True, "digits": on_target}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--keep", action="store_true", help="keep the containers after a failure, to look at them")
    args = parser.parse_args()
    harness = Pairing(args.image)
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
