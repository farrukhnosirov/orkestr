/**
 * A small, safe Markdown renderer for extension READMEs (untrusted text).
 *
 * Output is built from React elements only — no `dangerouslySetInnerHTML`,
 * so raw HTML in the README is shown as plain text (tags stripped), scripts
 * never run. Images are not loaded (the CSP allows no remote images, and
 * README images are tracking-capable): their alt text is shown instead.
 * Links are not navigable inside Orkestr; their target is shown on hover.
 */
import type { ReactNode } from "react";

export type MdBlock =
  | { type: "heading"; level: number; text: string }
  | { type: "paragraph"; text: string }
  | { type: "code"; lang: string; text: string }
  | { type: "list"; ordered: boolean; items: string[] }
  | { type: "quote"; text: string }
  | { type: "rule" };

/** Strips HTML tags (keeps their text) and decodes a few entities. */
export function stripHtml(s: string): string {
  return s
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<(script|style)\b[\s\S]*?<\/\1\s*>/gi, "")
    .replace(/<img\b[^>]*\balt\s*=\s*["']([^"']*)["'][^>]*>/gi, "$1")
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");
}

/** Parses Markdown into blocks (CommonMark subset). */
export function parseMarkdown(src: string): MdBlock[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const out: MdBlock[] = [];
  let para: string[] = [];
  const flush = () => {
    const text = stripHtml(para.join(" ")).trim();
    if (text) out.push({ type: "paragraph", text });
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    const fence = /^\s*(```|~~~)\s*([\w+-]*)/.exec(line);
    if (fence) {
      flush();
      const close = fence[1] ?? "```";
      const body: string[] = [];
      i++;
      while (i < lines.length && !(lines[i] ?? "").trim().startsWith(close)) body.push(lines[i++] ?? "");
      out.push({ type: "code", lang: fence[2] ?? "", text: body.join("\n") });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      flush();
      const text = stripHtml(heading[2] ?? "").trim();
      if (text) out.push({ type: "heading", level: (heading[1] ?? "#").length, text });
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      out.push({ type: "rule" });
      continue;
    }
    const item = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      flush();
      const ordered = /\d/.test(item[1] ?? "");
      const items = [stripHtml(item[2] ?? "")];
      while (i + 1 < lines.length) {
        const next = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i + 1] ?? "");
        if (!next) break;
        items.push(stripHtml(next[2] ?? ""));
        i++;
      }
      out.push({ type: "list", ordered, items });
      continue;
    }
    const quote = /^\s*>\s?(.*)$/.exec(line);
    if (quote) {
      flush();
      out.push({ type: "quote", text: stripHtml(quote[1] ?? "") });
      continue;
    }
    if (line.trim() === "") {
      flush();
      continue;
    }
    para.push(line.trim());
  }
  flush();
  return out;
}

/** Inline Markdown → React nodes (code, bold, italic, links as text, images as alt text). */
export function renderInline(text: string, keyPrefix = "i"): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`]+`)|(!\[([^\]]*)\]\([^)]*\))|(\[([^\]]+)\]\(([^)\s]+)[^)]*\))|(\*\*([^*]+)\*\*)|(__([^_]+)__)|(\*([^*]+)\*)|(_([^_]+)_)/g;
  let last = 0;
  let n = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const key = `${keyPrefix}${n++}`;
    if (m[1]) out.push(<code key={key}>{m[1].slice(1, -1)}</code>);
    else if (m[2] !== undefined) {
      if (m[3]) out.push(<span key={key} className="ext-md__img">{m[3]}</span>);
    } else if (m[4]) out.push(<span key={key} className="ext-md__link" title={m[6]}>{m[5]}</span>);
    else if (m[8] ?? m[10]) out.push(<strong key={key}>{m[8] ?? m[10]}</strong>);
    else if (m[12] ?? m[14]) out.push(<em key={key}>{m[12] ?? m[14]}</em>);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Renders untrusted Markdown safely. */
export function SafeMarkdown({ source, maxBlocks = 400 }: { source: string; maxBlocks?: number }) {
  const blocks = parseMarkdown(source).slice(0, maxBlocks);
  return (
    <div className="ext-md">
      {blocks.map((b, i) => {
        const k = `b${i}`;
        switch (b.type) {
          case "heading": {
            const level = Math.min(6, b.level + 1);
            const Tag = `h${level}` as "h2";
            return <Tag key={k}>{renderInline(b.text, k)}</Tag>;
          }
          case "paragraph":
            return <p key={k}>{renderInline(b.text, k)}</p>;
          case "code":
            return (
              <pre key={k}>
                <code>{b.text}</code>
              </pre>
            );
          case "list": {
            const items = b.items.map((it, j) => <li key={`${k}-${j}`}>{renderInline(it, `${k}-${j}`)}</li>);
            return b.ordered ? <ol key={k}>{items}</ol> : <ul key={k}>{items}</ul>;
          }
          case "quote":
            return <blockquote key={k}>{renderInline(b.text, k)}</blockquote>;
          case "rule":
            return <hr key={k} />;
        }
      })}
    </div>
  );
}
