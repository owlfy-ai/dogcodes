import { LoadingStatus } from "./LoadingStatus";
import { useCallback, useEffect, useState } from "react";
import type { CoreClient, RecoveryCode, RecoveryItem, RecoverySnapshot, TaskId } from "@dogcodes/protocol";

export const recoveryMessages: Record<RecoveryCode, [string, string]> = {
  installation_unconfigured: ["需要登录或配置", "请在 Agent 官方客户端完成登录，然后重新启动 dog.codes 检测。"],
  installation_broken: ["无法连接 Agent", "请检查官方客户端是否正常启动，以及本机网络与代理配置。"],
  installation_unsupported: ["版本不兼容", "当前安装版本未通过兼容性检查。请使用已支持版本。"],
  installation_not_ready: ["暂时无法使用", "请确认客户端已安装并完成登录，再到「Agent 管理」重新检测。"],
  driver_unavailable: ["正在恢复连接", "暂时无法连接已安装的 Agent，请稍后刷新。"],
  run_ownership_unknown: ["执行连接已中断", "后台服务恢复后会释放旧轮次占用，可返回会话直接发送消息继续。"],
  run_interrupting: ["正在等待停止", "已发送停止请求，正在等待 Agent 确认。请勿重复提交任务。"],
  approval_pending: ["等待你的审批", "返回会话查看完整工具请求并批准或拒绝，也可以停止本次运行。"],
  session_unavailable: ["对话连接需要恢复", "可直接返回会话发送消息，应用会自动重建执行连接并提供已有历史。"],
};

export function RecoveryCenter({ client, onBack, onOpenTask }: {
  client: CoreClient; onBack: () => void; onOpenTask: (taskId: TaskId) => void;
}) {
  const [snapshot, setSnapshot] = useState<RecoverySnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const refresh = useCallback(async () => {
    setBusy(true);
    try { setSnapshot(await client.recoverySnapshot()); setNotice(""); }
    catch { setNotice("无法读取恢复信息，请稍后重试。"); }
    finally { setBusy(false); }
  }, [client]);
  useEffect(() => { void refresh(); }, [refresh]);
  const cancel = async (item: RecoveryItem) => {
    setBusy(true);
    try {
      const current = await client.bootstrap();
      const task = current.tasks.find((task) => task.taskId === item.taskId);
      if (!task || task.activePrimaryRun?.runId !== item.runId || task.streamVersion !== item.expectedStreamVersion) {
        setNotice("运行状态已变化，请刷新后重新检查。"); return;
      }
      await client.cancelTurn(task);
      await refresh();
    } catch { setNotice("停止请求未被接受，请刷新后查看当前运行状态。"); }
    finally { setBusy(false); }
  };
  const rebuild = async (item: RecoveryItem) => {
    if (!item.taskId || item.expectedStreamVersion == null || !client.prepareSessionRebuild) return;
    setBusy(true);
    try { await client.prepareSessionRebuild(item.taskId, item.expectedStreamVersion); await refresh(); setNotice("已准备重建。返回会话发送下一条消息时，会建立新会话并附带历史，不会自动重发旧指令。"); }
    catch { setNotice("无法准备重建，运行或会话状态可能已变化，请刷新后重试。"); }
    finally {setBusy(false);}
  };
  const exportSummary = async () => {
    setBusy(true);
    try { setNotice(await client.exportDiagnostics() === "saved" ? "诊断摘要已保存。" : "已取消导出。"); }
    catch { setNotice("导出未完成。请在桌面应用中选择可写目录和新文件名。"); }
    finally { setBusy(false); }
  };
  return <section className="workspace quota-center">
    <div className="quota-center-content">
      <button type="button" onClick={onBack}>← 返回对话</button>
      <div className="recovery-page-heading"><h2>帮助与恢复</h2><div className="task-tools">
        <button type="button" disabled={busy} onClick={() => void refresh()}>刷新检查</button>
        <button type="button" disabled={busy || !snapshot} onClick={() => void exportSummary()}>导出诊断摘要</button>
      </div></div>
      <p>诊断摘要仅包含状态码、内部标识和时间，不包含对话、文件路径、工具参数或凭据。</p>
      {notice && <p role="status">{notice}</p>}
      {snapshot && <p className="quota-source">检查时间：{new Date(snapshot.generatedAt).toLocaleString()} · {snapshot.items.length} 项
        {snapshot.truncated && " · 仅检查前 100 个任务和安装，结果不完整"}</p>}
      <div className="quota-panel">
        {snapshot?.items.map((item, index) => <section key={index}>
          <h3>{recoveryMessages[item.code][0]}{item.providerId && ` · ${item.providerId}`}</h3>
          <p>{recoveryMessages[item.code][1]}</p>
          {item.taskId && <p className="quota-source">会话 {item.taskId.slice(-8)}</p>}
          <div className="task-tools">
            {item.taskId && <button type="button" onClick={() => onOpenTask(item.taskId!)}>返回会话处理</button>}
            {item.code === "session_unavailable" && !item.runId && client.prepareSessionRebuild && <button type="button" disabled={busy} onClick={() => void rebuild(item)}>重建会话并保留历史</button>}
            {item.canCancel && <button type="button" disabled={busy} onClick={() => void cancel(item)}>停止运行</button>}
          </div>
        </section>)}
      </div>
      {snapshot?.items.length === 0 && <p>本次检查未发现需要处理的连接、会话或运行状态。</p>}
      {!snapshot && busy && <LoadingStatus layout="panel">正在检查…</LoadingStatus>}
    </div>
  </section>;
}
