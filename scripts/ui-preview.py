#!/usr/bin/env python3
"""Browser preview of the desktop UI backed by a real Master service in Docker.

    docker build -f tests/containers/Dockerfile -t sshcc-e2e:local .
    python3 scripts/ui-preview.py            # then open http://127.0.0.1:8765/

The page is desktop/ui unchanged; Tauri's invoke() is bridged to `sshcc request` inside an owned
Master container, with demo Ubuntu Targets enrolled over real OpenSSH. With --docker-socket the first
demo Target sees this computer's Docker (read the Datacenter Containers tab; actions there act on
real containers, so a throwaway sshcc-ct-probe container is started to try them on).
--scenario sites builds two sites instead: master-a with a1/a2 and master-b with b1/b2, where master-b
is also a Target of master-a and merged into it, so master-a reaches b1/b2 through master-b (1 hop);
Datacenters DC-A and DC-B are created. All containers share one network. Native-only commands
(shell window, MCP client links, file dialogs) are not available in a browser. Ctrl+C removes
every container and network labelled with this run.
"""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import signal
import subprocess
import sys
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parents[1]
UI = ROOT / "desktop/ui"
PID_LABEL = "sshcc.ui-preview.pid"
BRIDGE = r"""
window.__TAURI__ = { core: { invoke: async (command, args = {}) => {
  const response = await fetch("/invoke", { method: "POST", body: JSON.stringify({ command, args }) });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error);
  return body.result;
}}};
"""


def run(*args, stdin=None, check=True):
    result = subprocess.run(list(map(str, args)), input=stdin, text=True, capture_output=True, encoding="utf-8")
    if check and result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip())
    return result


