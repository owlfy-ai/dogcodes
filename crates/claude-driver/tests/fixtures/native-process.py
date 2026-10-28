#!/usr/bin/env python3
import os,sys,json,signal,time,subprocess
# The driver supplies the private test workspace as cwd. Only data files
# vary per run; the executable is never rewritten while other tests fork.
# Static readiness probes have no workspace argument; a private symlink names it.
if os.path.basename(sys.argv[0]) == 'readiness-fixture':
 os.chdir(os.path.dirname(os.path.abspath(sys.argv[0])))
with open('fixture-mode', encoding='utf-8') as f:mode=f.read().strip()
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'],**({'stdout':subprocess.DEVNULL,'stderr':subprocess.DEVNULL} if mode=='eof' else {}))
marker='pids.'+str(os.getpid())+'.tmp'
with open(marker,'w') as f:json.dump([os.getpid(),child.pid],f)
os.replace(marker,'pids.json')
if mode=='writer':
 while True:time.sleep(60)
if mode=='leader':sys.exit(0)
if mode=='eof':
 os.close(1);os.close(2)
 while True:time.sleep(60)
if mode.startswith('catalog-'):
 previous=[]
 if os.path.exists('previous-pids.json'):
  with open('previous-pids.json') as f:previous=json.load(f)
 def alive(pid):
  try:os.kill(pid,0);return True
  except ProcessLookupError:return False
 with open('catalog-start.json','w') as f:json.dump({'previousAlive':[pid for pid in previous if alive(pid)]},f)
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 if mode.startswith('probe-'):
  assert r['type']=='control_request' and r['request']['subtype']=='initialize'
  assert r['request_id']=='dogcodes-probe'
  if mode=='probe-stderr':
   sys.stderr.write('private diagnostic ' * (128 * 1024));sys.stderr.flush()
   print(json.dumps({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'success','response':{}}}),flush=True)
  if mode=='probe-wrong-id':
   print(json.dumps({'type':'control_response','response':{'request_id':'unrelated-control','subtype':'success','response':{}}}),flush=True)
  if mode=='probe-oversize':
   sys.stdout.write('x' * (2 * 1024 * 1024 + 128));sys.stdout.flush()
 if mode.startswith('catalog-') and r.get('request',{}).get('subtype')=='initialize':
  if mode=='catalog-blocked':
   while True:time.sleep(60)
  if mode=='catalog-stderr':
   sys.stderr.write('diagnostic ' * (128 * 1024));sys.stderr.flush()
  models=[] if mode=='catalog-empty' else [{'value':'fixture-model','displayName':'Fixture','supportsAutoMode':True}]
  print(json.dumps({'type':'control_response','response':{'request_id':r['request_id'],'response':{'models':models}}}),flush=True)
 if mode in ['configuration','history'] and r.get('type')=='control_request':time.sleep(60)
if mode.startswith('probe-'):
 with open('probe-eof','w') as f:f.write('stdin closed')
 if mode=='probe-cancel':
  while not os.path.exists('allow-exit'):time.sleep(0.01)
 else:time.sleep(0.7)
 with open('probe-finished','w') as f:f.write('private finish, no credentials')
 sys.exit(0)
while True:time.sleep(60)
