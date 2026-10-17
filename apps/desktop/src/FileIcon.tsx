import { languageForPath } from "./syntaxHighlight";
import "./fileIcon.css";

type Icon = { kind: string; label: string; color: string; text?: string };
const languageIcons: Record<string, [string, string]> = {
  arduino: ["∞", "teal"], bash: [">_", "green"], c: ["C", "blue"], cpp: ["C+", "blue"],
  csharp: ["C#", "purple"], css: ["#", "blue"], diff: ["±", "green"], go: ["Go", "teal"],
  graphql: ["G", "pink"], ini: ["", "gray"], java: ["J", "orange"], javascript: ["JS", "yellow"],
  json: ["{}", "yellow"], kotlin: ["K", "purple"], less: ["#", "blue"], lua: ["Lu", "blue"],
  makefile: ["", "gray"], markdown: ["MD", "blue"], "objectivec": ["Oc", "blue"],
  perl: ["Pl", "purple"], php: ["php", "purple"], "php-template": ["php", "purple"],
  plaintext: ["", "gray"], python: ["Py", "yellow"], "python-repl": ["Py", "yellow"],
  r: ["R", "blue"], ruby: ["◆", "red"], rust: ["Rs", "orange"], scss: ["#", "pink"],
  shell: [">_", "green"], sql: ["", "yellow"], swift: ["Sw", "orange"], typescript: ["TS", "blue"],
  vbnet: ["VB", "purple"], wasm: ["W", "purple"], xml: ["<>", "orange"], yaml: ["Y", "purple"],
};

export function fileIconForPath(path: string): Icon {
  const name = path.split(/[\\/]/).at(-1)?.toLowerCase() ?? "";
  const ext = name.split(".").at(-1) ?? "";
  if (["tsx", "jsx"].includes(ext)) return { kind: "react", label: "React", color: "blue" };
  if (ext === "vue") return { kind: "label", label: "Vue", color: "green", text: "V" };
  if (ext === "svelte") return { kind: "label", label: "Svelte", color: "orange", text: "S" };
  if (ext === "astro") return { kind: "label", label: "Astro", color: "purple", text: "A" };
  if (/^(dockerfile|compose\.ya?ml|docker-compose\.ya?ml)(\..*)?$/.test(name)) return { kind: "label", label: "Docker", color: "blue", text: "D" };
  if (/^\.git(ignore|attributes|modules|config)$/.test(name)) return { kind: "git", label: "Git", color: "orange" };
  if (name === ".env" || name.startsWith(".env.") || [".editorconfig", ".npmrc", ".prettierrc", ".eslintrc"].includes(name)) return { kind: "config", label: "配置", color: "gray" };
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif", "heic", "tiff"].includes(ext)) return { kind: "image", label: "图片", color: "purple" };
  if (["mp3", "wav", "flac", "ogg", "m4a", "aac"].includes(ext)) return { kind: "audio", label: "音频", color: "purple" };
  if (["mp4", "mov", "webm", "avi", "mkv"].includes(ext)) return { kind: "video", label: "视频", color: "pink" };
  if (["zip", "gz", "tar", "bz2", "xz", "7z", "rar"].includes(ext)) return { kind: "archive", label: "压缩包", color: "orange" };
  if (["woff", "woff2", "ttf", "otf"].includes(ext)) return { kind: "label", label: "字体", color: "red", text: "Aa" };
  if (["db", "sqlite", "sqlite3"].includes(ext)) return { kind: "database", label: "数据库", color: "yellow" };
  if (ext === "pdf") return { kind: "label", label: "PDF", color: "red", text: "PDF" };
  if (ext === "lock" || ["package-lock.json", "pnpm-lock.yaml", "bun.lockb"].includes(name)) return { kind: "lock", label: "锁文件", color: "gray" };
  const language = languageForPath(path);
  if (language && languageIcons[language]) {
    const [text, color] = languageIcons[language];
    return { kind: language === "sql" ? "database" : ["ini", "makefile"].includes(language) ? "config" : text ? "label" : "file", label: language, color, text };
  }
  return { kind: "file", label: "文件", color: "gray" };
}

const shapes: Record<string, string> = {
  file: "M4 2h7l3 3v11H4zM11 2v4h3M6 9h6M6 12h5",
  config: "M7 2h4l.5 2 2 .9 1.8-.5 1.5 2.6-1.4 1.5v2l1.4 1.5-1.5 2.6-1.8-.5-2 .9-.5 2H7l-.5-2-2-.9-1.8.5L1.2 12l1.4-1.5v-2L1.2 7l1.5-2.6 1.8.5 2-.9Z M12 9a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
  image: "M2 3h14v12H2zM2 13l4-4 3 3 3-4 4 5M7 6h.01",
  audio: "M7 13V4l8-2v9M7 4v3l8-2M7 13c0 3-5 3-5 1s5-3 5-1m8-2c0 3-5 3-5 1s5-3 5-1",
  video: "M2 4h10v10H2zM12 7l4-3v10l-4-3",
  archive: "M3 2h12v14H3zM8 2v7h3v4H8v-3M8 4h2m-2 2h2",
  lock: "M4 8h10v8H4zM6 8V5a3 3 0 0 1 6 0v3M9 11v2",
  database: "M3 5c0-4 12-4 12 0s-12 4-12 0Zm0 0v8c0 4 12 4 12 0V5M3 9c0 4 12 4 12 0",
  git: "m9 1 8 8-8 8-8-8ZM6 5l6 6M6 5v7M5 5h2M5 12h2M11 11h2",
};
export function FileIcon({ path }: { path: string }) {
  const icon = fileIconForPath(path);
  return <span className={`file-type-icon file-type-${icon.color}`} data-tooltip={icon.label} aria-hidden="true" data-file-type={icon.kind === "label" ? icon.label : icon.kind}>
    {icon.kind === "label" ? icon.text : <svg viewBox="0 0 18 18" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">{icon.kind === "react" ? <><ellipse cx="9" cy="9" rx="8" ry="3" /><ellipse cx="9" cy="9" rx="8" ry="3" transform="rotate(60 9 9)" /><ellipse cx="9" cy="9" rx="8" ry="3" transform="rotate(120 9 9)" /><circle cx="9" cy="9" r="1.4" fill="currentColor" stroke="none" /></> : <path d={shapes[icon.kind]} />}</svg>}
  </span>;
}
