#!/usr/bin/env python3
"""Isolated Linux normal-close and duplicate-activation acceptance."""
import ctypes
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import psutil

if not Path("/.dockerenv").exists():
    raise SystemExit("This fixture runs only inside an isolated Docker container.")
out = Path(sys.argv[1])
if not str(out).startswith("/tmp/"):
    raise SystemExit("Fixture output must be below /tmp.")
out.mkdir(parents=True, exist_ok=False)
env = dict(os.environ, SSHCC_DATA_DIR=str(out/"state"), SSHCC_SOCKET=str(out/"control.sock"),
           WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_DMABUF_RENDERER="1",
           LIBGL_ALWAYS_SOFTWARE="1", GDK_BACKEND="x11", SSHCC_WINDOW_IDLE_SECS="0")
exe = "/src/target/release/sshcc"
tray_exe = "/src/target/release/sshcc-desktop"
subprocess.run([exe, "init", "--role", "both", "--owner", str(os.getuid()),
                "--label", "benchmark:normal-close"], env=env, check=True, stdout=subprocess.DEVNULL)
service = subprocess.Popen([exe, "service", "run"], env=env, stdout=(out/"service.log").open("w"), stderr=subprocess.STDOUT)
tray = None

def wait_until(check, message, seconds=30):
    deadline = time.monotonic()+seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise RuntimeError(message)

def windows():
    text = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
    return re.findall(r'(0x[0-9a-f]+) "SSH Control Center":', text)

def close_window(window):
    # Send the same ICCCM WM_DELETE_WINDOW protocol as a window-manager close.
    class Data(ctypes.Union):
        _fields_ = [("b", ctypes.c_char*20), ("s", ctypes.c_short*10), ("l", ctypes.c_long*5)]
    class ClientMessage(ctypes.Structure):
        _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                    ("display", ctypes.c_void_p), ("window", ctypes.c_ulong),
                    ("message_type", ctypes.c_ulong), ("format", ctypes.c_int), ("data", Data)]
    class Event(ctypes.Union):
        _fields_ = [("client", ClientMessage), ("pad", ctypes.c_long*24)]
    x = ctypes.CDLL("libX11.so.6")
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x.XInternAtom.restype = ctypes.c_ulong
    x.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x.XFlush.argtypes = [ctypes.c_void_p]
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x.XOpenDisplay(None)
    if not display:
        raise RuntimeError("Cannot open fixture X display")
    event = Event()
    event.client.type = 33
    event.client.send_event = 1
    event.client.display = display
    event.client.window = int(window, 16)
    event.client.message_type = x.XInternAtom(display, b"WM_PROTOCOLS", 0)
    event.client.format = 32
    event.client.data.l[0] = x.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
    if not x.XSendEvent(display, int(window, 16), 0, 0, ctypes.byref(event)):
        raise RuntimeError("XSendEvent rejected close request")
    x.XFlush(display)
    x.XCloseDisplay(display)

def alive(process):
    try:
        return process.is_running() and process.status() != psutil.STATUS_ZOMBIE
    except psutil.NoSuchProcess:
        return False

try:
    wait_until(lambda: Path(env["SSHCC_SOCKET"]).exists(), "Service socket did not appear")
    tray = subprocess.Popen([tray_exe, "--show"], env=env, stdout=(out/"tray.log").open("w"), stderr=subprocess.STDOUT)
    first = wait_until(windows, "Normal management window did not open")
    assert len(first) == 1, first
    time.sleep(2)
    subprocess.run([tray_exe, "--show"], env=env, check=True, timeout=10)
    time.sleep(2)
    duplicate = windows()
    assert len(duplicate) == 1, duplicate
    descendants = psutil.Process(tray.pid).children(recursive=True)
    assert any(p.name() == "sshcc-window" for p in descendants), "No native child window process"
    close_window(duplicate[0])
    wait_until(lambda: not windows(), "Normal window close did not destroy window")
    wait_until(lambda: not any(alive(p) for p in descendants), "Window or WebKit descendants remained alive")
    assert tray.poll() is None and service.poll() is None
    subprocess.run([tray_exe, "--show"], env=env, check=True, timeout=10)
    reopened = wait_until(windows, "Tray did not reopen a management window")
    assert len(reopened) == 1
    descendants2 = psutil.Process(tray.pid).children(recursive=True)
    close_window(reopened[0])
    wait_until(lambda: not windows(), "Reopened window did not close")
    wait_until(lambda: not any(alive(p) for p in descendants2), "Second window descendants remained alive")
    status = json.loads(subprocess.check_output([exe, "request", "system.status"], env=env, text=True))
    report = {"ok": True, "normalCloseUses": "ICCCM WM_DELETE_WINDOW",
              "duplicateWindowCount": len(duplicate), "nativeWindowProcessExited": True,
              "observedWindowDescendantsExited": True, "reopenedAndClosed": True,
              "trayStayedAlive": tray.poll() is None, "serviceStayedAlive": service.poll() is None,
              "serviceStatus": status}
    (out/"result.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
finally:
    if tray is not None:
        tray.terminate()
        tray.wait(timeout=10)
    service.terminate()
    service.wait(timeout=10)