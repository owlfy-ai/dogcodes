#!/usr/bin/env python3
import asyncio,importlib.util,json,os,pathlib,sys,time,uuid
ROOT=pathlib.Path(__file__).resolve().parent
RESUME_ROOT=os.environ.get('HISTORY_PROBE_RESUME_ROOT')
spec=importlib.util.spec_from_file_location('base',ROOT.parent/'subagent-observability/probe.py');base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)
base.RUN=ROOT/'runs'/time.strftime('%Y%m%d-%H%M%S')
PROMPT='Use the supplied searchHistory tool once with text "test fact" and report its fact verbatim. You may use tool discovery to locate it. Do not use shell, file tools, web, or unrelated tools. If the tool is unavailable, reply HISTORY_TOOL_UNAVAILABLE.'
class Probe(base.Probe):
 def __init__(self,name):
  super().__init__(name);self.old=json.loads((pathlib.Path(RESUME_ROOT)/name/'summary.json').read_text()) if RESUME_ROOT else None;self.fact='HISTORY_'+uuid.uuid4().hex;self.cfg=self.dir/'server-fixture.json';self.calls=self.dir/'calls.jsonl';self.cfg.write_text(json.dumps({'fact':self.fact,'log':str(self.calls)}));self.mcp={'name':'dogcodes_history_probe','command':sys.executable,'args':['-S',str(ROOT/'server.py'),str(self.cfg)],'env':[]}
 async def server(self,x):
  m=x.get('method','');p=x.get('params',{})
  if m=='session/requestRuntimePreferences':return await self.reply(x,{'nativeSearchEnhancementsEnabled':False,'memoryEnabled':False,'askUserQuestionAutoResolutionEnabled':False,'modelContextBudgetStrategy':'preflight-v1'})
  if m in ['session/request_permission','interaction/requestPermission']:
   # Only the named, read-only MCP tool is authorized; no arbitrary native tool.
   allowed='searchHistory' in json.dumps(p) or 'dogcodes_history_probe' in json.dumps(p)
   o=next((o for o in p.get('options',[]) if o.get('kind')==('allow_once' if allowed else 'reject_once')),None)
   if self.name=='zcode':return await self.reply(x,o['response'] if o and 'response' in o else {'decision':'deny'})
   return await self.reply(x,{'outcome':{'outcome':'selected','optionId':o['optionId']}} if o else {'outcome':{'outcome':'cancelled'}})
  if m.startswith('cursor/') or m.startswith('_cursor/'):return await self.reply(x,{})
  if m in ['item/commandExecution/requestApproval','item/fileChange/requestApproval']:return await self.reply(x,{'decision':'decline'})
  return await self.reply(x,error={'code':-32601,'message':'Only fixed read-only history tool is permitted'})
 async def reader(self):
  while line:=await self.proc.stdout.readline():
   try:x=json.loads(line)
   except Exception:continue
   self.record('in',x)
   if x.get('type')=='control_request':
    r=x.get('request',{});tool=r.get('tool_name','');allow='searchHistory' in tool and 'dogcodes_history_probe' in tool
    await self.send({'type':'control_response','response':{'subtype':'success','request_id':x['request_id'],'response':{'behavior':'allow','updatedInput':r.get('input',{})} if allow else {'behavior':'deny','message':'Only fixed history tool authorized'}}})
   elif x.get('method') and 'id' in x:await self.server(x)
   elif 'id' in x and x['id'] in self.pending:
    f=self.pending.pop(x['id'])
    if not f.done():
     if 'error' in x:f.set_exception(RuntimeError(json.dumps(base.clean(x['error']))))
     else:f.set_result(x.get('result'))
   if self.name=='claude' and x.get('type')=='result':self.done.set()
   if self.name=='codex' and x.get('method')=='turn/completed':self.done.set()
   if self.name=='zcode' and (x.get('method')=='turn.completed' or x.get('params',{}).get('type')=='turn.completed'):self.done.set()
  self.done.set()
 async def run(self):
  self.start=time.monotonic();env={k:v for k,v in os.environ.items() if k in ['PATH','HOME','TMPDIR','LANG','TZ','TERM'] or k.lower() in ['http_proxy','https_proxy','all_proxy','no_proxy']}
  commands=dict(base.COMMANDS);commands['codebuddy']=[base.agent_executable('codebuddy','codebuddy'),'--acp']
  cmd=commands[self.name]
  if self.name=='claude':
   if self.old:cmd=cmd[:-2]+['--resume',self.old['sessionId']]
   self.session=cmd[-1]
  if self.name=='claude':
   config=self.dir/'mcp.json';config.write_text(json.dumps({'mcpServers':{'dogcodes_history_probe':{k:v for k,v in self.mcp.items() if k in ['command','args']}}}));cmd=cmd+['--mcp-config',str(config)]
  if self.name=='zcode':env['ELECTRON_RUN_AS_NODE']='1'
  self.proc=await asyncio.create_subprocess_exec(*cmd,cwd=self.dir,env=env,stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE,limit=8*1024*1024)
  reader=asyncio.create_task(self.reader());drain=asyncio.create_task(self.drain())
  try:
   if self.name in ['cursor','kimi','grok','codebuddy']:
    init=await self.request('initialize',{'protocolVersion':1,'clientCapabilities':{'fs':{'readTextFile':False,'writeTextFile':False},'terminal':False},'clientInfo':{'name':'dogcodes-history-probe','version':'1'}})
    self.summary['capabilities']=init.get('agentCapabilities',{})
    r=await self.request('session/load' if self.old else 'session/new',{'cwd':str(pathlib.Path(RESUME_ROOT)/self.name) if self.old else str(self.dir),'mcpServers':[self.mcp],**({'sessionId':self.old['sessionId']} if self.old else {})},70);self.session=self.old['sessionId'] if self.old else r['sessionId']
    self.summary['terminal']=await self.request('session/prompt',{'sessionId':self.session,'prompt':[{'type':'text','text':PROMPT}]},150)
   elif self.name=='claude':
    await self.send({'type':'control_request','request_id':'init','request':{'subtype':'initialize','hooks':{}}})
    await self.send({'type':'user','message':{'role':'user','content':PROMPT},'parent_tool_use_id':None});await asyncio.wait_for(self.done.wait(),150)
   elif self.name=='codex':
    await self.request('initialize',{'clientInfo':{'name':'dogcodes-history-probe','version':'1'},'capabilities':{'experimentalApi':True}});await self.send({'method':'initialized','params':{}})
    r=await self.request('thread/resume' if self.old else 'thread/start',{**({'threadId':self.old['sessionId']} if self.old else {}),'cwd':str(pathlib.Path(RESUME_ROOT)/self.name) if self.old else str(self.dir),'approvalPolicy':'on-request','sandbox':'read-only','config':{'mcp_servers':{'dogcodes_history_probe':{k:v for k,v in self.mcp.items() if k in ['command','args']}}}});self.session=r['thread']['id']
    await self.request('turn/start',{'threadId':self.session,'input':[{'type':'text','text':PROMPT}]});await asyncio.wait_for(self.done.wait(),150)
   elif self.name=='zcode':
    r=await self.request('session/resume' if self.old else 'session/create',{**({'sessionId':self.old['sessionId']} if self.old else {}),'workspace':{'workspacePath':str(pathlib.Path(RESUME_ROOT)/self.name) if self.old else str(self.dir),'workspaceKey':str(pathlib.Path(RESUME_ROOT)/self.name) if self.old else str(self.dir)},**({} if self.old else {'titleGenerationEnabled':False}),'mcpServers':[self.mcp]});self.session=r.get('sessionId') or r.get('snapshot',r).get('session',{}).get('sessionId')
    if self.old:
     model=r['settings']['model']['current'];workspace=r['session']['workspace']
     state=await self.request('workspace/readState',{'workspace':workspace,'preferWorkspaceDefaults':True})
     definition=next(m for p in state['modelCatalog']['providers'] if p['providerId']==model['providerId'] for m in p['models'] if m['modelId']==model['modelId'])
     src=ROOT.parent.parent/'crates/zcode-driver/src'
     js=(src/'quota-reader.cjs').read_text()+'\n'+(src/'runtime-reader.cjs').read_text()+'\nruntimeMain();'
     helper=await asyncio.create_subprocess_exec(cmd[0],'-e',js,env=env,stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.DEVNULL)
     raw,_=await asyncio.wait_for(helper.communicate(json.dumps({'model':model,'definition':definition,'thoughtLevel':r['settings']['thoughtLevel'].get('current')}).encode()),15)
     if helper.returncode:raise RuntimeError('Native model runtime reader failed')
     await self.request('session/updateRuntimeModelConfig',{'sessionId':self.session,'runtimeModel':json.loads(raw)})
    await self.request('session/subscribe',{'sessionId':self.session,'deliveryKind':'desktop-continuous'});await self.request('session/send',{'sessionId':self.session,'content':PROMPT});await asyncio.wait_for(self.done.wait(),150)
   self.summary['status']='finished'
  except Exception as e:self.summary.update(status='error',error=type(e).__name__+': '+str(e))
  finally:
   self.summary.update(sessionId=self.session,toolCalls=len(self.calls.read_text().splitlines()) if self.calls.exists() else 0,factReturned=self.fact in json.dumps(self.frames) or self.fact in ''.join(x['frame'].get('params',{}).get('update',{}).get('content',{}).get('text','') for x in self.frames if x['frame'].get('params',{}).get('update',{}).get('sessionUpdate')=='agent_message_chunk'),elapsed=round(time.monotonic()-self.start,1),userPrompt=PROMPT)
   (self.dir/'summary.json').write_text(json.dumps(self.summary,ensure_ascii=False,indent=2));print(json.dumps(self.summary,ensure_ascii=False),flush=True)
   self.proc.stdin.close()
   try:await asyncio.wait_for(self.proc.wait(),8)
   except asyncio.TimeoutError:
    self.proc.terminate()
    try:await asyncio.wait_for(self.proc.wait(),5)
    except asyncio.TimeoutError:self.proc.kill();await self.proc.wait()
   reader.cancel();drain.cancel()
async def main():
 print(base.RUN,flush=True);sem=asyncio.Semaphore(3)
 async def one(n):
  async with sem:await Probe(n).run()
 await asyncio.gather(*(one(n) for n in sys.argv[1:]))
if __name__=='__main__':asyncio.run(main())
