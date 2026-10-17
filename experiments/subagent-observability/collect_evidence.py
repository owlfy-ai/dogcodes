"""Extract selected probe-owned frames; no access to unrelated sessions."""
import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
for folder in sorted(root.iterdir()):
 if not folder.is_dir() or not (folder/'wire.jsonl').exists(): continue
 rows=[json.loads(x) for x in (folder/'wire.jsonl').read_text().splitlines()]
 selected=[]; chunks=[]
 for i,row in enumerate(rows,1):
  f=row['frame'];p=f.get('params',{});u=p.get('update',{});item=p.get('item',{});payload=p.get('payload',{});name=folder.name
  content=u.get('content',{})
  if row['direction']=='in' and isinstance(content,dict):chunks.append(content.get('text',''))
  keep=(
   name=='codex' and (item.get('type') in ['subAgentActivity','collabAgentToolCall'] or f.get('method')=='turn/completed') or
   name=='claude' and (f.get('subtype') in ['task_started','task_progress','task_notification'] or f.get('parent_tool_use_id') and f.get('type') in ['assistant','user']) or
   name=='zcode' and (payload.get('source')=='subagent' or p.get('type')=='turn.completed') or
   name=='cursor' and (f.get('method')=='cursor/task' or u.get('sessionUpdate') in ['tool_call','tool_call_update']) or
   name=='kimi' and (f.get('method')=='fs/read_text_file' or u.get('sessionUpdate')=='tool_call' or u.get('status')=='completed') or
   name=='grok' and (u.get('sessionUpdate') in ['subagent_spawned','subagent_progress','subagent_finished'] or u.get('sessionUpdate') in ['tool_call','tool_call_update']) or
   name=='antigravity' and (f.get('event')=='result' or f.get('step_update',{}).get('step_type')=='subagent'))
  if keep:selected.append({'wireLine':i,**row})
 marker=(folder/'fixture.txt').read_text().strip()
 received='\n'.join(json.dumps(x['frame']) for x in rows if x['direction']=='in')+'\n'+''.join(chunks)
 out={'agent':folder.name,'markerObservedInIncomingData':marker in received,'sum42ObservedInIncomingData':'42' in received,'note':'42 presence alone is not proof; inspect selected completion and final response.','selectedEvents':selected}
 if folder.name=='zcode':out['harnessNote']='Native turn.completed at 38.136 seconds, wire line 553, resultType success. Original harness checked payload.type instead of params.type, so summary.json reports harness TimeoutError after 240 seconds. Not an agent failure; probe source fixed subsequently.'
 if folder.name=='antigravity':out['harnessNote']='Parent result SUCCESS is not child success: denied_actions read_file; child transcript contains requested view_file without successful output.'
 (folder/'evidence.json').write_text(json.dumps(out,ensure_ascii=False,indent=2))
 print(folder.name,len(selected),'marker',out['markerObservedInIncomingData'])
