#!/usr/bin/env python3
"""Private metadata helper; no installed Agent or credential is accessed."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
root = Path(os.path.abspath(__file__)).parent
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(90)'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
marker = root / 'started.tmp'
marker.write_text(json.dumps([os.getpid(), child.pid]))
marker.replace(root / 'started.json')
while not (root / 'release').exists():
    time.sleep(0.01)
if 'models' in sys.argv:
    print('fixture-model\tFixture Model', flush=True)
else:
    print('{}', flush=True)
