/**
 * Reviews & merges (Phase 2 data in Mission Control): the focus project's
 * latest code / security reviews (`reviews.list`) and its Merge Manager
 * proposals in flight (`merge.list`), kept live by `ReviewUpdated` /
 * `MergeProposalUpdated`.
 */
import type { MergeManagerProposal, Review } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, type Tone } from "@orkestr/ui";
import { clsx } from "clsx";
import { GitMerge, ShieldAlert, ShieldCheck } from "lucide-react";

import { activeMerges, openReviewCount } from "../../../features/quality/signals";
import { useFormat, useT } from "../../../i18n";
import { useRpcQuery } from "../../../lib/rpcQuery";
import { type WidgetProps, useMission } from "../context";
import { Stat, WidgetEmpty, WidgetStatus } from "./common";

const VERDICT_TONE: Record<NonNullable<Review["verdict"]>, Tone> = { Approve: "success", RequestChanges: "warning", Reject: "danger" };

const MERGE_TONE: Partial<Record<MergeManagerProposal["status"], Tone>> = {
  Proposed: "info",
  AwaitingApproval: "warning",
  Merging: "info",
  Testing: "info",
  Conflicted: "danger",
};

function ReviewRow({ r }: { r: Review }) {
  const { t } = useT(["missionControl", "reviews"]);
  const fmt = useFormat();
  const Icon = r.kind === "Adversarial" ? ShieldAlert : ShieldCheck;
  const tone: Tone = r.status === "Finished" && r.verdict ? VERDICT_TONE[r.verdict] : r.status === "Failed" ? "danger" : "info";
  const status = r.status === "Finished" && r.verdict ? t(`reviews:verdict.${r.verdict}`) : t(`missionControl:reviews.status.${r.status}`);
  return (
    <li className={clsx("mc-quality", "mc-review")}>
      <Icon size={14} aria-hidden="true" className="mc-quality__kind" />
      <span className="mc-quality__label" title={r.summary || undefined}>
        {t(`reviews:kind.${r.kind}`)}
      </span>
      {r.findings.length > 0 && <span className="mc-quality__counts">{t("missionControl:reviews.findings", { count: r.findings.length })}</span>}
      <span className="mc-quality__time">{fmt.relative(r.updatedAt)}</span>
      <Badge tone={tone}>{status}</Badge>
    </li>
  );
}

function MergeRow({ p }: { p: MergeManagerProposal }) {
  const { t } = useT(["missionControl", "merge"]);
  return (
    <li className={clsx("mc-quality", "mc-merge")}>
      <GitMerge size={14} aria-hidden="true" className="mc-quality__kind" />
      <span className="mc-quality__label" title={p.order.join(" → ")}>
        {t("missionControl:reviews.mergeInto", { target: p.target, count: p.branches.length })}
      </span>
      {p.conflicts.length > 0 && <span className="mc-quality__counts">{t("missionControl:reviews.conflicts", { count: p.conflicts.length })}</span>}
      <Badge tone={MERGE_TONE[p.status] ?? "neutral"}>{t(`merge:status.${p.status}`)}</Badge>
    </li>
  );
}

export function ReviewsWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { focusProjectId } = useMission();
  const q = useRpcQuery(
    `mc.reviews:${focusProjectId ?? ""}`,
    async (c) => {
      if (!focusProjectId) return { reviews: [] as Review[], merges: [] as MergeManagerProposal[] };
      const [reviews, merges] = await Promise.all([
        c.call("reviews.list", { projectId: focusProjectId, limit: 30 }).then((r) => r.reviews),
        c.call("merge.list", { projectId: focusProjectId, limit: 10 }).then((r) => r.proposals),
      ]);
      return { reviews, merges };
    },
    {
      enabled: focusProjectId !== null,
      topics: focusProjectId ? [topic.project(focusProjectId)] : [],
      reloadOn: ["ReviewUpdated", "MergeProposalUpdated"],
    },
  );
  if (!focusProjectId) return <WidgetEmpty icon={<ShieldCheck size={20} />} title={t("reviews.noProject")} />;
  if (!q.data) return <WidgetStatus loading={q.loading} error={q.error} />;
  const { reviews, merges } = q.data;
  const open = openReviewCount(reviews);
  const inFlight = activeMerges(merges);
  if (reviews.length === 0 && inFlight.length === 0) {
    return <WidgetEmpty icon={<ShieldCheck size={20} />} title={t("reviews.empty")} hint={t("reviews.emptyHint")} />;
  }
  return (
    <div className="mc-reviews">
      <div className="mc-reviews__stats">
        <Stat value={open} label={t("reviews.open")} tone={open > 0 ? "warning" : undefined} />
        <Stat value={inFlight.length} label={t("reviews.merges")} tone={inFlight.some((p) => p.status === "Conflicted") ? "danger" : undefined} />
      </div>
      <ul className="mc-qualities" aria-label={t("reviews.listLabel")}>
        {inFlight.slice(0, 3).map((p) => (
          <MergeRow key={p.id} p={p} />
        ))}
        {reviews.slice(0, 6).map((r) => (
          <ReviewRow key={r.id} r={r} />
        ))}
      </ul>
    </div>
  );
}
