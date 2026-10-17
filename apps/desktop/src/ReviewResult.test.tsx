// @vitest-environment jsdom
import {act} from "react";
import {createRoot, type Root} from "react-dom/client";
import {afterEach, beforeEach, expect, it, vi} from "vitest";
import type {CoreClient, ReviewFinding, ReviewRepairCall, ReviewRepairEntry, TurnReviewRequest, TurnReviewRun} from "@dogcodes/protocol";
import {ReviewResult} from "./ReviewResult";
import {sourceDiff} from "./reviewSourceDiff";

function submittedRepair(call: Extract<ReviewRepairCall, {action: "start"}>): ReviewRepairEntry {
  return {intent: {operationId: call.operationId, taskId: call.taskId, baseVersion: 7, reference: call.reference, target: call.target ?? {providerId: "codex", installationId: "ins_main"}, context: null, createdAt: "now"},
    updatedAt: "now", state: "submitted", acknowledgement: {commandId: call.operationId, accepted: true, firstSequence: 8, lastSequence: 8, requestFingerprint: "v1:hash"}};
}
let container: HTMLDivElement, root: Root;
const finding: ReviewFinding = {fileId:"file-1", rootId:"root", path:"a.ts", severity:"high", title:"错误路径",description:"可触发的错误",suggestion:"处理错误",side:"after",startLine:12,endLine:12,evidence:"line 12",locationVerified:true,locationNote:"位置与冻结代码一致"};
const lines = Array.from({length:20},(_,i)=>`line ${i+1}\n`).join("");
function fixture() {
  const reviewSource=vi.fn(async()=>({fileId:"file-1",rootId:"root",path:"a.ts",before:lines,after:lines}));
  const client={reviewSource} as unknown as CoreClient;
  const run:TurnReviewRun={request:{taskId:"tsk_test",turnId:null,sourceSnapshot:"old-snapshot",runId:"run_old",executor:{providerId:"kimi",installationId:null},kind:"code",retryOf:null},status:"completed",createdAt:"2026-09-23T10:00:00Z",finishedAt:null,nativeSessionId:null,sourceSequence:1,sourceArtifact:null,inputSha256:"hash",coverage:"冻结改动",result:"raw",error:null,
    report:{schemaVersion:1,structured:true,findings:[finding],files:[{fileId:"file-1",rootId:"root",path:"a.ts",status:"reported",rules:["检查异步错误"]},{fileId:"file-2",rootId:"two",path:"a.ts",status:"unavailable",rules:[]}],warnings:["审查覆盖不完整"]}};
  return {client,reviewSource,run};
}
async function openExecution() {
  const details = container.querySelector<HTMLDetailsElement>(".review-execution")!;
  await act(async () => { details.open = true; details.dispatchEvent(new Event("toggle", {bubbles:true})); });
}
beforeEach(()=>{localStorage.clear();vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);HTMLDialogElement.prototype.showModal = function(this: HTMLDialogElement){this.open=true;};container=document.createElement("div");document.body.append(container);root=createRoot(container);});
afterEach(async()=>{await act(async()=>root.unmount());container.remove();vi.restoreAllMocks();vi.unstubAllGlobals();});
it("opens the persisted review version and reveals a finding inside folded unchanged context",async()=>{
  const f=fixture();await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).toContain("文件覆盖：1/2");
  expect(container.textContent).not.toContain("审查覆盖不完整");
  expect(container.querySelector(".review-finding")!.compareDocumentPosition(container.querySelector(".review-execution")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await openExecution();
  expect(container.textContent).toContain("审查覆盖不完整");
  await act(async()=>container.querySelector<HTMLButtonElement>(".review-location")!.click());
  expect(f.reviewSource).toHaveBeenCalledWith("tsk_test","run_old","file-1");
  expect(container.querySelector("dialog")?.textContent).not.toContain("old-snapshot");
  const focused=container.querySelector('[data-review-focused="true"]');
  expect(focused?.getAttribute("data-review-new-line")).toBe("11");
  expect(focused?.textContent).toContain("line 12");
});
it("never links unverified positions or replaces missing historical code with current files",async()=>{
  const f=fixture();f.run.report!.findings=[{...finding,locationVerified:false}];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.querySelector<HTMLButtonElement>(".review-location")!.disabled).toBe(true);
  f.run.report!.findings=[finding];f.reviewSource.mockRejectedValueOnce(new Error("gone"));
  await act(async()=>root.render(<ReviewResult client={f.client} run={{...f.run}}/>));
  await act(async()=>container.querySelector<HTMLButtonElement>(".review-location")!.click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("未改用当前文件");
  expect(container.querySelector(".review-text")).toBeNull();
});
it("rejects a source from another same-named root",async()=>{
  const f=fixture();f.reviewSource.mockResolvedValueOnce({fileId:"file-1",rootId:"other",path:"a.ts",before:"wrong",after:"wrong"});
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  await act(async()=>container.querySelector<HTMLButtonElement>(".review-location")!.click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("来源不匹配");
  expect(container.querySelector(".review-text")).toBeNull();
});
it("preserves old Markdown and warns when a new Agent response is unstructured",async()=>{
  const f=fixture();await act(async()=>root.render(<ReviewResult client={f.client} run={{...f.run,report:undefined,result:"旧报告"}}/>));
  expect(container.textContent).toBe("审查执行过程");
  await openExecution();
  expect(container.querySelector(".review-execution")?.textContent).toContain("旧报告");
  f.run.report!.structured=false;f.run.report!.warnings=["格式无法验证"];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  await openExecution();
  expect(container.querySelector(".review-execution")?.textContent).toContain("格式无法验证");
  expect(container.textContent).toContain("raw");
  expect(container.querySelector(".review-location")).toBeNull();
});
it("compares saved additions and deletions with correct line counts and refuses incomplete sources",()=>{
  const base={fileId:"f",rootId:"r",path:"a",before:"old\nkeep\n",after:"new\nkeep\n"};
  const diff=sourceDiff(base)!;
  expect(diff.additions).toBe(1);expect(diff.deletions).toBe(1);
  expect(sourceDiff({...base,before:""})!.additions).toBe(2);
  expect(sourceDiff({...base,after:""})!.deletions).toBe(2);
  expect(sourceDiff({...base,after:null})).toBeNull();
  expect(sourceDiff({...base,after:"x\n".repeat(20001)})).toBeNull();
});
it("shows live grouped progress and preserves an AI-refuted finding with its reason",async()=>{
  const f=fixture();
  f.run.status="running";f.run.result="";
  f.run.workflow={contextScope:"仅冻结版本",groups:[{id:"g1",fileIds:["file-1"],status:"verifying",sessions:["first","second"],contextRequests:2,initialOutput:"初审",verificationOutput:"",error:null}]};
  f.run.report!.findings=[{...finding,verification:{verdict:"refuted",reason:"调用方已经校验"}}];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).toContain("分组审查：0/1");
  expect(container.textContent).toContain("独立复核中");
  expect(container.textContent).not.toContain("次上下文请求");
  expect(container.textContent).not.toContain("冻结上下文检索记录");
  expect(container.textContent).toContain("AI 复核不支持");
  expect(container.textContent).toContain("调用方已经校验");
  expect(container.querySelector(".review-location")).not.toBeNull();
  expect(container.querySelector("details")?.open).toBe(true);
});

it("submits only selected findings to the current primary agent and rechecks the repair version", async () => {
  const f = fixture();
  const primary = {taskId:"tsk_test",preferredExecutor:{providerId:"codex",installationId:"ins_main"},activePrimaryRun:null,streamVersion:7};
  const repair = {turnId:"trn_fix",findingIndices:[0],status:"completed",recheckRuns:[]};
  const reviewRepair = vi.fn(async (call: ReviewRepairCall) => call.action === "start" ? submittedRepair(call) : null), startTurnReview = vi.fn(async (request: TurnReviewRequest) => ({ ...f.run, request }));
  const client = {...f.client, reviewRepair, startTurnReview, bootstrap:vi.fn(async () => ({tasks:[primary]})), turnReviews:vi.fn(async () => [{...f.run,repairs:[repair]}])} as unknown as CoreClient;
  const executor = {providerId:"kimi",installationId:"ins_review"};
  await act(async()=>root.render(<ReviewResult client={client} run={f.run} executor={executor}/>));
  const repairButton = () => [...container.querySelectorAll("button")].find(b=>b.textContent === "开始修复")!;
  expect(repairButton().disabled).toBe(true);
  await act(async()=>container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  expect(repairButton().textContent).toBe("开始修复");
  await act(async()=>repairButton().click());
  expect(reviewRepair).toHaveBeenCalledWith({action:"start",taskId:"tsk_test",reference:{runId:"run_old",inputSha256:"hash",findingIndices:[0]},target:null,operationId:expect.stringMatching(/^cmd_/)});
  const recheck = [...container.querySelectorAll("button")].find(b=>b.textContent?.startsWith("复审修复后的版本"))!;
  await act(async()=>recheck.click());
  expect(startTurnReview).toHaveBeenCalledWith(expect.objectContaining({turnId:"trn_fix",executor,retryOf:null,recheckOf:{runId:"run_old",repairTurnId:"trn_fix"}}));
  expect(container.textContent).not.toContain("问题已解决");
});

it("separates security evidence, AI support and unexecuted runtime validation", async () => {
  const f = fixture(); f.run.request.kind = "security";
  const citation = {sourceId:"file-1",path:"a.ts",rootLabel:"/frozen",side:"after" as const,startLine:12,endLine:12,evidence:"line 12"};
  f.run.report!.findings = [{...finding, security:{evidenceStatus:"source_checked",runtimeValidation:"not_run",notes:[],evidence:{attackClass:"agent_actions",attacker:"untrusted peer",boundary:"tool handler",preconditions:"reachable tool",controls:"guard checked",impact:"unexpected write",trace:[{...citation,role:"entry",explanation:"input"},{...citation,role:"sink",explanation:"write"}],blockers:[],validationPlan:"isolated reproduction"}},verification:{verdict:"confirmed",reason:"static path supported",securityReview:{boundaryAnalysis:"caller reaches handler",controlAnalysis:"guard misses final argument",impactAnalysis:"limited to selected file",unresolved:[]}}}];
  f.run.report!.files[0].securityChecks = [
    {attackClass:"agent_actions",label:"Agent 工具与审批",status:"reviewed",reason:"checked final arguments",citations:[citation]},
    {attackClass:"access_control",label:"身份与权限",status:"not_applicable",reason:"no extra principal",citations:[citation]},
    {attackClass:"local_boundary",label:"桌面与本地通信",status:"needs_context",reason:"packaging not included",citations:[]},
    {attackClass:"supply_chain",label:"安装与更新",status:"not_reported",reason:"missing",citations:[]},
  ];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).toContain("1 项已检查 · 1 项不适用 · 2 项待补充");
  expect(container.textContent).toContain("潜在高风险");
  expect(container.textContent).toContain("AI 复核支持");
  expect(container.textContent).toContain("未执行漏洞复现");
  expect(container.textContent).toContain("guard misses final argument");
  expect(container.textContent).toContain("/frozen/a.ts");
  expect(container.textContent).not.toContain("漏洞已确认");
});

it("keeps incomplete and old security findings visible without inventing evidence", async () => {
  const f = fixture(); f.run.request.kind = "security";
  f.run.report!.findings = [{...finding, security:{evidence:null,evidenceStatus:"needs_validation",runtimeValidation:"not_run",notes:["缺少入口原文"]},verification:{verdict:"uncertain",reason:"防护尚未核实"}}];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).toContain("安全证据待补充");
  expect(container.textContent).toContain("缺少入口原文");
  expect(container.querySelector(".review-location")).not.toBeNull();
  f.run.report!.findings = [finding];
  await act(async()=>root.render(<ReviewResult client={f.client} run={{...f.run}}/>));
  expect(container.textContent).not.toContain("此报告未记录安全证据链与实际验证状态");
  expect(container.textContent).not.toContain("证据链引用已校验");
});

