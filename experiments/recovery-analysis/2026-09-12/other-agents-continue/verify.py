"""Isolated native-provider acceptance of continuation after service SIGKILL."""
import importlib.util, pathlib, tempfile, subprocess, time, json, traceback
OUT = pathlib.Path(__file__).resolve().parent
ROOT = OUT.parents[3]
spec = importlib.util.spec_from_file_location('daemon_verify', ROOT / 'experiments/daemon-acceptance/2026-09-12/verify.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
results = json.loads((OUT / 'results.json').read_text()) if (OUT / 'results.json').exists() else []

def wait_for(fn, seconds=180):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            value = fn()
            if value: return value
        except (FileNotFoundError, ConnectionError, OSError): pass
        time.sleep(.5)
    raise TimeoutError('condition timed out')

for provider in ['codex', 'claude-code', 'zcode', 'antigravity', 'grok', 'cursor', 'codebuddy']:
    if any(r['provider'] == provider and r['passed'] for r in results): continue
    results = [r for r in results if r['provider'] != provider]
    directory = pathlib.Path(tempfile.mkdtemp(prefix='dogcodes-continue-' + provider + '-'))
    workspace = directory / 'workspace'; workspace.mkdir()
    c = base.Client(directory / 'data')
    log = open(OUT / (provider + '-service.log'), 'w')
    service = None; task = None
    result = dict(provider=provider, directory=str(directory), passed=False)
    def launch():
        return subprocess.Popen([str(base.BIN), '--core-service', str(directory / 'data')], stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
    def candidates():
        return [e for e in c.call('registered_executors', []) if e['providerId'] == provider]
    def events():
        return [e for e in c.call('sync_events', [0, 10000]) if e.get('taskId') == task or e.get('payload', {}).get('taskId') == task]
    try:
        service = launch(); result['oldServicePid'] = service.pid
        choices = wait_for(candidates, 90)
        executor = choices[0]
        if provider == 'codex':
            good = {i['installationId'] for i in c.call('bootstrap', [])['installations'] if i.get('executable', '').startswith('/Applications/ChatGPT.app/')}
            executor = next((e for e in choices if e.get('installationId') in good), executor)
        project = base.uid('prj'); task = base.uid('tsk')
        result.update(taskId=task, executor=executor)
        c.execute('register_project', dict(projectId=project, displayName='Recovery acceptance', path=str(workspace), additionalDirectories=[]))
        c.execute('create_task', dict(taskId=task, projectId=project, title='Interrupted turn continuation', preferredExecutor=executor))
        catalog = c.call('permission_catalog', [executor, task])
        mode = catalog['defaultModeId']
        result['permissionMode'] = mode
        def submit(prompt):
            return c.execute('submit_turn', dict(taskId=task, prompt=prompt, model=None, thinking=None, permissionMode=mode, executorOverride=None), c.task(task)['streamVersion'])
        submit('请执行一次终端命令 printf hello > pending.txt，结束后回答完成。不要执行其他命令。')
        def get_pending():
            pending = c.call('pending_approvals', [task, 20])
            if not pending and not c.task(task).get('activePrimaryRun'):
                raise RuntimeError('initial turn ended without pending approval')
            return pending
        pending = wait_for(get_pending)
        (OUT / (provider + '-pending.json')).write_text(json.dumps(pending, ensure_ascii=False, indent=2))
        assert not (workspace / 'pending.txt').exists(), 'command executed before approval'
        old_run = c.task(task)['activePrimaryRun']; result['oldRun'] = old_run
        service.kill(); service.wait(timeout=10)
        service = launch(); result['newServicePid'] = service.pid
        wait_for(lambda: c.call('ping', [])['pid'] == service.pid, 90)
        wait_for(lambda: not c.task(task).get('activePrimaryRun'), 30)
        assert not c.call('pending_approvals', [task, 20]), 'old approval still pending'
        result.update(oldRunReleased=True, approvalExpired=True)
        wait_for(candidates, 90)
        submit('继续。本次仅回答 RECOVERY_CONTINUE_OK，不执行任何命令，也不要重发上一条写文件操作。')
        result['directContinueAccepted'] = True
        wait_for(lambda: not c.task(task).get('activePrimaryRun'))
        own = events(); timeline = c.call('timeline', [task])
        (OUT / (provider + '-events.json')).write_text(json.dumps(own, ensure_ascii=False, indent=2))
        (OUT / (provider + '-timeline.json')).write_text(json.dumps(timeline, ensure_ascii=False, indent=2))
        completed = [e for e in own if e['eventType'] == 'turn.completed']
        assert len(completed) == 1, 'continued turn did not complete exactly once'
        assert completed[0]['payload'].get('turnId') != old_run['turnId'], 'old turn marked complete'
        assert any(e['eventType'] == 'turn.connection_lost' for e in own)
        assert len([e for e in own if e['eventType'] == 'turn.queued']) == 2
        # Exclude the user's prompt: native messages may be activity JSON or assistant text.
        native = []
        for item in timeline:
            if item.get('kind') == 'assistant_message': native.append(item.get('text', ''))
            elif item.get('kind') == 'activity':
                try:
                    activity = json.loads(item.get('text', ''))
                    if activity.get('type') == 'message': native.append(activity.get('text', ''))
                except (ValueError, AttributeError): pass
        assert any(text.strip() == 'RECOVERY_CONTINUE_OK' for text in native), 'native reply missing'
        assert not (workspace / 'pending.txt').exists(), 'old command replayed'
        result.update(passed=True, continuedCompleted=True, nativeReply=True, noOldWrite=True, noOldTurnCompleted=True, queuedTurns=2)
    except Exception as error:
        result['error'] = str(error)
        (OUT / (provider + '-error.txt')).write_text(traceback.format_exc())
        try:
            (OUT / (provider + '-events.json')).write_text(json.dumps(events(), ensure_ascii=False, indent=2))
            (OUT / (provider + '-timeline.json')).write_text(json.dumps(c.call('timeline', [task]), ensure_ascii=False, indent=2))
        except Exception: pass
    finally:
        if service and service.poll() is None:
            try:
                if task:
                    current = c.task(task); run = current.get('activePrimaryRun')
                    if run:
                        c.execute('cancel_turn', dict(taskId=task, turnId=run['turnId'], runId=run['runId']), current['streamVersion'])
                wait_for(lambda: c.call('retire_if_idle', []), 15)
                service.wait(timeout=10)
            except Exception:
                service.terminate()
                try: service.wait(timeout=10)
                except subprocess.TimeoutExpired: service.kill(); service.wait()
        result['serviceExited'] = service is None or service.poll() is not None
        log.close(); results.append(result)
        (OUT / 'results.json').write_text(json.dumps(results, ensure_ascii=False, indent=2))
        print(json.dumps(result, ensure_ascii=False), flush=True)
