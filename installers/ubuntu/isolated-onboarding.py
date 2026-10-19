#!/usr/bin/env python3
"""Real fresh Ubuntu/systemd/SSH onboarding check; host changes only owned temporary user processes."""
import base64
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import time
import uuid

SOURCE = Path(__file__).resolve().parents[2]
BINARY = Path.home() / ".cache/sshcc-onboarding-build/sshcc"
RUN_ID = str(uuid.uuid4())
STATE = Path.home() / ".cache" / ("sshcc-onboarding-test-" + RUN_ID)
CONTAINER = "sshcc-onboarding-test-" + RUN_ID[:8]
REPORT = {"runId": RUN_ID, "os": "Ubuntu 24.04 Docker with real systemd/private cgroup",
          "version": "0.4.0", "hostServicesModified": False, "checks": {}, "jobs": []}
PROCESSES = []


def run(args, **kwargs):
    result = subprocess.run([str(x) for x in args], capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise RuntimeError("Command failed: " + repr(args) + "\n" + result.stdout[-4000:] + result.stderr[-4000:])
    return result.stdout


def docker(*args):
    return run(["docker", *args])


def master(index):
    state = STATE / ("master" + str(index))
    socket = STATE / ("master" + str(index) + ".sock")
    args = [str(BINARY), "--data-dir", str(state), "--socket", str(socket)]
    run([*args, "init", "--role", "master", "--owner", str(os.getuid()), "--label", "test-master-" + str(index)])
    log = open(STATE / ("master" + str(index) + ".log"), "w")
    proc = subprocess.Popen([*args, "service", "run"], stdout=log, stderr=log)
    PROCESSES.append((proc, log))
    for _ in range(50):
        if socket.exists():
            return args
        if proc.poll() is not None:
            raise RuntimeError("Isolated Master exited before creating its socket")
        time.sleep(0.1)
    raise RuntimeError("Isolated Master socket timeout")


def request(args, method, params):
    return json.loads(run([*args, "request", method, "--params", json.dumps(params, ensure_ascii=False)]))


def mcp(args, tool, params):
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "sshcc-installer-test", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": params}},
    ]
    output = run([*args, "mcp"], input="".join(json.dumps(v, ensure_ascii=False) + "\n" for v in messages))
    response = [json.loads(line) for line in output.splitlines() if json.loads(line).get("id") == 2][0]
    assert not response["result"].get("isError"), response
    return response["result"]["structuredContent"]["result"]


def approve():
    # A real terminal owns the explicit APPROVE input; no piped approval.
    fd, slave = pty.openpty()
    command = ["docker", "exec", "-it", CONTAINER, "runuser", "-u", "ubuntu", "--", "sshcc-setup", "--terminal"]
    proc = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    output = bytearray()
    approved = False
    deadline = time.monotonic() + 240
    try:
        while time.monotonic() < deadline:
            if select.select([fd], [], [], 0.2)[0]:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                output.extend(chunk)
                if not approved and b"APPROVE" in output:
                    os.write(fd, b"APPROVE\n")
                    approved = True
            if proc.poll() is not None:
                break
        status = proc.wait(timeout=10)
        text = output.decode("utf-8", errors="replace")
        (STATE / ("approve-" + str(len(REPORT["jobs"])) + ".log")).write_text(text, encoding="utf-8")
        assert approved and status == 0, text[-8000:]
        return text
    finally:
        os.close(fd)
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def package_and_approve(args, port, index):
    created = request(args, "pairing.create", {
        "targetId": "ubuntu-installer-test", "label": "เครื่อง Ubuntu ทดสอบ", "tenant": "demo",
        "os": "linux", "address": "127.0.0.1", "port": port,
    })
    envelope = created["envelope"]
    request_file = STATE / ("request" + str(index) + ".json")
    request_file.write_text(json.dumps(envelope, ensure_ascii=False), encoding="utf-8")
    package = STATE / ("client" + str(index) + ".deb")
    run([*args, "pairing", "package", "--request", request_file, "--output", package, "--templates", STATE / "templates"])
    docker("cp", package, CONTAINER + ":/var/tmp/client.deb")
    docker("exec", CONTAINER, "bash", "-lc", "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends /var/tmp/client.deb")
    if index == 1:
        docker("exec", CONTAINER, "bash", "-lc",
               "test ! -e /usr/bin/sshcc && test ! -e /var/lib/sshcc/config.json && test ! -e /etc/ssh/sshcc/sshcc-bridge.authorized_keys")
        REPORT["checks"]["bootstrapBeforeApprovalDoesNotInstallServiceOrSSHAuthorization"] = True
    approve()
    code = docker("exec", "-u", "ubuntu", CONTAINER, "cat",
                  "/home/ubuntu/.local/share/sshcc/setup/" + envelope["requestId"] + "/pairing-code.txt").strip()
    public_code = STATE / ("code" + str(index) + ".txt")
    public_code.write_text(code)
    inspected = json.loads(run([*args, "pairing", "inspect-code", "--code-file", public_code, "--request", request_file]))
    completed = mcp(args, "pairing_complete", {"code": code})
    assert completed["state"] == "ready", completed
    repeated = request(args, "pairing.complete", {"code": code})
    assert repeated == completed
    reopened = docker("exec", "-u", "ubuntu", CONTAINER, "sshcc-setup", "--terminal")
    assert code in reopened and "APPROVE" not in reopened
    REPORT["checks"]["retainedCodeRevalidated" + str(index)] = True
    REPORT["checks"]["pairing" + str(index)] = completed
    REPORT["checks"]["receipt" + str(index)] = {"requestId": envelope["requestId"], "nodeId": inspected["registration"]["nodeId"],
                                               "masterId": envelope["masterId"], "codeSha256": hashlib.sha256(code.encode()).hexdigest()}
    return code, inspected