it("shows independently supplemented evidence while retaining initial blockers", async () => {
  const f = fixture(); f.run.request.kind = "security";
  const initial = {attackClass:"access_control",attacker:"caller",boundary:"document owner",preconditions:"other owner",controls:"unknown guard",impact:"read document",trace:[],blockers:["initial guard unavailable"],validationPlan:"test with dummy identities"};
  f.run.report!.findings = [{...finding,security:{evidence:initial,evidenceStatus:"needs_validation",runtimeValidation:"not_run",notes:["initial missing evidence"]},verification:{verdict:"confirmed",reason:"frozen helper resolves missing condition",securityEvidence:{evidence:{...initial,controls:"helper has no ownership check",blockers:[]},evidenceStatus:"source_checked",runtimeValidation:"not_run",notes:[]}}}];
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).toContain("复核补充的证据与验证条件");
  expect(container.textContent).toContain("保留的初审证据（复核前）");
  expect(container.textContent).toContain("helper has no ownership check");
  expect(container.textContent).toContain("initial guard unavailable");
  expect(container.textContent).toContain("未执行漏洞复现");
});

it("shows live findings without frozen coverage, second-pass claims or historical links", async () => {
  const f = fixture(); f.run.workspaceReview = true; f.run.request.kind = "security";
  f.run.report = {schemaVersion:2, structured:true, findings:[{...finding,locationVerified:false,locationNote:"Agent 报告的位置"}], files:[], warnings:["已检查当前实现"]};
  await act(async()=>root.render(<ReviewResult client={f.client} run={f.run}/>));
  expect(container.textContent).not.toContain("已检查当前实现");
  await openExecution();
  expect(container.querySelector(".review-execution")?.textContent).toContain("已检查当前实现");
  expect(container.textContent).toContain("错误路径");
  for (const text of ["文件覆盖", "安全专项", "分组审查", "AI 复核", "查看历史 Diff"]) expect(container.textContent).not.toContain(text);
  expect(f.reviewSource).not.toHaveBeenCalled();
});

