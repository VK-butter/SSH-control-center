#!/usr/bin/env python3
"""Real SSH integration tests in owned Ubuntu Docker containers only."""
from __future__ import annotations
import argparse
import base64
import concurrent.futures
import hashlib
import json
import pathlib
import subprocess
import tempfile
import time
import traceback
import uuid

class Harness:
    def __init__(self, image, output):
        self.image, self.output = image, pathlib.Path(output).resolve()
        self.run_id = uuid.uuid4().hex[:12]
        self.network = "sshcc-e2e-" + self.run_id
        self.containers = []
        self.report = {"runId": self.run_id, "image": image, "startedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                       "environment": "Ubuntu 24.04 containers; real OpenSSH; synthetic LAN/Tailscale/public route labels on one internal network",
                       "checks": [], "passed": False}
        self.output.mkdir(parents=True, exist_ok=True)

    def command(self, args, *, stdin=None, check=True, timeout=120):
        result = subprocess.run(args, input=stdin, text=True, capture_output=True, timeout=timeout, encoding="utf-8")
        if check and result.returncode:
            raise RuntimeError(f"{args[:4]} exited {result.returncode}: {result.stderr.strip()} {result.stdout.strip()}")
        return result

    def docker(self, *args, **kwargs):
        return self.command(["docker", *map(str, args)], **kwargs)

    def execute(self, container, *args, stdin=None, check=True, timeout=120, user=None):
        command = ["exec"]
        if stdin is not None:
            command.append("-i")
        if user:
            command += ["--user", str(user)]
        return self.docker(*command, container, *args, stdin=stdin, check=check, timeout=timeout)

    def rpc(self, container, method, params=None, *, check=True, user=None):
        result = self.execute(container, "sshcc", "request", method, "--params",
                              json.dumps(params or {}, separators=(",", ":")), check=check, user=user)
        return json.loads(result.stdout) if result.returncode == 0 else result

    def write(self, container, path, data, mode=0o600):
        if isinstance(data, dict):
            data = json.dumps(data)
        script = "import os,pathlib,sys;p=pathlib.Path(sys.argv[1]);p.parent.mkdir(parents=True,exist_ok=True);p.write_text(sys.stdin.read());os.chmod(p,int(sys.argv[2]))"
        self.execute(container, "python3", "-c", script, path, str(mode), stdin=data)

    def container(self, alias, role):
        name = self.network + "-" + alias
        args = ["run", "-d", "--name", name, "--label", "sshcc.e2e.run=" + self.run_id,
                "--network", self.network, "--network-alias", alias, "--network-alias", alias + "-alternate",
                self.image]
        self.docker(*args)
        self.containers.append(name)
        arguments = ["sshcc", "init", "--role", role, "--owner", "0", "--label", alias]
        if role == "target":
            arguments += ["--allow-principal", "1100"]
        self.execute(name, *arguments)
        self.start_service(name)
        return name

    def start_service(self, container):
        self.docker("exec", "-d", container, "sh", "-c", "exec sshcc service run >>/tmp/service.log 2>&1")
        for _ in range(100):
            result = self.execute(container, "sshcc", "service", "status", check=False)
            if result.returncode == 0:
                return
            time.sleep(0.05)
        raise AssertionError("service did not start: " + self.execute(container, "cat", "/tmp/service.log").stdout)

    def check(self, name, action):
        started = time.monotonic()
        try:
            detail = action()
            self.report["checks"].append({"name": name, "passed": True, "seconds": round(time.monotonic()-started, 3), "detail": detail})
            print("PASS " + name, flush=True)
        except Exception as error:
            self.report["checks"].append({"name": name, "passed": False, "seconds": round(time.monotonic()-started, 3), "error": str(error)})
            raise

    def wait_job(self, master, target_id, job_id, expected="succeeded", timeout=30):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            job = self.rpc(master, "jobs.status", {"targetId": target_id, "jobId": job_id})
            if job["status"] not in ("queued", "running"):
                assert job["status"] == expected, job
                return job
            time.sleep(0.15)
        raise AssertionError("job did not complete: " + job_id)

    def enroll(self, master, target, alias, append=True):
        routes = [{"kind": "lan", "address": alias, "port": 22}]
        bundle = self.rpc(master, "enrollment.export", {"targetId": alias, "tenant": "demo", "os": "linux",
                                                     "user": "sshcc-bridge", "routes": routes})
        enrolled = self.rpc(target, "enrollment.accept", {"bundle": bundle})
        forced = f'/usr/bin/sshcc bridge --master-id {enrolled["id"]} --master-key {enrolled["fingerprint"]}'
        line = 'restrict,command="' + forced + '" ' + enrolled["publicKey"] + " sshcc:" + enrolled["id"] + "\n"
        if append:
            existing = self.execute(target, "cat", "/home/sshcc-bridge/.ssh/authorized_keys", check=False).stdout
            self.write(target, "/home/sshcc-bridge/.ssh/authorized_keys", existing + line)
            self.execute(target, "chown", "-R", "sshcc-bridge:sshcc-bridge", "/home/sshcc-bridge/.ssh")
            self.execute(target, "chmod", "700", "/home/sshcc-bridge/.ssh")
        host_key = self.execute(target, "cat", "/etc/ssh/ssh_host_ed25519_key.pub").stdout.strip()
        registration = self.rpc(target, "enrollment.registration", {"targetId": alias, "tenant": "demo", "os": "linux",
                                                                 "user": "sshcc-bridge", "routes": routes, "hostKeys": [host_key]})
        self.rpc(master, "targets.import", {"registration": registration})
        return bundle, registration

    def setup(self):
        self.docker("network", "create", "--internal", "--label", "sshcc.e2e.run=" + self.run_id, self.network)
        self.master_a = self.container("master-a", "master")
        self.master_b = self.container("master-b", "master")
        self.targets, self.registrations, self.bundles = {}, {}, {}
        for n in range(1, 6):
            alias = "target" + str(n)
            target = self.container(alias, "target")
            self.targets[alias] = target
            # Every fixture gets a unique host key; keys in the image are not reused.
            self.execute(target, "sh", "-c", "rm -f /etc/ssh/ssh_host_* && ssh-keygen -A >/dev/null && usermod -p '*' sshcc-bridge")
            self.write(target, "/etc/ssh/sshd_config", "Port 22\nHostKey /etc/ssh/ssh_host_ed25519_key\n"
                       "PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nUsePAM no\n"
                       "AuthorizedKeysFile .ssh/authorized_keys\nLogLevel VERBOSE\nPidFile /run/sshd.pid\n")
            self.docker("exec", "-d", target, "/usr/sbin/sshd", "-D", "-e")
            bundle, registration = self.enroll(self.master_a, target, alias)
            self.bundles[alias], self.registrations[alias] = bundle, registration
            self.enroll(self.master_b, target, alias)
        with tempfile.TemporaryDirectory(prefix="sshcc-e2e-artifact-") as directory:
            binary = pathlib.Path(directory) / "sshcc"
            self.docker("cp", self.master_a + ":/usr/bin/sshcc", binary)
            data = binary.read_bytes()
            (self.output / "sshcc").write_bytes(data)
            self.report["binarySha256"] = hashlib.sha256(data).hexdigest()
        self.report["runtime"] = self.execute(self.master_a, "cat", "/etc/os-release").stdout
        self.report["cliVersion"] = self.execute(self.master_a, "sshcc", "--version").stdout.strip()

    def run_checks(self):
        def all_doctors():
            for master in (self.master_a, self.master_b):
                for alias in self.targets:
                    result = self.rpc(master, "targets.doctor", {"targetId": alias})
                    assert result["ok"], result
            return "Both independent Masters authenticate to all five real SSH Targets"
        self.check("two_masters_five_targets", all_doctors)

        def parallel_jobs():
            def launch(alias):
                job_id = str(uuid.uuid4())
                script = f"mkdir -p /tmp/project /tmp/deployed; printf 'print(42)\\n' >/tmp/project/main.py; python3 /tmp/project/main.py; cp /tmp/project/main.py /tmp/deployed/main.py; sleep 2; id -u"
                job = self.rpc(self.master_a, "jobs.start", {"targetId": alias, "jobId": job_id, "cwd": "/tmp", "script": script})
                return alias, job["id"]
            with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
                jobs = list(pool.map(launch, self.targets))
            for alias, job_id in jobs:
                self.wait_job(self.master_a, alias, job_id)
                logs = self.rpc(self.master_a, "jobs.logs", {"targetId": alias, "jobId": job_id})["content"]
                assert "42\n" in logs and logs.rstrip().endswith("0"), logs
            return {"concurrentTargets": 5, "runsAs": "root", "workflow": "create project, execute, deploy copy"}
        self.check("five_concurrent_root_build_deploy_jobs", parallel_jobs)

        def files():
            data = "hello UTF-8 ไทย\n".encode()
            self.rpc(self.master_a, "files.write", {"targetId": "target1", "path": "/tmp/transfer.txt", "contentBase64": base64.b64encode(data).decode()})
            result = self.rpc(self.master_a, "files.read", {"targetId": "target1", "path": "/tmp/transfer.txt"})
            assert base64.b64decode(result["contentBase64"]) == data
            assert result["sha256"] == hashlib.sha256(data).hexdigest()
            denied = self.rpc(self.master_a, "files.write", {"targetId": "target1", "path": "/tmp/transfer.txt",
                              "contentBase64": base64.b64encode(b"bad").decode(), "expectedSha256": "0"*64}, check=False)
            assert isinstance(denied, subprocess.CompletedProcess) and denied.returncode
            return "UTF-8 content and SHA-256 preserved; stale optimistic write rejected"
        self.check("atomic_file_transfer_and_hash_conflict", files)

        def fallback():
            registration = dict(self.registrations["target1"])
            registration["routes"] = [{"kind": "lan", "address": "127.0.0.1", "port": 1},
                                      {"kind": "tailscale", "address": "target1-alternate", "port": 22}]
            self.rpc(self.master_a, "targets.update", {"target": registration})
            result = self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})
            assert result["ok"], result
            stages = [s for s in result["stages"] if s["name"] == "ssh-runner"]
            assert len(stages) == 2 and not stages[0]["ok"] and stages[1]["ok"], result
            self.rpc(self.master_a, "targets.update", {"target": self.registrations["target1"]})
            return "Connection refusal falls back from first route to second with actual SSH"
        self.check("ordered_route_fallback", fallback)

        def disconnected():
            job_id = str(uuid.uuid4())
            params = {"targetId": "target1", "jobId": job_id, "cwd": "/tmp", "script": "echo once >>/tmp/once.txt; sleep 2; echo durable"}
            request = json.dumps({"method": "jobs.start", "params": params}) + "\n"
            code = "import socket,sys;s=socket.socket(socket.AF_UNIX);s.connect('/run/sshcc/control.sock');s.sendall(sys.stdin.buffer.read());s.close()"
            self.execute(self.master_a, "python3", "-c", code, stdin=request)
            time.sleep(0.5)
            retry = self.rpc(self.master_a, "jobs.start", params)
            assert retry["id"] == job_id
            self.wait_job(self.master_a, "target1", job_id)
            content = self.execute(self.targets["target1"], "cat", "/tmp/once.txt").stdout
            assert content == "once\n", content
            self.rpc(self.master_a, "system.stop")
            self.start_service(self.master_a)
            assert self.rpc(self.master_a, "jobs.status", {"targetId": "target1", "jobId": job_id})["status"] == "succeeded"
            return "Lost client reply, same-ID retry and Master restart executed deploy once"
        self.check("client_disconnect_retry_and_master_restart", disconnected)

        def multiple_masters():
            ids = []
            for master in (self.master_a, self.master_b):
                job = self.rpc(master, "jobs.start", {"targetId": "target2", "jobId": str(uuid.uuid4()), "cwd": "/tmp", "script": "sleep 1; echo multi"})
                ids.append(job)
            assert ids[0]["masterId"] != ids[1]["masterId"]
            for master, job in zip((self.master_a, self.master_b), ids):
                self.wait_job(master, "target2", job["id"])
            return "Concurrent jobs on one Target have separate authenticated Master ownership"
        self.check("concurrent_master_attribution", multiple_masters)

        def revocation():
            target = self.targets["target1"]
            master = self.bundles["target1"]["master"]
            self.rpc(target, "masters.revoke", {"masterId": master["id"]})
            assert not self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})["ok"]
            assert self.rpc(self.master_b, "targets.doctor", {"targetId": "target1"})["ok"]
            # Rotate enrollment metadata without removing old key line; old forced
            # command must remain rejected because its bound fingerprint is stale.
            self.execute(target, "ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", "/tmp/replacement-key")
            public = self.execute(target, "cat", "/tmp/replacement-key.pub").stdout.strip()
            changed = json.loads(json.dumps(self.bundles["target1"]))
            changed["master"]["publicKey"] = public
            self.rpc(target, "enrollment.accept", {"bundle": changed})
            assert not self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})["ok"]
            self.rpc(target, "masters.revoke", {"masterId": master["id"]})
            self.rpc(target, "enrollment.accept", {"bundle": self.bundles["target1"]})
            assert self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})["ok"]
            return "Revocation immediate; other Master remains active; stale key fingerprint stays rejected after rotation"
        self.check("revoke_and_stale_key_fingerprint", revocation)


        def proxy_jump():
            jump = self.targets["target5"]
            self.execute(jump, "sh", "-c", "useradd -m -u 1102 -s /bin/sh jump && usermod -p '*' jump")
            public = self.bundles["target5"]["master"]["publicKey"]
            self.write(jump, "/home/jump/.ssh/authorized_keys",
                       'restrict,port-forwarding,permitopen="target1:22" ' + public + "\n")
            self.execute(jump, "chown", "-R", "jump:jump", "/home/jump/.ssh")
            self.execute(jump, "chmod", "700", "/home/jump/.ssh")
            registration = json.loads(json.dumps(self.registrations["target1"]))
            registration["routes"][0]["proxyJump"] = {
                "address": "target5", "port": 22, "user": "jump",
                "hostKeys": self.registrations["target5"]["hostKeys"],
                "identityFile": "/var/lib/sshcc/keys/demo.ed25519",
            }
            self.rpc(self.master_a, "targets.update", {"target": registration})
            result = self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})
            assert result["ok"], result
            registration["routes"][0]["proxyJump"]["hostKeys"] = self.registrations["target4"]["hostKeys"]
            self.rpc(self.master_a, "targets.update", {"target": registration})
            result = self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})
            assert not result["ok"] and "HOST_KEY_MISMATCH" in json.dumps(result), result
            self.rpc(self.master_a, "targets.update", {"target": self.registrations["target1"]})
            return "Actual SSH ProxyJump works with service identity; incorrect jump host pin stops connection"
        self.check("pinned_proxy_jump_and_jump_mismatch", proxy_jump)

        def lost_ssh_reply():
            target = self.targets["target1"]
            master = self.bundles["target1"]["master"]
            enrolled = self.rpc(target, "enrollment.accept", {"bundle": self.bundles["target1"]})
            original = self.execute(target, "cat", "/home/sshcc-bridge/.ssh/authorized_keys").stdout
            forced = f'/usr/bin/sshcc bridge --master-id {master["id"]} --master-key {enrolled["fingerprint"]}'
            wrapper = (
                "import pathlib,subprocess,sys\n"
                "data=sys.stdin.buffer.read()\n"
                "result=subprocess.run(['/usr/bin/sshcc','bridge',*sys.argv[1:]],input=data,capture_output=True)\n"
                "marker=pathlib.Path('/tmp/sshcc-reply-dropped')\n"
                "if not marker.exists():\n"
                " marker.touch()\n"
                " sys.exit(255)\n"
                "sys.stdout.buffer.write(result.stdout)\n"
                "sys.stderr.buffer.write(result.stderr)\n"
                "sys.exit(result.returncode)\n"
            )
            self.write(target, "/tmp/drop-reply.py", wrapper, 0o644)
            wrapped = original.replace(forced, f'/usr/bin/python3 /tmp/drop-reply.py --master-id {master["id"]} --master-key {enrolled["fingerprint"]}')
            self.write(target, "/home/sshcc-bridge/.ssh/authorized_keys", wrapped)
            self.execute(target, "chown", "sshcc-bridge:sshcc-bridge", "/home/sshcc-bridge/.ssh/authorized_keys")
            registration = json.loads(json.dumps(self.registrations["target1"]))
            registration["routes"].append({"kind": "public", "address": "target1-alternate", "port": 22})
            self.rpc(self.master_a, "targets.update", {"target": registration})
            job_id = str(uuid.uuid4())
            job = self.rpc(self.master_a, "jobs.start", {"targetId": "target1", "jobId": job_id, "cwd": "/tmp",
                           "script": "echo once >>/tmp/ssh-reply-once.txt; sleep 1; echo completed"})
            assert job["id"] == job_id
            self.wait_job(self.master_a, "target1", job_id)
            assert self.execute(target, "cat", "/tmp/ssh-reply-once.txt").stdout == "once\n"
            self.write(target, "/home/sshcc-bridge/.ssh/authorized_keys", original)
            self.execute(target, "chown", "sshcc-bridge:sshcc-bridge", "/home/sshcc-bridge/.ssh/authorized_keys")
            self.rpc(self.master_a, "targets.update", {"target": self.registrations["target1"]})
            return "Target accepted job then SSH dropped reply; next route retried identical job ID and deploy ran once"
        self.check("lost_ssh_reply_fallback_executes_once", lost_ssh_reply)

        def wrong_host():
            bad = json.loads(json.dumps(self.registrations["target1"]))
            bad["hostKeys"] = self.registrations["target2"]["hostKeys"]
            bad["routes"] = [{"kind": "lan", "address": "target1", "port": 22},
                             {"kind": "public", "address": "target2-alternate", "port": 22}]
            self.rpc(self.master_a, "targets.remove", {"targetId": "target1"})
            self.rpc(self.master_a, "targets.import", {"registration": bad})
            result = self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})
            stages = [s for s in result["stages"] if s["name"] == "ssh-runner"]
            assert not result["ok"] and len(stages) == 1 and "HOST_KEY_MISMATCH" in stages[0]["message"], result
            self.rpc(self.master_a, "targets.remove", {"targetId": "target1"})
            self.rpc(self.master_a, "targets.import", {"registration": self.registrations["target1"]})
            return "Wrong host pin stops before trying a second otherwise reachable route"
        self.check("host_key_mismatch_stops_fallback", wrong_host)

        def wrong_identity():
            bad = json.loads(json.dumps(self.registrations["target1"]))
            bad["nodeId"] = str(uuid.uuid4())
            self.rpc(self.master_a, "targets.remove", {"targetId": "target1"})
            self.rpc(self.master_a, "targets.import", {"registration": bad})
            result = self.rpc(self.master_a, "targets.doctor", {"targetId": "target1"})
            assert not result["ok"] and "E_IDENTITY" in json.dumps(result), result
            self.rpc(self.master_a, "targets.remove", {"targetId": "target1"})
            self.rpc(self.master_a, "targets.import", {"registration": self.registrations["target1"]})
            return "Valid SSH key cannot substitute a different Target node identity"
        self.check("target_identity_binding", wrong_identity)

        def restart_target():
            job = self.rpc(self.master_a, "jobs.start", {"targetId": "target3", "jobId": str(uuid.uuid4()), "cwd": "/tmp", "script": "echo before-crash; sleep 30; echo should-not-restart"})
            time.sleep(0.3)
            self.execute(self.targets["target3"], "pkill", "-KILL", "-x", "sshcc")
            self.start_service(self.targets["target3"])
            status = self.rpc(self.master_a, "jobs.status", {"targetId": "target3", "jobId": job["id"]})
            assert status["status"] == "interrupted", status
            logs = self.rpc(self.master_a, "jobs.logs", {"targetId": "target3", "jobId": job["id"]})
            assert "before-crash" in logs["content"]
            return "Target service crash marks job interrupted and preserves preceding logs"
        self.check("target_service_restart_interrupted", restart_target)

        def local_auth():
            denied = self.rpc(self.targets["target1"], "system.status", check=False, user="1101")
            assert isinstance(denied, subprocess.CompletedProcess) and denied.returncode and "E_PERMISSION" in denied.stderr
            denied = self.rpc(self.targets["target1"], "system.status", check=False, user="1100")
            assert isinstance(denied, subprocess.CompletedProcess) and denied.returncode and "E_PERMISSION" in denied.stderr
            return "Unenrolled local UID and restricted bridge UID cannot invoke owner API"
        self.check("local_principal_rejection", local_auth)

    def cleanup(self):
        for container in reversed(self.containers):
            try:
                label = self.docker("inspect", "--format", '{{index .Config.Labels "sshcc.e2e.run"}}', container).stdout.strip()
                if label == self.run_id:
                    logs = self.execute(container, "cat", "/tmp/service.log", check=False).stdout
                    (self.output / (container + "-service.log")).write_text(logs, encoding="utf-8")
                    self.docker("rm", "-f", container)
            except Exception as error:
                self.report.setdefault("cleanupErrors", []).append(str(error))
        try:
            label = self.docker("network", "inspect", "--format", '{{index .Labels "sshcc.e2e.run"}}', self.network).stdout.strip()
            if label == self.run_id:
                self.docker("network", "rm", self.network)
        except Exception as error:
            self.report.setdefault("cleanupErrors", []).append(str(error))

    def run(self):
        try:
            self.setup()
            self.run_checks()
            self.report["passed"] = True
        except Exception:
            self.report["failure"] = traceback.format_exc()
            print(self.report["failure"], flush=True)
        finally:
            self.cleanup()
            self.report["finishedAt"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            (self.output / "ssh-e2e-report.json").write_text(json.dumps(self.report, indent=2), encoding="utf-8")
        return 0 if self.report["passed"] and not self.report.get("cleanupErrors") else 1

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--output", default="artifacts/linux")
    args = parser.parse_args()
    raise SystemExit(Harness(args.image, args.output).run())
