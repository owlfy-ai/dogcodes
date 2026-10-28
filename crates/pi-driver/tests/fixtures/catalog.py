#!/usr/bin/env python3
"""Private RPC fixture. It reads only the test workspace and never a model/account."""
import json
import os
import signal
import subprocess
import sys
import time

with open('fixture-mode', encoding='utf-8') as source:
    mode = source.read().strip()
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(90)'])
previous = []
if os.path.exists('previous-pids.json'):
    with open('previous-pids.json', encoding='utf-8') as source:
        previous = json.load(source)
def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
with open('start.json', 'w', encoding='utf-8') as output:
    json.dump({'args': sys.argv[1:], 'previousAlive': [pid for pid in previous if alive(pid)]}, output)
with open('pids.tmp', 'w', encoding='utf-8') as output:
    json.dump([os.getpid(), child.pid], output)
os.replace('pids.tmp', 'pids.json')
model = {'provider': 'fixture', 'id': 'base', 'name': 'Base'}
level = 'medium'
path = os.path.join(os.getcwd(), 'native.jsonl')
def reply(request, data=None, success=True):
    print(json.dumps({'type': 'response', 'id': request['id'], 'success': success, 'data': data, 'error': None if success else 'private fixture rejection'}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    with open('requests.jsonl', 'a', encoding='utf-8') as output:
        output.write(json.dumps(request) + '\n')
    kind = request['type']
    if kind == 'get_state':
        reply(request, {'sessionFile': path, 'model': model, 'thinkingLevel': level, 'isStreaming': False})
    elif kind == 'set_model':
        model = {'provider': request['provider'], 'id': request['modelId'], 'name': 'Other'}
        level = 'high'
        reply(request, {'model': model})
    elif kind == 'get_available_models':
        if mode == 'blocked':
            while True:
                time.sleep(60)
        reply(request, {'models': [{'provider': 'fixture', 'id': 'base', 'name': 'Base'}, {'provider': 'fixture', 'id': 'other', 'name': 'Other'}]}, mode != 'error')
    elif kind == 'get_available_thinking_levels':
        reply(request, {'levels': ['off', 'high'] if model['id'] == 'other' else ['off', 'medium']})
    else:
        reply(request)
