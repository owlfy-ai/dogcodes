// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { QuestionCard } from "./QuestionCard";
import type { QuestionForm } from "@dogcodes/protocol";
let node: HTMLDivElement, root: ReturnType<typeof createRoot>;
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); node=document.createElement("div"); document.body.append(node); root=createRoot(node); });
afterEach(async()=>{await act(async()=>root.unmount());node.remove();vi.unstubAllGlobals();});
const form:QuestionForm={title:"Choose",schema:{type:"object",required:["color"],properties:{color:{type:"string",title:"Color",oneOf:[{const:"native-red",title:"Red"},{const:"native-blue",title:"Blue"}]}}}};
it("submits native option IDs only after selection",async()=>{
 const answer=vi.fn();await act(async()=>root.render(<QuestionCard form={form} onAnswer={answer}/>));
 expect(node.querySelector<HTMLButtonElement>('[type="submit"]')!.disabled).toBe(true);
 await act(async()=>node.querySelector<HTMLInputElement>('input[type="radio"]')!.click());
 await act(async()=>node.querySelector('form')!.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true})));
 expect(answer).toHaveBeenCalledWith({cancelled:false,values:{color:"native-red"}});
});
it("cancellation does not submit defaults or require all fields",async()=>{
 const answer=vi.fn();await act(async()=>root.render(<QuestionCard form={form} onAnswer={answer}/>));
 await act(async()=>node.querySelector<HTMLButtonElement>('button[type="button"]')!.click());
 expect(answer).toHaveBeenCalledWith({cancelled:true,values:{}});
});
it("multi-select retains separate values and busy blocks input",async()=>{
 const answer=vi.fn();const multi:QuestionForm={...form,schema:{...form.schema,properties:{color:{type:"array",title:"Color",items:form.schema.properties!.color}}}};
 await act(async()=>root.render(<QuestionCard form={multi} onAnswer={answer}/>));
 for(const option of node.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'))await act(async()=>option.click());
 await act(async()=>node.querySelector('form')!.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true})));
 expect(answer).toHaveBeenCalledWith({cancelled:false,values:{color:["native-red","native-blue"]}});
 await act(async()=>root.render(<QuestionCard form={multi} onAnswer={answer} busy/>));
 expect(node.querySelector<HTMLButtonElement>('[type="submit"]')!.disabled).toBe(true);
});
it("moves between questions without submitting and preserves native answers for final submission",async()=>{
 const answer=vi.fn();const wizard:QuestionForm={title:"请回答以下问题",schema:{type:"object",required:["color","size"],properties:{color:form.schema.properties!.color,size:{type:"string",title:"Size",oneOf:[{const:"large-id",title:"Large (Recommended)"},{const:"small-id",title:"Small"}]}}}};
 await act(async()=>root.render(<QuestionCard form={wizard} onAnswer={answer}/>));
 expect(node.querySelectorAll('.question-field:not([hidden])')).toHaveLength(1);
 expect(node.querySelector<HTMLButtonElement>('[type="submit"]')!.textContent).toContain("下一题");
 await act(async()=>node.querySelector<HTMLInputElement>('.question-field:not([hidden]) input')!.click());
 await act(async()=>node.querySelector<HTMLButtonElement>('[type="submit"]')!.click());
 expect(answer).not.toHaveBeenCalled();expect(node.querySelector('.question-field:not([hidden]) legend')!.textContent).toContain("Size");
 expect(node.querySelector('.question-recommended')!.textContent).toBe("推荐");
 await act(async()=>node.querySelector<HTMLInputElement>('.question-field:not([hidden]) input')!.click());
 await act(async()=>node.querySelector<HTMLButtonElement>('.question-previous')!.click());
 expect(node.querySelector<HTMLInputElement>('.question-field:not([hidden]) input')!.checked).toBe(true);
 await act(async()=>node.querySelector<HTMLButtonElement>('[type="submit"]')!.click());
 await act(async()=>node.querySelector<HTMLButtonElement>('[type="submit"]')!.click());
 expect(answer).toHaveBeenCalledWith({cancelled:false,values:{color:"native-red",size:"large-id"}});
});
it("validates a previous hidden field before submitting the whole form",async()=>{
 const answer=vi.fn();const wizard:QuestionForm={title:"Questions",schema:{type:"object",required:["count","color"],properties:{count:{type:"integer",title:"Count",minimum:2},color:form.schema.properties!.color}}};
 await act(async()=>root.render(<QuestionCard form={wizard} onAnswer={answer}/>));
 const input=node.querySelector<HTMLInputElement>('input[type="number"]')!;
 await act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,"value")!.set!.call(input,"1");input.dispatchEvent(new Event("input",{bubbles:true}));});
 await act(async()=>node.querySelector<HTMLButtonElement>('.question-nav button:nth-child(2)')!.click());
 await act(async()=>node.querySelector<HTMLInputElement>('.question-field:not([hidden]) input')!.click());
 await act(async()=>node.querySelector<HTMLButtonElement>('[type="submit"]')!.click());
 expect(answer).not.toHaveBeenCalled();expect(node.querySelector('.question-field:not([hidden]) legend')!.textContent).toContain("Count");
});
it("retains a custom answer when its editor is collapsed",async()=>{
 const answer=vi.fn();const customForm:QuestionForm={...form,schema:{...form.schema,properties:{color:{...form.schema.properties!.color,"x-allowOther":true}}}};
 await act(async()=>root.render(<QuestionCard form={customForm} onAnswer={answer}/>));
 expect(node.querySelector('.question-custom input')).toBeNull();
 await act(async()=>node.querySelector<HTMLButtonElement>('.question-custom-toggle')!.click());
 const input=node.querySelector<HTMLInputElement>('.question-custom input')!;
 await act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,"value")!.set!.call(input,"Purple");input.dispatchEvent(new Event("input",{bubbles:true}));});
 await act(async()=>node.querySelector<HTMLButtonElement>('.question-custom-toggle')!.click());
 expect(node.querySelector('.question-custom input')).toBeNull();
 await act(async()=>node.querySelector<HTMLButtonElement>('[type="submit"]')!.click());
 expect(answer).toHaveBeenCalledWith({cancelled:false,values:{color:"Purple"}});
});
