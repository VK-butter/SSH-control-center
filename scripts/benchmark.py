#!/usr/bin/env python3
"""Sample service+tray+descendants before window cycles and for >=600s afterwards."""
import argparse
import datetime
import hashlib
import platform
import json
import os
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sshcc", required=True)
    parser.add_argument("--service-pid", required=True, type=int)
    parser.add_argument("--tray-pid", required=True, type=int)
    parser.add_argument("--window-report", required=True, type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=600)
    parser.add_argument("--interval", type=float, default=1.0)
    args = parser.parse_args()
    if args.seconds < 600 or args.interval <= 0 or args.interval > 5:
        parser.error("Acceptance requires >=600 seconds and a sampling interval in (0,5].")
    try:
        import psutil
    except ImportError:
        raise SystemExit("Install the test-only dependency: python -m pip install psutil")
    flags = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    targets = subprocess.run([args.sshcc, "request", "targets.list"], capture_output=True, text=True, check=True, **flags)
    target_count = len(json.loads(targets.stdout))
    if target_count != 50:
        raise SystemExit(f"Expected exactly 50 registered targets; found {target_count}. Use an isolated benchmark registry.")
    roots = [psutil.Process(args.service_pid), psutil.Process(args.tray_pid)]
    if Path(roots[0].exe()).stem != "sshcc" or Path(roots[1].exe()).stem != "sshcc-desktop":
        raise SystemExit("Service/tray PIDs must refer to the release sshcc and sshcc-desktop binaries.")
    status_process = subprocess.run([args.sshcc, "request", "system.status"], capture_output=True, text=True, encoding="utf-8", check=True, **flags)
    status = json.loads(status_process.stdout)
    def binary_digest(path):
        value = Path(path)
        digest = hashlib.sha256()
        with value.open("rb") as source:
            for chunk in iter(lambda: source.read(1024*1024), b""):
                digest.update(chunk)
        return {"path": str(value), "sha256": digest.hexdigest()}
    binaries = {"service": binary_digest(roots[0].exe()), "tray": binary_digest(roots[1].exe())}
    window = Path(roots[1].exe()).with_name("sshcc-window.exe" if os.name=="nt" else "sshcc-window")
    if window.is_file():
        binaries["window"] = binary_digest(window)
    provenance = {
        "platform": platform.platform(), "system": platform.system(), "release": platform.release(),
        "version": platform.version(), "architecture": platform.machine(), "pythonVersion": platform.python_version(),
        "binaries": binaries,
        "service": {"nodeId": status.get("nodeId"), "version": status.get("version"),
                    "executionPrincipal": status.get("executionPrincipal"), "privileged": status.get("privileged")}
    }
    seen = {(p.pid, p.create_time()): p for p in roots}
    previous_cpu = {}

    def snapshot():
        for root in roots:
            if not root.is_running():
                raise RuntimeError(f"Root process {root.pid} exited; benchmark invalid.")
        # Track descendants of every previously observed process, including reparented WebViews.
        for identity, parent in list(seen.items()):
            try:
                if parent.is_running() and parent.create_time() == identity[1]:
                    for child in parent.children(recursive=True):
                        seen[(child.pid, child.create_time())] = child
            except psutil.NoSuchProcess:
                continue
            # AccessDenied intentionally fails the benchmark rather than undercounting.
        memory = 0
        cpu_delta = 0
        live = []
        for identity, process in list(seen.items()):
            try:
                if process.create_time() != identity[1] or not process.is_running():
                    continue
                usage = process.memory_info().rss
                cpu = sum(process.cpu_times()[:2])
                cpu_delta += max(0, cpu - previous_cpu.get(identity, cpu))
                previous_cpu[identity] = cpu
                memory += usage
                live.append({"pid": process.pid, "name": process.name(), "rssBytes": usage})
            except psutil.NoSuchProcess:
                continue
        return memory, cpu_delta, live

    # Start this collector immediately after the tray launches, while its cycle hook is running.
    # A preexisting completion report cannot establish that residual descendants were observed.
    if args.window_report.exists():
        raise SystemExit("Window report already exists; use a fresh report and start collector before cycles finish.")
    cycle_started = time.monotonic()
    cycle_samples = []
    deadline = cycle_started + 300
    while True:
        memory, _, live = snapshot()
        cycle_samples.append({"elapsedSeconds":time.monotonic()-cycle_started, "rssBytes":memory, "processes":live})
        if args.window_report.exists():
            window_report = json.loads(args.window_report.read_text(encoding="utf-8"))
            if not (window_report.get("pid") == args.tray_pid and window_report.get("ok") is True
                    and window_report.get("cyclesCompleted", 0) >= 10 and window_report.get("windowAbsent") is True):
                raise RuntimeError(f"Window lifecycle proof failed: {window_report}")
            break
        if time.monotonic() > deadline:
            raise RuntimeError("Tray did not complete ten window cycles within five minutes.")
        time.sleep(args.interval)

    started = last = time.monotonic()
    start_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    snapshot()  # Discard cycle CPU deltas before the idle measurement.
    samples = []
    total_cpu = 0.0
    while True:
        time.sleep(min(args.interval, max(0, args.seconds-(time.monotonic()-started))))
        now = time.monotonic()
        memory, cpu_delta, live = snapshot()
        total_cpu += cpu_delta
        samples.append({"elapsedSeconds":now-started, "rssBytes":memory,
                        "cpuPercentOfOneCore":100*cpu_delta/(now-last), "processes":live})
        last = now
        if now-started >= args.seconds:
            break
    elapsed = last-started
    mean_cpu = 100*total_cpu/elapsed
    max_rss = max(s["rssBytes"] for s in samples)
    report = {
        "startedAt":start_utc, "durationSeconds":elapsed, "targetCount":target_count, "provenance":provenance,
        "windowLifecycle":window_report, "memoryMetric":"sum resident bytes (shared pages counted per process)",
        "cpuMetric":"sum process CPU / one logical core; not divided by core count",
        "maxRssBytes":max_rss, "meanCpuPercent":mean_cpu, "sampleCount":len(samples),
        "passed":max_rss <= 100_000_000 and mean_cpu < 1.0,
        "limitations":["Descendants shorter than the sampling interval may be missed.",
                       "Run quiet release binaries with no active jobs; every observed descendant must be readable."],
        "cycleSamples":cycle_samples, "samples":samples
    }
    args.output.write_text(json.dumps(report,indent=2),encoding="utf-8")
    print(json.dumps({k:v for k,v in report.items() if k not in ("samples","cycleSamples")},indent=2))
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