class Preview:
    def __init__(self, image, targets, output, docker_socket=False, scenario="demo"):
        self.image, self.output, self.docker_socket, self.scenario = image, output, docker_socket, scenario
        self.probe = None
        self.run_id = uuid.uuid4().hex[:8]
        self.label = "sshcc.ui-preview=" + self.run_id
        # The owning process, so a later preview sweeps only leftovers whose preview has died.
        self.labels = ["--label", self.label, "--label", f"{PID_LABEL}={os.getpid()}"]
        self.network = "sshcc-ui-preview-" + self.run_id
        self.containers = []
        self.target_count = targets

    def exec(self, container, *args, stdin=None, check=True):
        return run("docker", "exec", *(["-i"] if stdin is not None else []), container, *args, stdin=stdin, check=check)

    def request(self, method, params=None, master=None):
        text = json.dumps(params or {})
        # Large params (file uploads) go through stdin: a command line is capped at about 128 KiB.
        if len(text) > 32768:
            result = self.exec(master or self.master, "sshcc", "request", method, "--params-file", "/dev/stdin", stdin=text)
        else:
            result = self.exec(master or self.master, "sshcc", "request", method, "--params", text)
        return json.loads(result.stdout)

    def container(self, alias, role, label, extra=()):
        name = f"{self.network}-{alias}"
        run("docker", "run", "-d", "--name", name, "--hostname", alias, *self.labels,
            "--network", self.network, "--network-alias", alias, *extra, self.image)
        self.containers.append(name)
        arguments = ["sshcc", "init", "--role", role, "--owner", "0", "--label", label]
        if role in ("target", "both"):
            # Containers have no logind, so nobody can be "logged in": run files/jobs as the service.
            arguments += ["--allow-principal", "1100", "--execution-mode", "service"]
        self.exec(name, *arguments)
        run("docker", "exec", "-d", name, "sh", "-c", "exec sshcc service run >>/tmp/service.log 2>&1")
        for _ in range(100):
            if self.exec(name, "sshcc", "service", "status", check=False).returncode == 0:
                return name
            time.sleep(0.05)
        raise RuntimeError("service did not start in " + name)

    def enroll(self, target, alias, tenant, master=None):
        # Same real-SSH enrollment path as tests/ssh_e2e.py.
        self.exec(target, "sh", "-c", "rm -f /etc/ssh/ssh_host_* && ssh-keygen -A >/dev/null && usermod -p '*' sshcc-bridge")
        config = ("Port 22\nHostKey /etc/ssh/ssh_host_ed25519_key\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
                  "PermitRootLogin no\nUsePAM no\nAuthorizedKeysFile .ssh/authorized_keys\nPidFile /run/sshd.pid\n")
        self.exec(target, "sh", "-c", "cat > /etc/ssh/sshd_config", stdin=config)
        run("docker", "exec", "-d", target, "/usr/sbin/sshd", "-D", "-e")
        routes = [{"kind": "lan", "address": alias, "port": 22}]
        bundle = self.request("enrollment.export", {"targetId": alias, "tenant": tenant, "os": "linux", "user": "sshcc-bridge", "routes": routes}, master)
        enrolled = json.loads(self.exec(target, "sshcc", "request", "enrollment.accept", "--params", json.dumps({"bundle": bundle})).stdout)
        forced = f'/usr/bin/sshcc bridge --master-id {enrolled["id"]} --master-key {enrolled["fingerprint"]}'
        line = 'restrict,command="' + forced + '" ' + enrolled["publicKey"] + " sshcc:" + enrolled["id"] + "\n"
        self.exec(target, "sh", "-c", "mkdir -p /home/sshcc-bridge/.ssh && cat >> /home/sshcc-bridge/.ssh/authorized_keys && "
                  "chown -R sshcc-bridge:sshcc-bridge /home/sshcc-bridge/.ssh && chmod 700 /home/sshcc-bridge/.ssh", stdin=line)
        host_key = self.exec(target, "cat", "/etc/ssh/ssh_host_ed25519_key.pub").stdout.strip()
        registration = json.loads(self.exec(target, "sshcc", "request", "enrollment.registration", "--params", json.dumps(
            {"targetId": alias, "tenant": tenant, "os": "linux", "user": "sshcc-bridge", "routes": routes, "hostKeys": [host_key]})).stdout)
        self.request("targets.import", {"registration": registration}, master)

    def start(self):
        run("docker", "network", "create", "--internal", *self.labels, self.network)
        if self.scenario == "sites":
            return self.start_sites()
        self.master = self.container("master", "master", "ui-preview-master")
        templates = [path for path in sorted((ROOT / "dist").glob("sshcc-client-*-template.*"))]
        entries = []
        self.exec(self.master, "mkdir", "-p", "/usr/lib/sshcc/templates")
        for path in templates:
            kind = {"sshcc-client-windows-template.exe": "windows", "sshcc-client-ubuntu-template.deb": "linux"}.get(path.name)
            if kind:
                run("docker", "cp", path, f"{self.master}:/usr/lib/sshcc/templates/{path.name}")
                entries.append({"os": kind, "fileName": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
        self.exec(self.master, "sh", "-c", "cat > /usr/lib/sshcc/templates/template-manifest.json",
                  stdin=json.dumps({"schemaVersion": 1, "templates": entries}))
        self.templates = [entry["os"] for entry in entries]
        for index in range(1, self.target_count + 1):
            alias = f"demo-ubuntu-{index}"
            socket = self.docker_socket and index == 1
            extra = ("-v", "/var/run/docker.sock:/var/run/docker.sock") if socket else ()
            target = self.container(alias, "target", f"Demo Ubuntu {index}", extra)
            self.enroll(target, alias, "demo" if index % 2 else "lab")
            if socket:
                self.install_docker_cli(target)
        if self.docker_socket:
            self.probe = "sshcc-ct-probe-" + self.run_id
            run("docker", "run", "-d", "--name", self.probe, *self.labels, "alpine:3.20",
                "sh", "-c", "while true; do date -u +probe-%H:%M:%S; sleep 5; done")

    def start_sites(self):
        # Site A: master-a (this window) with a1, a2. Site B: master-b with b1, b2.
        self.master = self.container("master-a", "master", "master-a")
        self.templates = []
        master_b = self.container("master-b", "both", "master-b")
        for site, master in (("a", self.master), ("b", master_b)):
            for index in (1, 2):
                alias = f"{site}{index}"
                socket = self.docker_socket and alias == "a1"
                extra = ("-v", "/var/run/docker.sock:/var/run/docker.sock") if socket else ()
                target = self.container(alias, "target", f"{site.upper()}{index} server", extra)
                self.enroll(target, alias, f"site-{site}", master)
                if socket:
                    self.install_docker_cli(target)
        # master-a controls master-b directly, then merges it to reach b1/b2 through it.
        self.enroll(master_b, "master-b", "masters", self.master)
        merged = self.request("merges.add", {"targetId": "master-b"})
        assert merged.get("status") == "merged", merged
        for name, master in (("DC-A", "local"), ("DC-B", "master-b")):
            datacenter = self.request("datacenters.create", {"name": name})
            self.request("datacenters.assign", {"master": master, "datacenterId": datacenter["id"]})
        if self.docker_socket:
            self.probe = "sshcc-ct-probe-" + self.run_id
            run("docker", "run", "-d", "--name", self.probe, *self.labels, "alpine:3.20",
                "sh", "-c", "while true; do date -u +probe-%H:%M:%S; sleep 5; done")

    def install_docker_cli(self, target):
        # Copy the docker client from the official CLI image; the socket is this computer's daemon.
        holder = run("docker", "create", *self.labels, "docker:27-cli").stdout.strip()
        try:
            local = self.output / "docker-cli"
            self.output.mkdir(parents=True, exist_ok=True)
            run("docker", "cp", f"{holder}:/usr/local/bin/docker", local)
            run("docker", "cp", local, f"{target}:/usr/bin/docker")
        finally:
            run("docker", "rm", "-f", holder, check=False)

    def stop(self):
        if self.probe:
            run("docker", "rm", "-f", self.probe, check=False)
        for name in reversed(self.containers):
            run("docker", "rm", "-f", name, check=False)
        run("docker", "network", "rm", self.network, check=False)

    def invoke(self, command, args):
        if command == "rpc":
            return self.request(args["method"], args.get("params") or {})
        if command == "client_links":
            return []
        if command == "save_setup":
            status = self.request("pairing.status", {"requestId": args["requestId"]})
            bundle = status["envelope"]["bundle"]
            extension = "exe" if bundle["os"] == "windows" else "deb"
            self.exec(self.master, "sh", "-c", "cat > /tmp/preview-request.json", stdin=json.dumps(status["envelope"]))
            remote = f"/tmp/sshcc-client-{status['targetId']}.{extension}"
            self.exec(self.master, "sshcc", "pairing", "package", "--request", "/tmp/preview-request.json", "--output", remote)
            self.output.mkdir(parents=True, exist_ok=True)
            local = self.output / pathlib.Path(remote).name
            run("docker", "cp", f"{self.master}:{remote}", local)
            return {"requestId": args["requestId"], "path": str(local)}
        if command == "save_remote_file":
            # The browser has no save dialog: chunks are written to the preview output folder instead.
            self.output.mkdir(parents=True, exist_ok=True)
            local, offset = self.output / pathlib.Path(args["name"]).name, 0
            with open(local, "wb") as file:
                while True:
                    chunk = self.request("files.read", {"targetId": args["targetId"], "path": args["path"], "offset": offset})
                    data = base64.b64decode(chunk["contentBase64"])
                    file.write(data)
                    offset += len(data)
                    if chunk["eof"] or not data:
                        break
            return str(local)
        raise RuntimeError(f"'{command}' works only in the desktop app, not in the browser preview")


# Same policy as desktop/src-tauri/tauri.conf.json, with fetch to this server instead of ipc:.
CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'"


def handler(preview):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            name = "index.html" if self.path in ("/", "/index.html") else self.path.lstrip("/")
            fonts = {"fonts/" + font.name for font in (UI / "fonts").glob("*.woff2")}
            if name not in {"index.html", "app.js", "app.css", "sky.js", "sky.css", "strings.js", "bridge.js"} | fonts:
                self.send_error(404)
                return
            # The bridge is served as a file so the page runs under the app's real CSP.
            data = BRIDGE.encode() if name == "bridge.js" else (UI / name).read_bytes()
            kind = {"html": "text/html; charset=utf-8", "js": "text/javascript; charset=utf-8", "css": "text/css; charset=utf-8", "woff2": "font/woff2", "svg": "image/svg+xml"}[name.rsplit(".", 1)[1]]
            if name == "index.html":
                data = data.replace(b"<head>", b'<head><script src="bridge.js"></script>', 1)
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Security-Policy", CSP)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            if self.path != "/invoke":
                self.send_error(404)
                return
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            try:
                payload, status = {"result": preview.invoke(body["command"], body.get("args") or {})}, 200
            except Exception as error:
                payload, status = {"error": str(error)}, 500
            data = json.dumps(payload, ensure_ascii=False).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass
    return Handler


def alive(pid):
    try:
        os.kill(int(pid), 0)
    except (ValueError, ProcessLookupError):
        return False
    except PermissionError:
        pass  # the process exists but belongs to another user
    return True


def stale(kind):
    """IDs of preview containers or networks whose owning preview process is gone; unlabelled ones are kept."""
    command = ["network", "ls"] if kind == "network" else ["ps", "-a"]
    listing = run("docker", *command, "--filter", "label=sshcc.ui-preview",
                  "--format", '{{.ID}} {{.Label "' + PID_LABEL + '"}}').stdout.splitlines()
    rows = [line.split(" ", 1) + [""] for line in listing if line.strip()]
    return [row[0] for row in rows if row[1].strip() and not alive(row[1].strip())], [row[0] for row in rows if not row[1].strip()]


def sweep_stale():
    """Remove containers and networks left by an earlier preview that was killed before its cleanup ran.
    Previews still running (in another terminal or session) are left alone."""
    containers, unowned = stale("ps")
    if containers:
        run("docker", "rm", "-f", *containers, check=False)
    networks, _ = stale("network")
    if networks:
        run("docker", "network", "rm", *networks, check=False)
    if containers:
        print(f"removed {len(containers)} containers left by an earlier preview", file=sys.stderr)
    if unowned:
        print(f"kept {len(unowned)} preview containers from an older script; if none is running, remove them with "
              "docker rm -f $(docker ps -aq --filter label=sshcc.ui-preview)", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="sshcc-e2e:local")
    parser.add_argument("--targets", type=int, default=2, help="demo Ubuntu Targets to enroll")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "artifacts/ui-preview")
    parser.add_argument("--docker-socket", action="store_true", help="let the first demo Target see this computer's Docker")
    parser.add_argument("--scenario", choices=["demo", "sites"], default="demo", help="sites: two Masters in DC-A and DC-B, A reaching B's machines through B")
    args = parser.parse_args()
    preview = Preview(args.image, args.targets, args.output, args.docker_socket, args.scenario)
    # Bind first: if another preview holds the port, fail before touching any containers.
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(preview))
    # A stop (SIGTERM/SIGHUP) cleans up like Ctrl-C does.
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    sweep_stale()
    try:
        preview.start()
        print(json.dumps({"url": f"http://127.0.0.1:{args.port}/", "scenario": args.scenario, "templates": preview.templates}), flush=True)
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        preview.stop()
        print("preview stopped; containers removed", file=sys.stderr)


if __name__ == "__main__":
    main()
