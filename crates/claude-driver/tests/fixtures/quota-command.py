#!/usr/bin/env python3
"""Private curl replacement. Never opens a network connection."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

root = Path.cwd()
mode = (root / 'mode').read_text().strip()
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([
    sys.executable, '-c',
    'import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(120)',
    str(root),
])
previous = []
index = len(list(root.glob('query-*.json')))
if index:
    for pid in json.loads((root / f'query-{index - 1}.json').read_text())['pids']:
        state = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True, text=True).stdout.strip()
        if state and not state.startswith('Z'):
            previous.append(pid)
record = {'pids': [os.getpid(), child.pid], 'previousAlive': previous}
pending = root / 'query.pending'
pending.write_text(json.dumps(record))
pending.replace(root / f'query-{index}.json')
if mode == 'credential':
    sys.stdout.write('{"private":true}')
    sys.stdout.flush()
    sys.exit(0)
if mode == 'blocked-input':
    time.sleep(120)
data = sys.stdin.buffer.read()
assert b'Authorization: Bearer private-fixture-token' in data
assert sys.argv[-1] == 'https://api.anthropic.com/api/oauth/usage'
(root / f'input-{index}.json').write_text(json.dumps({'bytes': len(data)}))
if mode == 'blocked':
    time.sleep(120)
if mode == 'limited':
    sys.stdout.write('\n120\n429')
    sys.exit(22)
if mode == 'login':
    sys.stdout.write('\n\n401')
    sys.exit(22)
if mode == 'overflow':
    sys.stdout.write('x' * 1100000)
    sys.stdout.flush()
    time.sleep(120)
sys.stdout.write('{"five_hour":{"utilization":25},"seven_day":{"utilization":50}}\n\n200')
sys.stdout.flush()
# The descendant deliberately retains stdout: natural leader exit must retire it.
