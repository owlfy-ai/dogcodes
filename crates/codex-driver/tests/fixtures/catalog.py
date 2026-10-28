#!/usr/bin/env python3
"""Private app-server fixture. Never imports an installed Agent or credentials."""
import json
import os
import signal
import subprocess
import sys
import time

# Tests hard-link this fixed executable into a private fixture directory. The
# driver canonicalizes executable paths, so a symlink cannot carry test data.
os.chdir(os.path.dirname(os.path.abspath(__file__)))
with open('plan.json', encoding='utf-8') as source:
    plan = json.load(source)
previous = []
if os.path.exists('starts.jsonl'):
    with open('starts.jsonl', encoding='utf-8') as source:
        previous = [json.loads(line) for line in source]
ordinal = len(previous)
mode = plan[min(ordinal, len(plan) - 1)]
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(90)'])
def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
with open('starts.jsonl', 'a', encoding='utf-8') as output:
    json.dump({'ordinal': ordinal, 'mode': mode, 'pids': [os.getpid(), child.pid], 'previousAlive': [pid for row in previous if row['mode'] != 'peer' for pid in row['pids'] if alive(pid)]}, output)
    output.write('\n')
def result(request, data):
    print(json.dumps({'id': request['id'], 'result': data}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    with open('requests.jsonl', 'a', encoding='utf-8') as output:
        output.write(json.dumps({'ordinal': ordinal, 'request': request}) + '\n')
    method = request.get('method')
    if 'id' not in request:
        continue
    if method == 'initialize':
        if mode == 'blocked-initialize':
            while True:
                time.sleep(60)
        result(request, {})
    elif method == 'model/list':
        if mode == 'blocked-list':
            while True:
                time.sleep(60)
        if mode == 'error':
            print(json.dumps({'id': request['id'], 'error': {'code': -32000, 'message': 'private fixture rejection'}}), flush=True)
        elif request.get('params', {}).get('cursor') == 'second':
            result(request, {'data': [{'id': 'fixture-alt', 'model': 'fixture-alt', 'displayName': 'Other', 'supportedReasoningEfforts': [{'reasoningEffort': 'high'}]}], 'nextCursor': None})
        else:
            result(request, {'data': [{'id': 'fixture-default', 'model': 'fixture-default', 'displayName': 'Default', 'isDefault': True, 'defaultReasoningEffort': 'medium', 'supportedReasoningEfforts': [{'reasoningEffort': 'medium'}, {'reasoningEffort': 'high'}]}], 'nextCursor': 'second'})
    elif method == 'thread/read':
        if mode == 'blocked-read':
            while True:
                time.sleep(60)
        if mode == 'error':
            print(json.dumps({'id': request['id'], 'error': {'code': -32000, 'message': 'private fixture rejection'}}), flush=True)
        else:
            result(request, {'thread': {'turns': [{'status': 'completed', 'items': [{'type': 'agentMessage', 'text': 'private child answer'}]}]}})
    elif method in ['thread/start', 'thread/resume']:
        result(request, {'thread': {'id': 'peer', 'model': 'fixture-default', 'status': 'idle'}})
    elif method == 'turn/start':
        result(request, {'turn': {'id': 'peer-turn'}})
    elif method == 'account/read':
        result(request, {'account': {'type': 'apiKey'}, 'requiresOpenaiAuth': True})
    elif method == 'config/read':
        result(request, {'config': {}})
    else:
        result(request, {})
