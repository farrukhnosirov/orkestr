/**
 * Ctrl+P quick open: fuzzy file search via `files.find`, recent tabs first
 * when the query is empty.
 */
import type { FindMatch } from "@orkestr/protocol";
import { Kbd, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { FileSearch, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { fileIcon } from "../../features/files/icons";
import { basename } from "../../features/files/paths";
import { type Segment, segmentsFromPositions, splitPathPositions } from "../../features/files/highlight";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useEditorStore } from "../../store/editor";

function Highlighted({ segments }: { segments: Segment[] }) {
  return (
    <>
      {segments.map((s, i) =>
        s.match ? (
          <mark key={i} className="qo__mark">
            {s.text}
          </mark>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </>
  );
}

export function QuickOpen({ projectId, onOpen }: { projectId: string; onOpen: (path: string) => void }) {
  const { t } = useT("code");
  const open = useEditorStore((s) => s.quickOpen);
  const setOpen = useEditorStore((s) => s.setQuickOpen);
  const tabs = useEditorStore((s) => s.tabs);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FindMatch[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setIndex(0);
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    const timer = window.setTimeout(() => {
      getClient()
        .call("files.find", { projectId, query, maxResults: 60 })
        .then((res) => {
          if (cancelled) return;
          setResults(res.matches);
          setError(null);
          setIndex(0);
        })
        .catch((e: unknown) => !cancelled && setError(describeError(e)))
        .finally(() => !cancelled && setLoading(false));
    }, 70);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [open, query, projectId]);

  const items = useMemo(() => {
    if (query.trim()) return results;
    const recent: FindMatch[] = [...tabs].reverse().map((t) => ({ path: t.path, score: 0, positions: [] }));
    const seen = new Set(recent.map((r) => r.path));
    return [...recent, ...results.filter((r) => !seen.has(r.path))];
  }, [query, results, tabs]);

  useEffect(() => {
    listRef.current?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!open) return null;

  const choose = (path: string | undefined) => {
    if (!path) return;
    setOpen(false);
    onOpen(path);
  };

  return (
    <div className="overlay overlay--top" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <div className="palette qo" role="dialog" aria-modal="true" aria-label={t("quickOpen.label")}>
        <div className="palette__search">
          <Search size={18} />
          <input
            ref={inputRef}
            className="palette__input"
            placeholder={t("quickOpen.placeholder")}
            value={query}
            role="combobox"
            aria-expanded="true"
            aria-controls="qo-list"
            aria-activedescendant={items[index] ? `qo-item-${index}` : undefined}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setOpen(false);
              } else if (e.key === "ArrowDown") {
                e.preventDefault();
                setIndex((i) => Math.min(items.length - 1, i + 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setIndex((i) => Math.max(0, i - 1));
              } else if (e.key === "Enter") {
                e.preventDefault();
                choose(items[index]?.path);
              }
            }}
          />
          {loading && <Spinner size={14} />}
        </div>
        <div className="palette__list" id="qo-list" role="listbox" ref={listRef} aria-label={t("quickOpen.list")}>
          {error ? (
            <div className="palette__empty" role="alert">
              {error}
            </div>
          ) : items.length === 0 ? (
            <div className="palette__empty">
              <FileSearch size={18} /> {loading ? t("quickOpen.searching") : t("quickOpen.noMatches")}
            </div>
          ) : (
            items.map((m, i) => {
              const parts = splitPathPositions(m.path, m.positions);
              const { icon: Icon, color } = fileIcon(basename(m.path), "file");
              return (
                <div
                  key={m.path}
                  id={`qo-item-${i}`}
                  data-index={i}
                  role="option"
                  aria-selected={i === index}
                  className={clsx("palette__item", i === index && "palette__item--active")}
                  onMouseEnter={() => setIndex(i)}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    choose(m.path);
                  }}
                >
                  <span className="palette__icon" style={{ color }}>
                    <Icon size={15} />
                  </span>
                  <span className="qo__name">
                    <Highlighted segments={segmentsFromPositions(parts.name, parts.namePositions)} />
                  </span>
                  <span className="qo__dir">
                    <Highlighted segments={segmentsFromPositions(parts.dir, parts.dirPositions)} />
                  </span>
                </div>
              );
            })
          )}
        </div>
        <div className="palette__footer">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> {t("quickOpen.navigate")}
          </span>
          <span>
            <Kbd>Enter</Kbd> {t("quickOpen.open")}
          </span>
          <span>
            <Kbd>Esc</Kbd> {t("quickOpen.close")}
          </span>
        </div>
      </div>
    </div>
  );
}
