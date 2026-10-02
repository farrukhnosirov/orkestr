/**
 * Snapshots tab: list + create (with label), preview a snapshot against the
 * current workspace (Monaco), restore single files, and Rollback Workspace
 * with a plan preview, per-file selection and explicit confirmation.
 */
import type { RollbackPlan, SnapshotDiffResult, WorkspaceSnapshot } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Camera, History, RotateCcw, Trash2, Undo2 } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";

import { getClient } from "../../core/client";
import { ConfirmDialog, type DiffContent, DiffPane, Notice } from "./components";
import { formatBytes, reasonLabel, relativeTime, shortId, statusBadge, summarizePlan } from "./model";
import { errorMessage, useAsync, useGitUi } from "./state";

interface Props {
  projectId: string;
  worktree: string | null;
  refresh: number;
}

export function SnapshotsTab({ projectId, worktree, refresh }: Props) {
  const { data, error, loading, reload } = useAsync(
    () => getClient().call("snapshots.list", worktree ? { projectId, workspacePath: worktree, limit: 300 } : { projectId, limit: 300 }),
    [projectId, worktree, refresh],
  );
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selected, setSelected] = useState<WorkspaceSnapshot | null>(null);
  const [changes, setChanges] = useState<SnapshotDiffResult | null>(null);
  const [changesLoading, setChangesLoading] = useState(false);
  const [file, setFile] = useState<string | null>(null);
  const [diff, setDiff] = useState<DiffContent | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [rollback, setRollback] = useState<WorkspaceSnapshot | null>(null);
  const [restoreFile, setRestoreFile] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<WorkspaceSnapshot | null>(null);
  const intent = useGitUi((s) => s.intent);
  const setIntent = useGitUi((s) => s.setIntent);
  const snapshots = data?.snapshots ?? [];

  // "Rollback Workspace" command: open the dialog for the selected/latest snapshot.
  useEffect(() => {
    if (intent !== "rollback" || !data) return;
    setIntent(null);
    const target = selected ?? snapshots[0] ?? null;
    if (target) setRollback(target);
    else setNotice("There is no snapshot to roll back to yet — create one first.");
  }, [intent, data, selected, snapshots, setIntent]);

  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    setChangesLoading(true);
    setChanges(null);
    setFile(null);
    setDiff(null);
    getClient()
      .call("snapshots.diff", { snapshotId: selected.id })
      .then((r) => !cancelled && setChanges(r))
      .catch((e: unknown) => !cancelled && setDiffError(errorMessage(e)))
      .finally(() => !cancelled && setChangesLoading(false));
    return () => {
      cancelled = true;
    };
  }, [selected, refresh]);

  useEffect(() => {
    if (!selected || !file) return;
    let cancelled = false;
    setDiffLoading(true);
    setDiffError(null);
    getClient()
      .call("snapshots.diff", { snapshotId: selected.id, path: file })
      .then((r) => {
        if (cancelled || !r.file) return;
        setDiff({
          path: r.file.path,
          original: r.file.snapshot ?? "",
          modified: r.file.current ?? "",
          binary: r.file.binary,
          originalLabel: `Snapshot · ${selected.label}`,
          modifiedLabel: "Current",
        });
      })
      .catch((e: unknown) => !cancelled && setDiffError(errorMessage(e)))
      .finally(() => !cancelled && setDiffLoading(false));
    return () => {
      cancelled = true;
    };
  }, [selected, file, refresh]);

  const run = async (name: string, fn: () => Promise<void>) => {
    setBusy(name);
    setActionError(null);
    setNotice(null);
    try {
      await fn();
    } catch (e) {
      setActionError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const create = (e: FormEvent) => {
    e.preventDefault();
    void run("create", async () => {
      const params = { projectId, label: label.trim() || "Manual snapshot", ...(worktree ? { workspacePath: worktree } : {}) };
      const r = await getClient().call("snapshots.create", params);
      setLabel("");
      setNotice(`Snapshot “${r.snapshot.label}” saved: ${r.snapshot.fileCount} files, ${formatBytes(r.snapshot.sizeBytes)}.`);
      reload();
    });
  };

  const doRestoreFile = (path: string) =>
    run("restore", async () => {
      if (!selected) return;
      const r = await getClient().call("snapshots.restoreFile", { snapshotId: selected.id, path, confirm: true });
      setNotice(r.unchanged.length ? `${path} already matches the snapshot.` : `Restored ${path}. A safety snapshot was taken first.`);
      setRestoreFile(null);
      reload();
    });

  const doDelete = (s: WorkspaceSnapshot) =>
    run("delete", async () => {
      await getClient().call("snapshots.delete", { snapshotId: s.id });
      if (selected?.id === s.id) setSelected(null);
      setDeleting(null);
      reload();
    });

  return (
    <div className="git-split">
      <div className="git-split__list">
        <form className="git-form" onSubmit={create}>
          <Input placeholder="Snapshot label" aria-label="Snapshot label" value={label} maxLength={200} onChange={(e) => setLabel(e.target.value)} leftIcon={<Camera size={14} />} />
          <Button type="submit" size="sm" variant="primary" leftIcon={<Camera size={14} />} loading={busy === "create"}>
            Snapshot
          </Button>
        </form>
        <p className="git-muted git-small git-hint">Snapshots include uncommitted and untracked files and never touch git history.</p>
        {actionError && <Notice tone="danger">{actionError}</Notice>}
        {notice && <Notice tone="success">{notice}</Notice>}
        {error && <Notice tone="danger">{error}</Notice>}
        {loading && !data ? (
          <div className="git-center">
            <Spinner size={18} /> Loading snapshots…
          </div>
        ) : snapshots.length === 0 ? (
          <EmptyState icon={<History size={24} strokeWidth={1.6} />} title="No snapshots yet" description="Take one before risky changes — Orkestr also snapshots automatically before discards and rollbacks." />
        ) : (
          <ul className="git-list" aria-label="Snapshots">
            {snapshots.map((s) => (
              <li key={s.id} className={clsx("git-branch", selected?.id === s.id && "git-branch--current")}>
                <button type="button" className="git-branch__main git-branch__button" onClick={() => setSelected(s)} aria-pressed={selected?.id === s.id}>
                  <span className="git-branch__name">
                    {s.label}
                    <Badge tone={s.reason === "Manual" ? "accent" : s.reason === "PreRollback" || s.reason === "BeforeRiskyAction" ? "warning" : "violet"}>
                      {reasonLabel(s)}
                    </Badge>
                    {s.dirty && <Badge tone="neutral">uncommitted</Badge>}
                  </span>
                  <span className="git-branch__meta">
                    {relativeTime(s.createdAt)} · {s.fileCount} files · {formatBytes(s.sizeBytes)}
                    {s.branch && <> · {s.branch}</>}
                    {s.baseCommit && <> @ <code>{shortId(s.baseCommit)}</code></>}
                  </span>
                </button>
                <span className="git-row__actions git-row__actions--always">
                  <IconButton size="sm" label="Rollback workspace to this snapshot…" icon={<RotateCcw size={14} />} onClick={() => setRollback(s)} disabled={busy !== null} />
                  <IconButton size="sm" variant="danger" label="Delete snapshot…" icon={<Trash2 size={14} />} onClick={() => setDeleting(s)} disabled={busy !== null} />
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="git-split__detail">
        {selected && (
          <div className="git-snap-changes">
            <div className="git-snap-changes__head">
              <span className="git-heading">Changes since “{selected.label}”</span>
              <div className="git-spacer" />
              <Button size="sm" variant="secondary" leftIcon={<RotateCcw size={14} />} onClick={() => setRollback(selected)}>
                Rollback…
              </Button>
            </div>
            {changesLoading ? (
              <div className="git-center git-small">
                <Spinner size={14} /> Comparing with the workspace…
              </div>
            ) : changes && changes.changes.length === 0 ? (
              <div className="git-muted git-small git-pad">The workspace matches this snapshot.</div>
            ) : (
              <ul className="git-snap-files">
                {changes?.changes.map((c) => {
                  const b = statusBadge(c.kind);
                  return (
                    <li key={c.path} className={clsx("git-row", file === c.path && "git-row--selected")}>
                      <button type="button" className="git-row__main" onClick={() => setFile(c.path)}>
                        <span className="git-row__name">{c.path}</span>
                        <span className={clsx("git-letter", `git-letter--${b.tone}`)} title={b.label}>
                          {b.letter}
                        </span>
                      </button>
                      {c.kind !== "Added" && (
                        <span className="git-row__actions">
                          <IconButton size="sm" label={`Restore ${c.path} from snapshot…`} icon={<Undo2 size={14} />} onClick={() => setRestoreFile(c.path)} />
                        </span>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        )}
        <DiffPane
          content={diff}
          loading={diffLoading}
          error={diffError}
          emptyText={selected ? "Pick a changed file to compare the snapshot with the current version." : "Select a snapshot to preview it."}
        />
      </div>

      {rollback && (
        <RollbackDialog
          snapshot={rollback}
          onClose={() => setRollback(null)}
          onDone={(msg) => {
            setRollback(null);
            setNotice(msg);
            reload();
          }}
        />
      )}
      {restoreFile && selected && (
        <ConfirmDialog
          title={`Restore ${restoreFile}?`}
          description={`The current file is replaced with its version from “${selected.label}”. A safety snapshot is taken first.`}
          confirmLabel="Restore file"
          danger
          busy={busy === "restore"}
          error={actionError}
          onConfirm={() => void doRestoreFile(restoreFile)}
          onCancel={() => setRestoreFile(null)}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title={`Delete snapshot “${deleting.label}”?`}
          description="The snapshot record is removed; files in your workspace are not touched."
          confirmLabel="Delete snapshot"
          danger
          busy={busy === "delete"}
          error={actionError}
          onConfirm={() => void doDelete(deleting)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </div>
  );
}

/** Rollback Workspace: plan preview → per-file selection → confirm. */
function RollbackDialog({ snapshot, onClose, onDone }: { snapshot: WorkspaceSnapshot; onClose: () => void; onDone: (msg: string) => void }) {
  const [deleteCreated, setDeleteCreated] = useState(false);
  const [plan, setPlan] = useState<RollbackPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPlan(null);
    setPlanError(null);
    getClient()
      .call("snapshots.planRollback", { snapshotId: snapshot.id, deleteCreated })
      .then((p) => !cancelled && setPlan(p))
      .catch((e: unknown) => !cancelled && setPlanError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [snapshot.id, deleteCreated]);

  const actionable = useMemo(() => plan?.items.filter((i) => i.action !== "Keep") ?? [], [plan]);
  const selectedPaths = actionable.filter((i) => !excluded.has(i.path)).map((i) => i.path);
  const summary = plan ? summarizePlan(plan) : null;

  const apply = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      const r = await getClient().call("snapshots.rollbackWorkspace", {
        snapshotId: snapshot.id,
        confirm: true,
        deleteCreated,
        paths: selectedPaths,
        expectedFingerprint: plan.fingerprint,
      });
      const parts = [`${r.restored.length} restored`, `${r.deleted.length} deleted`];
      if (r.conflicts.length) parts.push(`${r.conflicts.length} skipped (changed meanwhile)`);
      onDone(`Rolled back to “${snapshot.label}”: ${parts.join(", ")}. Undo by rolling back to the pre-rollback snapshot.`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = (path: string) =>
    setExcluded((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });

  return (
    <ConfirmDialog
      wide
      title={`Rollback workspace to “${snapshot.label}”?`}
      description={`${relativeTime(snapshot.createdAt)} · ${snapshot.fileCount} files. Orkestr takes a safety snapshot first, so this can be undone.`}
      confirmLabel={`Rollback ${selectedPaths.length} file(s)`}
      danger
      busy={busy}
      error={error}
      confirmDisabled={!plan || selectedPaths.length === 0}
      onConfirm={() => void apply()}
      onCancel={onClose}
    >
      <label className="git-check">
        <input type="checkbox" checked={deleteCreated} onChange={(e) => setDeleteCreated(e.target.checked)} /> Also delete files created after the snapshot
      </label>
      {planError ? (
        <Notice tone="danger">{planError}</Notice>
      ) : !plan ? (
        <div className="git-center git-small">
          <Spinner size={14} /> Planning rollback…
        </div>
      ) : plan.items.length === 0 ? (
        <Notice tone="success">The workspace already matches this snapshot.</Notice>
      ) : (
        <>
          {summary && (
            <div className="git-plan-summary">
              <Badge tone="warning">{summary.restore} restore</Badge>
              <Badge tone="info">{summary.recreate} re-create</Badge>
              <Badge tone="danger">{summary.delete} delete</Badge>
              <Badge>{summary.keep} keep</Badge>
            </div>
          )}
          <ul className="git-plan" aria-label="Rollback plan">
            {plan.items.map((i) => {
              const keep = i.action === "Keep";
              return (
                <li key={i.path} className={clsx("git-plan__item", keep && "git-plan__item--keep")}>
                  <label>
                    <input type="checkbox" disabled={keep} checked={!keep && !excluded.has(i.path)} onChange={() => toggle(i.path)} />
                    <span className={clsx("git-plan__action", `git-plan__action--${i.action.toLowerCase()}`)}>{i.action}</span>
                    <span className="git-row__name" title={i.path}>
                      {i.path}
                    </span>
                  </label>
                  <span className="git-muted git-small">{i.reason}</span>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </ConfirmDialog>
  );
}
