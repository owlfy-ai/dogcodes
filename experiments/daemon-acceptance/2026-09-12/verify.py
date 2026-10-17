"""Opt-in isolated production service acceptance; never uses the user's app DB."""
import json, os, pathlib, socket, struct, subprocess, sys, tempfile, time, uuid, datetime, signal
OUT=pathlib.Path(__file__).resolve().parent
BIN=pathlib.Path(__file__).resolve().parents[3]/'target/debug/desktop'
def uid(prefix):
    alphabet='0123456789ABCDEFGHJKMNPQRSTVWXYZ';n=uuid.uuid4().int
    chars=''
    for _ in range(26): chars=alphabet[n&31]+chars;n>>=5
    return prefix+'_'+chars
class Client:
    def __init__(self,directory): self.directory=pathlib.Path(directory)
    def call(self,method,args):
        if method in ["bootstrap","registered_executors"]: args=None
        endpoint=json.loads((self.directory/'service.json').read_text())
        host,port=endpoint['address'].split(':')
        with socket.create_connection((host,int(port)),timeout=180) as s:
            data=json.dumps(dict(version=1,token=endpoint['token'],method=method,args=args)).encode()
            s.sendall(struct.pack('!I',len(data))+data)
            def read(n):
                data=b''
                while len(data)<n:
                    part=s.recv(n-len(data))
                    if not part: raise RuntimeError('service disconnected')
                    data+=part
                return data
            n=struct.unpack('!I',read(4))[0];r=json.loads(read(n))
            if 'Err' in r: raise RuntimeError(r['Err'])
            return r['Ok']
    def execute(self,kind,data,version=None):
        cmd=uid('cmd');return self.call('execute',[dict(schemaVersion=1,commandId=cmd,idempotencyKey=cmd,actor={'kind':'local_user'},expectedStreamVersion=version,issuedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),payload=dict(type=kind,data=data))])
    def task(self,id): return next(t for t in self.call('bootstrap',[])['tasks'] if t['taskId']==id)
def worker(directory,task,prompt):
    c=Client(directory)
    c.execute('submit_turn',dict(taskId=task,prompt=prompt,model=None,thinking=None,permissionMode='default',executorOverride=None),c.task(task)['streamVersion'])
    # This process stands in for the desktop RPC client. The parent kills only this PID.
    while True: time.sleep(1)
