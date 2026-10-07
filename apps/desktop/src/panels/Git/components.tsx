/**
 * Shared building blocks of the Git panel: diff pane (lazy Monaco), confirm
 * dialog, file tree rows, inline notices.
 */
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, ChevronDown, ChevronRight, Columns2, FileDiff, FolderClosed, Rows2, X } from "lucide-react";
import { type ReactNode, Suspense, lazy, useEffect, useId, useRef, useState } from "react";

import { useT } from "../../i18n";
import { type TreeNode, type Tone, languageForPath } from "./model";
import { Overlay } from "../../components/Overlay";

const MonacoDiff = lazy(() => import("./MonacoDiff"));

// ── Diff pane ─────────────────────────────────────────────

export interface DiffContent {
  path: string;
  original: string;
  modified: string;
  binary?: boolean;
  truncated?: boolean;
  /** Labels of both sides, e.g. "HEAD" / "Working tree". */
  originalLabel: string;
  modifiedLabel: string;
}

const SIDE_KEY = "orkestr.git.sideBySide";

function readSide(): boolean {
  try {
    return window.localStorage.getItem(SIDE_KEY) !== "false";
  } catch {
    return true;
  }
}

export interface DiffPaneProps {
  content: DiffContent | null;
  loading: boolean;
  error: string | null;
  emptyText: string;
  actions?: ReactNode;
}

/** Side-by-side / inline diff of one file with loading/empty/error states. */
export function DiffPane({ content, loading, error, emptyText, actions }: DiffPaneProps) {
  const { t } = useT("git");
  const [sideBySide, setSideBySide] = useState(readSide);
  const toggle = () => {
    setSideBySide((v) => {
      try {
        window.localStorage.setItem(SIDE_KEY, String(!v));
      } catch {
        /* storage unavailable */
      }
      return !v;
    });
  };
  return (
    <div className="git-diff">
      <div className="git-diff__bar">
        {content ? (
          <>
            <span className="git-diff__path" title={content.path}>
              {content.path}
            </span>
            <span className="git-diff__sides">
              {content.originalLabel} → {content.modifiedLabel}
            </span>
          </>
        ) : (
          <span className="git-diff__path git-muted">{t("diff.title")}</span>
        )}
        <div className="git-spacer" />
        {actions}
        <IconButton
          label={sideBySide ? t("diff.inline") : t("diff.sideBySide")}
          icon={sideBySide ? <Rows2 size={15} /> : <Columns2 size={15} />}
          onClick={toggle}
          tooltip="bottom"
        />
      </div>
      <div className="git-diff__body">
        {error ? (
          <Notice tone="danger">{error}</Notice>
        ) : loading && !content ? (
          <div className="git-center">
            <Spinner size={18} /> {t("diff.loading")}
          </div>
        ) : !content ? (
          <EmptyState icon={<FileDiff size={24} strokeWidth={1.6} />} title={t("diff.noFile")} description={emptyText} />
        ) : content.binary ? (
          <EmptyState title={t("diff.binary.title")} description={t("diff.binary.description")} />
        ) : content.truncated ? (
          <EmptyState title={t("diff.tooLarge.title")} description={t("diff.tooLarge.description")} />
        ) : (
          <Suspense
            fallback={
              <div className="git-center">
                <Spinner size={18} /> {t("diff.loadingEditor")}
              </div>
            }
          >
            <MonacoDiff
              original={content.original}
              modified={content.modified}
              language={languageForPath(content.path)}
              sideBySide={sideBySide}
            />
          </Suspense>
        )}
      </div>
    </div>
  );
}

// ── Confirm dialog ────────────────────────────────────────

export interface ConfirmDialogProps {
  title: string;
  description?: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  error?: string | null;
  confirmDisabled?: boolean;
  wide?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  children?: ReactNode;
}

