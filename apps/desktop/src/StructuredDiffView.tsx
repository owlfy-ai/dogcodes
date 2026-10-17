import { useEffect, useState } from "react";
import type { StructuredDiff } from "./diffView";
import { artifactVisibleText } from "./artifactView";

const changeLabels: Record<StructuredDiff["files"][number]["change"], string> = {
  modified: "修改", added: "新增", deleted: "删除", renamed: "重命名", copied: "复制", binary: "二进制",
};

const visiblePath = (path: string) => artifactVisibleText(path).replace(/\n/g, "\\n").replace(/\t/g, "\\t");
const modeLabels: Record<string, string> = { "100644": "普通文件", "100755": "可执行文件", "120000": "符号链接", "160000": "子模块" };
const modeLabel = (mode: string | null) => mode === null ? "—" : `${mode} ${modeLabels[mode] ?? ""}`;

export function StructuredDiffView({ diff, artifactKey, initialPath }: { diff: StructuredDiff; artifactKey: string; initialPath?: string }) {
  const [selectedIndex, setSelectedIndex] = useState(0);
  useEffect(() => setSelectedIndex(Math.max(0, diff.files.findIndex(file => file.newPath === initialPath || file.oldPath === initialPath))), [artifactKey, initialPath, diff.files]);
  const selected = diff.files[selectedIndex] ?? null;

  return <section className="diff-view" aria-label="结构化 Diff 只读视图">
    <header className="diff-summary">
      <b>{diff.files.length} 个文件{diff.truncated ? "（部分）" : ""}</b><span>文本</span><span className="diff-add">+{diff.additions}</span><span className="diff-delete">−{diff.deletions}</span>
    </header>
    {diff.truncated && <p className="artifact-warning" role="status">结构化展示已截断：{diff.truncationReason}。原始 Artifact 完整且 hash 已校验，可切换原始视图或导出。</p>}
    {diff.files.length === 0 ? <p>该冻结 Patch 没有文件变更。</p> : <div className="diff-layout">
      <nav aria-label="Diff 文件列表">
        {diff.files.map((file, index) => <button type="button" key={`${index}:${file.oldPath}:${file.newPath}`}
          className={index === selectedIndex ? "active" : ""} onClick={() => setSelectedIndex(index)}>
          <span>{visiblePath(file.newPath || file.oldPath)}</span>
          <small>{changeLabels[file.change]} · {file.binary ? "二进制内容" : `+${file.additions} −${file.deletions}`}</small>
        </button>)}
      </nav>
      {selected && <div className="diff-file">
        <header><b>{visiblePath(selected.newPath || selected.oldPath)}</b><span>{changeLabels[selected.change]}</span></header>
        {(selected.oldPath !== selected.newPath || selected.oldMode || selected.newMode || selected.similarity) && <dl>
          {selected.oldPath !== selected.newPath && <><dt>原路径</dt><dd>{visiblePath(selected.oldPath)}</dd><dt>新路径</dt><dd>{visiblePath(selected.newPath)}</dd></>}
          {(selected.oldMode || selected.newMode) && <><dt>模式</dt><dd>{selected.oldMode === selected.newMode ? modeLabel(selected.oldMode) : `${modeLabel(selected.oldMode)} → ${modeLabel(selected.newMode)}`}</dd></>}
          {selected.similarity && <><dt>相似检测</dt><dd>{artifactVisibleText(selected.similarity.startsWith("差异度 ") ? selected.similarity : `相似度 ${selected.similarity}`)}</dd></>}
        </dl>}
        {selected.binary ? <p className="artifact-warning">二进制变更：结构化视图不解释 binary patch 内容，请查看或导出原始证据。</p>
          : selected.hunks.length === 0 ? <p>仅元数据变化，没有文本 hunk。</p>
          : <div className="diff-hunks"><p className="diff-column-labels">左列：原行号 · 右列：新行号</p>{selected.hunks.map((hunk, hunkIndex) => <section key={`${hunk.header}:${hunkIndex}`}>
            <h3>{artifactVisibleText(hunk.header)}</h3>
            {hunk.lines.map((line, lineIndex) => <div className={`diff-line ${line.kind}`} key={lineIndex}>
              <span>{line.oldLine ?? ""}</span><span>{line.newLine ?? ""}</span>
              <code>{line.kind === "addition" ? "+" : line.kind === "deletion" ? "−" : line.kind === "context" ? " " : ""}{artifactVisibleText(line.text)}</code>
            </div>)}
          </section>)}</div>}
      </div>}
    </div>}
  </section>;
}