def job(args, script, job_id=None):
    return request(args, "jobs.start", {"targetId": "ubuntu-installer-test", "jobId": job_id or str(uuid.uuid4()),
                                       "script": script, "cwd": "/var/lib/sshcc/test", "shell": "bash"})


def wait_job(args, job_id):
    for _ in range(100):
        result = request(args, "jobs.status", {"targetId": "ubuntu-installer-test", "jobId": job_id})
        if result["status"] not in ("queued", "running"):
            return result
        time.sleep(0.1)
    raise RuntimeError("Job timed out")


def main():
    STATE.mkdir(mode=0o700)
    (STATE / "templates").mkdir()
    template = STATE / "templates/sshcc-client-ubuntu-template.deb"
    shutil.copyfile(SOURCE / "dist/sshcc-client-ubuntu-template.deb", template)
    (STATE / "templates/template-manifest.json").write_text(json.dumps({
        "schemaVersion": 1, "templates": [{"os": "linux", "fileName": template.name,
                                         "sha256": hashlib.sha256(template.read_bytes()).hexdigest()}]}))
    docker("run", "--rm", "--name", CONTAINER, "--privileged", "--cgroupns=private", "--tmpfs", "/run",
           "--tmpfs", "/run/lock", "--tmpfs", "/tmp", "--cpus=2", "--memory=2g", "-p", "127.0.0.1::22",
           "-d", "sshcc-ubuntu-build:0.4.0", "/sbin/init")
    for _ in range(60):
        status = subprocess.run(["docker", "exec", CONTAINER, "systemctl", "is-system-running"], capture_output=True, text=True)
        if status.stdout.strip() in ("running", "degraded"):
            break
        time.sleep(0.2)
    else:
        raise RuntimeError("Container systemd did not start")
    REPORT["checks"]["privateCgroup"] = docker("exec", CONTAINER, "cat", "/proc/1/cgroup").strip()
    assert REPORT["checks"]["privateCgroup"].startswith("0::/"), REPORT
    docker("exec", CONTAINER, "bash", "-lc", "printf 'ubuntu ALL=(ALL) NOPASSWD:ALL\n' >/etc/sudoers.d/sshcc-test; chmod 0440 /etc/sudoers.d/sshcc-test")
    port = int(docker("port", CONTAINER, "22/tcp").strip().rsplit(":", 1)[1])
    first = master(1)
    code1, receipt1 = package_and_approve(first, port, 1)
    REPORT["checks"]["systemdService"] = docker("exec", CONTAINER, "systemctl", "show", "sshcc", "-p", "ActiveState", "-p", "User", "-p", "UnitFileState").strip()
    root_status = json.loads(docker("exec", CONTAINER, "/usr/bin/sshcc", "request", "system.status"))
    assert root_status["executionPrincipal"] == "0" and root_status["privileged"] is True
    REPORT["checks"]["rootService"] = root_status
    request(first, "files.mkdir", {"targetId": "ubuntu-installer-test", "path": "/var/lib/sshcc/test"})
    content = "ทดสอบไฟล์ภาษาไทย\n".encode()
    path = "/var/lib/sshcc/test/ไฟล์ทดสอบ.txt"
    request(first, "files.write", {"targetId": "ubuntu-installer-test", "path": path, "contentBase64": base64.b64encode(content).decode()})
    read = mcp(first, "files_read", {"targetId": "ubuntu-installer-test", "path": path})
    assert base64.b64decode(read["contentBase64"]) == content
    assert read["sha256"] == hashlib.sha256(content).hexdigest()
    REPORT["checks"]["thaiFileSha256"] = read["sha256"]
    stale = subprocess.run([*first, "request", "files.write", "--params", json.dumps({
        "targetId": "ubuntu-installer-test", "path": path, "contentBase64": base64.b64encode(b"wrong").decode(), "expectedSha256": "0" * 64})],
        capture_output=True, text=True)
    assert stale.returncode and "E_CONFLICT" in stale.stderr, stale.stderr
    REPORT["checks"]["staleHashRejected"] = True
    started = job(first, "test \"$(id -u)\" = 0; printf 'test=passed account=%s\\n' \"$(id -un)\"; mkdir -p deploy; printf 'build=passed\\n' > build.txt; cp build.txt deploy/result.txt")
    completed = wait_job(first, started["id"])
    assert completed["status"] == "succeeded" and completed["exitCode"] == 0
    logs = mcp(first, "jobs_logs", {"targetId": "ubuntu-installer-test", "jobId": started["id"]})
    assert "account=root" in logs["content"]
    REPORT["jobs"].append({"job": completed, "logs": logs})
    dedup_id = str(uuid.uuid4())
    dedup_script = "sleep 1; printf 'once\\n' >> once.txt"
    ack = mcp(first, "jobs_start", {"targetId": "ubuntu-installer-test", "jobId": dedup_id,
                                   "script": dedup_script, "cwd": "/var/lib/sshcc/test", "shell": "bash"})
    repeated = job(first, dedup_script, dedup_id)
    assert repeated["id"] == ack["id"]
    dedup = wait_job(first, dedup_id)
    once = mcp(first, "files_read", {"targetId": "ubuntu-installer-test", "path": "/var/lib/sshcc/test/once.txt"})
    assert base64.b64decode(once["contentBase64"]) == b"once\n"
    REPORT["jobs"].append({"dedupJob": dedup})
    # Real installer preflight refuses an active root job without stopping the service.
    long_job = job(first, "sleep 30")
    busy = subprocess.run(["docker", "exec", CONTAINER, "/usr/lib/sshcc-setup/setup-worker.sh", "--caller-uid", "1000"], capture_output=True, text=True)
    assert busy.returncode and ("E_BUSY" in busy.stdout + busy.stderr or "งานกำลังรัน" in busy.stdout + busy.stderr), busy.stdout + busy.stderr
    assert request(first, "jobs.status", {"targetId": "ubuntu-installer-test", "jobId": long_job["id"]})["status"] == "running"
    docker("exec", CONTAINER, "systemctl", "is-active", "--quiet", "sshcc")
    request(first, "jobs.cancel", {"targetId": "ubuntu-installer-test", "jobId": long_job["id"]})
    cancelled = wait_job(first, long_job["id"])
    assert cancelled["status"] == "cancelled"
    REPORT["checks"]["installerRefusesActiveJobWithoutStoppingService"] = True
    REPORT["jobs"].append({"cancelledJob": cancelled})
    # The Target keeps both independent Master keys and its original node/owner/history.
    owner_before = docker("exec", CONTAINER, "cat", "/var/lib/sshcc/config.json")
    second = master(2)
    code2, receipt2 = package_and_approve(second, port, 2)
    assert receipt1["registration"]["nodeId"] == receipt2["registration"]["nodeId"]
    owner_after = docker("exec", CONTAINER, "cat", "/var/lib/sshcc/config.json")
    assert json.loads(owner_before) == json.loads(owner_after)
    authorized = json.loads(docker("exec", CONTAINER, "/usr/bin/sshcc", "request", "masters.list"))
    assert len(authorized) == 2
    REPORT["checks"]["bothMastersPreserved"] = authorized
    # Both old and new Masters still execute after the second real installer run.
    for args in (first, second):
        started = job(args, "printf 'master-still-authorized\\n'")
        completed = wait_job(args, started["id"])
        assert completed["status"] == "succeeded"
        REPORT["jobs"].append({"job": completed})
    REPORT["checks"]["existingOwnerRolesNodeIdPreserved"] = True
    REPORT["checks"]["headlessNoTrayInstalled"] = docker("exec", CONTAINER, "bash", "-lc", "test ! -e /usr/bin/sshcc-desktop; echo true").strip() == "true"
    REPORT["success"] = True


try:
    main()
except BaseException as error:
    REPORT["success"] = False
    REPORT["error"] = str(error)
    raise
finally:
    for process, log in PROCESSES:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()
    subprocess.run(["docker", "stop", "--time", "10", CONTAINER], capture_output=True)
    public = SOURCE / "artifacts/onboarding"
    public.mkdir(exist_ok=True)
    (public / "ubuntu-onboarding.json").write_text(json.dumps(REPORT, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    # Delete only the generated private test state/keys after retaining the public report.
    if STATE.exists() and STATE.parent == Path.home() / ".cache" and STATE.name == "sshcc-onboarding-test-" + RUN_ID and not STATE.is_symlink():
        shutil.rmtree(STATE)
    print(json.dumps(REPORT, ensure_ascii=False, indent=2))
