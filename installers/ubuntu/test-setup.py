#!/usr/bin/env python3
"""Isolated installer tests; run as root inside a disposable Ubuntu container only."""
import hashlib
import importlib.machinery
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
util = importlib.machinery.SourceFileLoader("setup_util", str(HERE / "setup-util.py")).load_module()
helper = importlib.machinery.SourceFileLoader("setup_helper", str(HERE / "setup-helper.py")).load_module()


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sshcc-setup-test-")
        self.home = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def codex(self, document):
        directory = self.home / ".codex"
        directory.mkdir()
        config = directory / "config.toml"
        config.write_text(document, encoding="utf-8")
        return config

    def test_codex_preserves_others_and_creates_private_backup(self):
        original = 'model = "example"\n[mcp_servers.existing]\ncommand = "existing"\nargs = ["ภาษาไทย"]\n'
        config = self.codex(original)
        result = util.install_codex_mcp(self.home)
        doc = tomllib.loads(config.read_text())
        self.assertEqual(doc["mcp_servers"]["existing"]["args"], ["ภาษาไทย"])
        self.assertEqual(doc["model"], "example")
        self.assertEqual(doc["mcp_servers"]["ssh_control_center"]["args"], ["mcp"])
        self.assertEqual(Path(result["backup"]).read_text(), original)
        self.assertEqual(config.stat().st_mode & 0o777, 0o600)
        second = util.install_codex_mcp(self.home)
        self.assertFalse(second["changed"])

    def test_codex_first_config_when_profile_exists(self):
        (self.home / ".codex").mkdir()
        result = util.install_codex_mcp(self.home)
        self.assertTrue(result["configured"])
        self.assertIsNone(result["backup"])
        self.assertEqual(tomllib.loads((self.home / ".codex/config.toml").read_text())["mcp_servers"]["ssh_control_center"]["command"], "/usr/bin/sshcc")

    def test_codex_custom_entry_is_preserved(self):
        original = '[mcp_servers.ssh_control_center]\ncommand = "/custom/sshcc"\n'
        config = self.codex(original)
        with self.assertRaises(ValueError):
            util.install_codex_mcp(self.home)
        self.assertEqual(config.read_text(), original)

    def test_codex_invalid_toml_is_preserved(self):
        config = self.codex("this is not toml")
        with self.assertRaises(tomllib.TOMLDecodeError):
            util.install_codex_mcp(self.home)
        self.assertEqual(config.read_text(), "this is not toml")

    def cached_receipt(self):
        bundle = {"targetId": "test-target", "tenant": "ทดสอบ", "os": "linux", "user": "sshcc-bridge",
                  "routes": [{"kind": "lan", "address": "10.0.0.1", "port": 22}],
                  "master": {"id": "master", "publicKey": "ssh-ed25519 public"}}
        envelope = {"requestId": "request", "masterId": "master", "bundleDigest": "digest", "bundle": bundle}
        receipt = {"requestId": "request", "masterId": "master", "bundleDigest": "digest",
                   "registration": {"id": "test-target", "tenant": "ทดสอบ", "os": "linux", "user": "sshcc-bridge",
                                    "routes": bundle["routes"], "nodeId": "actual-node"}}
        status = {"nodeId": "actual-node", "privileged": True, "executionPrincipal": "0", "roles": ["target"]}
        masters = [{"id": "master", "publicKey": "ssh-ed25519 public"}]
        return {"envelope": envelope}, receipt, status, masters

    def response(self, value):
        return subprocess.CompletedProcess([], 0, stdout=json.dumps(value), stderr="")

    def test_cached_code_requires_live_privilege_and_active_master(self):
        inspected, receipt, status, masters = self.cached_receipt()
        path = self.home / "code.txt"
        with patch.object(helper.subprocess, "run", side_effect=[self.response(receipt), self.response(status), self.response(masters)]):
            self.assertTrue(helper.validated_saved_code(path, inspected))
        with patch.object(helper.subprocess, "run", side_effect=[self.response(receipt), self.response(status), self.response([])]):
            self.assertFalse(helper.validated_saved_code(path, inspected))
        status["privileged"] = False
        with patch.object(helper.subprocess, "run", side_effect=[self.response(receipt), self.response(status)]):
            self.assertFalse(helper.validated_saved_code(path, inspected))

    def test_cached_code_refuses_wrong_request_and_unavailable_service(self):
        inspected, receipt, status, masters = self.cached_receipt()
        receipt["requestId"] = "different-request"
        with patch.object(helper.subprocess, "run", return_value=self.response(receipt)):
            self.assertFalse(helper.validated_saved_code(self.home / "code.txt", inspected))
        receipt["requestId"] = "request"
        with patch.object(helper.subprocess, "run", side_effect=[self.response(receipt), OSError("service uninstalled")]):
            self.assertFalse(helper.validated_saved_code(self.home / "code.txt", inspected))

    def assets(self):
        assets = self.home / "assets"
        assets.mkdir()
        for name in ("sshcc", "sshcc-service.deb"):
            (assets / name).write_bytes(name.encode())
        manifest = {"schemaVersion": 1, "files": {name: hashlib.sha256((assets / name).read_bytes()).hexdigest()
                                                  for name in ("sshcc", "sshcc-service.deb")}}
        (assets / "manifest.json").write_text(json.dumps(manifest))
        return assets

    def test_assets_reject_hash_mismatch_and_writable_payload(self):
        assets = self.assets()
        util.verify_assets(assets)
        (assets / "sshcc").chmod(0o666)
        with self.assertRaisesRegex(ValueError, "Untrusted"):
            util.verify_assets(assets)
        (assets / "sshcc").chmod(0o644)
        (assets / "sshcc").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            util.verify_assets(assets)

    def test_assets_reject_symlinks_and_parent_traversal(self):
        assets = self.assets()
        (assets / "sshcc").unlink()
        (assets / "sshcc").symlink_to(assets / "sshcc-service.deb")
        with self.assertRaisesRegex(ValueError, "Untrusted"):
            util.verify_assets(assets)
        manifest = {"schemaVersion": 1, "files": {"sshcc": "0" * 64, "sshcc-service.deb": "0" * 64, "../outside": "0" * 64}}
        (assets / "manifest.json").write_text(json.dumps(manifest))
        # Every asset name must be checked before it is used; symlink still rejects first.
        with self.assertRaises(ValueError):
            util.verify_assets(assets)

    def fake_helper(self):
        shutil.copy(HERE / "setup-helper.py", self.home / "setup-helper.py")
        shutil.copy(HERE / "setup-util.py", self.home / "setup-util.py")
        assets = self.assets()
        # Generic Master requires no inspection CLI call until the worker after Approve.
        marker = self.home / "worker-was-called"
        worker = self.home / "setup-worker.sh"
        worker.write_text("#!/bin/sh\ntouch '" + str(marker) + "'\nexit 0\n")
        worker.chmod(0o755)
        return marker

    def test_pipe_cannot_approve_and_worker_never_runs(self):
        marker = self.fake_helper()
        env = dict(os.environ, SUDO_UID="1000", DISPLAY="", WAYLAND_DISPLAY="")
        result = subprocess.run(["python3", str(self.home / "setup-helper.py"), "--terminal"],
                                input="APPROVE\n", text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse(marker.exists())

    def test_terminal_cancel_does_not_call_privileged_worker(self):
        marker = self.fake_helper()
        env = dict(os.environ, SUDO_UID="1000", DISPLAY="", WAYLAND_DISPLAY="")
        master, slave = pty.openpty()
        proc = subprocess.Popen(["python3", str(self.home / "setup-helper.py"), "--terminal"],
                                stdin=slave, stdout=slave, stderr=slave, env=env)
        os.close(slave)
        output = b""
        try:
            while b"APPROVE" not in output:
                if not select.select([master], [], [], 10)[0]:
                    self.fail("Approval prompt timed out")
                output += os.read(master, 4096)
            os.write(master, b"\n")
            self.assertEqual(proc.wait(timeout=10), 0, output)
            self.assertFalse(marker.exists())
        finally:
            os.close(master)
            if proc.poll() is None:
                proc.kill()
                proc.wait()


if __name__ == "__main__":
    if os.geteuid() != 0 or not Path("/.dockerenv").exists():
        raise SystemExit("Run only as root in a disposable Docker container")
    unittest.main(verbosity=2)
