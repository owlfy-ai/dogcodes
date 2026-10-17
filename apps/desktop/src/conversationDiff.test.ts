import { expect, it } from "vitest";
import { frozenDiff } from "./conversationDiff";
it("keeps same-name files in separate project roots distinct",()=>{
 const result=frozenDiff({patch:"",files:[{path:"/project a/same.txt",before:{hex:"610a"},after:{hex:"620a"}},{path:"/project b/same.txt",before:{hex:"610a"},after:{hex:"630a"}}]});
 expect(result.kind).toBe("structured");
 if(result.kind==="structured")expect(result.diff.files.map(f=>f.newPath)).toEqual(["/project a/same.txt","/project b/same.txt"]);
});
