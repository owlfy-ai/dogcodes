import type { FileVersion, ReviewChange } from "./reviewClient";

function describe(version: FileVersion | null): string {
  if (!version) return "不存在";
  if (version.kind === "gitlink") return "Git 子模块提交指针";
  const kind = version.kind === "symlink" ? "符号链接" : version.binary ? "二进制文件" : "文本文件";
  return `${kind} · ${version.bytes.toLocaleString()} 字节${version.kind === "file" && version.executable ? " · 可执行" : ""}`;
}

/** These are Git-style type/executable modes, not a claim about full OS ACLs. */
export function ReviewFileMetadata({ change }: { change: ReviewChange }) {
  const sameContent = change.before && change.after && change.before.hash === change.after.hash;
  const symlink = change.before?.kind === "symlink" || change.after?.kind === "symlink";
  const typeChanged = change.before && change.after && change.before.kind !== change.after.kind;
  return <section className="review-file-metadata" aria-label="文件变化信息">
    <strong>{change.kind === "renamed" ? "重命名文件" : change.kind === "added" ? "新增文件" : change.kind === "deleted" ? "删除文件" : "修改文件"}</strong>
    {change.previousPath && <p>{change.previousPath} → {change.path}</p>}
    <dl><dt>比较前</dt><dd>{describe(change.before)}</dd><dt>比较后</dt><dd>{describe(change.after)}</dd></dl>
    {sameContent && <p>{change.previousPath ? "文件内容未变，路径已变化。" : "内容未变，仅文件类型或可执行属性变化。"}</p>}
    {(change.kind === "added" && change.after?.bytes === 0 || change.kind === "deleted" && change.before?.bytes === 0) && <p>空文件的新增或删除没有文本行差异。</p>}
    {symlink && <p>符号链接显示链接目标路径，不读取目标文件内容。</p>}
    {(change.before?.kind === "gitlink" || change.after?.kind === "gitlink") && <>
      <p>{typeChanged ? "此路径在子模块与普通文件/链接之间发生类型变化。" : "这里只比较子模块提交指针。"}暂存或取消暂存仅更新父仓库索引，不修改子模块内部文件；内部未提交修改仍会保留。</p>
      <p>子模块仅展示提交指针与状态；可暂存或取消暂存提交指针。</p>
    </>}
    <small>模式表示文件类型与可执行属性，不包含完整系统权限或 ACL。</small>
  </section>;
}
