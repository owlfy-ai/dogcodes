#!/opt/homebrew/opt/python@3.14/bin/python3.14
from pathlib import Path
ROOT = Path(__file__).resolve().parents[5]
import sys,subprocess,threading,json
p=subprocess.Popen([str(ROOT / 'target/debug/examples/live_history_mcp')]+sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
def forward():
 try:
  for line in sys.stdin.buffer:p.stdin.write(line);p.stdin.flush()
 finally:p.stdin.close()
threading.Thread(target=forward,daemon=True).start()
for line in p.stdout:
 with open(str(Path(__file__).with_name('mcp-results.jsonl')),'ab') as log:log.write(line)
 sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
p.wait()
