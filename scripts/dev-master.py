#!/usr/bin/env python3
"""A long-lived development Master on this computer, in Docker, for real Targets.

    python3 scripts/dev-master.py up         # start (first run creates the Master), open http://127.0.0.1:8770/
    python3 scripts/dev-master.py rebuild    # rebuild sshcc from this tree, back up the state, restart
    python3 scripts/dev-master.py bundle --id win-01 --address 192.0.2.20 --os windows
    python3 scripts/dev-master.py import registration.json
    python3 scripts/dev-master.py templates v0.5.0   # installer templates from a Release (Add host, Update)
    python3 scripts/dev-master.py backup | logs | down

Unlike scripts/ui-preview.py nothing here is a demo: the Master's state lives in the Docker volume
sshcc-dev-master-state and survives rebuilds, restarts and `down`. The container restarts with Docker.
desktop/ui is mounted read-only, so HTML/CSS/JS edits show on a browser refresh; Rust edits need rebuild.

The page talks to the Master through POST /invoke, published on 127.0.0.1 only. A request needs the
X-SSHCC header (a cross-site page cannot send it without a CORS preflight, which is never answered) and
a 127.0.0.1/localhost Host header (against DNS rebinding). Like the desktop app, any process running on
this computer can still drive the Master. Native-only commands (shell window, MCP client links) are absent.
"""
import argparse
import base64
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parents[1]
NAME, VOLUME, IMAGE, PORT = "sshcc-dev-master", "sshcc-dev-master-state", "sshcc-dev:latest", 8770
OUT = ROOT / "artifacts/dev-master"
TEMPLATES = OUT / "templates"
TEMPLATE_FILES = ["sshcc-client-windows-template.exe", "sshcc-client-ubuntu-template.deb", "template-manifest.json"]
BRIDGE = r"""
window.__TAURI__ = { core: { invoke: async (command, args = {}) => {
  const response = await fetch("/invoke", { method: "POST", headers: { "X-SSHCC": "1" }, body: JSON.stringify({ command, args }) });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error);
  return body.result;
}}};
"""
CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'"


def run(*args, stdin=None, check=True):
    result = subprocess.run(list(map(str, args)), input=stdin, text=True, capture_output=True, encoding="utf-8")
    if check and result.returncode:
        raise SystemExit((result.stderr or result.stdout).strip())
    return result


# ---- inside the container -------------------------------------------------------------------

def request(method, params=None):
    text = json.dumps(params or {})
    result = subprocess.run(["sshcc", "request", method, "--params-file", "/dev/stdin"], input=text,
                            text=True, capture_output=True, encoding="utf-8")
    if result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip())
    return json.loads(result.stdout)


def invoke(command, args):
    if command == "rpc":
        return request(args["method"], args.get("params") or {})
    if command == "client_links":
        return []
    if command == "save_remote_file":
        local, offset = pathlib.Path("/out/downloads") / pathlib.Path(args["name"]).name, 0
        local.parent.mkdir(parents=True, exist_ok=True)
        with open(local, "wb") as file:
            while True:
                chunk = request("files.read", {"targetId": args["targetId"], "path": args["path"], "offset": offset})
                data = base64.b64decode(chunk["contentBase64"])
                file.write(data)
                offset += len(data)
                if chunk["eof"] or not data:
                    break
        return "artifacts/dev-master/downloads/" + local.name
    raise RuntimeError(f"'{command}' works only in the desktop app, not in the dev Master page")


