"""Run the release tray lifecycle + ten-minute resource acceptance in temporary state."""
import argparse, json, os, subprocess, sys, tempfile, time, uuid
from pathlib import Path

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--release",type=Path,required=True)
    parser.add_argument("--output-dir",type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[1]
    args.output_dir.mkdir(parents=True,exist_ok=True)
    work=Path(tempfile.mkdtemp(prefix="sshcc-native-benchmark-"))
    env=os.environ.copy()
    env["SSHCC_DATA_DIR"]=str(work/"state")
    env["SSHCC_SOCKET"]=r"\\.\pipe\sshcc-benchmark-"+str(uuid.uuid4())
    env["PYTHONUTF8"]="1"
    exe=args.release/"sshcc.exe"; tray_exe=args.release/"sshcc-desktop.exe"
    flags={"creationflags":subprocess.CREATE_NO_WINDOW}
    def call(*argv):
        p=subprocess.run([str(exe),*argv],env=env,capture_output=True,text=True,encoding="utf-8",timeout=30,**flags)
        if p.returncode: raise RuntimeError(p.stderr)
        return json.loads(p.stdout)
    owner=call("identity")["principal"]
    call("init","--role","master","--owner",owner,"--label","benchmark:Windows release")
    service_log=(work/"service.log").open("wb")
    service=subprocess.Popen([str(exe),"service","run"],env=env,stdout=service_log,stderr=service_log,**flags)
    tray=None
    try:
        for attempt in range(30):
            try:call("service","status");break
            except RuntimeError:
                if service.poll() is not None:raise
                time.sleep(.1)
        seed=subprocess.run([sys.executable,str(root/"scripts/seed-benchmark.py"),"--sshcc",str(exe),"--isolated-state",str(work/"state")],env=env,capture_output=True,text=True,encoding="utf-8",**flags)
        if seed.returncode: raise RuntimeError(seed.stdout+seed.stderr)
        print(seed.stdout,flush=True)
        window_report=work/"windows.json"
        tray=subprocess.Popen([str(tray_exe),"--benchmark-cycles","10","--benchmark-report",str(window_report)],env=env,**flags)
        print(json.dumps({"work":str(work),"servicePid":service.pid,"trayPid":tray.pid,"windowReport":str(window_report)}),flush=True)
        (args.output_dir/"benchmark-processes.json").write_text(json.dumps({"work":str(work),"servicePid":service.pid,"trayPid":tray.pid,"windowReport":str(window_report)}),encoding="utf-8")
        result=subprocess.run([sys.executable,str(root/"scripts/benchmark.py"),"--sshcc",str(exe),"--service-pid",str(service.pid),"--tray-pid",str(tray.pid),"--window-report",str(window_report),"--output",str(args.output_dir/"idle-windows.json")],env=env,capture_output=True,text=True,encoding="utf-8",**flags)
        (args.output_dir/"benchmark-collector.log").write_text(result.stdout+result.stderr,encoding="utf-8")
        print(result.stdout+result.stderr,flush=True)
        if window_report.exists():(args.output_dir/"window-lifecycle-windows.json").write_bytes(window_report.read_bytes())
        return result.returncode
    finally:
        if tray and tray.poll() is None:tray.terminate();tray.wait(timeout=15)
        try:call("service","stop")
        except Exception:pass
        try:service.wait(timeout=15)
        except subprocess.TimeoutExpired:service.kill();service.wait()
        service_log.close()
        print(json.dumps({"fixtureStateRetainedAt":str(work)}),flush=True)
if __name__=="__main__":raise SystemExit(main())
