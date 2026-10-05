/**
 * Merge Manager dialogs: new proposal (branches / AI Manager run + target)
 * and the project's merge policy.
 */
import type { ManagerRun, MergeCleanup, MergeManagerProposal, MergePolicy, MergePolicyMode } from "@orkestr/protocol";
import { Badge, Button, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, GitMerge, Settings2, X } from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";

import { getClient } from "../../../core/client";
import { useT } from "../../../i18n";
import { Notice } from "../components";
import { branchNameError } from "../model";
import { errorMessage, useAsync } from "../state";

// ── shared frame ──────────────────────────────────────────

interface FormDialogProps {
  title: string;
  description?: ReactNode;
  icon: ReactNode;
  submitLabel: string;
  busy: boolean;
  error: string | null;
  submitDisabled?: boolean;
  onSubmit: () => void;
  onCancel: () => void;
  children: ReactNode;
}

function FormDialog({ title, description, icon, submitLabel, busy, error, submitDisabled, onSubmit, onCancel, children }: FormDialogProps) {
  const { t } = useT("common");
  const titleId = useId();
  const first = useRef<HTMLDivElement>(null);
  useEffect(() => {
    first.current?.querySelector<HTMLElement>("input, select, button")?.focus();
  }, []);
  const cancel = () => {
    if (!busy) onCancel();
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!busy && !submitDisabled) onSubmit();
  };
  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && cancel()}>
      <form
        className="dialog git-dialog--wide"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onSubmit={submit}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            cancel();
          }
        }}
      >
        <header className="dialog__header">
          <div className="dialog__icon">{icon}</div>
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
        <div className="dialog__body mm-form" ref={first}>
          {children}
          {error && <Notice tone="danger">{error}</Notice>}
        </div>
        <footer className="dialog__footer">
          <Button type="button" variant="ghost" onClick={cancel} disabled={busy}>
            {t("actions.cancel")}
          </Button>
          <Button type="submit" variant="primary" loading={busy} disabled={submitDisabled}>
            {submitLabel}
          </Button>
        </footer>
      </form>
    </div>
  );
}

// ── new proposal ──────────────────────────────────────────

const AGENT_PREFIX = "agent/";

interface ProposeProps {
  projectId: string;
  defaultTarget: string;
  onCreated: (p: MergeManagerProposal) => void;
  onCancel: () => void;
}

