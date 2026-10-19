#!/usr/bin/env python3
"""Small setup helpers shared by the desktop and elevated installer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time
import tomllib


def verify_assets(assets: Path):
    for path in (assets, assets / "manifest.json"):
        mode = path.lstat()
        if stat.S_ISLNK(mode.st_mode) or mode.st_uid != 0 or mode.st_mode & 0o022:
            raise ValueError("Untrusted release asset directory or manifest")
    manifest = json.loads((assets / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("schemaVersion") != 1 or not isinstance(manifest.get("files"), dict):
        raise ValueError("Invalid release asset manifest")
    for name in ("sshcc", "sshcc-service.deb"):
        if name not in manifest["files"]:
            raise ValueError("Missing release asset: " + name)
    for name, expected in manifest["files"].items():
        rel = Path(name)
        if rel.is_absolute() or ".." in rel.parts or not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise ValueError("Invalid release asset name or hash")
        path = assets / rel
        current = assets
        for part in rel.parts:
            current = current / part
            mode = current.lstat()
            if stat.S_ISLNK(mode.st_mode) or mode.st_uid != 0 or mode.st_mode & 0o022:
                raise ValueError("Untrusted release asset: " + name)
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError("Release asset hash mismatch: " + name)
    return manifest


def inspect_request(assets: Path):
    if not (assets / "envelope.json").exists():
        return None
    result = subprocess.run(
        [str(assets / "sshcc"), "pairing", "inspect", "--request", str(assets / "envelope.json")],
        check=True, capture_output=True, text=True,
    )
    return json.loads(result.stdout)


def install_codex_mcp(home: Path):
    """Run as the captured owner: preserve unrelated TOML bytes and private permissions."""
    codex = home / ".codex"
    config = codex / "config.toml"
    if config.is_symlink() or codex.is_symlink():
        raise ValueError("Refuse a symlink Codex configuration")
    if not codex.is_dir() and not shutil.which("codex"):
        return {"configured": False, "reason": "Codex installation not found"}
    codex.mkdir(parents=True, exist_ok=True, mode=0o700)
    if config.exists() and not config.is_file():
        raise ValueError("Codex configuration is not a regular file")
    before = config.read_text(encoding="utf-8") if config.exists() else ""
    parsed = tomllib.loads(before)
    entry = parsed.get("mcp_servers", {}).get("ssh_control_center")
    if entry and entry.get("command") not in ("/usr/bin/sshcc", "sshcc"):
        raise ValueError("Existing ssh_control_center uses a custom executable; configure it manually")
    block = '\n[mcp_servers.ssh_control_center]\ncommand = "/usr/bin/sshcc"\nargs = ["mcp"]\nstartup_timeout_sec = 20\ntool_timeout_sec = 150\nenabled = true\n'
    pattern = re.compile(r"(?m)^\[mcp_servers\.ssh_control_center\][^\n]*\n.*?(?=^\[|\Z)", re.S)
    after = pattern.sub(block.lstrip("\n"), before) if pattern.search(before) else before.rstrip("\n") + "\n" + block
    # Parse the final document before writing or backing up anything.
    tomllib.loads(after)
    if after == before:
        return {"configured": True, "changed": False}
    stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime())
    backup = config.with_name("config.toml.sshcc-" + stamp + "-" + str(os.getpid()) + ".bak")
    if config.exists():
        fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(before)
    temp = config.with_name(".config.toml.sshcc-" + str(os.getpid()) + ".tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(after)
        os.replace(temp, config)
    finally:
        temp.unlink(missing_ok=True)
    return {"configured": True, "changed": True, "backup": str(backup) if backup.exists() else None}


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="action", required=True)
    verify = sub.add_parser("verify-assets")
    verify.add_argument("assets", type=Path)
    codex = sub.add_parser("codex-mcp")
    codex.add_argument("home", type=Path)
    args = parser.parse_args()
    if args.action == "verify-assets":
        print(json.dumps(verify_assets(args.assets)))
    else:
        print(json.dumps(install_codex_mcp(args.home)))


if __name__ == "__main__":
    main()
