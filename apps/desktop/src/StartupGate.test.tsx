import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { StartupRecovery } from "./StartupGate";

describe("startup recovery", () => {
  it("explains preservation and does not offer a composer or destructive repair", () => {
    const html = renderToStaticMarkup(<StartupRecovery status="storage_recovery_required" />);
    expect(html).toContain("为保护已有记录");
    expect(html).toContain("app-v5.db-wal");
    expect(html).toContain("不会自动修复数据或重发指令");
    expect(html).not.toContain("<textarea");
    expect(html).toContain("<details>");
    expect(html).not.toContain("<details open");
  });
  it("distinguishes startup transport failure from a diagnosed database failure", () => {
    const html = renderToStaticMarkup(<StartupRecovery status="unavailable" />);
    expect(html).toContain("暂时无法打开对话");
    expect(html).not.toContain("未通过完整性");
  });
});


it("keeps service disconnection separate from database recovery", () => {
  const html = renderToStaticMarkup(<StartupRecovery status="service_unavailable" />);
  expect(html).toContain("暂时无法打开对话");
  expect(html).toContain("正在进行的任务可能仍在继续");
  expect(html).not.toContain("完整备份应用数据目录");
  expect(html).not.toContain("未启动 Coding Agent");
});
