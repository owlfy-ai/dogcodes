import json,pathlib,time
from client import Client
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);last=f['cases']['运行门禁验收']
for _ in range(30):
 r=next(r for r in c.call('turn_reviews',[f['historicalTask']]) if r['request']['runId']==f['activeReviewRun'])
 if r['status'] not in ['queued','running','cancelling']:break
 time.sleep(.1)
assert r['status']=='cancelled',r['status'];assert c.task(last)['activePrimaryRun'] is None
b=c.call('bootstrap');cursor=b['lastSequence'];events=[]
with c.connect('subscribe',cursor) as sub:
 assert c.read(sub)=={'Ok':[]}
 for t in b['tasks']:
  if t['taskId']==last:continue
  cmd=c.envelope('delete_task',{'taskId':t['taskId']},t['streamVersion'])
  ack=c.call('execute',[cmd]);assert c.call('execute',[cmd])==ack
  event=c.read(sub);assert event['eventType']=='task.deleted';assert event['payload']['taskId']==t['taskId'];events.append(event)
assert len(c.call('bootstrap')['tasks'])==1
for method,args in [('timeline',[f['historicalTask']]),('turn_reviews',[f['historicalTask']]),('review_source',[f['historicalTask'],f['reviewRun'],'file-1'])]:
 try:c.call(method,args)
 except RuntimeError:pass
 else:raise AssertionError('deleted read accessible: '+method)
(out/'rpc-delete-results.json').write_text(json.dumps({'duplicateCommandsIdempotent':True,'eventStream':events,'deletedReadsRejected':True,'cancelledReview':r['status'],'servicePid':c.call('ping')['pid']},ensure_ascii=False,indent=2)+'\n')
print({'remainingTasks':1,'deletedTasks':len(c.call('bootstrap')['deletedTasks']),'servicePid':c.call('ping')['pid']})