/** Modal confirmation (Escape / backdrop cancel; focus starts on Cancel). */
export function ConfirmDialog({
  title,
  description,
  confirmLabel,
  danger = false,
  busy = false,
  error,
  confirmDisabled = false,
  wide = false,
  onConfirm,
  onCancel,
  children,
}: ConfirmDialogProps) {
  const { t } = useT("common");
  const titleId = useId();
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    cancelRef.current?.focus();
  }, []);
  const cancel = () => {
    if (!busy) onCancel();
  };
  return (
    <Overlay onDismiss={cancel}>
      <div
        className={clsx("dialog", wide && "git-dialog--wide")}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            cancel();
          }
        }}
      >
        <header className="dialog__header">
          <div className={clsx("dialog__icon", danger && "git-dialog__icon--danger")}>
            <AlertTriangle size={18} />
          </div>
          <div>
            <h2 id={titleId} className="dialog__title">
              {title}
            </h2>
            {description != null && <div className="dialog__subtitle">{description}</div>}
          </div>
          <button type="button" className="dialog__close" aria-label={t("actions.close")} onClick={cancel}>
            <X size={16} />
          </button>
        </header>
        {(children != null || error) && (
          <div className="dialog__body">
            {children}
            {error && <Notice tone="danger">{error}</Notice>}
          </div>
        )}
        <footer className="dialog__footer">
          <Button ref={cancelRef} variant="ghost" onClick={cancel} disabled={busy}>
            {t("actions.cancel")}
          </Button>
          <Button variant={danger ? "danger" : "primary"} onClick={onConfirm} loading={busy} disabled={confirmDisabled}>
            {confirmLabel}
          </Button>
        </footer>
      </div>
    </Overlay>
  );
}

// ── Notices ───────────────────────────────────────────────

export function Notice({ tone = "info", children, action }: { tone?: Tone; children: ReactNode; action?: ReactNode }) {
  return (
    <div className={clsx("git-notice", `git-notice--${tone}`)} role={tone === "danger" ? "alert" : "status"}>
      <span className="git-notice__text">{children}</span>
      {action}
    </div>
  );
}

// ── File tree ─────────────────────────────────────────────

export interface FileTreeProps<T> {
  nodes: TreeNode<T>[];
  selected: string | null;
  renderLeaf: (item: T, node: TreeNode<T>) => { label: ReactNode; badge: { letter: string; tone: Tone; label: string }; actions?: ReactNode; title?: string };
  onSelect: (item: T) => void;
  folderActions?: (node: TreeNode<T>) => ReactNode;
  depth?: number;
}

/** Folder tree of changed files; folders collapse, rows are keyboard focusable. */
export function FileTree<T>({ nodes, selected, renderLeaf, onSelect, folderActions, depth = 0 }: FileTreeProps<T>) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const toggle = (path: string) =>
    setCollapsed((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });
  return (
    <ul className="git-tree" role={depth === 0 ? "tree" : "group"}>
      {nodes.map((node) => {
        if (node.item !== undefined) {
          const item = node.item;
          const leaf = renderLeaf(item, node);
          const isSel = selected === node.path;
          return (
            <li key={`f:${node.path}`} role="treeitem" aria-selected={isSel} className={clsx("git-row", isSel && "git-row--selected")}>
              <button
                type="button"
                className="git-row__main"
                style={{ paddingLeft: 10 + depth * 14 }}
                onClick={() => onSelect(item)}
                title={leaf.title ?? node.path}
              >
                <span className="git-row__name">{leaf.label}</span>
                <span className={clsx("git-letter", `git-letter--${leaf.badge.tone}`)} aria-label={leaf.badge.label} title={leaf.badge.label}>
                  {leaf.badge.letter}
                </span>
              </button>
              {leaf.actions && <span className="git-row__actions">{leaf.actions}</span>}
            </li>
          );
        }
        const open = !collapsed.has(node.path);
        return (
          <li key={`d:${node.path}`} role="treeitem" aria-expanded={open} className="git-folder">
            <div className="git-row">
              <button
                type="button"
                className="git-row__main git-row__main--folder"
                style={{ paddingLeft: 6 + depth * 14 }}
                onClick={() => toggle(node.path)}
              >
                {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <FolderClosed size={13} className="git-muted" />
                <span className="git-row__name">{node.name}</span>
              </button>
              {folderActions && <span className="git-row__actions">{folderActions(node)}</span>}
            </div>
            {open && (
              <FileTree
                nodes={node.children}
                selected={selected}
                renderLeaf={renderLeaf}
                onSelect={onSelect}
                folderActions={folderActions}
                depth={depth + 1}
              />
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** Collapsible section header with a count and actions. */
export function SectionHeader({
  title,
  count,
  tone = "neutral",
  open,
  onToggle,
  actions,
}: {
  title: string;
  count: number;
  tone?: Tone;
  open: boolean;
  onToggle: () => void;
  actions?: ReactNode;
}) {
  return (
    <div className="git-section">
      <button type="button" className="git-section__toggle" onClick={onToggle} aria-expanded={open}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span>{title}</span>
        <Badge tone={tone}>{count}</Badge>
      </button>
      <span className="git-section__actions">{actions}</span>
    </div>
  );
}
