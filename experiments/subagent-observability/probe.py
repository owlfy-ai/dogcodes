#!/usr/bin/env python3
"""Operator-authorized native-wire delegation smoke test; only its own sessions."""
import asyncio, json, os, pathlib, sys, time, uuid, re
ROOT=pathlib.Path(__file__).resolve().parent
RUN=ROOT/'runs'/time.strftime('%Y%m%d-%H%M%S')
PROMPT='''Protocol observability test. You MUST use your built-in subagent/delegation tool to spawn exactly ONE foreground child agent (general-purpose or read-only explorer). Do NOT read the file yourself. Instruct that child to read the absolute file {file} using a file-read tool (or a read-only terminal command), then report its marker and compute 19+23. Wait for that child and return its result. No more children, no web, no writes, no unrelated files. If delegation is unavailable, report DELEGATION_UNAVAILABLE and stop. This is a tiny test, finish promptly.'''
def agent_executable(provider, default):
 # A single executable path, never a shell command. PATH resolves bare names.
 return os.path.expanduser(os.environ.get('DOGCODES_PROBE_'+provider.upper()+'_EXECUTABLE') or default)
COMMANDS={
'codex':[agent_executable('codex','codex'),'app-server'],
'claude':[agent_executable('claude','claude'),'-p','--input-format','stream-json','--output-format','stream-json','--verbose','--include-partial-messages','--permission-mode','manual','--permission-prompt-tool','stdio','--session-id',str(uuid.uuid4())],
'zcode':[agent_executable('zcode','/Applications/ZCode.app/Contents/MacOS/ZCode'),os.path.expanduser(os.environ.get('DOGCODES_PROBE_ZCODE_SERVER') or '/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs'),'app-server'],
'cursor':[agent_executable('cursor','cursor-agent'),'acp'],
'kimi':[agent_executable('kimi','kimi'),'acp'],
'grok':[agent_executable('grok','grok'),'--no-auto-update','--permission-mode','default','agent','stdio'],
'antigravity':[agent_executable('antigravity','agy'),'--input-format','stream-json','--output-format','stream-json']}
def clean(x):
 if isinstance(x,dict):return {k:('[REDACTED]' if re.search(r'api.?key|authorization|credential|cookie|access.?token|refresh.?token|secret|headers|environment|signature',k,re.I) else clean(v)) for k,v in x.items()}
 if isinstance(x,list):return [clean(v) for v in x]
 if isinstance(x,str):return re.sub(r'(?i)Bearer\s+\S+','Bearer [REDACTED]',x)[:60000]
 return x
