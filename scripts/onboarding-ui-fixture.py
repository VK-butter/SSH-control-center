"""Loopback-only UI test fixture. Never contacts service or actual Targets."""
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import json
ROOT = Path(__file__).resolve().parents[1] / "desktop/ui"
MOCK = r"""
const fixture = { requests: [], targets: [], calls: [], saves: [] };
window.__fixture = fixture;
window.__TAURI__ = { core: { invoke: async (command, args = {}) => {
  fixture.calls.push({command, args});
  if (command === "save_setup") { const saved = {requestId:args.requestId,path:"UI-test-only-client.exe"};fixture.saves.push(saved);return saved; }
  if (command !== "rpc") throw Error("unexpected native command");
  const p=args.params||{};
  switch(args.method) {
    case "system.status": return {label:"เครื่องหลักจำลองสำหรับทดสอบ UI",roles:["master"],version:"0.4.0"};
    case "targets.list": return fixture.targets;
    case "pairing.create": {
      const row={requestId:crypto.randomUUID(),targetId:"ui-test-only",label:p.label,tenant:p.tenant,os:p.os,state:"awaiting_approval",createdAt:new Date().toISOString(),envelope:{bundle:{os:p.os,master:{label:"เครื่องหลักจำลองสำหรับทดสอบ UI"},routes:[{kind:"lan",address:p.address,port:22}]}}};
      fixture.requests.push(row);return row;
    }
    case "pairing.status":
      if(!p.requestId) return fixture.requests;
      const item=fixture.requests.find(r=>r.requestId===p.requestId);
      if(p.retry){item.state="ready";item.diagnostics={ok:true,stages:[]};fixture.targets[0].routes=p.routes;}
      return item;
    case "pairing.complete":
      if(p.code!=="SSHCC1.UI-FIXTURE") throw Error("E_PAIRING_CODE: invalid fixture code");
      const item2=fixture.requests[0];item2.state="needs_attention";item2.diagnostics={ok:false,stages:[{ok:false,message:"เครื่องทดสอบจำลองยังไม่เชื่อมต่อ"}]};
      fixture.targets=[{id:item2.targetId,label:item2.label,tenant:item2.tenant,os:item2.os,runnerInstalled:true,routes:item2.envelope.bundle.routes}];return item2;
    default: throw Error("unexpected RPC "+args.method);
  }
}}};
"""
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/fixture.js":
            data = MOCK.encode(); kind = "text/javascript"
        else:
            name = "index.html" if self.path == "/" else self.path.lstrip("/")
            if name not in {"index.html","app.js","app.css"}:
                self.send_error(404); return
            data = (ROOT/name).read_bytes()
            kind = "text/html" if name.endswith("html") else "text/javascript" if name.endswith("js") else "text/css"
            if name == "index.html":
                data = data.replace(b"<head>",b'<head><script src="/fixture.js"></script>')
        self.send_response(200);self.send_header("Content-Type",kind+"; charset=utf-8");self.end_headers();self.wfile.write(data)
    def log_message(self,*args): pass
server = HTTPServer(("127.0.0.1",0),Handler)
print(json.dumps({"url":f"http://127.0.0.1:{server.server_port}/","fixture":True}),flush=True)
server.serve_forever()
