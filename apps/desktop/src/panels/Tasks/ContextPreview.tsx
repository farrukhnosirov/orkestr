import type { ContextItem, ContextPreviewResult, ContextReason } from "@orkestr/protocol";
import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Layers, RefreshCw } from "lucide-react";
import { useState } from "react";

import { formatPercent, t as translate, useT } from "../../i18n";
import { useRpcQuery } from "../../lib/rpcQuery";
import "./contextPreview.css";

/** Localized explanation of one reason. */
export function reasonText(r: ContextReason): string {
  switch (r.kind) {
    case "mentioned":
      return translate("contextPreview:reason.mentioned");
    case "focus":
      return translate("contextPreview:reason.focus");
    case "termMatch":
      return translate("contextPreview:reason.termMatch", { terms: r.terms.join(", ") });
    case "defines":
      return translate("contextPreview:reason.defines", { symbol: r.symbol });
    case "importedBy":
      return translate("contextPreview:reason.importedBy", { file: r.file });
    case "imports":
      return translate("contextPreview:reason.imports", { file: r.file });
    case "references":
      return translate("contextPreview:reason.references", { symbol: r.symbol, file: r.file });
    case "recentChange":
      return translate("contextPreview:reason.recentChange", { count: r.commits });
    case "rule":
      return r.overridesGlobal
        ? translate("contextPreview:reason.rule.overrides")
        : r.origin === "global"
          ? translate("contextPreview:reason.rule.global")
          : translate("contextPreview:reason.rule.project");
    case "pinned":
      return translate("contextPreview:reason.pinned");
    case "memory": {
      const pct = formatPercent(r.relevancePct / 100, 0);
      return r.tags.length > 0
        ? translate("contextPreview:reason.memoryTags", { pct, tags: r.tags.join(", ") })
        : translate("contextPreview:reason.memory", { pct });
    }
    case "mistake":
      return translate("contextPreview:reason.mistake");
    case "knownIssue":
      return translate("contextPreview:reason.knownIssue");
  }
}

const KIND_TONE = { Task: "accent", Handoff: "violet", Rule: "warning", Memory: "success", File: "neutral" } as const;

function Item({ item }: { item: ContextItem }) {
  const { t } = useT(["contextPreview", "memory"]);
  return (
    <li className="ctx-item">
      <div className="ctx-item__head">
        <Badge tone={KIND_TONE[item.kind]}>{t(`kind.${item.kind}`)}</Badge>
        <span className={clsx("ctx-item__title", item.kind === "File" && "tasks-mono")} title={item.source}>
          {item.title || item.source}
        </span>
        <span className="ctx-item__tokens">{t("tokens", { count: item.tokens })}</span>
      </div>
      <div className="ctx-item__meta">
        {item.memoryKind && <span>{t(`memory:kinds.${item.memoryKind}`)}</span>}
        {item.kind === "File" && <span>{t(`decision.${item.decision}`)}</span>}
        {item.ranges.length > 0 && item.decision === "Excerpt" && (
          <span className="tasks-mono">{item.ranges.map((r) => t("lines", { start: r.startLine, end: r.endLine })).join(", ")}</span>
        )}
      </div>
      {item.reasons.length > 0 && (
        <ul className="ctx-item__reasons">
          {item.reasons.map((r, i) => (
            <li key={`${r.kind}-${i}`}>{reasonText(r)}</li>
          ))}
        </ul>
      )}
    </li>
  );
}

/** Task drawer section: what the Context Optimizer would send an agent (§20). Loaded on demand. */
export function ContextPreview({ taskId, version }: { taskId: string; version: number }) {
  const { t } = useT("contextPreview");
  const [open, setOpen] = useState(false);
  const [rendered, setRendered] = useState(false);
  const query = useRpcQuery<ContextPreviewResult>(`ctx-preview:${taskId}:${version}`, (c) => c.call("context.preview", { taskId }), {
    enabled: open,
    reloadOn: ["MemoryUpdated"],
  });
  const p = query.data;

  return (
    <section className="tasks-section">
      <h4 className="tasks-section__title">
        <Layers size={13} /> {t("title")}
        <span className="tasks-drawer__spacer" />
        {open && <IconButton size="sm" label={t("refresh")} icon={<RefreshCw size={12} />} onClick={query.reload} disabled={query.loading} />}
      </h4>
      {!open ? (
        <Button size="sm" variant="secondary" onClick={() => setOpen(true)}>
          {t("show")}
        </Button>
      ) : (
        <>
          <p className="tasks-muted">{t("intro")}</p>
          {query.error ? (
            <p className="tasks-error" role="alert">
              {query.error}
            </p>
          ) : !p ? (
            <div className="tasks-muted">
              <Spinner size={14} /> {t("loading")}
            </div>
          ) : (
            <>
              <div className="ctx-summary">
                <strong>{t("summary", { used: p.budget.used, budget: p.budget.budget })}</strong>
                <span className="tasks-muted">
                  {p.budget.contextWindow != null
                    ? t("window", { window: p.budget.contextWindow, reserved: p.budget.reservedOutput })
                    : t("noWindow")}
                </span>
                <span className="ctx-meter" role="meter" aria-valuemin={0} aria-valuemax={p.budget.budget} aria-valuenow={p.budget.used} aria-label={t("summary", { used: p.budget.used, budget: p.budget.budget })}>
                  <span className="ctx-meter__fill" style={{ width: `${p.budget.budget > 0 ? Math.min(100, (p.budget.used / p.budget.budget) * 100) : 0}%` }} />
                </span>
                {p.index.hasRoot ? (
                  <span className="tasks-muted">
                    {[
                      t("files", { count: p.index.files }),
                      t("edges", { count: p.index.importEdges }),
                      ...(p.index.churnCommits > 0 ? [t("commits", { count: p.index.churnCommits })] : []),
                    ].join(" · ")}
                  </span>
                ) : (
                  <span className="tasks-muted">{t("noRoot")}</span>
                )}
                {p.handoffCompression && <span className="tasks-muted">{t(`handoff.${p.handoffCompression as "None"}`)}</span>}
              </div>
              <ul className="ctx-items">
                {p.items.map((item) => (
                  <Item key={`${item.kind}:${item.source}`} item={item} />
                ))}
              </ul>
              {p.omitted > 0 && <p className="tasks-muted">{t("omitted", { count: p.omitted })}</p>}
              <div className="tasks-actions">
                <Button size="sm" variant="ghost" onClick={() => setRendered((v) => !v)}>
                  {rendered ? t("hideRendered") : t("showRendered")}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
                  {t("hide")}
                </Button>
              </div>
              {rendered && <pre className="ctx-rendered">{p.rendered}</pre>}
            </>
          )}
        </>
      )}
    </section>
  );
}
