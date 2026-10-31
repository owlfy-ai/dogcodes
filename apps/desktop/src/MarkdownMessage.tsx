import { LoadingStatus } from "./LoadingStatus";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { mathSyntax } from "./mathSyntax";
import { StreamingMarkdown } from "./streamingMarkdown";
import { useSmoothText } from "./useSmoothText";
import {
  Children,
  createContext,
  useContext,
  isValidElement,
  memo,
  useMemo,
  useEffect,
  useId,
  useState,
  useRef,
  type ReactNode,
} from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import katex from "katex";
import rehypeHighlight from "rehype-highlight";
import "katex/dist/katex.min.css";
import "./conversation.css";
import { translate, useI18n } from "./i18n";

function textOf(node: ReactNode): string {
  return Children.toArray(node)
    .map((n) =>
      typeof n === "string" || typeof n === "number"
        ? String(n)
        : isValidElement<{ children?: ReactNode }>(n)
          ? textOf(n.props.children)
          : "",
    )
    .join("");
}
function Copy({ text }: { text: string }) {
  const { t } = useI18n();
  const [status, setStatus] = useState<"" | "incomplete" | "copied" | "failed">("");
  useEffect(() => {
    if (!status) return;
    const timer = setTimeout(() => setStatus(""), 2000);
    return () => clearTimeout(timer);
  }, [status]);
  return (
    <button
      type="button"
      onClick={() => {
        if (!navigator.clipboard) { setStatus("incomplete"); return; }
        void navigator.clipboard.writeText(text).then(
          () => setStatus("copied"),
          () => setStatus("failed"),
        );
      }}
      aria-label={status ? t(`markdown.copy.${status}`) : t("markdown.copySource")}
    >
      {status ? t(`markdown.copy.${status}`) : t("common.copy")}
    </button>
  );
}
function MathExpression({
  source,
  display = false,
}: {
  source: string;
  display?: boolean;
}) {
  const html = useMemo(
    () =>
      katex.renderToString(source, {
        displayMode: display,
        throwOnError: false,
        trust: false,
        strict: "ignore",
        maxExpand: 1000,
      }),
    [source, display],
  );
  return (
    <span
      className={display ? "math-block" : "math-inline"}
      data-tooltip={source}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
let diagramQueue = Promise.resolve();
function Diagram({ source, pending }: { source: string; pending: boolean }) {
  const { t } = useI18n();
  const id = useId().replace(/[^a-zA-Z0-9]/g, "");
  const [svg, setSvg] = useState("");
  const [error, setError] = useState("");
  const [code, setCode] = useState(false);
  const [large, setLarge] = useState(false);
  const [zoom, setZoom] = useState(1);
  const currentTheme = () =>
    document.documentElement.dataset.theme ||
    (window.matchMedia?.("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light");
  const [theme, setTheme] = useState(currentTheme);
  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(currentTheme()));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "class", "style"],
    });
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const changed = () => setTheme(currentTheme());
    media?.addEventListener("change", changed);
    return () => {
      observer.disconnect();
      media?.removeEventListener("change", changed);
    };
  }, []);
  useEffect(() => {
    let active = true;
    setSvg("");
    setError("");
    if (pending) return;
    if (source.length > 30000) {
      setError("tooLarge");
      return;
    }
    diagramQueue = diagramQueue
      .catch(() => {})
      .then(async () => {
        if (!active) return;
        try {
          const { default: mermaid } = await import("mermaid");
          mermaid.initialize({
            startOnLoad: false,
            securityLevel: "strict",
            suppressErrorRendering: true,
            maxTextSize: 30000,
            theme: theme === "dark" ? "dark" : "default",
          });
          const result = await mermaid.render(`diagram${id}`, source);
          if (active) setSvg(result.svg);
        } catch {
          if (active) setError("renderFailed");
        }
      });
    return () => {
      active = false;
    };
  }, [source, pending, id, theme]);
  return (
    <section
      className={`diagram-block ${large ? "expanded" : ""}`}
      aria-label={t("markdown.diagram")}
    >
      <header>
        <span>Mermaid</span>
        <button type="button" onClick={() => setCode(!code)}>
          {t(code ? "markdown.chart" : "markdown.source")}
        </button>
        <Copy text={source} />
        <button type="button" onClick={() => setLarge(!large)}>
          {t(large ? "markdown.collapse" : "markdown.expand")}
        </button>
        {large && (
          <>
            <button
              type="button"
              aria-label={t("markdown.zoomOut")}
              onClick={() => setZoom((z) => Math.max(0.5, z - 0.25))}
            >
              −
            </button>
            <button
              type="button"
              aria-label={t("markdown.zoomIn")}
              onClick={() => setZoom((z) => Math.min(3, z + 0.25))}
            >
              ＋
            </button>
          </>
        )}
      </header>
      {pending && <LoadingStatus>{t("markdown.generating")}</LoadingStatus>}
      {error && <small role="status">{t(`markdown.error.${error}`)}</small>}
      {code || error || pending ? (
        <pre>
          <code>{source}</code>
        </pre>
      ) : svg ? (
        <div className="diagram-scroll">
          <div
            role="img"
            aria-label={t("markdown.diagramImage")}
            style={{ width: `${zoom * 100}%` }}
            dangerouslySetInnerHTML={{ __html: svg }}
          />
        </div>
      ) : (
        <LoadingStatus>{t("markdown.drawing")}</LoadingStatus>
      )}
    </section>
  );
}
function CodeBlock({
  children,
  pending,
}: {
  children: ReactNode;
  pending: boolean;
}) {
  const { t } = useI18n();
  const child = Children.toArray(children)[0];
  const language = isValidElement<{ className?: string }>(child)
    ? /language-([^\s]+)/.exec(child.props.className ?? "")?.[1]
    : undefined;
  const source = textOf(children).replace(/\n$/, "");
  if (language === "math")
    return pending ? (
      <pre className="pending-math">{source}</pre>
    ) : (
      <MathExpression source={source} display />
    );
  if (language === "mermaid")
    return <Diagram source={source} pending={pending} />;
  return (
    <section className="code-block">
      <header>
        <span>{language || t("markdown.code")}</span>
        <Copy text={source} />
      </header>
      <pre>{children}</pre>
    </section>
  );
}
function WebsiteLink({ href, children }: { href?: string; children: ReactNode }) {
  const { t } = useI18n();
  const [error, setError] = useState(false);
  return <><a href={href} target="_blank" rel="noopener noreferrer" onClick={event => {
    if (!href || !/^https?:\/\//i.test(href) || !isTauri()) return;
    event.preventDefault();
    setError(false);
    void invoke("open_website", { url: href }).catch(() => setError(true));
  }}>{children}</a>{error && <small role="alert">{t("markdown.browserFailed")}</small>}</>;
}
const SourceContext = createContext({ text: "", streaming: false });
const messageComponents: Components = {
  code: ({ children, className }) =>
    className?.includes("math-inline") ? (
      <MathExpression source={textOf(children)} />
    ) : (
      <code className={className}>{children}</code>
    ),
  pre: function MarkdownPre({ children, node }) {
    const { text, streaming } = useContext(SourceContext);
    const end = node?.position?.end.offset ?? 0;
    const source = text.slice(node?.position?.start.offset ?? 0, end);
    const closed = source.trimStart().startsWith("$$")
      ? /\n\s*\$\$\s*$/.test(source)
      : /\n\s*(`{3,}|~{3,})\s*$/.test(source);
    const pending = streaming && end >= text.trimEnd().length && !closed;
    return <CodeBlock pending={pending}>{children}</CodeBlock>;
  },
  table: ({ children }) => (
    <div className="markdown-table">
      <table>{children}</table>
    </div>
  ),
  a: ({ children, href }) => (
    <WebsiteLink href={href}>{children}</WebsiteLink>
  ),
  img: ({ src, alt }) => (
    <WebsiteLink href={src}>
      <img src={src} alt={alt ?? translate("markdown.image")} loading="lazy" />
    </WebsiteLink>
  ),
};
const remarkPlugins = [remarkGfm, remarkMath];
const highlightedPlugins: NonNullable<Parameters<typeof Markdown>[0]["rehypePlugins"]> = [
  [rehypeHighlight, { detect: false, ignoreMissing: true }],
];
const noPlugins: typeof highlightedPlugins = [];
const MarkdownBlock = memo(function MarkdownBlock({ text, pending }: { text: string; pending: boolean }) {
  const source = useMemo(() => ({ text, streaming: pending }), [text, pending]);
  // Do not repeatedly tokenize a growing code block. Closed, stable blocks
  // retain their own provider and memoized Markdown tree while the tail grows.
  const openFence = pending && /^ {0,3}(`{3,}|~{3,})[^\n]*\n/.test(text)
    && !/\n {0,3}(`{3,}|~{3,})\s*$/.test(text.trimEnd());
  return <SourceContext.Provider value={source}>
    <Markdown skipHtml remarkPlugins={remarkPlugins}
      rehypePlugins={openFence ? noPlugins : highlightedPlugins} components={messageComponents}>
      {text}
    </Markdown>
  </SourceContext.Provider>;
});

export const MarkdownMessage = memo(function MarkdownMessage({
  text,
  streaming = false,
}: {
  text: string;
  streaming?: boolean;
}) {
  const normalized = useMemo(() => mathSyntax(text), [text]);
  const segmenter = useRef(new StreamingMarkdown());
  const blocks = useMemo(() => segmenter.current.update(normalized, streaming), [normalized, streaming]);
  return (
    <div className="markdown-body">
      {blocks.map(block => <MarkdownBlock key={block.offset} text={block.text} pending={block.pending} />)}
    </div>
  );
});

export const StreamingMarkdownMessage = memo(function StreamingMarkdownMessage({
  text, streaming = false,
}: { text: string; streaming?: boolean }) {
  const visible = useSmoothText(text, streaming);
  return <MarkdownMessage text={visible} streaming={streaming} />;
});
