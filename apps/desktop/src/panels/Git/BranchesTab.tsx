/**
 * Branches tab: switch, create, delete (safe; force with second confirmation)
 * and merge into the current branch after a conflict preview.
 */
import type { BranchInfo, MergePreview } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { ArrowRightLeft, GitBranch, GitMerge, Plus, Trash2 } from "lucide-react";
import { type FormEvent, useState } from "react";

import { getClient } from "../../core/client";
import { useFormat, useT } from "../../i18n";
import { ConfirmDialog, Notice } from "./components";
import { branchNameError, relativeTime, shortId } from "./model";
import { errorCode, errorMessage, rawMessage, useAsync } from "./state";

interface Props {
  projectId: string;
  worktree: string | null;
  refresh: number;
  onChanged: () => void;
}

type Pending =
  | { kind: "delete"; branch: BranchInfo; force: boolean }
  | { kind: "merge"; branch: BranchInfo; preview: MergePreview | null; error: string | null };

export function BranchesTab({ projectId, worktree, refresh, onChanged }: Props) {
  const { t } = useT(["git", "common"]);
  const fmt = useFormat();
  const base = worktree ? { projectId, worktree } : { projectId };
  const { data, error, loading, reload } = useAsync(() => getClient().call("vcs.branches", base), [projectId, worktree, refresh]);
  const [name, setName] = useState("");
  const [checkout, setCheckout] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const nameError = name ? branchNameError(name) : null;

  const act = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setActionError(null);
    setNotice(null);
    try {
      await fn();
      reload();
      onChanged();
    } catch (e) {
      setActionError(errorMessage(e));
      throw e;
    } finally {
      setBusy(null);
    }
  };

  const create = (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim() || nameError) return;
    void act("create", async () => {
      const r = await getClient().call("vcs.createBranch", { ...base, name: name.trim(), checkout });
      setName("");
      setNotice(checkout ? t("branches.switchedNew", { branch: r.branch }) : t("branches.created", { branch: r.branch, commit: shortId(r.commit) }));
    }).catch(() => undefined);
  };

  const switchTo = (b: BranchInfo) =>
    void act(`switch:${b.name}`, async () => {
      const r = await getClient().call("vcs.checkout", { ...base, branch: b.name });
      setNotice(t("branches.switched", { branch: r.branch }));
    }).catch(() => undefined);

  const confirmDelete = (b: BranchInfo, force: boolean) =>
    act("delete", async () => {
      const r = await getClient().call("vcs.deleteBranch", { projectId, name: b.name, force, confirm: force });
      setNotice(t("branches.deleted", { branch: r.name, commit: shortId(r.commit) }));
      setPending(null);
    }).catch((e: unknown) => {
      if (!force && errorCode(e) === "conflict" && rawMessage(e).includes("not fully merged")) {
        setActionError(null);
        setPending({ kind: "delete", branch: b, force: true });
      }
    });

  const startMerge = (b: BranchInfo) => {
    setPending({ kind: "merge", branch: b, preview: null, error: null });
    getClient()
      .call("vcs.mergePreview", { ...base, source: b.name })
      .then((preview) => setPending({ kind: "merge", branch: b, preview, error: null }))
      .catch((e: unknown) => setPending({ kind: "merge", branch: b, preview: null, error: errorMessage(e) }));
  };

  const confirmMerge = (b: BranchInfo) =>
    act("merge", async () => {
      const r = await getClient().call("vcs.merge", { ...base, source: b.name });
      setNotice(
        r.status === "UpToDate"
          ? t("branches.upToDate", { branch: b.name })
          : r.status === "Conflicted"
            ? t("branches.mergeConflicts", { count: r.conflicts.length })
            : t("branches.merged", { branch: b.name, commit: shortId(r.commit) }),
      );
      setPending(null);
    }).catch(() => undefined);

  const local = data?.branches.filter((b) => !b.remote) ?? [];
  const remote = data?.branches.filter((b) => b.remote) ?? [];

  const row = (b: BranchInfo) => (
    <li key={b.fullRef} className={clsx("git-branch", b.current && "git-branch--current")}>
      <div className="git-branch__main">
        <span className="git-branch__name">
          <GitBranch size={13} /> {b.name}
          {b.current && <Badge tone="accent">{t("branches.current")}</Badge>}
          {b.name.startsWith("agent/") && <Badge tone="violet">{t("branches.agent")}</Badge>}
          {b.gone && <Badge tone="warning">{t("branches.gone")}</Badge>}
        </span>
        <span className="git-branch__meta">
          <code>{shortId(b.commit)}</code> {b.subject}
          {b.committerDate && <> · {relativeTime(b.committerDate)}</>}
          {(b.ahead > 0 || b.behind > 0) && (
            <>
              {" "}
              · ↑{b.ahead} ↓{b.behind}
            </>
          )}
          {b.worktreePath && !b.current && <> · {t("branches.inWorktree")}</>}
        </span>
      </div>
      {!b.current && (
        <span className="git-row__actions git-row__actions--always">
          {!b.remote && (
            <IconButton size="sm" label={t("branches.merge", { branch: b.name, target: data?.current ?? t("branches.currentTarget") })} icon={<GitMerge size={14} />} onClick={() => startMerge(b)} disabled={busy !== null || !data?.current} />
          )}
          <IconButton
            size="sm"
            label={t("branches.switch", { branch: b.name })}
            icon={<ArrowRightLeft size={14} />}
            onClick={() => switchTo(b)}
            disabled={busy !== null || (b.worktreePath !== null && !b.remote)}
          />
          {!b.remote && (
            <IconButton size="sm" variant="danger" label={t("branches.delete", { branch: b.name })} icon={<Trash2 size={14} />} onClick={() => setPending({ kind: "delete", branch: b, force: false })} disabled={busy !== null || b.worktreePath !== null} />
          )}
        </span>
      )}
    </li>
  );

  return (
    <div className="git-pane">
      <form className="git-form" onSubmit={create}>
        <Input
          placeholder={t("branches.namePlaceholder")}
          aria-label={t("branches.name")}
          value={name}
          onChange={(e) => setName(e.target.value)}
          error={nameError ?? undefined}
          leftIcon={<GitBranch size={14} />}
        />
        <label className="git-check">
          <input type="checkbox" checked={checkout} onChange={(e) => setCheckout(e.target.checked)} /> {t("branches.switchToIt")}
        </label>
        <Button type="submit" size="sm" variant="primary" leftIcon={<Plus size={14} />} disabled={!name.trim() || nameError !== null} loading={busy === "create"}>
          {t("common:actions.create")}
        </Button>
      </form>
      {actionError && <Notice tone="danger">{actionError}</Notice>}
      {notice && <Notice tone="success">{notice}</Notice>}
      {error && <Notice tone="danger">{error}</Notice>}
      {loading && !data ? (
        <div className="git-center">
          <Spinner size={18} /> {t("branches.loading")}
        </div>
      ) : local.length === 0 && remote.length === 0 ? (
        <EmptyState icon={<GitBranch size={24} strokeWidth={1.6} />} title={t("branches.empty.title")} description={t("branches.empty.description")} />
      ) : (
        <div className="git-scroll">
          <h3 className="git-heading">{t("branches.local")}</h3>
          <ul className="git-list">{local.map(row)}</ul>
          {remote.length > 0 && (
            <>
              <h3 className="git-heading">{t("branches.remote")}</h3>
              <ul className="git-list">{remote.map(row)}</ul>
            </>
          )}
        </div>
      )}

      {pending?.kind === "delete" && (
        <ConfirmDialog
          title={pending.force ? t("branches.forceTitle", { branch: pending.branch.name }) : t("branches.deleteTitle", { branch: pending.branch.name })}
          description={
            pending.force
              ? t("branches.forceDescription")
              : t("branches.deleteDescription")
          }
          confirmLabel={pending.force ? t("branches.forceConfirm") : t("common:actions.delete")}
          danger
          busy={busy === "delete"}
          error={actionError}
          onConfirm={() => void confirmDelete(pending.branch, pending.force)}
          onCancel={() => setPending(null)}
        />
      )}
      {pending?.kind === "merge" && (
        <ConfirmDialog
          title={t("branches.mergeTitle", { branch: pending.branch.name, target: data?.current ?? t("branches.currentTarget") })}
          description={t("branches.mergeDescription")}
          confirmLabel={t("branches.mergeConfirm")}
          busy={busy === "merge"}
          error={actionError ?? pending.error}
          confirmDisabled={!pending.preview || !pending.preview.clean || pending.preview.upToDate}
          onConfirm={() => void confirmMerge(pending.branch)}
          onCancel={() => setPending(null)}
        >
          {!pending.preview && !pending.error ? (
            <div className="git-center git-small">
              <Spinner size={14} /> {t("branches.checking")}
            </div>
          ) : pending.preview?.upToDate ? (
            <Notice tone="info">{t("branches.previewUpToDate")}</Notice>
          ) : pending.preview?.clean ? (
            <Notice tone="success">{t("branches.previewClean", { count: pending.preview.commitsToMerge })}</Notice>
          ) : pending.preview ? (
            <Notice tone="danger">
              {t("branches.previewConflicts", { files: fmt.list(pending.preview.conflicts.map((c) => c.path)) })}
            </Notice>
          ) : null}
        </ConfirmDialog>
      )}
    </div>
  );
}
