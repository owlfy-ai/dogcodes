const spacedPath = /\]\(\s*((?:\/|~\/|\.{1,2}\/|file:|[A-Za-z]:[\\/])[^()<>"'\n]*?)\s*\)/g;

/**
 * CommonMark rejects a bare link destination that contains spaces, so a local
 * path such as `/Users/me/Library/Application Support/a.txt` stays as literal
 * text. Wrap such path-like destinations in `<...>`, the CommonMark form that
 * allows spaces. Callers must pass text outside code spans and fences.
 */
export function spacedLinkPaths(text: string): string {
  return text.replace(spacedPath, (match, path: string) =>
    /\s/.test(path) ? `](<${path}>)` : match);
}
