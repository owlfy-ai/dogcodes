"""Private ZCode protocol/account helper; no installed client or credentials."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

account = sys.argv[1] == '-e'
request = json.loads(sys.stdin.readline()) if account else None
root = ((Path(request['request']['fixtureRoot']) if request['action'] == 'runtime'
         else Path(request['builtinFile']).parents[3]) if account
        else Path(sys.argv[1]).parents[2])
mode = (root / 'metadata-mode').read_text().strip()
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([
    sys.executable, '-c',
    'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)',
], stdout=subprocess.DEVNULL if account and mode != 'quota-inherited' else None,
   stderr=subprocess.DEVNULL if account else None)

def record(method, params=None):
    fd = os.open(root / 'catalog.jsonl', os.O_CREAT | os.O_APPEND | os.O_WRONLY, 0o600)
    try:
        os.write(fd, (json.dumps({'pid': os.getpid(), 'child': child.pid,
                                 'method': method, 'params': params}) + '\n').encode())
    finally:
        os.close(fd)

def reply(r, value):
    print(json.dumps({'id': r['id'], 'result': value}), flush=True)

def snapshot(identifier):
    return {'sessionId': identifier, 'session': {'sessionId': identifier, 'workspace': {'workspacePath': str(root)}},
            'settings': {'model': {'available': [{'ref': {
                'providerId': 'fixture', 'modelId': 'fixture-model'}}]}}}

prior = []
if (root / 'catalog.jsonl').exists():
    for line in (root / 'catalog.jsonl').read_text().splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row['method'] not in ('native.spawn', 'account.spawn'):
            continue
        for pid in (row['pid'], row['child']):
            try:
                os.kill(pid, 0)
                prior.append(pid)
            except ProcessLookupError:
                pass
record('account.spawn' if account else 'native.spawn', {'priorAlive': prior,
       'action': request.get('action') if account else None})
if account:
    if request['action'] == 'runtime':
        if mode == 'runtime-blocked':
            while not (root / 'allow-runtime').exists():
                time.sleep(0.01)
        print(json.dumps({'headersApplied': True, 'requestAuth': {'apiKey': 'private-fixture-only'}}), flush=True)
        sys.exit(0)
    if request['action'] in ('quota_identity', 'quota'):
        if mode == 'quota-identity-blocked' or (mode == 'quota-blocked' and request['action'] == 'quota'):
            while True:
                time.sleep(60)
        if mode == 'quota-malformed':
            print('private unsupported account layout', flush=True)
            sys.exit(0)
        if mode == 'quota-overflow':
            sys.stdout.write('x' * 1100000)
            sys.stdout.flush()
            time.sleep(60)
        if mode == 'quota-login':
            print(json.dumps({'unavailable': 'quota_login_required'}), flush=True)
            sys.exit(0)
        identity = 'other-fixture-account' if mode == 'quota-other' else 'fixture-account'
        value = {'identity': identity}
        if request['action'] == 'quota':
            value['snapshot'] = {
                'sessionUsage': None, 'context': None,
                'windows': [{'id': 'private', 'scope': None, 'windowMinutes': 300,
                             'usedPercent': 75 if mode == 'quota-other' else 25,
                             'resetsAtUnix': None}],
                'planType': 'private', 'collectedAtUnixMs': 1,
                'staleAfterUnixMs': 60001, 'source': 'native_pull',
                'confidence': 'provider_reported', 'parserVersion': 'private-fixture',
            }
        print(json.dumps(value), flush=True)
        sys.exit(0)
    if mode == 'account-stuck':
        while True:
            time.sleep(60)
    print(json.dumps({'overlay': {'revision': 'fixture', 'providers': {}, 'states': {}},
                      'authorized': {}, 'checkedAt': 0, 'version': 'fixture'}), flush=True)
    sys.exit(0)
for line in sys.stdin:
    r = json.loads(line)
    method = r.get('method')
    params = r.get('params', {})
    record(method, params)
    if method == 'fixture/exit':
        sys.exit(0)
    if method == 'workspace/readState':
        if mode == 'direct':
            reply(r, snapshot('catalog-only'))
        else:
            print(json.dumps({'id': r['id'], 'error': {'code': -32601,
                                                      'message': 'Method not found'}}), flush=True)
    elif method == 'session/create':
        if mode.startswith('readiness-'):
            if params.get('persistence') != 'deferred':
                (root / 'persistent-probe').write_text('persistent create accepted')
            if mode == 'readiness-create-stuck':
                while True:
                    time.sleep(60)
        if params.get('persistence') == 'deferred' and mode == 'create-stuck':
            while not (root / 'allow-create').exists():
                time.sleep(0.01)
        reply(r, {} if mode == 'missing-id' else snapshot('probe' if params.get('persistence') == 'deferred' else 'primary'))
    elif method == 'session/list':
        if mode == 'readiness-bad-list' and any(
            json.loads(line)['method'] == 'session/close'
            for line in (root / 'catalog.jsonl').read_text().splitlines()
        ):
            reply(r, {})
        else:
            reply(r, {'sessions': []})
    elif method == 'session/close' and mode == 'readiness-close-missing':
        reply(r, {})
    elif method == 'session/read':
        reply(r, snapshot(params['sessionId']))
    elif method == 'session/send':
        reply(r, {})
        print(json.dumps({'method': 'turn.started', 'params': {
            'sessionId': params['sessionId'], 'turnId': 'original-turn'}}), flush=True)
    elif method == 'session/stop':
        reply(r, {})
        print(json.dumps({'method': 'turn.completed', 'params': {
            'sessionId': params['sessionId'], 'resultType': 'cancelled'}}), flush=True)
    elif method == 'provider/updateAccountConfig':
        reply(r, {'receivedRevision': params['revision']})
    else:
        reply(r, {'closed': True} if method == 'session/close' else {})
