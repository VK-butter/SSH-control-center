#!/usr/bin/env python3
"""Native GTK approval/copy smoke in disposable Ubuntu + Xvfb, no deployed hosts."""
import hashlib
import importlib.machinery
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent


def zenity_window():
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--class", "zenity"],
                               text=True, capture_output=True)
        if found.returncode == 0 and found.stdout.strip():
            return found.stdout.splitlines()[-1]
        time.sleep(0.1)
    raise AssertionError("Native Zenity window did not appear")


def close_window(window):
    subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)
    subprocess.run(["xdotool", "key", "--window", window, "Escape"], check=True)


class NativeSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sshcc-native-setup-")
        self.directory = Path(self.temp.name)
        shutil.copy(HERE / "setup-helper.py", self.directory / "setup-helper.py")
        shutil.copy(HERE / "setup-util.py", self.directory / "setup-util.py")
        self.assets = self.directory / "assets"
        self.assets.mkdir()
        for name in ("sshcc", "sshcc-service.deb"):
            (self.assets / name).write_bytes(b"public-test-fixture")
        (self.assets / "manifest.json").write_text(json.dumps({"schemaVersion": 1, "files": {
            name: hashlib.sha256((self.assets / name).read_bytes()).hexdigest()
            for name in ("sshcc", "sshcc-service.deb")}}))
        self.marker = self.directory / "privileged-worker-called"
        worker = self.directory / "setup-worker.sh"
        worker.write_text("#!/bin/sh\ntouch '" + str(self.marker) + "'\n")
        worker.chmod(0o755)

    def tearDown(self):
        self.temp.cleanup()

    def test_native_approve_cancel_never_calls_worker(self):
        env = dict(os.environ, SUDO_UID="1000")
        proc = subprocess.Popen(["python3", str(self.directory / "setup-helper.py")],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        try:
            close_window(zenity_window())
            stdout, stderr = proc.communicate(timeout=15)
            self.assertEqual(proc.returncode, 0, stdout + stderr)
            self.assertFalse(self.marker.exists())
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()

    def test_native_code_dialog_and_clipboard_paste(self):
        code = "SSHCC1." + ("publicPairingReceipt" * 256) + "." + ("a" * 64)
        receipt = self.directory / "pairing-code.txt"
        receipt.write_text(code, encoding="utf-8")
        code_script = (
            "import importlib.machinery,pathlib;"
            "m=importlib.machinery.SourceFileLoader('setup_helper'," + repr(str(self.directory / "setup-helper.py")) + ").load_module();"
            "m.show_code(pathlib.Path(" + repr(str(receipt)) + "),True)"
        )
        proc = subprocess.Popen(["python3", "-c", code_script], text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            window = zenity_window()
            geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window], text=True)
            sizes = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
            # Click the actual Copy extra button; its result drives the production dialog loop.
            subprocess.run(["xdotool", "mousemove", "--window", window,
                            str(int(sizes["WIDTH"]) * 5 // 6), str(int(sizes["HEIGHT"]) - 22), "click", "1"], check=True)
            pasted = None
            for _ in range(50):
                result = subprocess.run(["xclip", "-o", "-selection", "clipboard"], text=True, capture_output=True)
                if result.returncode == 0 and result.stdout == code:
                    pasted = result.stdout
                    break
                time.sleep(0.1)
            self.assertEqual(pasted, code)
            close_window(zenity_window())  # Copied confirmation.
            time.sleep(0.2)
            close_window(zenity_window())  # Code can be displayed again, then closed.
            stdout, stderr = proc.communicate(timeout=15)
            self.assertEqual(proc.returncode, 0, stdout + stderr)
            self.assertEqual(receipt.read_text(), code)
            # Clipboard ownership persists after helper exit until clipboard replacement.
            pasted_after_exit = subprocess.run(["xclip", "-o", "-selection", "clipboard"],
                                              check=True, text=True, capture_output=True).stdout
            self.assertEqual(pasted_after_exit, code)
            subprocess.run(["xclip", "-selection", "clipboard"], input="replaced", text=True, check=True)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()


if __name__ == "__main__":
    if not Path("/.dockerenv").exists() or not os.environ.get("DISPLAY"):
        raise SystemExit("Run in disposable container under xvfb-run")
    unittest.main(verbosity=2)
