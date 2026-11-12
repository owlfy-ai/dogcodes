import { translate } from "./i18n";

// Stored sentinels, kept identical to `SCRATCH_PROJECT_NAME` and `UNTITLED_TASK_TITLE`
// in the domain crate. Match them as data and translate only when displaying.
export const SCRATCH_PROJECT_NAME = "dogcodes:scratch";
export const UNTITLED_TASK_TITLE = "dogcodes:untitled";

export function projectDisplayName(name: string): string {
  return name === SCRATCH_PROJECT_NAME ? translate("app.noProject") : name;
}

export function taskDisplayTitle(title: string): string {
  return title === UNTITLED_TASK_TITLE ? translate("app.untitledTask") : title;
}
