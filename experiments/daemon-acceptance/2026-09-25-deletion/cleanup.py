import json,pathlib,subprocess,os,signal,time
from client import Client
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);snapshot=c.call('bootstrap');assert not snapshot['activeRuns'];assert all(w['secondaryAgentCount']==0 for w in snapshot['companion']['workspaces'])
service=c.call('ping')['pid'];rows=subprocess.check_output(['ps','-axo','pid=,ppid=,command='],text=True).splitlines();uis=[];children={}
for line in rows:
 pid,ppid,cmd=line.strip().split(None,2);pid=int(pid);ppid=int(ppid);children.setdefault(ppid,[]).append(pid)
 if cmd.startswith(f['root']+'/dogcodes Delete ') and cmd.endswith('/Contents/MacOS/desktop'):uis.append(pid)
for pid in uis:os.kill(pid,signal.SIGTERM)
assert c.call('ping')['pid']==service
assert c.call('retire_if_idle',[]) is True
# Only descendants of this isolated service; providers may leave idle bridge subprocesses.
def descendants(pid):
 for child in children.get(pid,[]):yield from descendants(child);yield child
for pid in descendants(service):
 try:os.kill(pid,signal.SIGTERM)
 except ProcessLookupError:pass
for _ in range(50):
 live={int(line.strip().split(None,1)[0]) for line in subprocess.check_output(['ps','-axo','pid='],text=True).splitlines()}
 if not live.intersection([service,*uis]):break
 time.sleep(.1)
else:raise AssertionError('owned UI/service still running')
result={'uiPidsStopped':uis,'servicePidStopped':service,'serviceSurvivedUiExit':True,'idleRetirement':True,'noActivePrimaryOrReview':True,'fixtureRetained':True,'productionUntouched':True}
(out/'cleanup.json').write_text(json.dumps(result,indent=2)+'\n');print(result)
