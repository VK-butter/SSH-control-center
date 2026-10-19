#!/usr/bin/env python3
"""Populate exactly 50 public fixtures in an explicitly isolated service. Never connects SSH."""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--sshcc", required=True)
parser.add_argument("--isolated-state", required=True, type=Path)
args = parser.parse_args()
state = args.isolated_state.resolve(strict=True)
assert os.environ.get("SSHCC_DATA_DIR") and Path(os.environ["SSHCC_DATA_DIR"]).resolve() == state, "SSHCC_DATA_DIR must match --isolated-state"
config = json.loads((state/"config.json").read_text())
assert config["label"].startswith("benchmark:"), "Use a fresh config labelled benchmark:<anything>"
flags = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
existing = json.loads(subprocess.check_output([args.sshcc,"request","targets.list"],text=True,**flags))
assert not existing, "Refuse to modify a non-empty registry"
raw = b"\0\0\0\x0bssh-ed25519\0\0\0\x20"+bytes(32)
public = "ssh-ed25519 "+base64.b64encode(raw).decode()
with tempfile.TemporaryDirectory(prefix="sshcc-fixtures-") as work:
    params = Path(work)/"params.json"
    # Import requires a tenant key owned by this Master. Export provisions it locally only.
    enrollment={"targetId":"benchmark-bootstrap","tenant":"benchmark","os":"linux","user":"sshcc-bridge",
                "routes":[{"kind":"lan","address":"192.0.2.1","port":22}]}
    params.write_text(json.dumps(enrollment),encoding="utf-8")
    subprocess.run([args.sshcc,"request","enrollment.export","--params-file",str(params)],check=True,stdout=subprocess.DEVNULL,**flags)
    for i in range(50):
        target = {"schemaVersion":2,"id":f"benchmark-{i:02d}","nodeId":str(uuid.uuid4()),"label":f"Offline fixture {i:02d}",
                  "tenant":"benchmark","os":"linux","user":"sshcc-bridge","hostKeys":[public],
                  "routes":[{"kind":"lan","address":"192.0.2."+str(i+1),"port":22}],
                  "bridgePath":"/usr/bin/sshcc","runnerInstalled":True}
        params.write_text(json.dumps({"registration":target}),encoding="utf-8")
        subprocess.run([args.sshcc,"request","targets.import","--params-file",str(params)],check=True,stdout=subprocess.DEVNULL,**flags)
print("Seeded 50 documentation-range offline fixtures; no network requests made.")
