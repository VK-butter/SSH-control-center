"""Validate release personalization with an isolated Master; no installation or SSH."""
import argparse,base64,gzip,hashlib,io,json,os,subprocess,tarfile,tempfile,time,uuid
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--sshcc',required=True);p.add_argument('--templates',required=True,type=Path);p.add_argument('--report',required=True,type=Path);a=p.parse_args()
exe=str(Path(a.sshcc).resolve());work=Path(tempfile.mkdtemp(prefix='sshcc-package-smoke-'));env=os.environ.copy();env['SSHCC_DATA_DIR']=str(work/'state');env['SSHCC_SOCKET']=r'\\.\pipe\sshcc-package-'+str(uuid.uuid4()) if os.name=='nt' else str(work/'control.sock')
options={'creationflags':subprocess.CREATE_NO_WINDOW} if os.name=='nt' else {};service=None;report={'os':os.name,'version':subprocess.check_output([exe,'--version'],text=True).strip().split()[-1],'installedServiceModified':False,'checks':[],'success':False};log=(work/'service.log').open('wb')
def cli(*args):
 r=subprocess.run([exe,*map(str,args)],env=env,capture_output=True,text=True,encoding='utf-8',timeout=45,**options);assert r.returncode==0,r.stderr;return json.loads(r.stdout)
def unpack_deb(path):
 blob=path.read_bytes();assert blob[:8]==b'!<arch>\n';pos=8;members={}
 while pos<len(blob):
  header=blob[pos:pos+60];assert header[58:60]==b'`\n';n=int(header[48:58]);name=header[:16].decode().strip().rstrip('/');members[name]=blob[pos+60:pos+60+n];pos+=60+n+(n%2)
 return members
try:
 owner=cli('identity')['principal'];cli('init','--role','master','--owner',owner,'--label','Package smoke Master')
 service=subprocess.Popen([exe,'service','run'],env=env,stdout=log,stderr=log,**options)
 for attempt in range(50):
  try: cli('request','system.status');break
  except AssertionError: assert service.poll() is None;time.sleep(.1)
 else: raise AssertionError('Master did not start')
 for system,ext in [('windows','exe'),('linux','deb')]:
  request=work/(system+'.json');result=cli('pairing','create','--label','Thai test เครื่องใหม่ '+system,'--tenant','demo','--os',system,'--address','192.0.2.1','--output',request)
  envelope=json.loads(request.read_text(encoding='utf-8'));target=work/('client.'+ext)
  cli('pairing','package','--request',request,'--templates',a.templates,'--output',target)
  if system=='windows':
   blob=target.read_bytes();assert blob[-16:]==b'SSHCCPAIRING0001';size=int.from_bytes(blob[-24:-16],'little');actual=json.loads(blob[-24-size:-24]);assert actual==envelope
   template=a.templates/'sshcc-client-windows-template.exe';assert blob[:-24-size]==template.read_bytes()
  else:
   members=unpack_deb(target)
   with tarfile.open(fileobj=io.BytesIO(members['data.tar.gz']),mode='r:gz') as archive:
    info=next(t for t in archive if t.name.lstrip('./')=='usr/lib/sshcc-setup/assets/envelope.json');actual=json.load(archive.extractfile(info));assert actual==envelope
   with tarfile.open(fileobj=io.BytesIO(members['control.tar.gz']),mode='r:gz') as archive:
    info=next(t for t in archive if t.name.lstrip('./')=='control');control=archive.extractfile(info).read().decode();assert 'Package: sshcc-setup' in control and '+pairing.' in control
  report['checks'].append({'export':system,'bytes':target.stat().st_size,'sha256':hashlib.sha256(target.read_bytes()).hexdigest(),'requestId':envelope['requestId'],'publicEnvelopeMatches':True})
  encoded=json.dumps(envelope,ensure_ascii=False);assert 'PRIVATE KEY' not in encoded and 'token' not in encoded.lower()
 report['success']=True
except Exception as error:
 report['error']=str(error);raise
finally:
 if service is not None:
  service.terminate();service.wait(timeout=10)
 report['temporaryMasterStopped']=service is None or service.poll() is not None
 log.close();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,ensure_ascii=False))
