import json,pathlib,time
from client import Client,uid
out=pathlib.Path(__file__).parent;f=json.loads((out/'fixture.json').read_text());c=Client(f['directory']);execs=c.call('registered_executors');primary=next(e for e in execs if e['providerId']=='codex');reviewer=next(e for e in execs if e['providerId']=='kimi');task=f['cases']['运行门禁验收']
c.execute('submit_turn',dict(taskId=task,prompt='隔离测试：只申请一次运行 sleep 20 的审批。不要读取或修改任何文件，不要调用其他工具。等待命令结束后简短回复。',model=None,thinking=None,permissionMode='default',executorOverride=primary,attachments=[]),c.task(task)['streamVersion'])
results={'primary':{},'review':{}}
def blocked(t):
    snapshot=c.task(t)
    try:c.execute('delete_task',{'taskId':t},snapshot['streamVersion'])
    except RuntimeError as e:
        assert '请先停止主任务和审查' in str(e),str(e)
        return str(e)
    raise AssertionError('active task was deleted')
results['primary']['rejection']=blocked(task);results['primary']['run']=c.task(task)['activePrimaryRun']
request=dict(taskId=f['historicalTask'],turnId=f['reviewTurn'],sourceSnapshot=None,runId=uid('run'),executor=reviewer,kind='code',retryOf=f['reviewRun'],recheckOf=None)
# Original report kind must match an identical-input retry.
original=next(r for r in c.call('turn_reviews',[f['historicalTask']]) if r['request']['runId']==f['reviewRun']);request['kind']=original['request']['kind']
r=c.call('start_turn_review',[request]);results['review']['runId']=request['runId'];results['review']['rejection']=blocked(f['historicalTask']);results['review']['initialStatus']=r['status']
(out/'active-guards.json').write_text(json.dumps(results,ensure_ascii=False,indent=2)+'\n');f['activeReviewRun']=request['runId'];(out/'fixture.json').write_text(json.dumps(f,ensure_ascii=False,indent=2)+'\n')
print(json.dumps({'primary':'blocked','review':'blocked','reviewStatus':r['status']},ensure_ascii=False))