class Probe:
 def __init__(self,name):
  self.name=name;self.dir=RUN/name;self.dir.mkdir(parents=True);self.fixture=self.dir/'fixture.txt';self.fixture.write_text('OBSERVABILITY_MARKER_'+uuid.uuid4().hex+'\n');self.seq=0;self.pending={};self.done=asyncio.Event();self.frames=[];self.childids=set();self.session=None;self.summary={'agent':name,'status':'starting'}
 def record(self,direction,x):
  row={'t':round(time.monotonic()-self.start,3),'direction':direction,'frame':clean(x)}
  self.frames.append(row)
  with (self.dir/'wire.jsonl').open('a') as f:f.write(json.dumps(row,ensure_ascii=False)+'\n')
 async def send(self,x):
  self.record('out',x);self.proc.stdin.write((json.dumps(x)+'\n').encode());await self.proc.stdin.drain()
 async def request(self,method,params,timeout=35):
  self.seq+=1;i=self.seq;f=asyncio.get_running_loop().create_future();self.pending[i]=f
  await self.send(dict({'id':i,'method':method,'params':params},**({} if self.name=='zcode' else {'jsonrpc':'2.0'})))
  return await asyncio.wait_for(f,timeout)
 async def reply(self,x,result=None,error=None):
  out={'id':x['id']};out.update({'error':error} if error else {'result':result})
  if self.name!='zcode':out['jsonrpc']='2.0'
  await self.send(out)
 async def server(self,x):
  m=x.get('method','');p=x.get('params',{})
  if m=='session/requestRuntimePreferences':return await self.reply(x,{'nativeSearchEnhancementsEnabled':False,'memoryEnabled':False,'askUserQuestionAutoResolutionEnabled':False,'modelContextBudgetStrategy':'preflight-v1'})
  if m in ['session/request_permission','interaction/requestPermission']:
   opts=p.get('options',[]);o=next((o for o in opts if o.get('kind')=='allow_once'),None)
   # Scope is this isolated read-only delegation task. Never grant persistent permission.
   await self.reply(x,{'optionId':o['optionId']} if self.name=='zcode' and o else {'outcome':{'outcome':'selected','optionId':o['optionId']}} if o else {'outcome':{'outcome':'cancelled'}});return
  if m=='fs/read_text_file':
   path=pathlib.Path(p.get('path','')).resolve()
   if path==self.fixture.resolve():return await self.reply(x,{'content':path.read_text()})
   return await self.reply(x,error={'code':-32000,'message':'Only test fixture permitted'})
  if m.startswith('cursor/') or m.startswith('_cursor/') :return await self.reply(x,{})
  if m in ['item/commandExecution/requestApproval','item/fileChange/requestApproval']:return await self.reply(x,{'decision':'decline'})
  return await self.reply(x,error={'code':-32601,'message':'Probe does not implement this client method'})
 async def reader(self):
  while line:=await self.proc.stdout.readline():
   try:x=json.loads(line)
   except Exception:continue
   self.record('in',x)
   if x.get('type')=='control_request':
    r=x.get('request',{});inp=r.get('input',{});tool=r.get('tool_name','')
    allowed=tool in ['Agent','Task','Read','Glob','Grep','TaskOutput','TaskList'] or (tool=='Bash' and (str(self.fixture) in inp.get('command','')) and not any(v in inp.get('command','') for v in ['>',';','&&','|']))
    response={'behavior':'allow','updatedInput':inp} if allowed else {'behavior':'deny','message':'Only read-only fixture delegation authorized'}
    await self.send({'type':'control_response','response':{'subtype':'success','request_id':x['request_id'],'response':response}})
   elif x.get('method') and 'id' in x:await self.server(x)
   elif 'id' in x and x['id'] in self.pending:
    f=self.pending.pop(x['id'])
    if not f.done():
     if 'error' in x:f.set_exception(RuntimeError(json.dumps(clean(x['error']))))
     else:f.set_result(x.get('result'))
   m=x.get('method','');p=x.get('params',{});payload=p.get('payload',p)
   if self.name=='codex':
    item=p.get('item',{})
    if item.get('type')=='collabAgentToolCall':self.childids.update(item.get('receiverThreadIds',[]))
    if item.get('type')=='subAgentActivity' and item.get('agentThreadId'):self.childids.add(item['agentThreadId'])
    if m=='turn/completed' and p.get('threadId')==self.session:self.done.set()
   if self.name=='zcode' and (m=='turn.completed' or (m=='session/event' and p.get('type')=='turn.completed')):self.done.set()
   if self.name=='claude' and x.get('type')=='result':self.done.set()
   if self.name=='antigravity' and x.get('event')=='result':self.done.set()
  self.done.set()
 async def drain(self):
  while await self.proc.stderr.read(65536):pass
 async def run(self):
  self.start=time.monotonic();env={k:v for k,v in os.environ.items() if k in ['PATH','HOME','TMPDIR','LANG','TZ','TERM'] or k.lower() in ['http_proxy','https_proxy','all_proxy','no_proxy']}
  if self.name=='zcode':env['ELECTRON_RUN_AS_NODE']='1'
  self.proc=await asyncio.create_subprocess_exec(*COMMANDS[self.name],cwd=self.dir,env=env,stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE,limit=8*1024*1024)
  reader=asyncio.create_task(self.reader());drain=asyncio.create_task(self.drain());prompt=PROMPT.format(file=self.fixture)
  try:
   if self.name in ['cursor','kimi','grok']:
    await self.request('initialize',{'protocolVersion':1,'clientCapabilities':{'fs':{'readTextFile':True,'writeTextFile':False},'terminal':False},'clientInfo':{'name':'dogcodes-observability','version':'0.1'}})
    r=await self.request('session/new',{'cwd':str(self.dir),'mcpServers':[]});self.session=r['sessionId'];self.summary['sessionId']=self.session
    r=await self.request('session/prompt',{'sessionId':self.session,'prompt':[{'type':'text','text':prompt}]},240);self.summary['terminal']=r
   elif self.name=='codex':
    await self.request('initialize',{'clientInfo':{'name':'dogcodes-observability','version':'0.1'},'capabilities':{'experimentalApi':True}});await self.send({'method':'initialized','params':{}})
    r=await self.request('thread/start',{'cwd':str(self.dir),'approvalPolicy':'never','sandbox':'read-only'});self.session=r['thread']['id'];self.summary['sessionId']=self.session
    await self.request('turn/start',{'threadId':self.session,'input':[{'type':'text','text':prompt}]});await asyncio.wait_for(self.done.wait(),240)
    for child in list(self.childids):
     try:await self.request('thread/read',{'threadId':child,'includeTurns':True})
     except Exception as e:self.summary.setdefault('childReadErrors',[]).append(str(e))
   elif self.name=='zcode':
    r=await self.request('session/create',{'workspace':{'workspacePath':str(self.dir),'workspaceKey':str(self.dir)},'titleGenerationEnabled':False,'mcpServers':[]});self.session=r.get('sessionId') or r.get('snapshot',r).get('session',{}).get('sessionId');self.summary['sessionId']=self.session
    await self.request('session/subscribe',{'sessionId':self.session,'deliveryKind':'desktop-continuous'});await self.request('session/send',{'sessionId':self.session,'content':prompt});await asyncio.wait_for(self.done.wait(),240)
    await self.request('session/subagents',{'sessionId':self.session})
   else:
    if self.name=='claude':await self.send({'type':'control_request','request_id':'init','request':{'subtype':'initialize','hooks':{}}})
    await self.send({'type':'user','message':{'role':'user','content':[{'type':'text','text':prompt}]},'parent_tool_use_id':None} if self.name=='claude' else {'event':'user','message':{'content':[{'type':'text','text':prompt}]}});await asyncio.wait_for(self.done.wait(),240)
   self.summary['status']='finished'
  except Exception as e:self.summary.update(status='error',error=type(e).__name__+': '+str(e))
  finally:
   self.summary['elapsed']=round(time.monotonic()-self.start,1);self.summary['frames']=len(self.frames)
   (self.dir/'summary.json').write_text(json.dumps(self.summary,ensure_ascii=False,indent=2))
   print(json.dumps(self.summary,ensure_ascii=False),flush=True)
   if self.proc.returncode is None:
    if self.summary['status']=='error' and self.session:
     try:
      if self.name in ['cursor','kimi','grok']:await self.send({'jsonrpc':'2.0','method':'session/cancel','params':{'sessionId':self.session}})
      elif self.name=='zcode':await self.request('session/stop',{'sessionId':self.session},10)
     except Exception:pass
    self.proc.stdin.close()
    try:await asyncio.wait_for(self.proc.wait(),20)
    except asyncio.TimeoutError:
     self.proc.terminate()
     try:await asyncio.wait_for(self.proc.wait(),10)
     except asyncio.TimeoutError:self.proc.kill();await self.proc.wait()
   reader.cancel();drain.cancel()
async def main():
 RUN.mkdir(parents=True);print(str(RUN),flush=True)
 sem=asyncio.Semaphore(3)
 async def one(name):
  async with sem:await Probe(name).run()
 await asyncio.gather(*(one(x) for x in sys.argv[1:]))
if __name__=='__main__':asyncio.run(main())
