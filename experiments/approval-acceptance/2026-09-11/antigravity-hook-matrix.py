import tempfile,pathlib,json,subprocess,os
for name,new_project,private in [('separate',False,True),('separate-new',True,True),('cwd-new',True,False)]:
 with tempfile.TemporaryDirectory(prefix='dogcodes-matrix-') as d:
  p=pathlib.Path(d); cwd=p/'work';cwd.mkdir();h=p/'hooks' if private else cwd;h.mkdir(exist_ok=True);(h/'.agents').mkdir()
  hook=h/'deny.sh';hook.write_text('#!/bin/sh\ncat >/dev/null\nprintf called > '+str(p/'hook-called')+'\nprintf \'{"decision":"deny","reason":"Fixture denied"}\'\n');hook.chmod(0o700)
  (h/'.agents/hooks.json').write_text(json.dumps({'codexhost-question-bridge':{'PreToolUse':[{'matcher':'.*','hooks':[{'type':'command','command':str(hook),'timeout':20}]}]}}))
  args=[os.path.expanduser('~/.local/bin/agy'),'--add-dir',str(cwd),'--add-dir',str(h),'--input-format','stream-json','--output-format','stream-json','--dangerously-skip-permissions','--print-timeout','2m']
  if new_project: args+=['--new-project']
  prompt={'event':'user','message':{'content':[{'type':'text','text':'Use run_command to execute exactly: printf TEST > approval.txt . Do not access any other paths. If denied stop immediately.'}]}}
  result=subprocess.run(args,cwd=cwd,input=json.dumps(prompt)+'\n',capture_output=True,text=True,timeout=150)
  print(name,'exit:',result.returncode,'hook_called:',(p/'hook-called').exists(),'file_written:',(cwd/'approval.txt').exists(),flush=True)
