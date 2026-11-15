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
if mode.startswith('catalog-') or mode.startswith('quota-'):
 previous=[]
 if os.path.exists('previous-pids.json'):
  with open('previous-pids.json') as f:previous=json.load(f)
 def alive(pid):
  try:os.kill(pid,0);return True
  except ProcessLookupError:return False
 with open('quota-start.json' if mode.startswith('quota-') else 'catalog-start.json','w') as f:json.dump({'previousAlive':[pid for pid in previous if alive(pid)]},f)
if mode.startswith('quota-'):
 with open('quota-args.json','w') as f:json.dump(sys.argv[1:],f)
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
 if mode.startswith('quota-'):
  subtype=r.get('request',{}).get('subtype')
  assert r['type']=='control_request' and subtype in ['initialize','get_usage']
  if subtype=='initialize':
   print(json.dumps({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'success','response':{}}}),flush=True)
  else:
   assert r['request']['skip_behaviors'] is True
   if mode=='quota-blocked':
    while True:time.sleep(60)
   if mode=='quota-expired':
    print(json.dumps({'type':'assistant','error':'authentication_failed','message':{'content':[{'type':'text','text':'Failed to authenticate: this login reached its time limit and cannot be renewed'}]}}),flush=True)
   else:
    value={'subscription_type':'pro','rate_limits_available':True,'rate_limits':None if mode=='quota-empty' else {'five_hour':{'utilization':40},'seven_day':{'utilization':20}}}
    print(json.dumps({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'error' if mode=='quota-rejected' else 'success','response':value}}),flush=True)
 if mode=='auth-session':
  if r.get('type')=='control_request':
   value={'subscription_type':'pro','rate_limits_available':True,'rate_limits':{'five_hour':{'utilization':10}}} if r.get('request',{}).get('subtype')=='get_usage' else {}
   print(json.dumps({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'success','response':value}}),flush=True)
  elif r.get('type')=='user':
   if '--resume' not in sys.argv:
    print(json.dumps({'type':'assistant','error':'authentication_failed','message':{'model':'<synthetic>','content':[{'type':'text','text':'Failed to authenticate: this login reached its time limit and cannot be renewed'}]}}),flush=True)
   else:
    print(json.dumps({'type':'assistant','message':{'model':'fixture-model','usage':{'input_tokens':1,'output_tokens':1},'content':[{'type':'text','text':'recovered'}]}}),flush=True)
   print(json.dumps({'type':'result','subtype':'success','result':'finished'}),flush=True)
 if mode in ['configuration','history'] and r.get('type')=='control_request':time.sleep(60)
if mode.startswith('quota-'):
 child.kill();child.wait();sys.exit(0)
if mode.startswith('probe-'):
 with open('probe-eof','w') as f:f.write('stdin closed')
 if mode=='probe-cancel':
  while not os.path.exists('allow-exit'):time.sleep(0.01)
 else:time.sleep(0.7)
 with open('probe-finished','w') as f:f.write('private finish, no credentials')
 sys.exit(0)
while True:time.sleep(60)
