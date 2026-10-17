/** Normalize common TeX delimiters only outside Markdown code. */
export function mathSyntax(text: string): string {
  let fence = "";
  return text
    .split("\n")
    .map((line) => {
      const found = /^\s{0,3}(`{3,}|~{3,})/.exec(line)?.[1];
      if (
        found &&
        (!fence || (found[0] === fence[0] && found.length >= fence.length))
      ) {
        fence = fence ? "" : found;
        return line;
      }
      if (fence) return line;
      return line
        .split(/(`+[^`]*`+)/g)
        .map((part) =>
          part.startsWith("`")
            ? part
            : part
                .replace(/\\\[/g, "$$$$")
                .replace(/\\\]/g, "$$$$")
                .replace(/\\\(/g, "$")
                .replace(/\\\)/g, "$")
                .replace(/(?<![\\$])\$(?=\d+(?:[.,]\d+)?(?:\s|$))/g, "\\$"),
        )
        .join("");
    })
    .join("\n");
}
