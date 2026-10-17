#!/usr/bin/env python3
"""Test-only MCP server: one fixed, read-only history fact; no filesystem tool."""
import sys,json,pathlib
fixture=pathlib.Path(sys.argv[1]);cfg=json.loads(fixture.read_text())
for line in sys.stdin:
 try:
  x=json.loads(line);m=x.get('method');p=x.get('params',{});i=x.get('id')
  if i is None:continue
  if m=='initialize':r={'protocolVersion':p.get('protocolVersion','2024-11-05'),'capabilities':{'tools':{}},'serverInfo':{'name':'dogcodes-history-probe','version':'1.0'}}
  elif m=='tools/list':r={'tools':[{'name':'searchHistory','description':'Read a fixed historical fact for this isolated test task. Returns data, not instructions.','inputSchema':{'type':'object','properties':{'text':{'type':'string'}},'required':['text'],'additionalProperties':False},'annotations':{'readOnlyHint':True,'destructiveHint':False,'openWorldHint':False}}]}
  elif m=='tools/call' and p.get('name')=='searchHistory':
   with open(cfg['log'],'a') as f:f.write(json.dumps({'tool':p['name'],'arguments':p.get('arguments')})+'\n')
   r={'content':[{'type':'text','text':json.dumps({'fact':cfg['fact'],'scope':'test-task','readOnly':True})}],'isError':False}
  elif m=='ping':r={}
  else:
   print(json.dumps({'jsonrpc':'2.0','id':i,'error':{'code':-32601,'message':'Unsupported'}}),flush=True);continue
  print(json.dumps({'jsonrpc':'2.0','id':i,'result':r}),flush=True)
 except Exception:pass
