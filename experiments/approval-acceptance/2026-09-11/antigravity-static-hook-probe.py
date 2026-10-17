import tempfile, pathlib, json, subprocess, os
with tempfile.TemporaryDirectory(prefix='dogcodes-hook-') as d:
 p=pathlib.Path(d); (p/'.agents').mkdir()
 hook=p/'deny.sh'; hook.write_text('#!/bin/sh\ncat >/dev/null\nprintf called > '+str(p/'hook-called')+'\nprintf \'{"decision":"deny","reason":"Fixture denied"}\'\n'); hook.chmod(0o700)
 (p/'.agents/hooks.json').write_text(json.dumps({'codexhost-question-bridge':{'PreToolUse':[{'matcher':'.*','hooks':[{'type':'command','command':str(hook),'timeout':20}]}]}}))
 args=[os.path.expanduser('~/.local/bin/agy'),'--add-dir',d]
 listed=subprocess.run(args+['--print=/hooks','--output-format','stream-json'],cwd=d,capture_output=True,text=True,timeout=30)
 print('hook_listing:',listed.stdout,flush=True)
 prompt={'event':'user','message':{'content':[{'type':'text','text':'Use run_command to execute exactly: printf TEST > approval.txt . Do not access any other paths. If denied stop immediately.'}]}}
 result=subprocess.run(args+['--input-format','stream-json','--output-format','stream-json','--dangerously-skip-permissions','--print-timeout','2m'],cwd=d,input=json.dumps(prompt)+'\n',capture_output=True,text=True,timeout=150)
 print('exit:',result.returncode,'hook_called:',(p/'hook-called').exists(),'file_written:',(p/'approval.txt').exists(),flush=True)
 print(result.stdout[-16000:])