class Handler(BaseHTTPRequestHandler):
    def allowed(self):
        host = (self.headers.get("Host") or "").split(":")[0]
        origin = self.headers.get("Origin")
        return host in ("127.0.0.1", "localhost") and (origin is None or origin in (f"http://127.0.0.1:{PORT}", f"http://localhost:{PORT}"))

    def do_GET(self):
        ui = pathlib.Path("/opt/ui")
        name = "index.html" if self.path in ("/", "/index.html") else self.path.lstrip("/")
        fonts = {"fonts/" + font.name for font in (ui / "fonts").glob("*.woff2")}
        if not self.allowed() or name not in {"index.html", "app.js", "strings.js", "app.css", "sky.js", "sky.css", "bridge.js"} | fonts:
            self.send_error(404)
            return
        data = BRIDGE.encode() if name == "bridge.js" else (ui / name).read_bytes()
        if name == "index.html":
            data = data.replace(b"<head>", b'<head><script src="bridge.js"></script>', 1)
        kind = {"html": "text/html; charset=utf-8", "js": "text/javascript; charset=utf-8", "css": "text/css; charset=utf-8", "woff2": "font/woff2", "svg": "image/svg+xml"}[name.rsplit(".", 1)[1]]
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Security-Policy", CSP)
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        if self.path != "/invoke" or not self.allowed() or self.headers.get("X-SSHCC") != "1":
            self.send_error(403)
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        try:
            payload, status = {"result": invoke(body["command"], body.get("args") or {})}, 200
        except Exception as error:
            payload, status = {"error": str(error)}, 500
        data = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def serve(label):
    # First start only: create the Master identity in the volume.
    if not any(pathlib.Path("/var/lib/sshcc").iterdir()):
        subprocess.run(["sshcc", "init", "--role", "master", "--owner", "0", "--label", label], check=True)
    service = subprocess.Popen(["sh", "-c", "exec sshcc service run >>/var/lib/sshcc/dev-service.log 2>&1"])
    for _ in range(200):
        if subprocess.run(["sshcc", "service", "status"], capture_output=True).returncode == 0:
            break
        time.sleep(0.05)
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    server.timeout = 1
    print(f"dev Master ready on http://127.0.0.1:{PORT}/", flush=True)
    # If the service dies the container exits and Docker restarts it.
    while service.poll() is None:
        server.handle_request()
    sys.exit(service.returncode or 1)


# ---- on this computer -----------------------------------------------------------------------

def exists():
    return run("docker", "ps", "-aq", "--filter", f"name=^{NAME}$").stdout.strip() != ""


def up(label):
    if exists():
        run("docker", "start", NAME)
    else:
        OUT.mkdir(parents=True, exist_ok=True)
        run("docker", "run", "-d", "--name", NAME, "--hostname", "mac-dev-master", "--restart", "unless-stopped",
            "--label", "sshcc.dev-master=1", "-p", f"127.0.0.1:{PORT}:{PORT}",
            "-v", f"{VOLUME}:/var/lib/sshcc",
            "-v", f"{ROOT / 'desktop/ui'}:/opt/ui:ro",
            "-v", f"{pathlib.Path(__file__).resolve()}:/opt/dev-master.py:ro",
            "-v", f"{OUT}:/out",
            # Installer templates from a Release (`templates`), so Add host and Update work from this Master.
            *(["-v", f"{TEMPLATES}:/usr/lib/sshcc/templates:ro"] if (TEMPLATES / "template-manifest.json").is_file() else []),
            IMAGE, "python3", "/opt/dev-master.py", "serve", "--label", label)
    for _ in range(100):
        if f"ready on" in run("docker", "logs", NAME, check=False).stdout:
            print(f"dev Master is up: http://127.0.0.1:{PORT}/")
            return
        time.sleep(0.2)
    raise SystemExit("dev Master did not start; see: python3 scripts/dev-master.py logs")


def backup():
    OUT.mkdir(parents=True, exist_ok=True)
    name = "state-" + datetime.datetime.now().strftime("%Y%m%d-%H%M%S") + ".tar.gz"
    run("docker", "run", "--rm", "-v", f"{VOLUME}:/state:ro", "-v", f"{OUT}:/out", "alpine:3.20",
        "tar", "czf", f"/out/{name}", "-C", "/state", ".")
    print(f"state backed up to {OUT / name}")


