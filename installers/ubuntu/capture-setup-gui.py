#!/usr/bin/env python3
"""Capture actual setup GTK dialogs with public example data; disposable Xvfb only."""
import base64,hashlib,json,os,subprocess,tempfile,time
from pathlib import Path
import gi
gi.require_version('Gdk','3.0')
from gi.repository import Gdk
SOURCE=Path(__file__).resolve().parents[2]
OUT=Path(os.environ['SSHCC_SCREENSHOT_DIR']);OUT.mkdir(exist_ok=True)
HERE=Path(__file__).resolve().parent
report=json.loads((SOURCE/'artifacts/onboarding/ubuntu-onboarding.json').read_text())
inspected={'envelope':report['checks']['pairing1']['envelope']}
# Representative public receipt for the UI illustration; never presented as enrollable production data.
receipt={'schemaVersion':1,'requestId':inspected['envelope']['requestId'],'masterId':inspected['envelope']['masterId'],'bundleDigest':inspected['envelope']['bundleDigest'],'registration':{'schemaVersion':2,'id':'ubuntu-example','label':'เครื่อง Ubuntu ตัวอย่าง','tenant':'demo','nodeId':report['checks']['receipt1']['nodeId'],'os':'linux','user':'sshcc-bridge','bridgePath':'/usr/bin/sshcc','runnerInstalled':True,'routes':inspected['envelope']['bundle']['routes'],'hostKeys':['ssh-ed25519 EXAMPLE_PUBLIC_HOST_KEY']}}
payload=json.dumps(receipt,ensure_ascii=False,separators=(',',':')).encode()
code='SSHCC1.'+base64.urlsafe_b64encode(payload).decode().rstrip('=')+'.'+hashlib.sha256(payload).hexdigest()
Gdk.init([])
def visible():
    for _ in range(100):
        p=subprocess.run(['xdotool','search','--onlyvisible','--class','zenity'],capture_output=True,text=True)
        if p.returncode==0 and p.stdout.strip(): return p.stdout.splitlines()[-1]
        time.sleep(.1)
    raise AssertionError('No native dialog')
def capture(process,name):
    try:
        win=visible();time.sleep(.5)
        geometry=subprocess.check_output(['xdotool','getwindowgeometry','--shell',win],text=True)
        v=dict(line.split('=',1) for line in geometry.splitlines() if '=' in line)
        pixels=Gdk.pixbuf_get_from_window(Gdk.get_default_root_window(),int(v['X']),int(v['Y']),int(v['WIDTH']),int(v['HEIGHT']))
        assert pixels
        pixels.savev(str(OUT/name),'png',[],[])
        subprocess.run(['xdotool','windowfocus','--sync',win],check=True)
        subprocess.run(['xdotool','key','--window',win,'Escape'],check=True)
        process.wait(timeout=10)
    finally:
        if process.poll() is None: process.kill();process.wait()
loader="import importlib.machinery,pathlib;m=importlib.machinery.SourceFileLoader('setup_helper',"+repr(str(HERE/'setup-helper.py'))+").load_module();"
script=loader+"m.dialog('--question','--title=อนุญาตเชื่อมต่อเครื่อง','--text='+m.approve_text("+repr(inspected)+"),'--ok-label=Approve — อนุญาต','--cancel-label=ยกเลิก','--width=580')"
capture(subprocess.Popen(['python3','-c',script]),'ubuntu-approve-example.png')
with tempfile.TemporaryDirectory(prefix='sshcc-screenshot-') as tmp:
    path=Path(tmp)/'pairing-code.txt';path.write_text(code)
    script=loader+'m.show_code(pathlib.Path('+repr(str(path))+'),True)'
    capture(subprocess.Popen(['python3','-c',script]),'ubuntu-copy-code-example.png')
(OUT/'ubuntu-ui-screenshots.json').write_text(json.dumps({'source':'Actual setup-helper.py Zenity dialogs under disposable Ubuntu/Xvfb','data':'Public illustration fixtures; screenshots are not a production enrollment receipt','files':['ubuntu-approve-example.png','ubuntu-copy-code-example.png']},indent=2)+'\n')
print('Native screenshots saved to '+str(OUT))
