import asyncio,json,pathlib,sys
async def main():
 d=pathlib.Path(sys.argv[1]);rows=[json.loads(x)['frame'] for x in (d/'wire.jsonl').read_text().splitlines()];ids={f.get('params',{}).get('item',{}).get('agentThreadId') for f in rows};ids.discard(None)
 p=await asyncio.create_subprocess_exec('/Applications/ChatGPT.app/Contents/Resources/codex','app-server',stdin=-1,stdout=-1,stderr=-1,limit=8*1024*1024)
 async def drain():
  while await p.stderr.read(65536):pass
 t=asyncio.create_task(drain())
 async def req(i,m,v):
  p.stdin.write((json.dumps({'id':i,'method':m,'params':v})+'\n').encode());await p.stdin.drain()
  while True:
   f=json.loads(await asyncio.wait_for(p.stdout.readline(),30))
   if f.get('id')==i:return f
 await req(1,'initialize',{'clientInfo':{'name':'observability-history','version':'0.1'}})
 result=[]
 for i,id in enumerate(ids,2):result.append(await req(i,'thread/read',{'threadId':id,'includeTurns':True}))
 (d/'child-history.json').write_text(json.dumps(result,indent=2))
 print('children',len(ids),'results',[(x.get('error'),len(x.get('result',{}).get('thread',{}).get('turns',[]))) for x in result])
 p.stdin.close();await asyncio.wait_for(p.wait(),15);t.cancel()
asyncio.run(main())
