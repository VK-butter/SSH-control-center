#!/usr/bin/env python3
"""Isolated CLI/service/MCP smoke: never installs a service or contacts a target."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sshcc", required=True, type=Path)
    args = parser.parse_args()
    exe = str(args.sshcc.resolve(strict=True))
    work = Path(tempfile.mkdtemp(prefix="sshcc-smoke-"))
    state = work / "state"
    env = os.environ.copy()
    env["SSHCC_DATA_DIR"] = str(state)
    env["SSHCC_SOCKET"] = (r"\\.\pipe\sshcc-test-" + str(uuid.uuid4())) if os.name == "nt" else str(work / "control.sock")
    env["PYTHONUTF8"] = "1"
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    service = None
    log = (work / "service.log").open("wb")

    def cli(*argv, success=True):
        result = subprocess.run([exe, *argv], env=env, text=True, encoding="utf-8", capture_output=True, timeout=20, **options)
        if success:
            assert result.returncode == 0, result.stderr
            return json.loads(result.stdout)
        assert result.returncode != 0, f"Expected failure: {argv}"
        return result.stderr

    def rpc(method, params=None, success=True):
        # Data travels through a file, avoiding the Windows command-line quoting layer.
        params_file = work / ("params-" + str(uuid.uuid4()) + ".json")
        params_file.write_text(json.dumps(params or {}), encoding="utf-8")
        return cli("request", method, "--params-file", str(params_file), success=success)

    def start_service():
        nonlocal service
        service = subprocess.Popen([exe, "service", "run"], env=env, stdout=log, stderr=log, **options)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert service.poll() is None, (work / "service.log").read_text(errors="replace")
            try:
                return rpc("system.status")
            except AssertionError:
                time.sleep(0.1)
        raise AssertionError("Service did not become ready")

    def wait_job(job_id, expected, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            job = rpc("runner.status", {"jobId": job_id})
            if job["status"] in {"succeeded", "failed", "cancelled", "interrupted"}:
                assert job["status"] == expected, job
                return job
            time.sleep(0.1)
        raise AssertionError("Job timeout")

    try:
        identity = cli("identity")["principal"]
        config = cli("init", "--role", "both", "--owner", identity, "--label", "isolated smoke")
        repeated = cli("init", "--role", "both", "--owner", identity, "--label", "isolated smoke")
        assert config["nodeId"] == repeated["nodeId"], "init changed persistent identity"
        status = start_service()
        assert status["nodeId"] == config["nodeId"]

        shell = "powershell" if os.name == "nt" else "bash"
        script = ("Start-Sleep -Milliseconds 300; Write-Output $env:SSHCC_SMOKE; [Console]::Error.WriteLine('error stream')"
                  if os.name == "nt" else 'sleep 0.3; printf "%s\\n" "$SSHCC_SMOKE"; printf "error stream\\n" >&2')
        jobs = []
        master_ids = [str(uuid.uuid4()), str(uuid.uuid4())]
        for index in range(5):
            payload = dict(jobId=str(uuid.uuid4()), masterId=master_ids[index % 2], script=script,
                           cwd=str(work), shell=shell, env={"SSHCC_SMOKE": "hello quotes ' \" spaces"})
            first = rpc("runner.start", payload)
            retry = rpc("runner.start", payload)
            assert first["id"] == retry["id"] == payload["jobId"]
            rpc("runner.start", {**payload, "script": script + "\n# changed"}, success=False)
            jobs.append(payload)
        # Individual CLI clients have exited; Target service still owns all five jobs.
        for payload in jobs:
            record = wait_job(payload["jobId"], "succeeded")
            assert record["masterId"] == payload["masterId"] and record["exitCode"] == 0
            stdout = rpc("runner.logs", dict(jobId=payload["jobId"], stream="stdout", offset=0, limit=4096))
            stderr = rpc("runner.logs", dict(jobId=payload["jobId"], stream="stderr", offset=0, limit=4096))
            assert "hello quotes" in stdout["content"] and "error stream" in stderr["content"]
        assert len(rpc("runner.list")) == 5

        dest = work / "files with spaces" / "roundtrip.bin"
        rpc("fs.mkdir", {"path": str(dest.parent)})
        data = bytes(range(256)) * 16
        encoded = base64.b64encode(data).decode()
        rpc("fs.write", {"path": str(dest), "contentBase64": encoded})
        actual = rpc("fs.read", {"path": str(dest), "offset": 0, "limit": len(data)})
        assert base64.b64decode(actual["contentBase64"]) == data
        assert actual["sha256"] == hashlib.sha256(data).hexdigest()
        rpc("fs.write", {"path": str(dest), "contentBase64": "", "expectedSha256": "0" * 64}, success=False)
        rpc("fs.remove", {"path": str(dest.parent)}, success=False)
        rpc("fs.remove", {"path": str(dest)})
        rpc("fs.remove", {"path": str(dest.parent)})

        slow = "Start-Sleep -Seconds 60" if os.name == "nt" else "sleep 60"
        payload = dict(jobId=str(uuid.uuid4()), masterId=master_ids[0], script=slow, cwd=str(work), shell=shell)
        rpc("runner.start", payload)
        rpc("runner.cancel", {"jobId": payload["jobId"]})
        wait_job(payload["jobId"], "cancelled")

        payload["jobId"] = str(uuid.uuid4())
        rpc("runner.start", payload)
        deadline = time.monotonic() + 5
        while rpc("runner.status", {"jobId": payload["jobId"]})["status"] != "running":
            assert time.monotonic() < deadline
            time.sleep(0.1)
        service.kill()
        service.wait(timeout=10)
        start_service()
        wait_job(payload["jobId"], "interrupted")
        retry = rpc("runner.start", payload)
        assert retry["status"] == "interrupted", "restart silently reran an administrative script"
        rpc("bridge.request", {"masterId": master_ids[0], "expectedTargetId": config["nodeId"],
                               "request": {"method": "system.status", "params": {}}}, success=False)

        mcp_input = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "targets_list", "arguments": {}}},
        ]
        process = subprocess.run([exe, "mcp"], env=env, text=True, encoding="utf-8", capture_output=True,
                                 input="\n".join(json.dumps(v) for v in mcp_input) + "\n", timeout=15, **options)
        assert process.returncode == 0, process.stderr
        replies = [json.loads(line) for line in process.stdout.splitlines()]
        assert len(replies) == 3 and len(replies[1]["result"]["tools"]) >= 10
        assert replies[2]["result"]["isError"] is False
        print(json.dumps({"passed": True, "artifacts": str(work), "checks": [
            "idempotent init", "five concurrent local jobs/two Master IDs", "duplicate/conflict",
            "detached client", "stdout/stderr", "binary file/hash guard", "nonrecursive removal",
            "cancel", "crash/interrupted/no replay", "unenrolled bridge denied", "MCP initialize/list/call"
        ], "notCovered": ["real SSH", "remote machines", "SYSTEM/root installation", "IPC from another OS identity",
                          "tray", "resource budget", "reboot/logout", "installer upgrade/rollback"]}, indent=2))
    finally:
        if service and service.poll() is None:
            try:
                rpc("system.stop")
                service.wait(timeout=10)
            except Exception:
                service.kill()
                service.wait(timeout=10)
        log.close()


if __name__ == "__main__":
    main()
