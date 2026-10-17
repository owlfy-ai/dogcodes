import json,pathlib,subprocess,os,signal,time
from client import Client
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);snap=c.call('bootstrap');assert not snap['tasks'];assert len(snap['deletedTasks'])==6
service=c.call('ping')['pid'];rows=subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines();uis=[]
for row in rows:
 pid,cmd=row.strip().split(None,1)
 if cmd.startswith(f['root']+'/dogcodes Delete ') and cmd.endswith('/Contents/MacOS/desktop'):uis.append(int(pid))
assert len(uis)>=2
for pid in uis:os.kill(pid,signal.SIGTERM)
assert c.call('ping')['pid']==service
assert c.call('retire_if_idle',[]) is True
f['retiredServicePid']=service;f['closedUiPids']=uis;f['deletedBeforeRestart']=[t['taskId'] for t in snap['deletedTasks']]
(out/'fixture.json').write_text(json.dumps(f,ensure_ascii=False,indent=2)+'\n');print({'closedUis':uis,'retiredService':service,'deletedCount':6})
