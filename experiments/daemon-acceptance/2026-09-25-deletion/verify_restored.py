import hashlib,json,pathlib,sqlite3,time
from client import Client
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);before=json.loads((out/'before.json').read_text());historical=f['historicalTask']
assert c.call('timeline',[historical])==before['timeline']
reports={r['request']['runId']:r for r in c.call('turn_reviews',[historical])}
for r in before['reviews']:assert reports[r['request']['runId']]==r
assert c.call('review_source',[historical,f['reviewRun'],'file-1'])==before['source']
assert [s for s in c.call('bootstrap')['sessionMaps'] if s['taskId']==historical]==before['sessionMaps']
# Validate missing/stale versions and a committed delete whose client never reads the reply.
t=f['cases']['普通验收'];deleted=c.task(t,True)
try:c.execute('restore_task',{'taskId':t},deleted['streamVersion']-1)
except RuntimeError:pass
else:raise AssertionError('stale restore accepted')
try:c.call('query_history',[dict(taskId=t,afterSequence=0,throughSequence=None,limit=10,kind='search',text='验收')])
except RuntimeError as e:assert 'not found' in str(e)
else:raise AssertionError('deleted search visible')
c.execute('restore_task',{'taskId':t},deleted['streamVersion'])
try:c.execute('delete_task',{'taskId':t})
except RuntimeError as e:assert 'expectedStreamVersion' in str(e)
else:raise AssertionError('unversioned delete accepted')
current=c.task(t);cursor=c.call('bootstrap')['lastSequence'];cmd=c.envelope('delete_task',{'taskId':t},current['streamVersion']);sock=c.connect('execute',[cmd]);sock.close()
for _ in range(50):
 if any(x['taskId']==t for x in c.call('bootstrap')['deletedTasks']):break
 time.sleep(.02)
else:raise AssertionError('disconnected delete did not commit')
ack=c.call('execute',[cmd]);assert c.call('execute',[cmd])==ack
with c.connect('subscribe',cursor) as sub:
 events=c.read(sub)['Ok'];assert len([e for e in events if e['eventType']=='task.deleted' and e['payload']['taskId']==t])==1
# Restore every remaining original test conversation.
for task in c.call('bootstrap')['deletedTasks']:c.execute('restore_task',{'taskId':task['taskId']},task['streamVersion'])
assert not c.call('bootstrap')['deletedTasks']
root=pathlib.Path(f['sourceFixture'])/'live-project'
assert {str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file() and '.git' not in p.parts}==f['sourceProjectHashes']
assert (pathlib.Path(f['workspace'])/'unchanged.txt').read_text()=='delete must not touch project files\n'
src=sqlite3.connect(f"file:{f['sourceFixture']}/app-v5.db?mode=ro",uri=True);dst=sqlite3.connect(f"file:{f['directory']}/app-v5.db?mode=ro",uri=True)
blobs=src.execute('select payload_sha256,content from artifact_blobs').fetchall()
for sha,blob in blobs:assert dst.execute('select content from artifact_blobs where payload_sha256=?',(sha,)).fetchone()[0]==blob
lifecycle=dst.execute("select event_type,stream_id,stream_version from events where event_type in ('task.deleted','task.restored') and sequence>(select max(sequence) from events where event_type='project.registered') order by sequence").fetchall()
result={'servicePids':{'before':f['retiredServicePid'],'after':c.call('ping')['pid']},'serviceActuallyRestarted':f['retiredServicePid']!=c.call('ping')['pid'],'originalHistoryUnchanged':True,'originalReportsUnchanged':len(before['reviews']),'originalSessionMapsUnchanged':len(before['sessionMaps']),'frozenSourceUnchanged':True,'originalArtifactBlobsUnchanged':len(blobs),'projectHashesUnchanged':True,'deletedSearchRejected':True,'staleRestoreRejected':True,'missingDeleteVersionRejected':True,'lostReplyIdempotent':True,'reconnectedSubscriptionReplaysDeleteOnce':True,'allDeletedRestored':True,'primaryGuard':json.loads((out/'active-guards.json').read_text())['primary']['rejection'],'reviewGuard':json.loads((out/'active-guards.json').read_text())['review']['rejection'],'lifecycleEvents':lifecycle,'scope':'macOS custom-protocol default remote Core; isolated data; two UI instances; no review-acceptance feature','guiChecks':['normal/pinned/archived delete','confirmation cancel','two-window removal and current-task fallback','last task to new conversation','service+UI restart retains six deleted tasks','pinned/archived location restored in originating window','restore propagated to other window','restored messages and review cards','restored historical sample.txt diff']}
(out/'results.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n');print(json.dumps({k:v for k,v in result.items() if k not in ['lifecycleEvents','guiChecks']},ensure_ascii=False))
