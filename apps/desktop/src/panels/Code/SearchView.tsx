/**
 * Ctrl+Shift+F project search (`files.search`): literal/regex, case, whole
 * word, include/exclude globs; results grouped by file, click opens the
 * file at the match.
 */
import type { SearchFilesResult, SearchMatch } from "@orkestr/protocol";
import { EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { CaseSensitive, ChevronDown, ChevronRight, Regex, Search, SlidersHorizontal, WholeWord, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { segmentsFromRange } from "../../features/files/highlight";
import { fileIcon } from "../../features/files/icons";
import { basename, dirname } from "../../features/files/paths";
import { errorMessage } from "../../features/files/rpc";
import { type RevealTarget, useEditorStore } from "../../store/editor";

interface Group {
  path: string;
  matches: SearchMatch[];
}

function splitGlobs(s: string): string[] {
  return s
    .split(",")
    .map((g) => g.trim())
    .filter(Boolean);
}

export function SearchView({ projectId, onOpen }: { projectId: string; onOpen: (path: string, at: RevealTarget) => void }) {
  const setSearchOpen = useEditorStore((s) => s.setSearchOpen);
  const [query, setQuery] = useState("");
  const [isRegex, setRegex] = useState(false);
  const [caseSensitive, setCase] = useState(false);
  const [wholeWord, setWord] = useState(false);
  const [showGlobs, setShowGlobs] = useState(false);
  const [include, setInclude] = useState("");
  const [exclude, setExclude] = useState("");
  const [result, setResult] = useState<SearchFilesResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const inputRef = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    const q = query;
    if (!q.trim()) {
      setResult(null);
      setError(null);
      return;
    }
    const id = ++seq.current;
    const timer = window.setTimeout(() => {
      setLoading(true);
      getClient()
        .call("files.search", {
          projectId,
          query: q,
          isRegex,
          caseSensitive,
          wholeWord,
          include: splitGlobs(include),
          exclude: splitGlobs(exclude),
          maxResults: 1000,
        })
        .then((res) => {
          if (id !== seq.current) return;
          setResult(res);
          setError(null);
          setCollapsed(new Set());
        })
        .catch((e: unknown) => id === seq.current && setError(errorMessage(e)))
        .finally(() => id === seq.current && setLoading(false));
    }, 250);
    return () => window.clearTimeout(timer);
  }, [projectId, query, isRegex, caseSensitive, wholeWord, include, exclude]);

  const groups = useMemo<Group[]>(() => {
    const map = new Map<string, SearchMatch[]>();
    for (const m of result?.matches ?? []) {
      const list = map.get(m.path);
      if (list) list.push(m);
      else map.set(m.path, [m]);
    }
    return [...map].map(([path, matches]) => ({ path, matches }));
  }, [result]);

  const toggle = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <aside className="code-search" aria-label="Search in project">
      <div className="code-search__header">
        <span className="code-search__title">Search</span>
        <IconButton label="Close search" icon={<X size={14} />} size="sm" onClick={() => setSearchOpen(false)} />
      </div>
      <div className="code-search__form">
        <Input
          ref={inputRef}
          placeholder="Search"
          aria-label="Search text"
          value={query}
          leftIcon={<Search size={14} />}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setSearchOpen(false)}
          rightSlot={
            <span className="code-search__toggles">
              <IconButton label="Match case" icon={<CaseSensitive size={14} />} size="sm" active={caseSensitive} aria-pressed={caseSensitive} onClick={() => setCase((v) => !v)} />
              <IconButton label="Match whole word" icon={<WholeWord size={14} />} size="sm" active={wholeWord} aria-pressed={wholeWord} onClick={() => setWord((v) => !v)} />
              <IconButton label="Use regular expression" icon={<Regex size={14} />} size="sm" active={isRegex} aria-pressed={isRegex} onClick={() => setRegex((v) => !v)} />
            </span>
          }
        />
        <button type="button" className="code-search__globs-toggle" aria-expanded={showGlobs} onClick={() => setShowGlobs((v) => !v)}>
          <SlidersHorizontal size={12} /> Files to include / exclude
        </button>
        {showGlobs && (
          <>
            <Input placeholder="e.g. src/**, *.ts" aria-label="Files to include" value={include} onChange={(e) => setInclude(e.target.value)} />
            <Input placeholder="e.g. **/*.test.ts, dist/**" aria-label="Files to exclude" value={exclude} onChange={(e) => setExclude(e.target.value)} />
          </>
        )}
      </div>
      <div className="code-search__summary" aria-live="polite">
        {loading ? (
          <>
            <Spinner size={12} /> Searching…
          </>
        ) : result ? (
          <>
            {result.matches.length} result{result.matches.length === 1 ? "" : "s"} in {groups.length} file
            {groups.length === 1 ? "" : "s"}
            {result.truncated ? " (limited)" : ""} · {result.elapsedMs} ms
          </>
        ) : null}
      </div>
      <div className="code-search__results">
        {error ? (
          <div className="code-search__error" role="alert">
            {error}
          </div>
        ) : !query.trim() ? (
          <EmptyState icon={<Search size={22} strokeWidth={1.6} />} title="Search the project" description="Respects .gitignore. Use globs to narrow the files." />
        ) : result && groups.length === 0 && !loading ? (
          <EmptyState icon={<Search size={22} strokeWidth={1.6} />} title="No results" description="Try a different query or fewer filters." />
        ) : (
          groups.map((g) => {
            const open = !collapsed.has(g.path);
            const { icon: Icon, color } = fileIcon(basename(g.path), "file");
            return (
              <div key={g.path} className="code-search__group">
                <button type="button" className="code-search__file" aria-expanded={open} onClick={() => toggle(g.path)}>
                  {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  <Icon size={14} style={{ color }} />
                  <span className="code-search__file-name">{basename(g.path)}</span>
                  <span className="code-search__file-dir">{dirname(g.path)}</span>
                  <span className="code-search__count">{g.matches.length}</span>
                </button>
                {open &&
                  g.matches.map((m) => (
                    <button
                      key={`${m.line}:${m.column}`}
                      type="button"
                      className={clsx("code-search__hit")}
                      title={`${g.path}:${m.line}:${m.column}`}
                      onClick={() => onOpen(m.path, { line: m.line, column: m.column, length: m.length })}
                    >
                      <span className="code-search__line">{m.line}</span>
                      <span className="code-search__preview">
                        {segmentsFromRange(m.preview, m.previewStart, m.previewEnd).map((s, i) =>
                          s.match ? (
                            <mark key={i} className="code-search__mark">
                              {s.text}
                            </mark>
                          ) : (
                            <span key={i}>{s.text}</span>
                          ),
                        )}
                      </span>
                    </button>
                  ))}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}
