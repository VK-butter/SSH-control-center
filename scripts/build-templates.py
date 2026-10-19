"""Make the public release template manifest after both native thin builds."""
import argparse, hashlib, json
from pathlib import Path
parser=argparse.ArgumentParser()
parser.add_argument("--directory",type=Path,required=True)
args=parser.parse_args()
entries=[]
for system,name in [("windows","sshcc-client-windows-template.exe"),("linux","sshcc-client-ubuntu-template.deb")]:
    path=args.directory/name
    if not path.is_file(): raise SystemExit("Missing release template: "+str(path))
    entries.append({"os":system,"fileName":name,"sha256":hashlib.sha256(path.read_bytes()).hexdigest()})
path=args.directory/"template-manifest.json"
path.write_text(json.dumps({"schemaVersion":1,"templates":entries},indent=2)+"\n",encoding="utf-8")
print(path)
