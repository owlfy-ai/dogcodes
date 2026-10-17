import asyncio,pathlib,time,json,sys
ROOT=pathlib.Path(__file__).resolve().parent
RUN=ROOT/time.strftime('%Y%m%d-%H%M%S');RUN.mkdir();print(RUN,flush=True)
BIN=ROOT.parents[2]/'target/debug/examples/live_history_mcp'
async def one(name,sem):
 async with sem:
  d=RUN/name;d.mkdir()
  with (d/'run.log').open('w') as log:
   p=await asyncio.create_subprocess_exec(str(BIN),name,str(d),stdout=log,stderr=log)
   code=await p.wait()
  print(json.dumps({'provider':name,'exit':code,'summary':json.loads((d/'summary.json').read_text()) if (d/'summary.json').exists() else None}),flush=True)
async def main():
 sem=asyncio.Semaphore(2)
 await asyncio.gather(*(one(n,sem) for n in (sys.argv[1:] or ['codex','zcode','claude','antigravity','kimi','grok','cursor','codebuddy'])))
asyncio.run(main())
