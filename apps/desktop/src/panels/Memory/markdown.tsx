/**
 * Lightweight, safe markdown renderer. Produces React elements only — raw
 * HTML in the source is shown as text, never injected. Supports headings,
 * paragraphs, bullet / ordered lists, block quotes, fenced code, inline code,
 * bold, italic and http(s) links.
 */
import type { ReactNode } from "react";

export type Block =
  | { type: "heading"; level: number; text: string }
  | { type: "paragraph"; text: string }
  | { type: "list"; ordered: boolean; items: string[] }
  | { type: "quote"; text: string }
  | { type: "code"; lang: string; text: string };

/** Splits markdown into blocks. */
export function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    const fence = /^\s*(```|~~~)\s*([\w+-]*)\s*$/.exec(line);
    if (fence) {
      const marker = fence[1] ?? "```";
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] ?? "").trim().startsWith(marker)) {
        body.push(lines[i] ?? "");
        i += 1;
      }
      i += 1; // closing fence (or EOF)
      blocks.push({ type: "code", lang: fence[2] ?? "", text: body.join("\n") });
      continue;
    }
    const heading = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      blocks.push({ type: "heading", level: (heading[1] ?? "#").length, text: heading[2] ?? "" });
      i += 1;
      continue;
    }
    if (/^\s*$/.test(line)) {
      i += 1;
      continue;
    }
    const bullet = /^\s*[-*+]\s+(.*)$/;
    const ordered = /^\s*\d+[.)]\s+(.*)$/;
    if (bullet.test(line) || ordered.test(line)) {
      const isOrdered = !bullet.test(line);
      const re = isOrdered ? ordered : bullet;
      const items: string[] = [];
      while (i < lines.length) {
        const m = re.exec(lines[i] ?? "");
        if (!m) break;
        items.push(m[1] ?? "");
        i += 1;
      }
      blocks.push({ type: "list", ordered: isOrdered, items });
      continue;
    }
    if (/^\s*>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i] ?? "")) {
        body.push((lines[i] ?? "").replace(/^\s*>\s?/, ""));
        i += 1;
      }
      blocks.push({ type: "quote", text: body.join(" ") });
      continue;
    }
    const para: string[] = [];
    while (i < lines.length) {
      const l = lines[i] ?? "";
      if (/^\s*$/.test(l) || /^\s*(```|~~~)/.test(l) || /^\s{0,3}#{1,6}\s/.test(l) || /^\s*([-*+]|\d+[.)])\s/.test(l) || /^\s*>/.test(l)) break;
      para.push(l.trim());
      i += 1;
    }
    blocks.push({ type: "paragraph", text: para.join(" ") });
  }
  return blocks;
}

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*)|(_[^_\s][^_]*_)|(\[[^\]]+\]\([^)\s]+\))/g;

/** True for links that may be rendered as anchors. */
export function isSafeUrl(url: string): boolean {
  return /^https?:\/\/[^\s]+$/i.test(url);
}

/** Renders inline markdown to React nodes. */
export function renderInline(text: string, keyPrefix = "i"): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let n = 0;
  for (const m of text.matchAll(INLINE)) {
    const idx = m.index ?? 0;
    if (idx > last) out.push(text.slice(last, idx));
    const tok = m[0];
    const key = `${keyPrefix}-${n++}`;
    if (m[1]) out.push(<code key={key}>{tok.slice(1, -1)}</code>);
    else if (m[2]) out.push(<strong key={key}>{renderInline(tok.slice(2, -2), key)}</strong>);
    else if (m[3] || m[4]) out.push(<em key={key}>{renderInline(tok.slice(1, -1), key)}</em>);
    else {
      const link = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(tok);
      const label = link?.[1] ?? tok;
      const url = link?.[2] ?? "";
      out.push(
        isSafeUrl(url) ? (
          <a key={key} href={url} target="_blank" rel="noreferrer noopener">
            {label}
          </a>
        ) : (
          <span key={key} title={url}>
            {label}
          </span>
        ),
      );
    }
    last = idx + tok.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Safe markdown view. */
export function Markdown({ source, className }: { source: string; className?: string }) {
  const blocks = parseBlocks(source);
  return (
    <div className={className ?? "md"}>
      {blocks.map((b, i) => {
        const key = `b${i}`;
        switch (b.type) {
          case "heading": {
            const level = Math.min(6, b.level + 2);
            const Tag = `h${level}` as "h3" | "h4" | "h5" | "h6";
            return <Tag key={key}>{renderInline(b.text, key)}</Tag>;
          }
          case "paragraph":
            return <p key={key}>{renderInline(b.text, key)}</p>;
          case "list": {
            const items = b.items.map((it, j) => <li key={`${key}-${j}`}>{renderInline(it, `${key}-${j}`)}</li>);
            return b.ordered ? <ol key={key}>{items}</ol> : <ul key={key}>{items}</ul>;
          }
          case "quote":
            return <blockquote key={key}>{renderInline(b.text, key)}</blockquote>;
          case "code":
            return (
              <pre key={key} data-lang={b.lang || undefined}>
                <code>{b.text}</code>
              </pre>
            );
          default:
            return null;
        }
      })}
    </div>
  );
}

/** Renders an FTS snippet whose matches are wrapped in U+0002 … U+0003 as `<mark>`. */
export function Highlighted({ text }: { text: string }) {
  return <>{splitHighlights(text).map((s, i) => (s.mark ? <mark key={i}>{s.text}</mark> : <span key={i}>{s.text}</span>))}</>;
}

/** Splits a snippet into plain / marked segments. */
export function splitHighlights(text: string): { text: string; mark: boolean }[] {
  const out: { text: string; mark: boolean }[] = [];
  let mark = false;
  let buf = "";
  for (const ch of text) {
    if (ch === "\u0002" || ch === "\u0003") {
      if (buf) out.push({ text: buf, mark });
      buf = "";
      mark = ch === "\u0002";
      continue;
    }
    buf += ch;
  }
  if (buf) out.push({ text: buf, mark });
  return out;
}
