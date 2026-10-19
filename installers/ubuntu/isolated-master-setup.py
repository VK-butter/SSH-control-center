#!/usr/bin/env python3
"""Fresh generic Master installer in disposable systemd Ubuntu; cross-OS export and Codex wiring."""
import hashlib,json,os,pty,select,subprocess,time,uuid
from pathlib import Path
SOURCE=Path(__file__).resolve().parents[2]
RUN=str(uuid.uuid4()); NAME='sshcc-master-setup-test-'+RUN[:8]
REPORT={'runId':RUN,'version':'0.4.0','os':'Ubuntu 24.04 disposable Docker with real systemd','hostServicesModified':False,'checks':{}}
def run(args,**kwargs):
    p=subprocess.run([str(x) for x in args],capture_output=True,text=True,**kwargs)
    assert p.returncode==0,p.stdout[-4000:]+p.stderr[-4000:]
    return p.stdout
def docker(*args): return run(['docker',*args])
def approve():
    fd,slave=pty.openpty(); p=subprocess.Popen(['docker','exec','-it',NAME,'runuser','-u','ubuntu','--','sshcc-setup','--terminal'],stdin=slave,stdout=slave,stderr=slave);os.close(slave)
    data=bytearray();sent=False;deadline=time.monotonic()+240
    try:
        while time.monotonic()<deadline:
            if select.select([fd],[],[],0.2)[0]:
                try: block=os.read(fd,65536)
                except OSError: break
                data.extend(block)
                if not sent and b'APPROVE' in data: os.write(fd,b'APPROVE\n');sent=True
            if p.poll() is not None: break
        status=p.wait(timeout=10); assert sent and status==0,data.decode(errors='replace')[-8000:]
    finally:
        os.close(fd)
        if p.poll() is None: p.kill();p.wait()
def main():
    docker('run','--rm','--name',NAME,'--privileged','--cgroupns=private','--tmpfs','/run','--tmpfs','/run/lock','--tmpfs','/tmp','--cpus=2','--memory=2g','-d','sshcc-ubuntu-build:0.4.0','/sbin/init')
    for _ in range(60):
        state=subprocess.run(['docker','exec',NAME,'systemctl','is-system-running'],capture_output=True,text=True)
        if state.stdout.strip() in ('running','degraded'): break
        time.sleep(.2)
    docker('exec',NAME,'bash','-lc',"printf 'ubuntu ALL=(ALL) NOPASSWD:ALL\\n' >/etc/sudoers.d/sshcc-test; chmod 0440 /etc/sudoers.d/sshcc-test")
    docker('exec','-u','ubuntu',NAME,'bash','-lc',"mkdir -p /home/ubuntu/.codex; printf '# existing settings\\n[mcp_servers.existing]\\ncommand = \\\"old-tool\\\"\\n' > /home/ubuntu/.codex/config.toml")
    original=docker('exec',NAME,'cat','/home/ubuntu/.codex/config.toml')
    package=SOURCE/'dist/sshcc-0.4.0-ubuntu-setup.deb'
    REPORT['fullPackageSha256']=hashlib.sha256(package.read_bytes()).hexdigest()
    docker('cp',package,NAME+':/var/tmp/master.deb')
    docker('exec',NAME,'bash','-lc','apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends /var/tmp/master.deb')
    docker('exec',NAME,'bash','-lc','test ! -e /usr/bin/sshcc && test ! -e /var/lib/sshcc/config.json')
    REPORT['checks']['bootstrapBeforeApprovalNoService']=True
    approve()
    status=json.loads(docker('exec','-u','ubuntu',NAME,'sshcc','request','system.status'))
    assert status['executionPrincipal']=='0' and status['privileged'] and status['roles']==['master']
    config=json.loads(docker('exec',NAME,'cat','/var/lib/sshcc/config.json'));assert config['owner']=='1000'
    REPORT['checks']['rootServiceOwnerCaptured']=status
    updated=docker('exec',NAME,'cat','/home/ubuntu/.codex/config.toml')
    assert updated.startswith(original) and '[mcp_servers.ssh_control_center]' in updated
    backup=docker('exec',NAME,'bash','-lc','cat /home/ubuntu/.codex/config.toml.sshcc-*.bak')
    assert backup==original
    REPORT['checks']['codexMcpPreservedOtherAndBackedUp']=True
    original_manifest=(SOURCE/'dist/template-manifest.json').read_text()
    installed=docker('exec',NAME,'cat','/usr/lib/sshcc/templates/template-manifest.json')
    assert installed==original_manifest
    REPORT['checks']['exactFrozenManifestInstalled']=json.loads(installed)
    exports=[]
    for os_name,extension in [('linux','deb'),('windows','exe')]:
        created=json.loads(docker('exec','-u','ubuntu',NAME,'sshcc','pairing','create','--label','เครื่องใหม่ '+os_name,'--tenant','demo','--os',os_name,'--address','192.0.2.25','--output','/home/ubuntu/request-'+os_name+'.json'))
        envelope=created['envelope']
        output='/home/ubuntu/client-'+os_name+'.'+extension
        exported=json.loads(docker('exec','-u','ubuntu',NAME,'sshcc','pairing','package','--request','/home/ubuntu/request-'+os_name+'.json','--output',output))
        assert exported['requestId']==envelope['requestId']
        if os_name=='linux':
            actual=docker('exec',NAME,'bash','-lc','dpkg-deb --fsys-tarfile '+output+' | tar -xO usr/lib/sshcc-setup/assets/envelope.json')
            assert json.loads(actual)==envelope
        else:
            docker('exec',NAME,'python3','-c',"import pathlib;thin=pathlib.Path('/usr/lib/sshcc/templates/sshcc-client-windows-template.exe').read_bytes();out=pathlib.Path('/home/ubuntu/client-windows.exe').read_bytes();assert out.startswith(thin) and out[:2]==b'MZ' and b'"+envelope['requestId']+"' in out[len(thin):]")
        exports.append(exported)
    REPORT['checks']['linuxMasterCreatesWindowsAndUbuntuSingleFile']=exports
    REPORT['checks']['headlessNoTray']=docker('exec',NAME,'bash','-lc','test ! -e /usr/bin/sshcc-desktop; echo yes').strip()=='yes'
    REPORT['success']=True
try: main()
except BaseException as e: REPORT['success']=False;REPORT['error']=str(e);raise
finally:
    subprocess.run(['docker','stop','--time','10',NAME],capture_output=True)
    public=SOURCE/'artifacts/onboarding';public.mkdir(exist_ok=True)
    (public/'ubuntu-master-setup.json').write_text(json.dumps(REPORT,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(REPORT,ensure_ascii=False,indent=2))
