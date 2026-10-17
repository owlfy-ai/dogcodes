import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { ApprovalDetails } from "./ApprovalDetails";

it("keeps the tail of long approval details and escapes markup", () => {
  const summary = `${"x".repeat(1000)} END_OF_PATCH <script>alert(1)</script>`;
  const html = renderToStaticMarkup(<ApprovalDetails summary={summary} />);
  expect(html).toContain("END_OF_PATCH");
  expect(html).toContain("&lt;script&gt;");
  expect(html).not.toContain("<script>");
});

it("exposes complete structured details and warns when scope cannot be verified", () => {
  const summary = JSON.stringify({ scope: "reported_files", files: [{ path: "sample.txt", diff: "before\nafter" }] });
  const html = renderToStaticMarkup(<ApprovalDetails summary={summary} />);
  expect(html).toContain("sample.txt");
  expect(html).toContain("before\nafter");
  expect(html).not.toContain("reported_files");
  expect(html).not.toContain('role="alert"');
  for (const value of ["", '{"scope":"unknown"}']) {
    expect(renderToStaticMarkup(<ApprovalDetails summary={value} />)).toContain('role="alert"');
  }
});
