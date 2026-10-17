"""Test-only stdio relay: verify real MCP output when native UI events omit it."""
import pathlib,sys,time,subprocess,json,concurrent.futures
ROOT=pathlib.Path(__file__).resolve().parent
RUN=ROOT/time.strftime('%Y%m%d-%H%M%S');RUN.mkdir();print(RUN,flush=True)
BIN=ROOT.parents[2]/'target/debug/examples/live_history_mcp'
def one(name):
 d=RUN/name;d.mkdir();wrapper=d/'mcp-relay.py'
 wrapper.write_text('#!'+sys.executable+'\n'+'''import sys,subprocess,threading,json
p=subprocess.Popen('''+repr([str(BIN)])+'''+sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
def forward():
 try:
  for line in sys.stdin.buffer:p.stdin.write(line);p.stdin.flush()
 finally:p.stdin.close()
threading.Thread(target=forward,daemon=True).start()
for line in p.stdout:
 with open('''+repr(str(d/'mcp-results.jsonl'))+''','ab') as log:log.write(line)
 sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
p.wait()
''');wrapper.chmod(0o700)
 with (d/'run.log').open('w') as log:r=subprocess.run([str(BIN),name,str(d),str(wrapper)],stdout=log,stderr=log)
 print(json.dumps({'provider':name,'exit':r.returncode,'summary':json.loads((d/'summary.json').read_text()) if (d/'summary.json').exists() else None}),flush=True)
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:list(pool.map(one,sys.argv[1:]))
