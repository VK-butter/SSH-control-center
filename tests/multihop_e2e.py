#!/usr/bin/env python3
"""Multi-hop merge over real OpenSSH, in owned Docker containers only.

    docker build -f tests/containers/Dockerfile -t sshcc-e2e:local .
    python3 tests/multihop_e2e.py --image sshcc-e2e:local

Masters A → B → C and one Ubuntu Target T behind C, each pair on its own internal network, so A can reach B only,
B reaches C, and only C reaches T. B merges C, then A merges B; A must then see T two Masters away with no other action.
From A, through B and C: run a job and read its output, write a file and read it back, and open a Shell stream to
T (it reaches T's terminal service, which needs a logged-in Owner the containers don't have). Each relaying Master's audit log must hold the path. B then gives A View only: the job and the Shell are
refused with E_MERGE_ROLE. Finally B is paused: A's request must say B didn't answer in time. Every container and
network is removed at the end.
"""
import argparse
import base64
import json
import time

from rotation_e2e import Harness, run


class MultiHop(Harness):
    def __init__(self, image):
        super().__init__(image)
        self.networks = [self.network + "-" + n for n in ("ab", "bc", "ct")]

    def container_on(self, alias, role, label, network):
        """A container on `network`. A Master that others merge is also their Target ("both"), so its bridge user
        must be allowed, as for a plain Target."""
        name = f"{self.network}-{alias}"
        run("docker", "run", "-d", "--name", name, "--hostname", alias, "--network", network, "--network-alias", alias, self.image)
        self.containers.append(name)
        args = ["sshcc", "init", "--role", role, "--owner", "0", "--label", label]
        if role in ("target", "both"):
            args += ["--allow-principal", "1100", "--execution-mode", "service"]
        self.exec(name, *args)
        self.start_service(name)
        return name

    def connect(self, name, network, alias):
        run("docker", "network", "connect", "--alias", alias, network, name)

    def audit(self, name):
        text = self.exec(name, "sh", "-c", "cat /var/lib/sshcc/audit.jsonl 2>/dev/null || true").stdout
        return [json.loads(line) for line in text.splitlines() if line.strip()]

    def wait_job(self, master, target, job):
        for _ in range(120):
            status = self.request(master, "jobs.status", {"targetId": target, "jobId": job})
            if status["status"] in ("succeeded", "failed", "cancelled", "interrupted"):
                return status
            time.sleep(0.5)
        raise RuntimeError("job did not finish")

    def refused(self, name, method, params):
        result = self.exec(name, "sshcc", "request", method, "--params", json.dumps(params), check=False)
        assert result.returncode != 0, f"{method} should have been refused: {result.stdout}"
        return result.stderr + result.stdout

    def run_checks(self):
        ab, bc, ct = self.networks
        for network in self.networks:
            run("docker", "network", "create", "--internal", network)
        a = self.container_on("a", "master", "Office", ab)
        b = self.container_on("b", "both", "Branch", ab)
        self.connect(b, bc, "b")
        c = self.container_on("c", "both", "Lab", bc)
        self.connect(c, ct, "c")
        t = self.container_on("t", "target", "t", ct)
        # Pairing order: C pairs T, B pairs C and merges it, A pairs B and merges it.
        self.enroll(c, t, "t")
        self.enroll(b, c, "c")
        self.request(b, "merges.add", {"targetId": "c"})
        self.enroll(a, b, "b")
        self.request(a, "merges.add", {"targetId": "b"})
        hosts = {row["id"]: row for row in self.request(a, "targets.list")}
        far = hosts.get("b/c/t")
        assert far, f"T two Masters away is listed after the merge: {sorted(hosts)}"
        assert far["path"] == ["Branch", "Lab"] and far["hops"] == 2 and far["access"] == "full", far
        assert all(not row["id"].startswith("b/c/") or row["id"] == "b/c/t" for row in hosts.values() if row.get("relayed")), sorted(hosts)

        # A job, its output, a file round trip.
        job = self.request(a, "jobs.start", {"targetId": "b/c/t", "script": "hostname", "cwd": "/"})["id"]
        status = self.wait_job(a, "b/c/t", job)
        assert status["status"] == "succeeded", status
        out = self.request(a, "jobs.logs", {"targetId": "b/c/t", "jobId": job, "stream": "stdout", "offset": 0})
        assert out["content"].strip() == "t", out
        content = base64.b64encode(b"hello from Office\n").decode()
        self.request(a, "files.write", {"targetId": "b/c/t", "path": "/tmp/multihop.txt", "contentBase64": content})
        read = self.request(a, "files.read", {"targetId": "b/c/t", "path": "/tmp/multihop.txt", "offset": 0, "download": True})
        assert base64.b64decode(read["contentBase64"]) == b"hello from Office\n", read

        # A Shell on T through B and C: the stream is opened hop by hop and reaches T's own terminal service. These
        # containers have no logged-in Owner (no logind), so T answers that a Shell needs owner-session mode: that
        # answer can only come from T. Typing in a real Shell is checked on real machines (end-of-roadmap session).
        shell = self.exec(a, "sh", "-c", "printf 'exit\\n' | timeout 60 script -qec 'sshcc shell b/c/t' /dev/null", check=False)
        assert "select owner-session" in shell.stdout + shell.stderr, shell.stdout + shell.stderr

        # The path in each relaying Master's audit log.
        on_b = [l for l in self.audit(b) if l["action"] == "jobs.start"]
        on_c = [l for l in self.audit(c) if l["action"] == "jobs.start"]
        assert on_b and on_b[-1]["channel"] == "merge: Office", on_b
        assert on_c and on_c[-1]["channel"] == "merge: Office → Branch", on_c

        # View only, set on B for A: run and Shell refused, reads still work.
        a_id = self.request(a, "system.status")["nodeId"]
        self.request(b, "relays.set", {"masterId": a_id, "role": "view"})
        self.request(a, "merges.sync", {"targetId": "b"})
        assert {row["id"]: row for row in self.request(a, "targets.list")}["b/c/t"]["access"] == "view"
        error = self.refused(a, "jobs.start", {"targetId": "b/c/t", "script": "id", "cwd": "/"})
        assert "E_MERGE_ROLE" in error and "Branch gives this Master View only" in error, error
        error = self.refused(a, "terminal.open", {"targetId": "b/c/t"})
        assert "E_MERGE_ROLE" in error, error
        assert isinstance(self.request(a, "jobs.list", {"targetId": "b/c/t"}), list)

        # B stops answering: A names it.
        run("docker", "pause", b)
        started = time.time()
        error = self.refused(a, "jobs.list", {"targetId": "b/c/t"})
        took = time.time() - started
        run("docker", "unpause", b)
        assert "Branch didn't answer in time" in error, error
        assert took < 75, took
        print(json.dumps({"ok": True, "silentHopSeconds": round(took, 1)}))

    def stop(self):
        super().stop()
        for network in self.networks:
            run("docker", "network", "rm", network, check=False)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--keep", action="store_true", help="keep the containers after a failure, to look at them")
    args = parser.parse_args()
    harness = MultiHop(args.image)
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
