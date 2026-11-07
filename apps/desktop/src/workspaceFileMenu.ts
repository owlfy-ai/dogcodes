import { createContext } from "react";
import { invoke } from "@tauri-apps/api/core";
import { copyPendingText, copyText } from "./clipboard";
import { replaceContextMenu, type ContextMenuEntry } from "./ContextMenu";
import { translate as t } from "./i18n";

/** Whether the conversation on screen runs on another machine. */
export const RemoteConversationContext = createContext(false);

interface Applications { applications: { id: string; name: string; isDefault: boolean }[]; error: string | null }

export function joinWorkspacePath(root: string, path: string) {
  if (!path) return root;
  const separator = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${separator}${separator === "\\" ? path.replaceAll("/", "\\") : path}`;
}

/** macOS localizes Finder's name by the system language, not the app's. */
export function revealLabelKey() {
  const name = /Mac/.test(navigator.platform)
    ? /^zh(?:-CN|-SG|-Hans|$)/i.test(navigator.language) ? "finderLocalized" : "finder"
    : /Win/.test(navigator.platform) ? "explorer" : "fileManager";
  return `openFile.reveal.${name}`;
}

async function chooseApplication(taskId: string, path: string) {
  const { applications, error } = await invoke<Applications>("core_workspace_file_applications", { taskId, path });
  if (error) throw new Error(error);
  replaceContextMenu(applications.length
    ? applications.map(app => ({
      label: app.name, hint: app.isDefault ? t("modelPicker.default") : undefined,
      onSelect: () => invoke("core_open_workspace_file", { taskId, path, applicationId: app.id }),
    }))
    : [{ label: t("openFile.noApps"), disabled: true, onSelect: () => {} }]);
}

/** A file or folder in the file tree. `path` is relative to its root, and empty for the
 * root itself. What opens on this computer isn't offered for a server's project. */
export function workspaceEntryMenu({ taskId, root, path, directory, remote }: { taskId: string; root: string; path: string; directory: boolean; remote: boolean }): ContextMenuEntry[] {
  const absolute = joinWorkspacePath(root, path);
  return [
    !remote && !directory && { label: t("contextMenu.open"), onSelect: () => invoke("core_open_workspace_file", { taskId, path: absolute }) },
    !remote && !directory && { label: t("contextMenu.openWith"), onSelect: () => chooseApplication(taskId, absolute) },
    !remote && { label: t(revealLabelKey()), onSelect: () => invoke(directory ? "core_reveal_workspace_directory" : "core_reveal_workspace_file", { taskId, path: absolute }) },
    "separator",
    { label: t("openFile.copyPath"), onSelect: () => copyText(absolute) },
    !!path && { label: t("contextMenu.copyRelativePath"), onSelect: () => copyText(path) },
  ];
}

/** A file a message mentions. Its path is as the agent reported it, so it may be relative
 * and only the task's machine can say where it is; a deleted file can no longer be found. */
export function messageFileMenu({ taskId, path, remote, deleted = false }: { taskId: string; path: string; remote: boolean; deleted?: boolean }): ContextMenuEntry[] {
  const local = !remote && !deleted;
  const absolute = /^(?:[\\/]|[A-Za-z]:[\\/])/.test(path);
  return [
    local && { label: t("contextMenu.open"), onSelect: () => invoke("core_open_workspace_file", { taskId, path }) },
    local && { label: t("contextMenu.openWith"), onSelect: () => chooseApplication(taskId, path) },
    local && { label: t(revealLabelKey()), onSelect: () => invoke("core_reveal_workspace_file", { taskId, path }) },
    "separator",
    absolute && { label: t("openFile.copyPath"), onSelect: () => copyText(path) },
    !absolute && !deleted && { label: t("openFile.copyPath"), onSelect: () => copyPendingText(invoke<string>("core_workspace_file_path", { taskId, path })) },
    !absolute && { label: t("contextMenu.copyRelativePath"), onSelect: () => copyText(path) },
  ];
}