it("hides result JSON in both live and saved review transcripts without losing findings or process text", async () => {
  for (const status of ["running", "completed"] as const) {
    const f = fixture();
    const payload = '```json\n{"summary":"已完成检查","findings":[]}\n```';
    f.run.status = status;
    f.run.result = payload;
    f.run.activities = [
      {itemId:"progress",data:{type:"message",text:"正在检查调用关系。"}},
      {itemId:"legacy",legacy:true,data:{type:"message",text:payload}},
      {itemId:"final",data:{type:"message",text:payload}},
    ];
    await act(async () => root.render(<ReviewResult client={f.client} run={f.run}/>));
    if (status === "completed") await openExecution();
    expect(container.textContent).toContain("正在检查调用关系。");
    expect(container.textContent).toContain("错误路径");
    expect(container.textContent).not.toContain('"summary"');
    expect(container.textContent).not.toContain('"findings"');
    expect(container.querySelector('.review-activity .code-block')).toBeNull();
    expect(f.run.result).toBe(payload);
  }
});

it("keeps a neutral repair label and hands the Agent chosen in the composer to the service", async () => {
  const {PendingExecutorContext} = await import("./automaticContext");
  const f = fixture();
  const primary = {taskId:"tsk_test",preferredExecutor:{providerId:"codex",installationId:"ins_main"},activePrimaryRun:null,streamVersion:7};
  const reviewRepair = vi.fn(async (call: ReviewRepairCall) => call.action === "start" ? submittedRepair(call) : null);
  const client = {...f.client, reviewRepair, bootstrap: vi.fn(async () => ({tasks: [primary]})), turnReviews: vi.fn(async () => [])} as unknown as CoreClient;
  const pending = {tsk_test: {providerId: "fake-peer", installationId: null}};
  await act(async()=>root.render(<PendingExecutorContext.Provider value={pending}><ReviewResult client={client} run={f.run}/></PendingExecutorContext.Provider>));
  const repairButton = () => [...container.querySelectorAll("button")].find(b=>b.textContent === "开始修复")!;
  expect(repairButton().textContent).toBe("开始修复");
  await act(async()=>container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  await act(async () => { await vi.waitFor(() => expect(repairButton().disabled).toBe(false)); });
  await act(async()=>repairButton().click());
  expect(reviewRepair).toHaveBeenCalledWith(expect.objectContaining({action: "start", target: {providerId: "fake-peer", installationId: null}}));
});

it("recovers a repair recheck with its original Agent and linkage after reopening", async () => {
  const f = fixture();
  const repair = { turnId: "trn_fix", findingIndices: [0], status: "completed", recheckRuns: [] };
  const startTurnReview = vi.fn<(request: TurnReviewRequest) => Promise<TurnReviewRun>>()
    .mockRejectedValueOnce(new Error("offline")).mockImplementation(async request => ({ ...f.run, request }));
  const client = { ...f.client, reviewRepair: vi.fn(async () => null), startTurnReview, bootstrap: vi.fn(async () => ({ tasks: [] })), turnReviews: vi.fn(async () => [{ ...f.run, repairs: [repair] }]) } as unknown as CoreClient;
  const executor = { providerId: "kimi", installationId: "original" };
  const render = (agent = executor) => act(async () => root.render(<ReviewResult client={client} run={f.run} executor={agent}/>));
  const byText = (text: string) => [...container.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent === text)!;
  await render();
  await act(async () => [...container.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent?.startsWith("复审修复后的版本"))!.click());
  const original = startTurnReview.mock.calls[0][0];
  await act(async () => root.render(null)); await render({ providerId: "codex", installationId: "different" });
  expect(startTurnReview).toHaveBeenCalledTimes(1);
  await act(async () => byText("核实审查结果").click()); await act(async () => byText("重发原审查").click());
  expect(startTurnReview.mock.calls[1][0]).toEqual(original);
  expect(original.recheckOf).toEqual({ runId: f.run.request.runId, repairTurnId: "trn_fix" });
});
