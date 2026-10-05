/**
 * Git panel → Merge Manager tab (§16): merge proposals of the project,
 * live via `MergeProposalUpdated` events; detail with branch cards, order,
 * gates, conflicts and approval-gated execution. Data via `merge.*` RPC.
 */
import "./merge.css";

import type { Event, MergeManagerProposal } from "@orkestr/protocol";
import { isEventKind, topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { GitMerge, Plus, RefreshCw, Settings2 } from "lucide-react";
import { useEffect, useState } from "react";

import { getClient, maybeClient } from "../../../core/client";
import { useFormat, useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { Notice } from "../components";
import { useAsync } from "../state";
import { PolicyDialog, ProposeDialog } from "./dialogs";
import { statusTone, upsertProposal } from "./model";
import { ProposalDetail } from "./ProposalDetail";

interface Props {
  projectId: string;
  currentBranch: string | null;
}

export function MergeTab({ projectId, currentBranch }: Props) {
  const { t } = useT("merge");
  const fmt = useFormat();
  const connected = useAppStore((s) => s.connection.state === "open");
  const { data, error, loading, reload } = useAsync(() => getClient().call("merge.list", { projectId }), [projectId]);
  const [proposals, setProposals] = useState<MergeManagerProposal[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"propose" | "policy" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [defaultTarget, setDefaultTarget] = useState<string>(currentBranch ?? "");

  useEffect(() => {
    if (!data) return;
    setProposals(data.proposals);
    setSelected((s) => (s && data.proposals.some((p) => p.id === s) ? s : (data.proposals[0]?.id ?? null)));
  }, [data]);

  // Live progress (approval, merging, conflicts, result).
  useEffect(() => {
    if (!connected) return;
    const client = maybeClient();
    if (!client) return;
    return client.subscribe([topic.project(projectId)], (ev: Event) => {
      if (isEventKind(ev, "MergeProposalUpdated")) setProposals((list) => upsertProposal(list, ev.payload.proposal));
    });
  }, [projectId, connected]);

  // Default target for new proposals: policy target, else the current branch.
  useEffect(() => {
    let cancelled = false;
    getClient()
      .call("merge.policy", { projectId })
      .then((r) => {
        if (!cancelled) setDefaultTarget(r.policy.targetBranch ?? currentBranch ?? "");
      })
      .catch(() => {
        /* keep the current branch */
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, currentBranch, dialog]);

  const update = (p: MergeManagerProposal) => setProposals((list) => upsertProposal(list, p));
  const current = proposals.find((p) => p.id === selected) ?? null;

  return (
    <div className="git-pane">
      <div className="mm-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setDialog("propose")}>
          {t("toolbar.new")}
        </Button>
        <Button size="sm" variant="ghost" leftIcon={<Settings2 size={14} />} onClick={() => setDialog("policy")}>
          {t("toolbar.policy")}
        </Button>
        <div className="git-spacer" />
        {loading && <Spinner size={14} />}
        <IconButton label={t("toolbar.refresh")} icon={<RefreshCw size={14} />} onClick={reload} />
      </div>
      {notice && <Notice tone="success">{notice}</Notice>}
      {error && <Notice tone="danger">{error}</Notice>}
      {loading && !data ? (
        <div className="git-center">
          <Spinner size={18} /> {t("list.loading")}
        </div>
      ) : proposals.length === 0 ? (
        <EmptyState
          icon={<GitMerge size={26} strokeWidth={1.6} />}
          title={t("list.empty.title")}
          description={t("list.empty.description")}
          action={
            <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setDialog("propose")}>
              {t("toolbar.new")}
            </Button>
          }
        />
      ) : (
        <div className="git-split">
          <ul className="git-split__list git-list" aria-label={t("list.label")}>
            {proposals.map((p) => (
              <li key={p.id} className={clsx("git-row mm-item", p.id === selected && "git-row--selected")}>
                <button type="button" className="mm-item__main" onClick={() => setSelected(p.id)} aria-current={p.id === selected}>
                  <span className="mm-item__top">
                    <GitMerge size={13} className="git-accent" />
                    <span className="mm-mono mm-item__target">{t("list.into", { target: p.target })}</span>
                    <Badge tone={statusTone(p.status)}>{t(`status.${p.status}`)}</Badge>
                  </span>
                  <span className="git-small git-muted mm-item__meta">
                    {t("list.branches", { count: p.branches.length })} · {fmt.relative(p.createdAt)}
                  </span>
                  <span className="git-small mm-item__names">{p.order.join(", ")}</span>
                </button>
              </li>
            ))}
          </ul>
          <div className="git-split__detail">
            {current ? (
              <ProposalDetail proposal={current} onUpdated={update} />
            ) : (
              <EmptyState icon={<GitMerge size={24} strokeWidth={1.6} />} title={t("detail.select")} description={t("detail.selectHint")} />
            )}
          </div>
        </div>
      )}
      {dialog === "propose" && (
        <ProposeDialog
          projectId={projectId}
          defaultTarget={defaultTarget}
          onCreated={(p) => {
            update(p);
            setSelected(p.id);
            setDialog(null);
          }}
          onCancel={() => setDialog(null)}
        />
      )}
      {dialog === "policy" && (
        <PolicyDialog
          projectId={projectId}
          onSaved={() => {
            setNotice(t("policy.saved"));
            setDialog(null);
          }}
          onCancel={() => setDialog(null)}
        />
      )}
    </div>
  );
}
