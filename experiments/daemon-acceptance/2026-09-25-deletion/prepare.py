import json,pathlib,hashlib
from client import Client,uid
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);b=c.call('bootstrap');f['servicePid']=c.call('ping')['pid']
f['projectId']=uid('prj');c.execute('register_project',dict(projectId=f['projectId'],displayName='删除服务验收',path=f['workspace'],additionalDirectories=[]))
f['cases']={}
for label in ['普通验收','置顶验收','归档验收','运行门禁验收']:
 task=uid('tsk');c.execute('create_task',dict(taskId=task,projectId=f['projectId'],title=label,preferredExecutor={'providerId':'fake','installationId':None}));f['cases'][label]=task
historical=f['historicalTask'];reports=c.call('turn_reviews',[historical]);report=next(r for r in reports if r['sourceArtifact'])
f['reviewRun']=report['request']['runId'];f['reviewTurn']=report['request']['turnId']
source=c.call('review_source',[historical,f['reviewRun'],'file-1'])
before={'timeline':c.call('timeline',[historical]),'reviews':reports,'source':source,'sessionMaps':[s for s in b['sessionMaps'] if s['taskId']==historical]}
(out/'before.json').write_text(json.dumps(before,ensure_ascii=False,indent=2)+'\n')
f['sourceProjectHashes']={str(p.relative_to(pathlib.Path(f['sourceFixture'])/'live-project')):hashlib.sha256(p.read_bytes()).hexdigest() for p in (pathlib.Path(f['sourceFixture'])/'live-project').rglob('*') if p.is_file() and '.git' not in p.parts}
(out/'fixture.json').write_text(json.dumps(f,ensure_ascii=False,indent=2)+'\n');print(json.dumps({'servicePid':f['servicePid'],'uiPids':[f['uiPidA'],f['uiPidB']],'cases':f['cases'],'historicalReports':len(reports)},ensure_ascii=False))
