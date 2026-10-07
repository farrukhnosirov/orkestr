import "./files.css";

import type { FileEntry } from "@orkestr/protocol";
import { Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  ArchiveRestore,
  Bot,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  Copy,
  ExternalLink,
  Eye,
  EyeOff,
  FilePlus,
  FolderOpen,
  FolderPlus,
  FolderTree,
  Pencil,
  RefreshCw,
  Search,
  SquareCode,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { type DragEvent, type KeyboardEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { openInEditor } from "../../features/files/actions";
import { ActionDialog } from "../../features/files/Dialog";
import { fileIcon } from "../../features/files/icons";
import { basename, dirname, isWithin, joinPath } from "../../features/files/paths";
import { RootFolderForm } from "../../features/files/RootFolderForm";
import { errorInfo, useFileEvents } from "../../features/files/rpc";
import { flattenTree, type TreeRow } from "../../features/files/tree";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useActiveProject } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import type { PanelProps } from "../types";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { nameError } from "./nameError";
import { TrashDialog } from "./TrashDialog";
import { useTrashUi } from "./trashStore";
import { useFileTree } from "./useFileTree";

const ROW_H = 24;
const OVERSCAN = 10;
const DRAG_TYPE = "application/x-orkestr-path";

type Editing = { mode: "rename"; path: string } | { mode: "create"; dir: string; kind: "file" | "dir" };

type Row = TreeRow | { type: "edit"; depth: number; dir: string; kind: "file" | "dir" };

/** Files panel (§14): lazy, virtualized, live-updating project tree. */
export default function FilesPanel({ projectId }: PanelProps) {
  const { t } = useT("files");
  const project = useActiveProject();
  useEffect(() => {
    useEditorStore.getState().setProject(projectId);
  }, [projectId]);
  if (!projectId) {
    return (
      <EmptyState icon={<FolderTree size={26} strokeWidth={1.6} />} title={t("noProject.title")} description={t("noProject.description")} />
    );
  }
  if (!project?.rootPath) return <RootFolderForm projectId={projectId} title={t("connectFolder")} />;
  return <FileExplorer key={projectId} projectId={projectId} rootPath={project.rootPath} />;
}

function absolutePath(rootPath: string, rel: string): string {
  const sep = rootPath.includes("\\") ? "\\" : "/";
  const base = rootPath.replace(/[\\/]+$/, "");
  return rel ? `${base}${sep}${rel.split("/").join(sep)}` : base;
}

function copyText(text: string): void {
  navigator.clipboard?.writeText(text).catch(() => undefined);
}

function FileExplorer({ projectId, rootPath }: { projectId: string; rootPath: string }) {
  const { t } = useT(["files", "common"]);
  const [showIgnored, setShowIgnored] = useState(false);
  const tree = useFileTree(projectId, showIgnored);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: FileEntry | null } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<FileEntry | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const trashOpen = useTrashUi((s) => s.open);
  const setTrashOpen = useTrashUi((s) => s.setOpen);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const dragSource = useRef<string | null>(null);
  const agentModified = useEditorStore((s) => s.agentModified);
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(400);

  useFileEvents(projectId, (change) => {
    tree.onChange(change);
    const store = useEditorStore.getState();
    if (change.actor === "agent" && change.path) store.markAgentModified(change.path);
    if (change.change === "Renamed" && change.oldPath != null) store.movePath(change.oldPath, change.path);
    if (selected && change.change === "Deleted" && isWithin(selected, change.path)) setSelected(null);
  });

  // Rows (+ the inline "new file/folder" editor row).
  const rows = useMemo<Row[]>(() => {
    const base: Row[] = flattenTree(tree.dirs, tree.expanded, filter);
    if (editing?.mode !== "create") return base;
    const edit: Row = { type: "edit", dir: editing.dir, kind: editing.kind, depth: 0 };
    if (editing.dir === "") return [edit, ...base];
    const idx = base.findIndex((r) => r.type === "entry" && r.entry.path === editing.dir);
    if (idx < 0) return [edit, ...base];
    const parent = base[idx] as Extract<TreeRow, { type: "entry" }>;
    edit.depth = parent.depth + 1;
    return [...base.slice(0, idx + 1), edit, ...base.slice(idx + 1)];
  }, [tree.dirs, tree.expanded, filter, editing]);

  const entryRows = useMemo(() => rows.filter((r): r is Extract<TreeRow, { type: "entry" }> => r.type === "entry"), [rows]);

  // Virtualization.
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewport(el.clientHeight));
    ro.observe(el);
    setViewport(el.clientHeight);
    return () => ro.disconnect();
  }, []);
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewport) / ROW_H) + OVERSCAN);

  const ensureVisible = useCallback(
    (path: string) => {
      const idx = rows.findIndex((r) => r.type === "entry" && r.entry.path === path);
      const el = listRef.current;
      if (idx < 0 || !el) return;
      const top = idx * ROW_H;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H - el.clientHeight;
    },
    [rows],
  );
  useEffect(() => {
    if (selected) ensureVisible(selected);
  }, [selected, ensureVisible]);

  // ── Actions ─────────────────────────────────────────────
  const fail = (e: unknown) => setNotice(describeError(e));

  const activate = (entry: FileEntry) => {
    setSelected(entry.path);
    if (entry.kind === "dir") tree.toggle(entry.path);
    else openInEditor(entry.path);
  };

  const startCreate = (kind: "file" | "dir", at: FileEntry | null) => {
    const dir = at ? (at.kind === "dir" ? at.path : dirname(at.path)) : "";
    if (dir) tree.expand(dir);
    setFilter("");
    setEditing({ mode: "create", dir, kind });
  };

  const commitEdit = async (value: string) => {
    const current = editing;
    if (!current) return;
    const name = value.trim();
    if (current.mode === "rename" && name === basename(current.path)) {
      setEditing(null);
      return;
    }
    if (!name) {
      setEditing(null);
      return;
    }
    const invalid = nameError(name);
    if (invalid) {
      setNotice(invalid);
      return;
    }
    setEditing(null);
    const client = getClient();
    try {
      if (current.mode === "create") {
        const path = joinPath(current.dir, name);
        if (current.kind === "file") {
          await client.call("files.create", { projectId, path });
          openInEditor(path);
        } else {
          await client.call("files.mkdir", { projectId, path });
          tree.expand(path);
        }
        setSelected(path);
        void tree.load(current.dir);
      } else {
        const to = joinPath(dirname(current.path), name);
        const res = await client.call("files.rename", { projectId, from: current.path, to });
        setSelected(res.entry.path);
        void tree.load(dirname(current.path));
      }
      setNotice(null);
    } catch (e) {
      const info = errorInfo(e);
      setNotice(info.reason === "exists" ? t("errors.exists", { name }) : describeError(e));
    }
  };

  const doDelete = async () => {
    if (!confirmDelete) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      await getClient().call("files.delete", { projectId, path: confirmDelete.path, recursive: confirmDelete.kind === "dir" });
      void tree.load(dirname(confirmDelete.path));
      if (selected && isWithin(selected, confirmDelete.path)) setSelected(null);
      setConfirmDelete(null);
    } catch (e) {
      setDeleteError(describeError(e));
    } finally {
      setDeleting(false);
    }
  };

  const openExternal = (path: string, target: "reveal" | "ide" | "default") => {
    getClient()
      .call("files.openExternal", { projectId, path, target })
      .catch(fail);
  };

  const move = async (from: string, targetDir: string) => {
    const to = joinPath(targetDir, basename(from));
    if (to === from || isWithin(targetDir, from)) return;
    try {
      await getClient().call("files.rename", { projectId, from, to });
      setSelected(to);
      if (targetDir) tree.expand(targetDir);
    } catch (e) {
      const info = errorInfo(e);
      setNotice(info.reason === "exists" ? t("errors.existsInTarget", { name: basename(from) }) : describeError(e));
    }
  };

  const menuItems = (entry: FileEntry | null): MenuItem[] => {
    const items: MenuItem[] = [];
    if (entry?.kind === "file") {
      items.push({ type: "item", id: "open", label: t("menu.open"), icon: <SquareCode size={14} />, shortcut: "Enter", onSelect: () => openInEditor(entry.path) });
      items.push({ type: "separator", id: "s0" });
    }
    items.push(
      { type: "item", id: "new-file", label: t("menu.newFile"), icon: <FilePlus size={14} />, onSelect: () => startCreate("file", entry) },
      { type: "item", id: "new-folder", label: t("menu.newFolder"), icon: <FolderPlus size={14} />, onSelect: () => startCreate("dir", entry) },
    );
    if (entry) {
      items.push(
        { type: "separator", id: "s1" },
        { type: "item", id: "rename", label: t("menu.rename"), icon: <Pencil size={14} />, shortcut: "F2", onSelect: () => setEditing({ mode: "rename", path: entry.path }) },
        { type: "item", id: "delete", label: t("menu.delete"), icon: <Trash2 size={14} />, shortcut: "Del", danger: true, onSelect: () => setConfirmDelete(entry) },
      );
    } else {
      items.push({ type: "item", id: "refresh", label: t("menu.refresh"), icon: <RefreshCw size={14} />, onSelect: () => tree.refreshAll() });
    }
    const path = entry?.path ?? "";
    items.push(
      { type: "separator", id: "s2" },
      { type: "item", id: "copy-path", label: t("menu.copyPath"), icon: <Copy size={14} />, onSelect: () => copyText(absolutePath(rootPath, path)) },
      { type: "item", id: "copy-rel", label: t("menu.copyRelative"), icon: <Copy size={14} />, disabled: !entry, onSelect: () => copyText(path) },
      { type: "item", id: "reveal", label: t("menu.reveal"), icon: <FolderOpen size={14} />, onSelect: () => openExternal(path, "reveal") },
      { type: "item", id: "ide", label: t("menu.openIde"), icon: <ExternalLink size={14} />, onSelect: () => openExternal(path, "ide") },
    );
    return items;
  };

  // ── Keyboard ─────────────────────────────────────────────
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (editing || e.target !== e.currentTarget) return;
    const idx = entryRows.findIndex((r) => r.entry.path === selected);
    const cur = entryRows[idx];
    const select = (r: (typeof entryRows)[number] | undefined) => r && setSelected(r.entry.path);
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        select(entryRows[Math.min(entryRows.length - 1, idx + 1)]);
        break;
      case "ArrowUp":
        e.preventDefault();
        select(entryRows[Math.max(0, idx - 1)]);
        break;
      case "Home":
        e.preventDefault();
        select(entryRows[0]);
        break;
      case "End":
        e.preventDefault();
        select(entryRows[entryRows.length - 1]);
        break;
      case "ArrowRight":
        e.preventDefault();
        if (cur?.entry.kind === "dir") {
          if (!cur.expanded) tree.toggle(cur.entry.path);
          else select(entryRows[idx + 1]?.depth === cur.depth + 1 ? entryRows[idx + 1] : undefined);
        }
        break;
      case "ArrowLeft":
        e.preventDefault();
        if (cur?.entry.kind === "dir" && cur.expanded) tree.toggle(cur.entry.path);
        else if (cur) {
          const parent = dirname(cur.entry.path);
          if (parent) setSelected(parent);
        }
        break;
      case "Enter":
        e.preventDefault();
        if (cur) activate(cur.entry);
        break;
      case "F2":
        e.preventDefault();
        if (cur) setEditing({ mode: "rename", path: cur.entry.path });
        break;
      case "Delete":
        e.preventDefault();
        if (cur) setConfirmDelete(cur.entry);
        break;
      case "ContextMenu":
        e.preventDefault();
        openMenuForSelection();
        break;
      case "F10":
        if (e.shiftKey) {
          e.preventDefault();
          openMenuForSelection();
        }
        break;
      default:
        if (!cur && entryRows[0] && (e.key === " " || e.key.length === 1)) select(entryRows[0]);
    }
  };

  const openMenuForSelection = () => {
    const idx = rows.findIndex((r) => r.type === "entry" && r.entry.path === selected);
    const row = rows[idx];
    const rect = listRef.current?.getBoundingClientRect();
    if (!rect) return;
    const y = rect.top + idx * ROW_H - (listRef.current?.scrollTop ?? 0) + ROW_H;
    setMenu({ x: rect.left + 24, y, entry: row?.type === "entry" ? row.entry : null });
  };

  // ── Drag & drop ─────────────────────────────────────────
  const dropDirFor = (entry: FileEntry | null): string => (entry ? (entry.kind === "dir" ? entry.path : dirname(entry.path)) : "");
  const canDrop = (target: string) => {
    const src = dragSource.current;
    return src !== null && !isWithin(target, src) && dirname(src) !== target;
  };
  const onDragOver = (e: DragEvent, entry: FileEntry | null) => {
    if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
    const target = dropDirFor(entry);
    if (!canDrop(target)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "move";
    setDropTarget(target);
  };
  const onDrop = (e: DragEvent, entry: FileEntry | null) => {
    const src = e.dataTransfer.getData(DRAG_TYPE) || dragSource.current;
    const target = dropDirFor(entry);
    setDropTarget(null);
    dragSource.current = null;
    if (!src || !canDropFrom(src, target)) return;
    e.preventDefault();
    e.stopPropagation();
    void move(src, target);
  };
  const canDropFrom = (src: string, target: string) => !isWithin(target, src) && dirname(src) !== target;

  // ── Render ───────────────────────────────────────────────
  const root = tree.dirs.get("");
  const activeId = selected ? `fx-row-${rows.findIndex((r) => r.type === "entry" && r.entry.path === selected)}` : undefined;

  return (
    <div className="fx-panel">
      <div className="fx-toolbar">
        <Input
          className="fx-filter"
          placeholder={t("toolbar.filterPlaceholder")}
          aria-label={t("toolbar.filter")}
          value={filter}
          leftIcon={<Search size={13} />}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setFilter("")}
          rightSlot={filter ? <IconButton label={t("toolbar.clearFilter")} icon={<X size={12} />} size="sm" onClick={() => setFilter("")} /> : undefined}
        />
        <IconButton label={t("toolbar.newFile")} icon={<FilePlus size={15} />} size="sm" tooltip="bottom" onClick={() => startCreate("file", entryRows.find((r) => r.entry.path === selected)?.entry ?? null)} />
        <IconButton label={t("toolbar.newFolder")} icon={<FolderPlus size={15} />} size="sm" tooltip="bottom" onClick={() => startCreate("dir", entryRows.find((r) => r.entry.path === selected)?.entry ?? null)} />
        <IconButton label={t("toolbar.refresh")} icon={<RefreshCw size={15} />} size="sm" tooltip="bottom" onClick={() => tree.refreshAll()} />
        <IconButton label={t("toolbar.collapseAll")} icon={<ChevronsDownUp size={15} />} size="sm" tooltip="bottom" onClick={() => tree.collapseAll()} />
        <IconButton
          label={showIgnored ? t("toolbar.hideIgnored") : t("toolbar.showIgnored")}
          icon={showIgnored ? <Eye size={15} /> : <EyeOff size={15} />}
          size="sm"
          tooltip="bottom"
          active={showIgnored}
          aria-pressed={showIgnored}
          onClick={() => setShowIgnored((v) => !v)}
        />
        <IconButton label={t("toolbar.trash")} icon={<ArchiveRestore size={15} />} size="sm" tooltip="bottom" onClick={() => setTrashOpen(true)} />
        <IconButton label={t("toolbar.openFolderIde")} icon={<ExternalLink size={15} />} size="sm" tooltip="bottom" onClick={() => openExternal("", "ide")} />
      </div>

      {notice && (
        <div className="fx-notice" role="alert">
          <TriangleAlert size={13} />
          <span>{notice}</span>
          <IconButton label={t("toolbar.dismiss")} icon={<X size={12} />} size="sm" onClick={() => setNotice(null)} />
        </div>
      )}

      {!root || (root.status === "loading" && !root.entries) ? (
        <div className="fx-loading">
          <Spinner size={16} /> {t("loading")}
        </div>
      ) : root.status === "error" && !root.entries ? (
        <EmptyState
          icon={<TriangleAlert size={24} strokeWidth={1.6} />}
          title={t("readError")}
          description={root.message}
          action={
            <Button size="sm" onClick={() => void tree.load("")}>
              {t("common:actions.retry")}
            </Button>
          }
        />
      ) : rows.length === 0 ? (
        filter ? (
          <EmptyState icon={<Search size={24} strokeWidth={1.6} />} title={t("noMatches.title")} description={t("noMatches.description")} />
        ) : (
          <EmptyState
            icon={<FolderTree size={24} strokeWidth={1.6} />}
            title={t("emptyFolder.title")}
            action={
              <Button size="sm" variant="primary" leftIcon={<FilePlus size={14} />} onClick={() => startCreate("file", null)}>
                {t("emptyFolder.action")}
              </Button>
            }
          />
        )
      ) : (
        <div
          ref={listRef}
          className={clsx("fx-tree", dropTarget === "" && "fx-tree--drop")}
          role="tree"
          aria-label={t("tree.label")}
          tabIndex={0}
          aria-activedescendant={activeId}
          onKeyDown={onKeyDown}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          onContextMenu={(e) => {
            if (e.target !== e.currentTarget && !(e.target as HTMLElement).classList.contains("fx-tree__inner")) return;
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, entry: null });
          }}
          onDragOver={(e) => onDragOver(e, null)}
          onDragLeave={(e) => e.currentTarget === e.target && setDropTarget(null)}
          onDrop={(e) => onDrop(e, null)}
        >
          <div className="fx-tree__inner" style={{ height: rows.length * ROW_H }}>
            {rows.slice(first, last).map((row, i) => {
              const index = first + i;
              const top = index * ROW_H;
              if (row.type === "edit") {
                return (
                  <div key="edit-row" className="fx-row fx-row--edit" style={{ top, paddingLeft: 8 + row.depth * 14 + 16 }}>
                    {(() => {
                      const { icon: Icon, color } = fileIcon("", row.kind);
                      return <Icon size={14} style={{ color }} />;
                    })()}
                    <InlineInput initial="" onCommit={(v) => void commitEdit(v)} onCancel={() => setEditing(null)} label={row.kind === "file" ? t("tree.newFileName") : t("tree.newFolderName")} />
                  </div>
                );
              }
              if (row.type === "status") {
                return (
                  <div key={`status-${row.path}-${row.status}`} className={clsx("fx-row fx-row--status", row.status === "error" && "fx-row--error")} style={{ top, paddingLeft: 8 + row.depth * 14 + 16 }}>
                    {row.status === "loading" ? (
                      <>
                        <Spinner size={11} /> {t("tree.loading")}
                      </>
                    ) : row.status === "error" ? (
                      row.message ?? t("tree.loadFailed")
                    ) : row.status === "truncated" ? (
                      t("tree.truncated")
                    ) : (
                      t("tree.empty")
                    )}
                  </div>
                );
              }
              const { entry, depth, expanded } = row;
              const isDir = entry.kind === "dir";
              const { icon: Icon, color } = fileIcon(entry.name, entry.kind, expanded);
              const isSelected = selected === entry.path;
              const agent = isDir
                ? Object.keys(agentModified).some((p) => isWithin(p, entry.path) && p !== entry.path)
                : agentModified[entry.path] !== undefined;
              const renaming = editing?.mode === "rename" && editing.path === entry.path;
              return (
                <div
                  key={entry.path}
                  id={`fx-row-${index}`}
                  role="treeitem"
                  aria-level={depth + 1}
                  aria-expanded={isDir ? expanded : undefined}
                  aria-selected={isSelected}
                  title={entry.path}
                  className={clsx(
                    "fx-row",
                    isSelected && "fx-row--selected",
                    entry.ignored && "fx-row--ignored",
                    entry.symlink && "fx-row--link",
                    isDir && dropTarget === entry.path && "fx-row--drop",
                  )}
                  style={{ top, paddingLeft: 8 + depth * 14 }}
                  draggable={!renaming}
                  onDragStart={(e) => {
                    dragSource.current = entry.path;
                    e.dataTransfer.setData(DRAG_TYPE, entry.path);
                    e.dataTransfer.setData("text/plain", entry.path);
                    e.dataTransfer.effectAllowed = "move";
                  }}
                  onDragEnd={() => {
                    dragSource.current = null;
                    setDropTarget(null);
                  }}
                  onDragOver={(e) => onDragOver(e, entry)}
                  onDrop={(e) => onDrop(e, entry)}
                  onClick={() => !renaming && activate(entry)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setSelected(entry.path);
                    setMenu({ x: e.clientX, y: e.clientY, entry });
                  }}
                >
                  {Array.from({ length: depth }, (_, d) => (
                    <span key={d} className="fx-guide" style={{ left: 8 + d * 14 + 6 }} aria-hidden="true" />
                  ))}
                  <span className="fx-row__chevron" aria-hidden="true">
                    {isDir ? row.loading ? <Spinner size={10} /> : expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}
                  </span>
                  <Icon size={14} style={{ color }} className="fx-row__icon" />
                  {renaming ? (
                    <InlineInput initial={entry.name} onCommit={(v) => void commitEdit(v)} onCancel={() => setEditing(null)} label={t("tree.rename", { name: entry.name })} />
                  ) : (
                    <span className="fx-row__name">{entry.name}</span>
                  )}
                  {agent && (
                    <span className="fx-row__agent" title={isDir ? t("tree.agentDir") : t("tree.agentFile")}>
                      <Bot size={11} />
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.entry)} onClose={() => setMenu(null)} label={t("menu.label")} />}

      {confirmDelete && (
        <ActionDialog
          title={t("delete.title", { name: confirmDelete.name })}
          subtitle={
            confirmDelete.kind === "dir"
              ? t("delete.folder")
              : t("delete.file")
          }
          icon={<Trash2 size={18} />}
          tone="danger"
          error={deleteError}
          onCancel={() => !deleting && setConfirmDelete(null)}
          actions={[
            { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", autoFocus: true, onSelect: () => setConfirmDelete(null) },
            { id: "delete", label: t("common:actions.delete"), variant: "danger", icon: <Trash2 size={14} />, busy: deleting, onSelect: () => void doDelete() },
          ]}
        >
          <p className="fx-dialog__text">{t("delete.trash")}</p>
        </ActionDialog>
      )}

      {trashOpen && (
        <TrashDialog
          projectId={projectId}
          onClose={() => setTrashOpen(false)}
          onRestored={(path) => {
            void tree.load(dirname(path));
            setSelected(path);
          }}
        />
      )}
    </div>
  );
}

function InlineInput({ initial, onCommit, onCancel, label }: { initial: string; onCommit: (v: string) => void; onCancel: () => void; label: string }) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  const [value, setValue] = useState(initial);
  const error = value.trim() ? nameError(value) : null;
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    const dot = initial.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, [initial]);
  const finish = (commit: boolean) => {
    if (done.current) return;
    done.current = true;
    if (commit) onCommit(value);
    else onCancel();
  };
  return (
    <input
      ref={ref}
      className={clsx("fx-edit", error && "fx-edit--invalid")}
      aria-label={label}
      aria-invalid={error ? true : undefined}
      title={error ?? undefined}
      value={value}
      spellCheck={false}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          if (!error) finish(true);
        } else if (e.key === "Escape") {
          e.preventDefault();
          finish(false);
        }
      }}
      onBlur={() => finish(Boolean(value.trim()) && value !== initial && !error)}
    />
  );
}
