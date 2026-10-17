"""Real commit-summary RPC acceptance (uses model quota, never commits/pushes).
Run against an isolated Core service:
  python3 experiments/agent-integration/commit_summary.py DATA_DIR all OUTPUT_DIR
Pass comma-separated provider IDs instead of all to retry only failed agents.
Service credentials are read by Client and never printed.
"""
import sys,pathlib,json,time,concurrent.futures
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parents[1]/'daemon-acceptance/2026-09-25-deletion'))
from client import Client
folder=pathlib.Path(sys.argv[1]); c=Client(folder)
if len(sys.argv)<3:
 print(json.dumps(c.call('registered_executors'),ensure_ascii=False));sys.exit()
changes='''diff --git a/src/retry.ts b/src/retry.ts
--- a/src/retry.ts
+++ b/src/retry.ts
@@ -1 +1 @@
-export const maxRetries = 1;
+export const maxRetries = 3;
diff --git a/src/retry.test.ts b/src/retry.test.ts
--- a/src/retry.test.ts
+++ b/src/retry.test.ts
@@ -1 +1 @@
-expect(maxRetries).toBe(1);
+expect(maxRetries).toBe(3);
'''
executors=[e for e in c.call('registered_executors') if e['providerId']!='fake' and (sys.argv[2]=='all' or e['providerId'] in sys.argv[2].split(','))]
out=pathlib.Path(sys.argv[3]);out.mkdir(exist_ok=True,parents=True)
def run(e):
 start=time.monotonic()
 try:
  with c.connect('summarize_commit',[e,changes]) as sock:
   sock.settimeout(210); reply=c.read(sock)
  result={'provider':e['providerId'],'executor':e,'seconds':round(time.monotonic()-start,2),**reply}
 except Exception as ex:result={'provider':e['providerId'],'seconds':round(time.monotonic()-start,2),'error':str(ex)}
 (out/(e['providerId']+'.json')).write_text(json.dumps(result,ensure_ascii=False,indent=2));print(json.dumps(result,ensure_ascii=False),flush=True)
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:list(pool.map(run,executors))
