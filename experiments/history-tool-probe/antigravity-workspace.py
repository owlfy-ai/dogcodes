#!/usr/bin/env python3
"""Isolated native Antigravity workspace MCP probe; no global config writes."""
import json, os, pathlib, subprocess, sys, tempfile, time, uuid, importlib.util, shlex
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('probe',ROOT/'probe.py');mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
RUN=ROOT/'runs'/time.strftime('%Y%m%d-%H%M%S')/'antigravity-workspace';RUN.mkdir(parents=True)
workspace=pathlib.Path(tempfile.mkdtemp(prefix='dogcodes-agy-mcp-')).resolve();(workspace/'.agents').mkdir()
private='--private' in sys.argv
gated='--gated' in sys.argv
config_root=pathlib.Path(tempfile.mkdtemp(prefix='dogcodes-agy-mcp-config-')).resolve() if private else workspace
(config_root/'.agents').mkdir(exist_ok=True)
if gated:
 hook=RUN/'approve.py'
 hook.write_text('import json,sys\nx=json.load(sys.stdin);t=x.get("toolCall",{});a=t.get("args",{})\nif isinstance(a,str):\n try:a=json.loads(a)\n except ValueError:a={}\nallow=t.get("name")=="call_mcp_tool" and a.get("ServerName")=="dogcodes_history_probe" and a.get("ToolName")=="searchHistory"\nwith open(sys.argv[1],"a") as f:f.write(json.dumps({"tool":t,"allowed":allow})+"\\n")\nprint(json.dumps({"decision":"allow" if allow else "deny","reason":"Only fixed read-only history fixture permitted"}))\n')
 (config_root/'.agents/hooks.json').write_text(json.dumps({'dogcodes-history-probe':{'PreToolUse':[{'matcher':'.*','hooks':[{'type':'command','command':shlex.join([sys.executable,'-S',str(hook),str(RUN/'approvals.jsonl')]),'timeout':20}]}]}}))
env={k:v for k,v in os.environ.items() if k in ['PATH','HOME','TMPDIR','LANG','TZ','TERM'] or k.lower() in ['http_proxy','https_proxy','all_proxy','no_proxy']}
session=None
print(str(RUN),flush=True)
for phase in ['fresh','resume']:
 d=RUN/phase;d.mkdir();fact='HISTORY_'+uuid.uuid4().hex;calls=d/'calls.jsonl';fixture=d/'fixture.json';fixture.write_text(json.dumps({'fact':fact,'log':str(calls)}))
 config={'mcpServers':{'dogcodes_history_probe':{'command':sys.executable,'args':['-S',str(ROOT/'server.py'),str(fixture)]}}}
 (config_root/'.agents/mcp_config.json').write_text(json.dumps(config));(d/'config.json').write_text(json.dumps(config,indent=2))
 cmd=[str(pathlib.Path.home()/'.local/bin/agy'),'--input-format','stream-json','--output-format','stream-json','--print-timeout','90s','--add-dir',str(workspace)]+(['--conversation',session] if session else ['--new-project'])
 cmd+=(['--add-dir',str(config_root)] if private else [])+(['--dangerously-skip-permissions'] if gated else [])
 prompt=mod.PROMPT
 start=time.monotonic()
 try:
  p=subprocess.run(cmd,cwd=workspace,env=env,input=json.dumps({'event':'user','message':{'content':[{'type':'text','text':prompt}]}})+'\n',capture_output=True,text=True,timeout=110)
  raw=p.stdout;code=p.returncode
 except subprocess.TimeoutExpired as e:
  raw=e.stdout or b'';raw=raw.decode() if isinstance(raw,bytes) else raw;code='timeout'
 frames=[]
 for line in raw.splitlines():
  try:frames.append(mod.base.clean(json.loads(line)))
  except ValueError:pass
 (d/'wire.json').write_text(json.dumps(frames,ensure_ascii=False,indent=2))
 ids=[x.get('conversation_id') or x.get('result',{}).get('conversation_id') for x in frames];new_session=next((x for x in ids if x),None)
 result={'phase':phase,'workspace':str(workspace),'configRoot':str(config_root),'gated':gated,'sessionId':new_session,'sameSession':phase=='fresh' or new_session==session,'exitCode':code,'toolCalls':len(calls.read_text().splitlines()) if calls.exists() else 0,'factReturned':any(f.get('result',{}).get('status')=='SUCCESS' and fact in f.get('result',{}).get('response','') for f in frames),'elapsed':round(time.monotonic()-start,1),'userPrompt':prompt}
 (d/'summary.json').write_text(json.dumps(result,indent=2));print(json.dumps(result),flush=True)
 session=new_session
 if not session or not result['factReturned']:break

# Remove only configurations created by this probe; evidence remains under runs/.
for name in ['mcp_config.json','hooks.json']:
 (config_root/'.agents'/name).unlink(missing_ok=True)
