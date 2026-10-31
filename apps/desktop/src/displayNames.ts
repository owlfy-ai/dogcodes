import { translate } from "./i18n";

// The core stores these Chinese sentinels as data: the scratch project's name and
// the title of a conversation that has not been named yet. Match them as data and
// translate only when displaying.
export const SCRATCH_PROJECT_NAME = "快速对话";
export const UNTITLED_TASK_TITLE = "新对话";

export function projectDisplayName(name: string): string {
  return name === SCRATCH_PROJECT_NAME ? translate("app.noProject") : name;
}

export function taskDisplayTitle(title: string): string {
  return title === UNTITLED_TASK_TITLE ? translate("app.untitledTask") : title;
}