def master(*args, stdin=None):
    return run("docker", "exec", *(["-i"] if stdin is not None else []), NAME, *args, stdin=stdin)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("up")
    p.add_argument("--label", default="mac-dev", help="Master name shown on Targets (first start only)")
    sub.add_parser("rebuild")
    sub.add_parser("down")
    sub.add_parser("backup")
    sub.add_parser("logs")
    p = sub.add_parser("bundle", help="enrollment bundle for a Target, for its Setup -EnrollmentBundle")
    p.add_argument("--id", required=True)
    p.add_argument("--address", required=True)
    p.add_argument("--port", type=int, default=22)
    p.add_argument("--os", choices=["windows", "linux"], required=True)
    p.add_argument("--group", default="lab")
    p = sub.add_parser("import", help="import a Target's registration.json and check the connection")
    p.add_argument("file", type=pathlib.Path)
    p = sub.add_parser("templates", help="download a Release's installer templates for this Master (then it restarts)")
    p.add_argument("tag", help="the Release tag, e.g. v0.5.0")
    p.add_argument("--repo", default="bbouq/SSH-control-center")
    p = sub.add_parser("serve")  # container entry point
    p.add_argument("--label", default="mac-dev")
    args = parser.parse_args()

    if args.command == "serve":
        serve(args.label)
    elif args.command == "up":
        up(args.label)
    elif args.command == "rebuild":
        subprocess.run(["docker", "build", "-f", str(ROOT / "tests/containers/Dockerfile"), "-t", IMAGE, str(ROOT)], check=True)
        if exists():
            backup()
            run("docker", "rm", "-f", NAME)
        up("mac-dev")
    elif args.command == "templates":
        # From the Release, checked against its SHA256SUMS; the Master checks them again against the manifest.
        TEMPLATES.mkdir(parents=True, exist_ok=True)
        run("gh", "release", "download", args.tag, "--repo", args.repo, "--dir", str(TEMPLATES), "--clobber",
            *sum((["--pattern", name] for name in TEMPLATE_FILES + ["SHA256SUMS"]), []))
        import hashlib
        sums = dict(line.split()[::-1] for line in (TEMPLATES / "SHA256SUMS").read_text().splitlines() if line.strip())
        for name in TEMPLATE_FILES:
            digest = hashlib.sha256((TEMPLATES / name).read_bytes()).hexdigest()
            if sums.get(name) != digest:
                raise SystemExit(f"{name} doesn't match the Release's SHA256SUMS")
        print(f"templates from {args.tag} in {TEMPLATES}")
        if exists():
            run("docker", "rm", "-f", NAME)
            up("mac-dev")
    elif args.command == "down":
        run("docker", "rm", "-f", NAME, check=False)
        print(f"stopped; the Master's state stays in volume {VOLUME}")
    elif args.command == "backup":
        backup()
    elif args.command == "logs":
        print(run("docker", "logs", NAME, check=False).stdout, end="")
        print(master("tail", "-n", "50", "/var/lib/sshcc/dev-service.log").stdout, end="")
    elif args.command == "bundle":
        params = {"targetId": args.id, "tenant": args.group, "os": args.os, "user": "sshcc-bridge",
                  "routes": [{"kind": "lan", "address": args.address, "port": args.port}]}
        bundle = master("sshcc", "request", "enrollment.export", "--params", json.dumps(params)).stdout
        OUT.mkdir(parents=True, exist_ok=True)
        path = OUT / f"{args.id}-bundle.json"
        path.write_text(bundle)
        print(f"bundle written to {path}")
    elif args.command == "import":
        registration = json.loads(args.file.read_text(encoding="utf-8-sig"))
        result = json.loads(master("sshcc", "request", "targets.import", "--params-file", "/dev/stdin",
                                   stdin=json.dumps({"registration": registration})).stdout)
        target = registration.get("id") or result.get("id")
        print(f"imported {target} ({registration.get('label')}, {registration.get('os')})")
        doctor = json.loads(master("sshcc", "request", "targets.doctor", "--params", json.dumps({"targetId": target})).stdout)
        for stage in doctor.get("stages", []):
            print(("  ok   " if stage.get("ok") else "  FAIL ") + stage.get("name", "") + (": " + stage["message"] if stage.get("message") else ""))
        print("ready" if doctor.get("ready") else "needs attention: python3 scripts/dev-master.py logs")


if __name__ == "__main__":
    main()
