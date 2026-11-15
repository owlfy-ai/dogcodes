import { expect, it } from "vitest";
import { frozenDiff } from "./conversationDiff";
it("keeps same-name files in separate project roots distinct",()=>{
 const result=frozenDiff({patch:"",files:[{path:"/project a/same.txt",before:{hex:"610a"},after:{hex:"620a"}},{path:"/project b/same.txt",before:{hex:"610a"},after:{hex:"630a"}}]});
 expect(result.kind).toBe("structured");
 if(result.kind==="structured")expect(result.diff.files.map(f=>f.newPath)).toEqual(["/project a/same.txt","/project b/same.txt"]);
});
it("keeps omitted files in the list and does not let one oversized diff hide other files", () => {
  const hex = (text: string) => ({ hex: Buffer.from(text).toString("hex") });
  const result = frozenDiff({ files: [
    { path: "large.ts", before: null, after: hex("one\n".repeat(10_001)), additions: 10_001, deletions: 0 },
    { path: "small.ts", before: hex("old\n"), after: hex("new\n") },
    { path: "omitted.ts", before: { contentOmitted: true, omissionReason: "file_limit" }, after: null },
  ] });
  expect(result.kind).toBe("structured");
  if (result.kind !== "structured") return;
  expect(result.diff.files.map(f => f.newPath)).toEqual(["large.ts", "small.ts", "omitted.ts"]);
  expect(result.diff.files[0].unavailableReason).toBeTruthy();
  expect(result.diff.files[0].additions).toBe(10_001);
  expect(result.diff.files[1].hunks).toHaveLength(1);
  expect(result.diff.files[2].binary).toBe(false);
  expect(result.diff.files[2].countsUnavailable).toBe(true);
});
