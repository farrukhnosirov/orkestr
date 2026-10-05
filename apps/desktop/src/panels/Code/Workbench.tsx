/**
 * The Monaco-backed part of the Code panel (lazy-loaded): tabs, breadcrumbs,
 * editor / diff view, save with conflict handling, live reload on
 * `FileChanged`, agent change markers and the status line.
 */
import type { FileChangedPayload } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  Bot,
  ChevronRight,
  Circle,
  ExternalLink,
  FileCode,
  FileWarning,
  GitCompare,
  RotateCcw,
  Save,
  TriangleAlert,
  Upload,
  X,
} from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useMemo, useReducer, useState } from "react";

import { getClient } from "../../core/client";
import { askManager } from "../../features/files/actions";
import { ActionDialog } from "../../features/files/Dialog";
import { fileIcon } from "../../features/files/icons";
import { ancestry, basename } from "../../features/files/paths";
import { useFileEvents } from "../../features/files/rpc";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { type EditorTab, useEditorStore } from "../../store/editor";
import { type Doc, registryFor, SaveConflictError } from "./documents";
import { CodeEditor, DiffEditor, type EditorCallbacks } from "./EditorHost";
import { applyTokensTheme, getMonaco } from "./monaco";
import { useThemeRevision } from "../../features/theme/useThemeRevision";

const ENCODINGS: Record<string, string> = {
  utf8: "UTF-8",
  utf8Bom: "UTF-8 BOM",
  utf16Le: "UTF-16 LE",
  utf16Be: "UTF-16 BE",
};

/** Encoding names are technical identifiers, identical in every language. */
function encodingLabel(encoding: string): string {
  return ENCODINGS[encoding] ?? "Latin-1";
}

function eolLabel(eol: string): string {
  return eol === "\r\n" ? "CRLF" : "LF";
}

interface Cursor {
  line: number;
  column: number;
  selected: number;
}

interface Compare {
  path: string;
  text: string;
  hash: string;
}

