// Isolated browser fixture. Never connects to a service or a real SSH Target.
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const ui = path.join(__dirname, "..", "ui");
const calls = [];
const target = { schemaVersion: 2, id: "ui-test", nodeId: "ui-test-node", label: "UI test target", tenant: "test-only", os: "linux", user: "sshcc-bridge", routes: [{ kind: "lan", address: "192.0.2.20", port: 22 }], hostKeys: ["ssh-ed25519 TEST-FIXTURE-ONLY"], bridgePath: "/usr/bin/sshcc", runnerInstalled: true };
const jobs = [];
let content = "Example text\n";
let firstStart = true;
const sha = () => crypto.createHash("sha256").update(content).digest("hex");
const bootstrap = 'window.__TAURI__={core:{invoke:async(command,args)=>{const r=await fetch("/rpc",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({command,args})});const data=await r.json();if(!r.ok)throw new Error(data.error);return data;}}};';
const server = http.createServer(async (req, res) => {
  try {
    if (req.url === "/calls") { res.setHeader("content-type", "application/json"); return res.end(JSON.stringify(calls)); }
    if (req.url === "/rpc" && req.method === "POST") {
      let text = ""; for await (const chunk of req) text += chunk;
      const { command, args } = JSON.parse(text);
      calls.push({ command, ...args });
      let result;
      if (command === "open_shell") result = !args.ownerSessionId && process.env.SSHCC_FIXTURE_SINGLE_SESSION !== "1"
        ? { needsSession: true, sessions: [{ id: "3", account: "fixture-owner", state: "active" }, { id: "7", account: "fixture-owner", state: "online" }] }
        : { opened: true, pid: 999, targetId: args.targetId, ownerSessionId: args.ownerSessionId || "3" };
      else if (command === "open_json" || command === "save_json") result = null;
      else {
        const { method, params: p } = args;
        switch (method) {
          case "system.status": result = { label: "UI fixture (no real service)", nodeId: "fixture", roles: ["master", "target"], version: "test", uptimeSeconds: 120 }; break;
          case "targets.list": result = [target]; break;
          case "targets.doctor": result = { targetId: p.targetId, ok: true, stages: [{ name: "fixture", ok: true, message: "Isolated UI test only" }] }; break;
          case "targets.update": Object.assign(target, p.target); result = target; break;
          case "jobs.list": result = jobs; break;
          case "jobs.start":
            result = jobs.find(job => job.id === p.jobId);
            if (!result) {
              result = { id: p.jobId, masterId: "fixture-master", script: p.script, cwd: p.cwd, shell: p.shell, status: "running", createdAt: new Date().toISOString(), startedAt: new Date().toISOString(), exitCode: null };
              jobs.unshift(result);
            }
            if (firstStart) { firstStart = false; res.statusCode = 504; return res.end(JSON.stringify({ error: "Simulated timeout AFTER acceptance. Retry same form." })); }
            break;
          case "jobs.status": result = jobs.find(job => job.id === p.jobId); break;
          case "jobs.logs": {
            const log = p.stream === "stderr" ? "Fixture stderr\n" : "Fixture output <script>escaped</script>\n";
            result = { content: log.slice(p.offset || 0), nextOffset: log.length, eof: true }; break;
          }
          case "jobs.cancel": result = jobs.find(job => job.id === p.jobId); result.status = "cancelled"; break;
          case "files.list": result = [{ name: "example.txt", path: "/tmp/example.txt", isDirectory: false, isSymlink: false, size: Buffer.byteLength(content) }]; break;
          case "files.read": result = { contentBase64: Buffer.from(content).toString("base64"), size: Buffer.byteLength(content), sha256: sha(), nextOffset: Buffer.byteLength(content), eof: true }; break;
          case "files.write":
            if (p.expectedSha256 !== sha()) throw new Error("optimistic write precondition missing");
            content = Buffer.from(p.contentBase64, "base64").toString("utf8"); result = { sha256: sha() }; break;
          case "enrollment.export": result = { schemaVersion: 2, ...p, master: { id: "fixture-master", label: "Fixture", publicKey: "TEST PUBLIC KEY" } }; break;
          case "masters.list": result = [{ id: "fixture-master", label: "UI fixture Master", publicKey: "TEST", revoked: false }]; break;
          default: throw new Error("Unsupported fixture method: " + method);
        }
      }
      res.setHeader("content-type", "application/json"); return res.end(JSON.stringify(result));
    }
    const files = { "/": "index.html", "/app.css": "app.css", "/app.js": "app.js" };
    if (req.url === "/fixture.js") { res.setHeader("content-type", "application/javascript"); return res.end(bootstrap); }
    const file = files[req.url]; if (!file) { res.statusCode = 404; return res.end(); }
    const types = { ".html": "text/html", ".css": "text/css", ".js": "application/javascript" };
    let body = fs.readFileSync(path.join(ui, file), "utf8");
    if (file === "index.html") body = body.replace('<script src="app.js"', '<script src="/fixture.js"></script><script src="app.js"');
    res.setHeader("content-type", types[path.extname(file)] + "; charset=utf-8"); res.end(body);
  } catch (error) { res.statusCode = 400; res.setHeader("content-type", "application/json"); res.end(JSON.stringify({ error: error.message })); }
});
server.listen(Number(process.env.SSHCC_UI_TEST_PORT || 42197), "127.0.0.1", () => console.log("SSHCC isolated UI fixture: http://127.0.0.1:" + server.address().port));
