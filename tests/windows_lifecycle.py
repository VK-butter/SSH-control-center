"""Exercise normal Windows UI close, duplicate activation, and tray crash cleanup."""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import traceback
import uuid

import psutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("This native lifecycle test requires Windows.")
    binary = args.release / "sshcc.exe"
    tray_binary = args.release / "sshcc-desktop.exe"
    window_binary = args.release / "sshcc-window.exe"
    work = Path(tempfile.mkdtemp(prefix="sshcc-windows-lifecycle-"))
    env = dict(os.environ, SSHCC_DATA_DIR=str(work / "state"),
               SSHCC_SOCKET=r"\\.\pipe\sshcc-lifecycle-" + str(uuid.uuid4()),
               SSHCC_WINDOW_IDLE_SECS="0")
    flags = {"creationflags": subprocess.CREATE_NO_WINDOW}
    report = {"fixture": str(work), "checks": [], "passed": False}
    processes = []
    seen = {}

    user = ctypes.WinDLL("user32", use_last_error=True)
    enum_callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [enum_callback, wintypes.LPARAM]
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]

    def call(*argv):
        result = subprocess.run([str(binary), *argv], env=env, capture_output=True,
                                text=True, encoding="utf-8", timeout=30, **flags)
        if result.returncode:
            raise RuntimeError(result.stderr)
        return json.loads(result.stdout)

    def remember(tray):
        try:
            parent = psutil.Process(tray.pid)
            for process in [parent] + parent.children(recursive=True):
                seen[(process.pid, process.create_time())] = process
        except psutil.NoSuchProcess:
            pass

    def live_tracked(excluding=()):
        result = []
        for (pid, created), process in list(seen.items()):
            try:
                if pid not in excluding and process.is_running() and process.create_time() == created:
                    result.append({"pid": pid, "name": process.name()})
            except psutil.NoSuchProcess:
                pass
        return result

    def windows(tray):
        remember(tray)
        pids = set()
        for (pid, created), process in list(seen.items()):
            try:
                if process.is_running() and process.create_time() == created and process.name() == "sshcc-window.exe":
                    pids.add(pid)
            except psutil.NoSuchProcess:
                pass
        found = []
        def callback(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            title = ctypes.create_unicode_buffer(256)
            user.GetWindowTextW(hwnd, title, len(title))
            if pid.value in pids and user.IsWindowVisible(hwnd) and title.value == "SSH Control Center":
                found.append((int(hwnd), pid.value))
            return True
        user.EnumWindows(enum_callback(callback), 0)
        return found

    def wait_for(test, message, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = test()
            if result:
                return result
            time.sleep(.1)
        raise RuntimeError(message)

    def one_window(tray):
        found = windows(tray)
        if len(found) > 1:
            raise RuntimeError(f"Multiple management windows: {found}")
        return found[0] if found else None

    service = tray = None
    log = (work / "service.log").open("wb")
    try:
        owner = call("identity")["principal"]
        call("init", "--role", "master", "--owner", owner, "--label", "lifecycle:Windows")
        service = subprocess.Popen([str(binary), "service", "run"], env=env,
                                   stdout=log, stderr=log, **flags)
        processes.append(service)
        def ready():
            try:
                return call("service", "status")
            except RuntimeError:
                return False
        wait_for(ready, "service did not start")
        tray = subprocess.Popen([str(tray_binary), "--show"], env=env, **flags)
        processes.append(tray)
        window = wait_for(lambda: one_window(tray), "management window did not open")
        wait_for(lambda: (remember(tray) or len(live_tracked(excluding=(tray.pid,))) >= 2),
                 "WebView descendants were not observed")
        report["checks"].append({"name": "normal-open", "windowPid": window[1]})

        duplicates = [subprocess.Popen([str(tray_binary), "--show"], env=env, **flags) for _ in range(4)]
        for process in duplicates:
            processes.append(process)
            assert process.wait(timeout=15) == 0
        direct = subprocess.Popen([str(window_binary), "--management-window"], env=env, **flags)
        processes.append(direct)
        assert direct.wait(timeout=15) == 0
        time.sleep(1)
        assert one_window(tray)[1] == window[1]
        assert len([item for item in live_tracked() if item["name"] == "sshcc-window.exe"]) == 1
        report["checks"].append({"name": "duplicate-tray-and-child-focus-existing", "count": 5})

        if not user.PostMessageW(window[0], 0x0010, 0, 0):  # WM_CLOSE: normal native close
            raise ctypes.WinError(ctypes.get_last_error())
        wait_for(lambda: not live_tracked(excluding=(tray.pid,)),
                 "normal close left management-window descendants")
        assert tray.poll() is None
        call("service", "status")
        report["checks"].append({"name": "normal-close-reaps-entire-window-job"})

        duplicate = subprocess.Popen([str(tray_binary), "--show"], env=env, **flags)
        processes.append(duplicate)
        assert duplicate.wait(timeout=15) == 0
        reopened = wait_for(lambda: one_window(tray), "second window did not open")
        assert reopened[1] != window[1]
        for _ in range(15):
            remember(tray)
            time.sleep(.1)
        report["checks"].append({"name": "reopen-after-normal-close", "windowPid": reopened[1]})

        tray.terminate()
        tray.wait(timeout=15)
        wait_for(lambda: not live_tracked(), "tray exit left window or WebView descendants")
        call("service", "status")
        report["checks"].append({"name": "tray-crash-closes-private-job-service-survives"})
        report["passed"] = True
    except Exception:
        report["error"] = traceback.format_exc()
    finally:
        # Only processes started by this fixture, identified by pid+creation time.
        for process in reversed(processes):
            if process.poll() is None and process is not service:
                process.terminate()
        for (pid, created), process in list(seen.items()):
            try:
                if process.is_running() and process.create_time() == created:
                    process.kill()
            except psutil.NoSuchProcess:
                pass
        if service is not None:
            try:
                call("service", "stop")
            except Exception:
                pass
            try:
                service.wait(timeout=15)
            except subprocess.TimeoutExpired:
                service.kill()
                service.wait()
        log.close()
        report["remainingDescendants"] = live_tracked()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report, indent=2))
    return 0 if report["passed"] and not report["remainingDescendants"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