export default function Workbench({ projectId }: { projectId: string }) {
  const { t } = useT(["code", "common"]);
  const monaco = useMemo(() => getMonaco(), []);
  const registry = useMemo(() => registryFor(monaco, projectId), [monaco, projectId]);
  const tabs = useEditorStore((s) => s.tabs);
  const activePath = useEditorStore((s) => s.activePath);
  const diff = useEditorStore((s) => s.diff);
  // Re-derive from tokens on every theme apply (switches and live editor previews).
  const themeId = useThemeRevision();
  const [, rerender] = useReducer((x: number) => x + 1, 0);
  const [cursor, setCursor] = useState<Cursor | null>(null);
  const [problems, setProblems] = useState({ errors: 0, warnings: 0 });
  const [conflict, setConflict] = useState<{ path: string; actualHash: string | null } | null>(null);
  const [compare, setCompare] = useState<Compare | null>(null);
  const [confirmClose, setConfirmClose] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => registry.onChange(rerender), [registry]);

  // Theme follows the app theme (tokens are applied to :root by App first).
  useEffect(() => {
    const id = requestAnimationFrame(() => applyTokensTheme());
    return () => cancelAnimationFrame(id);
  }, [themeId]);

  // Load documents for new tabs.
  useEffect(() => {
    for (const tab of tabs) if (tab.status === "loading") void registry.load(tab.path);
  }, [tabs, registry]);

  const active: EditorTab | null = tabs.find((tab) => tab.path === activePath) ?? null;
  const doc: Doc | undefined = active ? registry.get(active.path) : undefined;

  // Problems (diagnostics markers) of the active model.
  useEffect(() => {
    if (!doc) {
      setProblems({ errors: 0, warnings: 0 });
      return;
    }
    const count = () => {
      const markers = monaco.editor.getModelMarkers({ resource: doc.model.uri });
      setProblems({
        errors: markers.filter((m) => m.severity === monaco.MarkerSeverity.Error).length,
        warnings: markers.filter((m) => m.severity === monaco.MarkerSeverity.Warning).length,
      });
    };
    count();
    const sub = monaco.editor.onDidChangeMarkers((uris) => {
      if (uris.some((u) => u.toString() === doc.model.uri.toString())) count();
    });
    return () => sub.dispose();
  }, [doc, monaco]);

  // ── Live updates ──────────────────────────────────────
  const onFileChanged = useCallback(
    async (change: FileChangedPayload) => {
      const store = useEditorStore.getState();
      if (change.actor === "agent" && change.path) store.markAgentModified(change.path);
      if (change.change === "Renamed" && change.oldPath != null) {
        registry.move(change.oldPath, change.path);
        store.movePath(change.oldPath, change.path);
        return;
      }
      if (change.change === "Deleted") {
        store.markDeleted(change.path);
        return;
      }
      const paths = change.path === "" ? store.tabs.map((tab) => tab.path) : [change.path];
      for (const p of paths) {
        const d = registry.get(p);
        if (!d) continue;
        try {
          if (registry.isDirty(d)) {
            const disk = await registry.diskText(p);
            if (disk && disk.hash !== d.hash) store.updateTab(p, { stale: true });
            continue;
          }
          const lines = await registry.reloadFromDisk(p);
          if (lines && lines.length > 0 && change.actor === "agent") store.addAgentLines(p, lines);
        } catch (e) {
          console.warn("reload failed", p, e);
        }
      }
    },
    [registry],
  );
  useFileEvents(projectId, (c) => void onFileChanged(c));

  // ── Save / close ──────────────────────────────────────
  const save = useCallback(
    async (path: string, overwriteHash?: string | null) => {
      setBusy(true);
      try {
        await registry.save(path, overwriteHash);
        setNotice(null);
        if (compare?.path === path) setCompare(null);
      } catch (e) {
        if (e instanceof SaveConflictError) setConflict({ path: e.path, actualHash: e.actualHash });
        else setNotice(t("notice.saveFailed", { name: basename(path), error: describeError(e) }));
      } finally {
        setBusy(false);
      }
    },
    [registry, compare, t],
  );

  const doClose = useCallback(
    (path: string) => {
      registry.close(path);
      useEditorStore.getState().closeTab(path);
    },
    [registry],
  );

  const requestClose = useCallback(
    (path: string) => {
      const d = registry.get(path);
      if (d && registry.isDirty(d)) setConfirmClose(path);
      else doClose(path);
    },
    [registry, doClose],
  );

  const callbacks: EditorCallbacks = {
    onSave: () => active && void save(active.path),
    onQuickOpen: () => useEditorStore.getState().setQuickOpen(true),
    onSearch: () => useEditorStore.getState().setSearchOpen(true),
    onAskManager: (text, startLine, endLine) =>
      askManager({ text, projectId, path: active?.path, startLine, endLine, language: active?.language }),
    onCursor: (line, column, selected) => setCursor({ line, column, selected }),
  };

  const openCompare = async (path: string) => {
    const disk = await registry.diskText(path);
    if (disk) setCompare({ path, ...disk });
    else setNotice(t("notice.gone"));
  };

  const reload = async (path: string) => {
    try {
      await registry.reloadFromDisk(path);
      if (compare?.path === path) setCompare(null);
    } catch (e) {
      setNotice(describeError(e));
    }
  };

  const openExternal = (path: string) => {
    getClient()
      .call("files.openExternal", { projectId, path, target: "ide" })
      .catch((e: unknown) => setNotice(describeError(e)));
  };

  // ── Render ────────────────────────────────────────────
  const onTabKey = (e: KeyboardEvent<HTMLDivElement>, index: number) => {
    const target =
      e.key === "ArrowRight" ? tabs[(index + 1) % tabs.length] : e.key === "ArrowLeft" ? tabs[(index - 1 + tabs.length) % tabs.length] : null;
    if (target) {
      e.preventDefault();
      useEditorStore.getState().activate(target.path);
      requestAnimationFrame(() => document.getElementById(`code-tab-${target.path}`)?.focus());
    } else if (e.key === "Delete" || (e.key.toLowerCase() === "w" && (e.ctrlKey || e.metaKey))) {
      e.preventDefault();
      const tab = tabs[index];
      if (tab) requestClose(tab.path);
    }
  };

  const showCompare = compare && active && compare.path === active.path;

  return (
    <div className="code-wb">
      <div className="code-tabs" role="tablist" aria-label={t("tabs.label")}>
        {tabs.map((tab, i) => {
          const { icon: Icon, color } = fileIcon(basename(tab.path), "file");
          const selected = tab.path === activePath;
          return (
            <div
              key={tab.path}
              id={`code-tab-${tab.path}`}
              role="tab"
              tabIndex={selected ? 0 : -1}
              aria-selected={selected}
              title={tab.path}
              className={clsx("code-tab", selected && "code-tab--active", tab.deleted && "code-tab--deleted")}
              onClick={() => useEditorStore.getState().activate(tab.path)}
              onAuxClick={(e) => e.button === 1 && requestClose(tab.path)}
              onKeyDown={(e) => onTabKey(e, i)}
            >
              <Icon size={14} style={{ color }} />
              <span className="code-tab__name">{basename(tab.path)}</span>
              {tab.agentLines.length > 0 && <Bot size={12} className="code-tab__agent" aria-label={t("tabs.agentChanged")} />}
              {tab.stale && <TriangleAlert size={12} className="code-tab__stale" aria-label={t("tabs.stale")} />}
              <button
                type="button"
                className={clsx("code-tab__close", tab.dirty && "code-tab__close--dirty")}
                aria-label={tab.dirty ? t("tabs.closeDirty", { name: basename(tab.path) }) : t("tabs.close", { name: basename(tab.path) })}
                tabIndex={-1}
                onClick={(e) => {
                  e.stopPropagation();
                  requestClose(tab.path);
                }}
              >
                {tab.dirty ? <Circle size={9} className="code-tab__dot" fill="currentColor" /> : null}
                <X size={13} className="code-tab__x" />
              </button>
            </div>
          );
        })}
      </div>

      {active && (
        <div className="code-bar">
          <nav className="code-crumbs" aria-label={t("bar.breadcrumbs")}>
            {ancestry(active.path).map((part, i, all) => {
              const last = i === all.length - 1;
              const { icon: Icon, color } = fileIcon(part.name, last ? "file" : "dir");
              return (
                <span key={part.path} className={clsx("code-crumbs__part", last && "code-crumbs__part--last")}>
                  {i > 0 && <ChevronRight size={12} className="code-crumbs__sep" />}
                  <Icon size={13} style={{ color }} />
                  {part.name}
                </span>
              );
            })}
            {active.deleted && <Badge tone="danger">{t("bar.deletedOnDisk")}</Badge>}
            {active.stale && <Badge tone="warning">{t("bar.changedOnDisk")}</Badge>}
          </nav>
          <div className="code-bar__actions">
            {active.agentLines.length > 0 && (
              <Button
                size="sm"
                variant="ghost"
                leftIcon={<Bot size={13} />}
                onClick={() => useEditorStore.getState().clearAgentMarks(active.path)}
                title={t("bar.clearAgentMarks")}
              >
                {t("bar.agentLines", { count: active.agentLines.length })}
              </Button>
            )}
            {active.stale && (
              <IconButton label={t("bar.reloadDiscard")} icon={<RotateCcw size={14} />} size="sm" tooltip="bottom" onClick={() => void reload(active.path)} />
            )}
            <IconButton
              label={diff ? t("bar.closeDiff") : t("bar.compareSaved")}
              icon={<GitCompare size={14} />}
              size="sm"
              tooltip="bottom"
              active={diff}
              aria-pressed={diff}
              disabled={!doc}
              onClick={() => {
                setCompare(null);
                useEditorStore.getState().setDiff(!diff);
              }}
            />
            <IconButton label={t("bar.openExternal")} icon={<ExternalLink size={14} />} size="sm" tooltip="bottom" onClick={() => openExternal(active.path)} />
            <IconButton
              label={t("bar.save")}
              icon={busy ? <Spinner size={13} /> : <Save size={14} />}
              size="sm"
              tooltip="bottom"
              disabled={!active.dirty || busy || !doc}
              onClick={() => void save(active.path)}
            />
          </div>
        </div>
      )}

      {showCompare && (
        <div className="code-compare" role="status">
          <span>{t("compare.banner")}</span>
          <Button size="sm" variant="primary" leftIcon={<Upload size={13} />} onClick={() => void save(compare.path, compare.hash)}>
            {t("compare.overwrite")}
          </Button>
          <Button size="sm" leftIcon={<RotateCcw size={13} />} onClick={() => void reload(compare.path)}>
            {t("compare.useDisk")}
          </Button>
          <IconButton label={t("compare.close")} icon={<X size={14} />} size="sm" onClick={() => setCompare(null)} />
        </div>
      )}
      {notice && (
        <div className="code-notice" role="alert">
          <TriangleAlert size={14} /> <span>{notice}</span>
          <IconButton label={t("notice.dismiss")} icon={<X size={13} />} size="sm" onClick={() => setNotice(null)} />
        </div>
      )}

      <div className="code-wb__body">
        {!active ? (
          <EmptyState
            icon={<FileCode size={26} strokeWidth={1.6} />}
            title={t("empty.title")}
            description={t("empty.description")}
            action={
              <Button size="sm" variant="primary" onClick={() => useEditorStore.getState().setQuickOpen(true)}>
                {t("empty.goToFile")}
              </Button>
            }
          />
        ) : active.status === "loading" ? (
          <div className="code-loading">
            <Spinner size={18} /> {t("opening", { name: basename(active.path) })}
          </div>
        ) : active.status === "binary" ? (
          <EmptyState icon={<FileWarning size={26} strokeWidth={1.6} />} title={t("binary.title")} description={t("binary.description")} action={<Button size="sm" onClick={() => openExternal(active.path)}>{t("binary.openExternal")}</Button>} />
        ) : active.status === "tooLarge" || active.status === "error" ? (
          <EmptyState
            icon={<TriangleAlert size={26} strokeWidth={1.6} />}
            title={active.status === "tooLarge" ? t("tooLarge") : t("openFailed")}
            description={active.error}
            action={
              <Button size="sm" onClick={() => {
                useEditorStore.getState().updateTab(active.path, { status: "loading", error: undefined });
              }}>
                {t("common:actions.retry")}
              </Button>
            }
          />
        ) : !doc ? (
          <div className="code-loading">
            <Spinner size={18} />
          </div>
        ) : showCompare ? (
          <DiffEditor monaco={monaco} doc={doc} original={compare.text} callbacks={callbacks} />
        ) : diff ? (
          <DiffEditor monaco={monaco} doc={doc} original={doc.savedText} callbacks={callbacks} />
        ) : (
          <CodeEditor monaco={monaco} doc={doc} agentLines={active.agentLines} callbacks={callbacks} />
        )}
      </div>

      {active && doc && (
        <footer className="code-status" aria-label={t("status.label")}>
          {cursor && (
            <span>
              {cursor.selected > 0
                ? t("status.cursorSelected", { line: cursor.line, column: cursor.column, count: cursor.selected })
                : t("status.cursor", { line: cursor.line, column: cursor.column })}
            </span>
          )}
          <span className={clsx(problems.errors > 0 && "code-status--error", problems.warnings > 0 && problems.errors === 0 && "code-status--warning")}>
            {t("status.problems", { errors: t("status.errors", { count: problems.errors }), warnings: t("status.warnings", { count: problems.warnings }) })}
          </span>
          <span className="code-status__spacer" />
          {doc.readonly && <span>{t("status.readOnly")}</span>}
          <span>{encodingLabel(doc.encoding)}</span>
          <span>{eolLabel(doc.model.getEOL())}</span>
          <span>{doc.model.getLanguageId()}</span>
        </footer>
      )}

      {conflict && (
        <ActionDialog
          title={t("conflict.title")}
          subtitle={t("conflict.subtitle", { name: basename(conflict.path) })}
          icon={<TriangleAlert size={18} />}
          tone="warning"
          onCancel={() => setConflict(null)}
          actions={[
            { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", onSelect: () => setConflict(null) },
            {
              id: "reload",
              label: t("conflict.reload"),
              icon: <RotateCcw size={14} />,
              onSelect: () => {
                const p = conflict.path;
                setConflict(null);
                void reload(p);
              },
            },
            {
              id: "compare",
              label: t("conflict.compare"),
              icon: <GitCompare size={14} />,
              autoFocus: true,
              onSelect: () => {
                const p = conflict.path;
                setConflict(null);
                useEditorStore.getState().activate(p);
                void openCompare(p);
              },
            },
            {
              id: "overwrite",
              label: t("conflict.overwrite"),
              variant: "danger",
              icon: <Upload size={14} />,
              onSelect: () => {
                const { path, actualHash } = conflict;
                setConflict(null);
                void save(path, actualHash);
              },
            },
          ]}
        >
          <p className="fx-dialog__text">{t("conflict.text")}</p>
        </ActionDialog>
      )}

      {confirmClose && (
        <ActionDialog
          title={t("closeDirty.title", { name: basename(confirmClose) })}
          subtitle={t("closeDirty.subtitle")}
          icon={<Save size={18} />}
          onCancel={() => setConfirmClose(null)}
          actions={[
            { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", onSelect: () => setConfirmClose(null) },
            {
              id: "discard",
              label: t("closeDirty.discard"),
              variant: "danger",
              onSelect: () => {
                const p = confirmClose;
                setConfirmClose(null);
                doClose(p);
              },
            },
            {
              id: "save",
              label: t("common:actions.save"),
              variant: "primary",
              autoFocus: true,
              onSelect: () => {
                const p = confirmClose;
                setConfirmClose(null);
                void (async () => {
                  await save(p);
                  const d = registry.get(p);
                  if (d && !registry.isDirty(d)) doClose(p);
                })();
              },
            },
          ]}
        />
      )}
    </div>
  );
}
