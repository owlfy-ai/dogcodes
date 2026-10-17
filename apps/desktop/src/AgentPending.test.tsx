// @vitest-environment jsdom
import {act} from 'react';
import {createRoot} from 'react-dom/client';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import type {CoreClient,TimelineItem} from '@dogcodes/protocol';
import {ConversationContent} from './ConversationActivity';
let host:HTMLDivElement,root:ReturnType<typeof createRoot>;
beforeEach(()=>{vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT',true);host=document.createElement('div');root=createRoot(host);});
afterEach(async()=>{await act(async()=>root.unmount());vi.unstubAllGlobals();});
const item=(id:string,kind:string,text:string,turnId='trn_new'):TimelineItem=>({attachments: [], itemId:id,taskId:'tsk_test',turnId:turnId as `trn_${string}`,kind,text,providerId:'kimi',sequence:1,occurredAt:'2026-09-15T00:00:00Z'});
const render=async(items:TimelineItem[],pending=true)=>act(async()=>root.render(<ConversationContent items={items} client={{} as CoreClient} activeTurnId="trn_new" pendingAgent={pending?{providerId:'kimi',turnId:'trn_new'}:undefined} renderAgent={id=><span data-agent={id}/>} onSubagent={()=>{}} renderEntry={i=><p>{i.text}</p>}/>));
it('shows the executing agent while the new turn has no output, ignoring prior responses and empty/status events',async()=>{
 await render([item('old','assistant_message','上轮回答','trn_old'),item('prompt','user_message','你好'),item('empty','assistant_message',''),item('status','activity',JSON.stringify({type:'status',status:'running'}))]);
 expect(host.querySelector('.agent-pending [data-agent="kimi"]')).not.toBeNull();
 expect(host.querySelector('.agent-pending')?.getAttribute('role')).toBe('status');
 expect(host.querySelectorAll('.agent-pending-dots i')).toHaveLength(3);
});
it.each([
 ['assistant_message','你好'],
 ['activity',JSON.stringify({type:'message',text:'你好'})],
 ['activity',JSON.stringify({type:'thought',status:'running'})],
 ['activity',JSON.stringify({type:'tool',name:'Read',status:'running',input:{path:'a.ts'}})],
])('replaces loading with the first visible %s event',async(kind,text)=>{
 await render([item('prompt','user_message','你好')]);
 expect(host.querySelector('.agent-pending')).not.toBeNull();
 await render([item('prompt','user_message','你好'),{...item('reply',kind,text),providerId:null}]);
 expect(host.querySelector('.agent-pending')).toBeNull();
 expect(host.querySelectorAll('[data-agent]')).toHaveLength(1);
});
it('removes loading when the run ends or waits for user interaction',async()=>{
 await render([item('prompt','user_message','你好')]);
 await render([item('prompt','user_message','你好')],false);
 expect(host.querySelector('.agent-pending')).toBeNull();
});
