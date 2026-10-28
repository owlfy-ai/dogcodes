#!/usr/bin/env python3
"""Private terminal fixture: local /usage only, no accounts or model requests."""
import json
import os
import signal
import subprocess
import sys
import time
import tty

assert sys.argv[1:] == ["--disable-auto-update", "--mode", "ask", "--trust"]
assert all(os.isatty(fd) for fd in (0, 1, 2))
assert os.tcgetpgrp(0) == os.getpgrp() == os.getpid() == os.getsid(0)
assert os.get_terminal_size(0).columns == 120
assert os.get_terminal_size(0).lines == 40
signal.signal(signal.SIGHUP, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
tty.setraw(0)
child = subprocess.Popen([
    sys.executable, "-c",
    "import signal,time; signal.signal(signal.SIGHUP,signal.SIG_IGN); "
    "signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(120)", os.getcwd(),
])
with open("pids.pending", "w") as output:
    json.dump([os.getpid(), child.pid], output)
os.replace("pids.pending", "pids.json")
mode = open("mode").read().strip()
if mode == "blocked":
    time.sleep(120)
elif mode == "invalid":
    print("unsupported private layout", flush=True)
    sys.exit(1)
elif mode != "normal":
    raise RuntimeError("unknown private fixture mode")


def receive(wanted):
    received = b""
    while len(received) < len(wanted):
        received += os.read(0, len(wanted) - len(received))
    assert received == wanted, (received, wanted)
    with open("commands.jsonl", "a") as output:
        output.write(json.dumps(received.decode()) + "\n")


print("Plan, search, build anything", flush=True)
receive(b"/usage")
print("Show plan and on-demand usage", flush=True)
receive(b"\r")
print("Usage • Pro      Resets Oct 4\nIncluded 65% used\nAuto 20% used\n"
      "API 100% used\nOn-demand usage is off\nEsc to close", flush=True)
receive(b"\x1b")
print("Plan, search, build anything", flush=True)
receive(b"/quit")
print("/quit Exit", flush=True)
receive(b"\r")
# Deliberately keep the descendant's slave fds open. The managed wait must
# observe this leader's exit and close the original group to deliver EOF.
