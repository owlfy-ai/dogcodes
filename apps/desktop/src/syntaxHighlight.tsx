import type { ReactNode } from "react";
import { common, createLowlight } from "lowlight";

const highlighter = createLowlight(common);
const extensions: Record<string, string> = {
  ino: "arduino", pde: "arduino",
  sh: "bash", bash: "bash", zsh: "bash",
  c: "c", h: "c", cpp: "cpp", cc: "cpp", cxx: "cpp", hpp: "cpp", hh: "cpp", hxx: "cpp",
  cs: "csharp", csx: "csharp", css: "css", diff: "diff", patch: "diff", go: "go",
  graphql: "graphql", gql: "graphql", ini: "ini", cfg: "ini", toml: "ini", properties: "ini",
  java: "java", js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  json: "json", jsonc: "json", kt: "kotlin", kts: "kotlin", less: "less", lua: "lua",
  mk: "makefile", mak: "makefile", md: "markdown", markdown: "markdown", mdx: "markdown",
  m: "objectivec", mm: "objectivec", pl: "perl", pm: "perl", pod: "perl",
  php: "php", php3: "php", php4: "php", php5: "php", php7: "php", php8: "php", phps: "php",
  phtml: "php-template", txt: "plaintext", text: "plaintext",
  py: "python", pyw: "python", pyi: "python", pycon: "python-repl",
  r: "r", rprofile: "r", rb: "ruby", rake: "ruby", gemspec: "ruby", rs: "rust",
  scss: "scss", console: "shell", shellsession: "shell", sql: "sql", swift: "swift",
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  vb: "vbnet", vbs: "vbnet", wat: "wasm", wast: "wasm",
  html: "xml", htm: "xml", xhtml: "xml", xml: "xml", svg: "xml", xsl: "xml", xslt: "xml", vue: "xml",
  yaml: "yaml", yml: "yaml",
};
const filenames: Record<string, string> = {
  makefile: "makefile", gnumakefile: "makefile", "makefile.am": "makefile", "makefile.in": "makefile",
  gemfile: "ruby", rakefile: "ruby", guardfile: "ruby", vagrantfile: "ruby", podfile: "ruby", brewfile: "ruby",
  ".bashrc": "bash", ".bash_profile": "bash", ".bash_login": "bash", ".bash_logout": "bash",
  ".zshrc": "bash", ".zprofile": "bash", ".zshenv": "bash", ".profile": "bash",
  ".rprofile": "r", ".gitconfig": "ini", ".editorconfig": "ini", ".npmrc": "ini",
};

export function languageForPath(path?: string): string | undefined {
  const name = path?.split(/[\\/]/).at(-1)?.toLowerCase() ?? "";
  if (Object.hasOwn(filenames, name)) return filenames[name];
  const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  return Object.hasOwn(extensions, extension) ? extensions[extension] : undefined;
}

interface Token { type: string; value?: string; properties?: { className?: unknown }; children?: Token[] }
export function highlightedLines(text: string, language: string | undefined): ReactNode[][] {
  const lines: ReactNode[][] = [[]];
  const walk = (node: Token, classes = "") => {
    if (node.type === "text") {
      (node.value ?? "").split("\n").forEach((part, index) => {
        if (index) lines.push([]);
        const line = lines[lines.length - 1];
        line.push(classes ? <span className={classes} key={line.length}>{part}</span> : part);
      });
    } else {
      const own = node.properties?.className;
      const next = [classes, Array.isArray(own) ? own.join(" ") : ""].filter(Boolean).join(" ");
      node.children?.forEach(child => walk(child, next));
    }
  };
  try {
    if (language && highlighter.registered(language)) walk(highlighter.highlight(language, text));
    else walk({ type: "text", value: text });
  } catch { return text.split("\n").map(line => [line]); }
  return lines;
}
