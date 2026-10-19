#!/usr/bin/env python3
"""User-facing setup. All privileged changes are deferred until explicit approval."""
import argparse
import json
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from importlib.machinery import SourceFileLoader
util = SourceFileLoader("sshcc_setup_util", str(HERE / "setup-util.py")).load_module()
ASSETS = HERE / "assets"


def caller():
    uid = int(os.environ.get("SUDO_UID") or os.environ.get("PKEXEC_UID") or os.getuid())
    if uid == 0:
        raise ValueError("เปิดตัวช่วยด้วยบัญชีผู้ใช้ปกติ เพื่อให้ระบบทราบเจ้าของเครื่อง")
    return pwd.getpwuid(uid)


def dialog(*args, **kwargs):
    return subprocess.run(["zenity", *args], text=True, capture_output=True, **kwargs)


def code_path(account, request_id):
    return Path(account.pw_dir) / ".local/share/sshcc/setup" / request_id / "pairing-code.txt"


def clipboard(code):
    commands = []
    if os.environ.get("WAYLAND_DISPLAY") and shutil.which("wl-copy"):
        commands.append(["wl-copy"])
    if os.environ.get("DISPLAY") and shutil.which("xclip"):
        commands.append(["xclip", "-selection", "clipboard"])
    for command in commands:
        try:
            subprocess.run(command, input=code, text=True, check=True, timeout=5,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return True
        except (subprocess.SubprocessError, OSError):
            pass
    return False


def show_code(path, gui):
    code = path.read_text(encoding="utf-8").strip()
    if not gui:
        print("\nติดตั้งสำเร็จ — คัดลอกรหัสด้านล่างไปวางที่เครื่องหลัก:\n" + code + "\n")
        return
    while True:
        answer = dialog("--text-info", "--title=พร้อมจับคู่กับเครื่องหลัก",
                        "--filename=" + str(path), "--width=720", "--height=320",
                        "--ok-label=เสร็จ", "--extra-button=คัดลอกรหัส",
                        "--text=นำรหัสนี้ไปวางในช่องรหัสจับคู่ที่เครื่องหลัก")
        if answer.stdout.strip() != "คัดลอกรหัส":
            return
        copied = clipboard(code)
        dialog("--info", "--title=รหัสจับคู่",
               "--text=" + ("คัดลอกรหัสแล้ว นำไปวางที่เครื่องหลักได้เลย" if copied else
                            "เลือกข้อความรหัสแล้วกด Ctrl+C เพื่อคัดลอก"))


def validated_saved_code(path, inspected):
    """A stale owner cache never proves that this Master is still approved."""
    try:
        result = subprocess.run(
            [str(ASSETS / "sshcc"), "pairing", "inspect-code", "--code-file", str(path),
             "--request", str(ASSETS / "envelope.json")],
            check=True, capture_output=True, text=True, timeout=10,
        )
        receipt = json.loads(result.stdout)
        envelope = inspected["envelope"]
        bundle = envelope["bundle"]
        for field in ("requestId", "masterId", "bundleDigest"):
            if receipt[field] != envelope[field]:
                return False
        registration = receipt["registration"]
        if (registration["id"] != bundle["targetId"] or registration["tenant"] != bundle["tenant"] or
                registration["os"] != "linux" or registration["user"] != bundle["user"] or
                registration["routes"] != bundle["routes"]):
            return False
        status = json.loads(subprocess.run(
            ["/usr/bin/sshcc", "request", "system.status"], check=True,
            capture_output=True, text=True, timeout=10,
        ).stdout)
        if not (status.get("privileged") is True and str(status.get("executionPrincipal")) == "0" and
                status.get("nodeId") == registration["nodeId"] and "target" in status.get("roles", [])):
            return False
        masters = json.loads(subprocess.run(
            ["/usr/bin/sshcc", "request", "masters.list"], check=True,
            capture_output=True, text=True, timeout=10,
        ).stdout)
        expected_key = " ".join(bundle["master"]["publicKey"].split()[:2])
        return any(master.get("id") == envelope["masterId"] and
                   " ".join(master.get("publicKey", "").split()[:2]) == expected_key
                   for master in masters)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        return False


def approve_text(inspected):
    if inspected:
        envelope = inspected["envelope"]
        return ("เครื่องหลัก: " + envelope["bundle"]["master"]["label"] +
                "\nเครื่องใหม่: " + envelope["targetLabel"] +
                "\nกลุ่ม: " + envelope["bundle"]["tenant"] +
                "\n\nอนุญาตให้ agent บนเครื่องหลักสั่งงานและจัดการไฟล์ด้วยสิทธิ์ระบบ" +
                "\nเครื่องหลักที่อนุญาตไว้ก่อนหน้านี้จะยังใช้งานได้")
    return ("ใช้เครื่องนี้เป็นเครื่องหลัก\n\nระบบจะติดตั้ง service และสร้างกุญแจให้เครื่องนี้" +
            "\nเมื่อพบ Codex จะเพิ่ม SSH Control Center ให้ agent ใช้งาน" +
            "\nเปิด session ใหม่ของ Codex หลังติดตั้งเสร็จ")


def run_worker(account, desktop, gui, execution_mode):
    base = [str(HERE / "setup-worker.sh"), "--caller-uid", str(account.pw_uid)]
    base += ["--execution-mode", execution_mode]
    if desktop:
        base.append("--desktop")
    if os.geteuid() == 0:
        command = base
    elif gui:
        command = ["pkexec", *base]
    else:
        command = ["sudo", *base]
    if not gui:
        return subprocess.run(command).returncode
    with tempfile.NamedTemporaryFile(prefix="sshcc-setup-", suffix=".log", mode="w+", encoding="utf-8") as log:
        proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        progress = subprocess.Popen(
            ["zenity", "--progress", "--title=ติดตั้ง SSH Control Center",
             "--text=กำลังตรวจและติดตั้ง...", "--pulsate", "--auto-close",
             "--no-cancel", "--width=540"], stdin=subprocess.PIPE, text=True
        )
        for line in proc.stdout:
            log.write(line)
            log.flush()
            if line.startswith("SSHCC_PROGRESS:") and progress.stdin:
                try:
                    progress.stdin.write("# " + line.split(":", 1)[1].strip() + "\n")
                    progress.stdin.flush()
                except BrokenPipeError:
                    pass
        status = proc.wait()
        if progress.stdin:
            progress.stdin.close()
        try:
            progress.wait(timeout=5)
        except subprocess.TimeoutExpired:
            progress.terminate()
            progress.wait()
        if status:
            log.seek(0)
            # Keep an owner-private diagnostic copy for a retry or support.
            destination = Path(account.pw_dir) / ".local/share/sshcc/setup/last-error.log"
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            destination.write_text(log.read(), encoding="utf-8")
            destination.chmod(0o600)
            dialog("--error", "--title=ติดตั้งยังไม่สำเร็จ",
                   "--text=ตรวจรายละเอียดในหน้าถัดไป แล้วกดลองใหม่ได้",
                   "--width=540")
            dialog("--text-info", "--title=รายละเอียดการติดตั้ง", "--filename=" + str(destination),
                   "--width=720", "--height=360")
        return status


def main():
    parser = argparse.ArgumentParser(description="SSH Control Center Setup")
    parser.add_argument("--terminal", action="store_true")
    parser.add_argument("--desktop", action="store_true", help="install tray on a desktop session")
    parser.add_argument("--execution-mode", choices=["preserve", "service", "owner-session"], default="preserve")
    args = parser.parse_args()
    account = caller()
    gui = not args.terminal and bool(os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")) and bool(shutil.which("zenity"))
    util.verify_assets(ASSETS)
    inspected = util.inspect_request(ASSETS)
    if inspected:
        saved = code_path(account, inspected["envelope"]["requestId"])
        if saved.is_file() and validated_saved_code(saved, inspected):
            show_code(saved, gui)
            return 0
    desktop = not inspected and (gui or args.desktop)
    text = approve_text(inspected)
    execution_mode = args.execution_mode
    if execution_mode == "preserve":
        existing = Path("/usr/bin/sshcc").exists()
        if not existing:
            execution_mode = "owner-session"
        elif gui:
            answer = dialog("--question", "--title=บัญชีสำหรับงาน", "--text=เปลี่ยนไปใช้ Owner เดิมสำหรับ jobs, files และ terminal? ต้อง login อยู่; งานปกติใช้ UID ของ Owner และเลือก admin เพื่อรัน root", "--ok-label=ใช้ Owner session", "--cancel-label=รักษาโหมดเดิม")
            if answer.returncode == 0:
                execution_mode = "owner-session"
        elif sys.stdin.isatty() and input("เปลี่ยนโหมดเป็น Owner session? พิมพ์ OWNER หรือ Enter เพื่อรักษาโหมดเดิม: ").strip() == "OWNER":
            execution_mode = "owner-session"
    actual_owner = account.pw_name
    effective_mode = execution_mode
    if Path("/usr/bin/sshcc").exists():
        result = subprocess.run([str(ASSETS / "sshcc"), "request", "system.status"], capture_output=True, text=True, timeout=8)
        if result.returncode == 0:
            installed = json.loads(result.stdout)
            actual_owner = pwd.getpwuid(int(installed.get("owner", account.pw_uid))).pw_name
            if execution_mode == "preserve":
                effective_mode = installed.get("executor", {}).get("mode", "service")
    text += "\nบัญชีสำหรับงาน: " + actual_owner + "\nโหมด: " + effective_mode + "\nสิทธิ์: " + ("Owner UID/GID; เลือก admin ใช้ root" if effective_mode == "owner-session" else "root (service)")
    if gui:
        accepted = dialog("--question", "--title=อนุญาตเชื่อมต่อเครื่อง",
                          "--text=" + text, "--ok-label=Approve — อนุญาต",
                          "--cancel-label=ยกเลิก", "--width=580")
        if accepted.returncode:
            return 0
    else:
        print(text)
        if not sys.stdin.isatty():
            print("ต้องอนุมัติใน terminal แบบโต้ตอบเท่านั้น ไม่รับการอนุมัติผ่าน pipe", file=sys.stderr)
            return 2
        if input("\nพิมพ์ APPROVE เพื่ออนุมัติ หรือกด Enter เพื่อยกเลิก: ").strip() != "APPROVE":
            print("ยกเลิกแล้ว — ไม่มีการเพิ่มสิทธิ์ SSH")
            return 0
    while True:
        status = run_worker(account, desktop, gui, execution_mode)
        if not status:
            break
        if not gui or dialog("--question", "--title=ลองติดตั้งใหม่",
                             "--text=แก้ปัญหาที่แจ้งแล้ว กดลองใหม่เพื่อทำต่อ",
                             "--ok-label=ลองใหม่", "--cancel-label=ปิด").returncode:
            return status
    if inspected:
        show_code(code_path(account, inspected["envelope"]["requestId"]), gui)
    elif gui:
        dialog("--info", "--title=เครื่องหลักพร้อมใช้งาน",
               "--text=ติดตั้งสำเร็จ\nเปิด session ใหม่ของ Codex เพื่อโหลด SSH Control Center")
        subprocess.Popen(["/usr/bin/sshcc-desktop", "--show"], start_new_session=True,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    else:
        print("เครื่องหลักพร้อมใช้งาน เปิด session ใหม่ของ Codex เพื่อโหลด SSH Control Center")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        message = "เปิดตัวช่วยไม่สำเร็จ: " + str(error)
        if (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")) and shutil.which("zenity"):
            dialog("--error", "--title=SSH Control Center Setup", "--text=" + message)
        else:
            print(message, file=sys.stderr)
        sys.exit(1)