export function ProposeDialog({ projectId, defaultTarget, onCreated, onCancel }: ProposeProps) {
  const { t } = useT("merge");
  const branches = useAsync(() => getClient().call("vcs.branches", { projectId }), [projectId]);
  const runs = useAsync(() => getClient().call("manager.status", { projectId }), [projectId]);
  const [target, setTarget] = useState(defaultTarget);
  const [runId, setRunId] = useState<string>("");
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const targetError = target ? branchNameError(target) : null;

  const runsWithBranches: ManagerRun[] = useMemo(
    () => (runs.data?.runs ?? []).filter((r) => r.tasks.some((x) => Boolean(x.branch))),
    [runs.data],
  );
  const local = useMemo(
    () => (branches.data?.branches ?? []).filter((b) => !b.remote && b.name !== target.trim()),
    [branches.data, target],
  );
  // Agent branches are preselected the first time the list arrives.
  const seeded = useRef(false);
  useEffect(() => {
    if (seeded.current || !branches.data) return;
    seeded.current = true;
    setPicked(new Set(branches.data.branches.filter((b) => !b.remote && b.name.startsWith(AGENT_PREFIX)).map((b) => b.name)));
  }, [branches.data]);

  const toggle = (name: string) =>
    setPicked((s) => {
      const n = new Set(s);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });

  const chosen = local.filter((b) => picked.has(b.name)).map((b) => ({ branch: b.name }));
  const canSubmit = (runId !== "" || chosen.length > 0) && !targetError;

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await getClient().call("merge.propose", {
        projectId,
        ...(runId ? { runId } : {}),
        ...(chosen.length ? { branches: chosen } : {}),
        ...(target.trim() ? { target: target.trim() } : {}),
      });
      onCreated(r.proposal);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <FormDialog
      title={t("propose.title")}
      description={t("propose.description")}
      icon={<GitMerge size={18} />}
      submitLabel={t("propose.submit")}
      busy={busy}
      error={error}
      submitDisabled={!canSubmit}
      onSubmit={() => void submit()}
      onCancel={onCancel}
    >
      <label className="mm-field">
        <span className="mm-field__label">{t("propose.target")}</span>
        <Input value={target} onChange={(e) => setTarget(e.target.value)} error={targetError ?? undefined} aria-label={t("propose.target")} />
      </label>
      {runsWithBranches.length > 0 && (
        <label className="mm-field">
          <span className="mm-field__label">{t("propose.run")}</span>
          <select className="mm-select" value={runId} onChange={(e) => setRunId(e.target.value)}>
            <option value="">{t("propose.noRun")}</option>
            {runsWithBranches.map((r) => (
              <option key={r.id} value={r.id}>
                {r.goal} — {t("propose.runBranches", { count: r.tasks.filter((x) => Boolean(x.branch)).length })}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className="mm-field">
        <span className="mm-field__label">{t("propose.branches")}</span>
        {branches.loading && !branches.data ? (
          <div className="git-center">
            <Spinner size={16} /> {t("propose.loadingBranches")}
          </div>
        ) : branches.error ? (
          <Notice tone="danger">{branches.error}</Notice>
        ) : local.length === 0 ? (
          <p className="git-muted git-small">{t("propose.noBranches")}</p>
        ) : (
          <ul className="git-plan mm-pick">
            {local.map((b) => (
              <li key={b.name} className="git-plan__item">
                <label>
                  <input type="checkbox" checked={picked.has(b.name)} onChange={() => toggle(b.name)} />
                  <span className="mm-mono">{b.name}</span>
                  {b.name.startsWith(AGENT_PREFIX) && (
                    <Badge tone="violet" icon={<Bot size={11} />}>
                      {t("propose.agentBranch")}
                    </Badge>
                  )}
                </label>
              </li>
            ))}
          </ul>
        )}
      </div>
    </FormDialog>
  );
}

// ── policy ────────────────────────────────────────────────

interface PolicyProps {
  projectId: string;
  onSaved: (p: MergePolicy) => void;
  onCancel: () => void;
}

const CLEANUPS: { value: MergeCleanup; key: "cleanupKeep" | "cleanupWorktrees" | "cleanupAll" }[] = [
  { value: "Keep", key: "cleanupKeep" },
  { value: "RemoveWorktrees", key: "cleanupWorktrees" },
  { value: "RemoveWorktreesAndBranches", key: "cleanupAll" },
];

export function PolicyDialog({ projectId, onSaved, onCancel }: PolicyProps) {
  const { t } = useT(["merge", "common"]);
  const loaded = useAsync(() => getClient().call("merge.policy", { projectId }), [projectId]);
  const [draft, setDraft] = useState<MergePolicy | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (loaded.data && !draft) setDraft(loaded.data.policy);
  }, [loaded.data, draft]);
  const target = draft?.targetBranch ?? "";
  const targetError = target ? branchNameError(target) : null;

  const patch = (p: Partial<MergePolicy>) => setDraft((d) => (d ? { ...d, ...p } : d));

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const policy: MergePolicy = { ...draft };
      if (!policy.targetBranch?.trim()) delete policy.targetBranch;
      const r = await getClient().call("merge.setPolicy", { projectId, policy });
      onSaved(r.policy);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const mode = (m: MergePolicyMode, label: string, hint: string) => (
    <label className={clsx("mm-radio", draft?.mode === m && "mm-radio--on")}>
      <input type="radio" name="mm-mode" checked={draft?.mode === m} onChange={() => patch({ mode: m })} />
      <span>
        <span className="mm-radio__title">{label}</span>
        <span className="git-muted git-small">{hint}</span>
      </span>
    </label>
  );
  const check = (key: "requireReview" | "requireTests" | "requireAcceptance" | "runQuality") => (
    <label className="mm-check">
      <input type="checkbox" checked={draft?.[key] ?? false} onChange={(e) => patch({ [key]: e.target.checked })} />
      <span>{t(`policy.${key}`)}</span>
    </label>
  );

  return (
    <FormDialog
      title={t("policy.title")}
      description={t("policy.description")}
      icon={<Settings2 size={18} />}
      submitLabel={t("common:actions.save")}
      busy={busy}
      error={error ?? loaded.error}
      submitDisabled={!draft || targetError !== null}
      onSubmit={() => void save()}
      onCancel={onCancel}
    >
      {!draft ? (
        <div className="git-center">
          <Spinner size={16} /> {t("common:state.loading")}
        </div>
      ) : (
        <>
          <fieldset className="mm-fieldset">
            <legend className="mm-field__label">{t("policy.mode")}</legend>
            {mode("Manual", t("policy.manual"), t("policy.manualHint"))}
            {mode("Auto", t("policy.auto"), t("policy.autoHint"))}
          </fieldset>
          <label className="mm-field">
            <span className="mm-field__label">{t("policy.target")}</span>
            <Input
              value={target}
              placeholder={t("policy.targetPlaceholder")}
              onChange={(e) => patch({ targetBranch: e.target.value })}
              error={targetError ?? undefined}
              aria-label={t("policy.target")}
            />
          </label>
          <div className="mm-checks">
            {check("requireReview")}
            {check("requireTests")}
            {check("requireAcceptance")}
            {check("runQuality")}
          </div>
          <label className="mm-field">
            <span className="mm-field__label">{t("policy.cleanup")}</span>
            <select className="mm-select" value={draft.cleanup} onChange={(e) => patch({ cleanup: e.target.value as MergeCleanup })}>
              {CLEANUPS.map((c) => (
                <option key={c.value} value={c.value}>
                  {t(`policy.${c.key}`)}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
    </FormDialog>
  );
}