def main():
    data=pathlib.Path(tempfile.mkdtemp(prefix='dogcodes-daemon-verify-'))
    (OUT/'fixture.json').write_text(json.dumps({'directory':str(data),'binary':str(BIN)}))
    log=open(OUT/'service.log','w')
    service=subprocess.Popen([str(BIN),'--core-service',str(data)],stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
    results=[];client_process=None
    try:
        c=Client(data)
        deadline=time.time()+60
        while True:
            try:
                executors=c.call('registered_executors',[])
                if all(any(e['providerId']==p for e in executors) for p in ['codex','kimi']): break
            except (FileNotFoundError,ConnectionError): pass
            if time.time()>deadline: raise TimeoutError('service discovery')
            time.sleep(.25)
        ping=c.call('ping',[]);assert ping['pid']==service.pid
        # A second service cannot take over this database.
        second=subprocess.run([str(BIN),'--core-service',str(data)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
        assert c.call('ping',[])['pid']==service.pid
        snapshot=c.call('bootstrap',[])
        for provider in ['codex','kimi']:
            candidates=[e for e in executors if e['providerId']==provider]
            if provider=='codex':
                good={i['installationId'] for i in snapshot['installations'] if i.get('executable','').startswith('/Applications/ChatGPT.app/')}
                executor=next((e for e in candidates if e.get('installationId') in good),candidates[0])
            else: executor=candidates[0]
            workspace=data/provider;workspace.mkdir()
            project=uid('prj');task=uid('tsk')
            c.execute('register_project',dict(projectId=project,displayName='Daemon '+provider,path=str(workspace),additionalDirectories=[]))
            c.execute('create_task',dict(taskId=task,projectId=project,title='Daemon disconnect acceptance',preferredExecutor=executor))
            before=c.call('bootstrap',[])['lastSequence']
            command='printf START > start.txt; sleep 12; printf END > end.txt'
            prompt='隔离验收：请只请求一次审批执行这条命令：'+command+'。等待执行结束后回答 DAEMON_CONTINUED_OK。不要执行其他命令，不重试，不读写其他文件。'
            client_process=subprocess.Popen([sys.executable,__file__,'worker',str(data),task,prompt],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=open(OUT/(provider+'-client.log'),'w'))
            deadline=time.time()+180;approved=False;pending_preserved=False;killed=False
            while time.time()<deadline:
                approvals=c.call('pending_approvals',[task,20])
                for approval in approvals:
                    summary=json.dumps(approval,ensure_ascii=False)
                    if approved: raise RuntimeError('unexpected second approval: '+approval['toolName'])
                    # Let the request remain pending without the submitting client making any calls.
                    time.sleep(.5)
                    assert any(a['approvalId']==approval['approvalId'] for a in Client(data).call('pending_approvals',[task,20]))
                    pending_preserved=True
                    assert 'start.txt' in summary and 'end.txt' in summary and 'sleep 12' in summary, summary
                    option=next(o for o in approval['options'] if o.get('semanticRole')=='approve_once')
                    c.execute('respond_to_approval',dict(taskId=task,approvalId=approval['approvalId'],optionId=option['optionId']),c.task(task)['streamVersion'])
                    approved=True
                if (workspace/'start.txt').exists() and not killed:
                    assert not (workspace/'end.txt').exists()
                    client_process.kill();client_process.wait(timeout=5);killed=True
                    assert c.call('ping',[])['pid']==service.pid
                events=c.call('sync_events',[before,10000]);own=[e for e in events if e.get('taskId')==task]
                # Task id is also carried by payload; allow protocol envelope variants.
                own=[e for e in events if e.get('taskId')==task or e.get('payload',{}).get('taskId')==task]
                if any(e['eventType']=='turn.failed' for e in own): raise RuntimeError(provider+' failed')
                if killed and any(e['eventType']=='turn.completed' for e in own):
                    assert (workspace/'end.txt').read_text()=='END'
                    assert not any(e['eventType']=='turn.connection_lost' for e in own)
                    assert not c.task(task).get('activePrimaryRun')
                    timeline=c.call('timeline',[task]);assert 'DAEMON_CONTINUED_OK' in json.dumps(timeline)
                    result=dict(provider=provider,executor=executor,servicePid=service.pid,clientKilled=True,pendingApprovalPreserved=pending_preserved,endPresent=True,completed=True,noConnectionLost=True,taskId=task)
                    (OUT/(provider+'-events.json')).write_text(json.dumps(own,ensure_ascii=False,indent=2))
                    results.append(result);print(json.dumps(result),flush=True);break
                time.sleep(.2)
            else: raise TimeoutError(provider+' completion')
        (OUT/'results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2))
    finally:
        if client_process and client_process.poll() is None: client_process.kill();client_process.wait()
        # Only this isolated service is terminated. Agent turns must already be terminal on success.
        if len(results)==2:
            assert c.call('retire_if_idle',[]) is True
        else:
            try:
                for task in c.call('bootstrap',[])['tasks']:
                    run=task.get('activePrimaryRun')
                    if run:
                        c.execute('cancel_turn',dict(taskId=task['taskId'],turnId=run['turnId'],runId=run['runId']),task['streamVersion'])
                time.sleep(1)
            except Exception: pass
            service.terminate()
        try: service.wait(timeout=10)
        except subprocess.TimeoutExpired: service.kill();service.wait()
        log.close()
if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='worker': worker(*sys.argv[2:])
    else: main()
