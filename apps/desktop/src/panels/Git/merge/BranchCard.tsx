/**
 * One branch of a merge proposal: diff stats, semantic summary (files,
 * hunks, symbols), risk flags, gates, dependencies, predicted conflicts.
 * Draggable when the order is editable; arrow buttons as the keyboard path.
 */
import type { MergeBranch, MergeGate } from "@orkestr/protocol";
import { Badge, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, ArrowDown, ArrowUp, ChevronDown, ChevronRight, GitBranch, GripVertical } from "lucide-react";
import { type DragEvent, useState } from "react";

import { useT } from "../../../i18n";
import { gateTone } from "./model";

interface Props {
  branch: MergeBranch;
  index: number;
  total: number;
  editable: boolean;
  dragging: boolean;
  dropTarget: boolean;
  onMove: (from: number, to: number) => void;
  onDragStart: (index: number) => void;
  onDragEnter: (index: number) => void;
  onDragEnd: () => void;
}

const GATES = ["task", "review", "tests", "acceptance"] as const;

export function BranchCard({ branch: b, index, total, editable, dragging, dropTarget, onMove, onDragStart, onDragEnter, onDragEnd }: Props) {
  const { t } = useT("merge");
  const [open, setOpen] = useState(false);
  const added = b.diff.symbols.filter((s) => s.change === "Added").length;
  const changed = b.diff.symbols.filter((s) => s.change === "Changed").length;
  const removed = b.diff.symbols.filter((s) => s.change === "Removed").length;

  const dragProps = editable
    ? {
        draggable: true,
        onDragStart: (e: DragEvent) => {
          e.dataTransfer.effectAllowed = "move";
          e.dataTransfer.setData("text/plain", b.branch);
          onDragStart(index);
        },
        onDragEnter: () => onDragEnter(index),
        onDragOver: (e: DragEvent) => e.preventDefault(),
        onDrop: (e: DragEvent) => {
          e.preventDefault();
          onDragEnd();
        },
        onDragEnd,
      }
    : {};

  const gate = (name: (typeof GATES)[number], g: MergeGate) => {
    const label = t("gates.label", { gate: t(`gates.${name}`), status: t(`gates.status.${g.status}`) });
    return (
      <Badge key={name} tone={gateTone(g.status)} title={g.detail ? `${label} — ${g.detail}` : label}>
        {label}
      </Badge>
    );
  };

  return (
    <li className={clsx("mm-card", dragging && "mm-card--dragging", dropTarget && "mm-card--drop")} {...dragProps}>
      <div className="mm-card__head">
        {editable && (
          <span className="mm-card__grip" title={t("order.drag")} aria-hidden="true">
            <GripVertical size={14} />
          </span>
        )}
        <span className="mm-card__index">{index + 1}</span>
        <div className="mm-card__title">
          <span className="mm-card__branch">
            <GitBranch size={13} />
            <span className="mm-mono">{b.branch}</span>
            {b.state !== "Pending" && (
              <Badge tone={b.state === "Conflicted" ? "danger" : "success"}>{t(`branch.state.${b.state}`)}</Badge>
            )}
            {b.dirty && <Badge tone="warning">{t("branch.dirty")}</Badge>}
          </span>
          {b.title !== b.branch && <span className="mm-card__task">{b.title}</span>}
        </div>
        <span className="mm-card__stats">
          <span className="mm-add">+{b.diff.additions}</span> <span className="mm-del">−{b.diff.deletions}</span>
        </span>
        {editable && (
          <span className="mm-card__moves">
            <IconButton size="sm" label={t("order.moveUp")} icon={<ArrowUp size={13} />} disabled={index === 0} onClick={() => onMove(index, index - 1)} />
            <IconButton
              size="sm"
              label={t("order.moveDown")}
              icon={<ArrowDown size={13} />}
              disabled={index === total - 1}
              onClick={() => onMove(index, index + 1)}
            />
          </span>
        )}
      </div>
      <div className="mm-card__meta git-small">
        <span className="git-muted">{b.commitsAhead > 0 ? t("branch.commits", { count: b.commitsAhead }) : t("branch.upToDateTarget")}</span>
        {b.diff.symbols.length > 0 && <span className="git-muted">{t("branch.symbols", { added, changed, removed })}</span>}
        {b.dependsOn.length > 0 && <span className="git-muted">{t("branch.dependsOn", { branches: b.dependsOn.join(", ") })}</span>}
      </div>
      <div className="mm-badges">
        {GATES.map((g) => gate(g, b.gates[g]))}
        {b.diff.risks.map((r) => (
          <Badge key={r.flag} tone={r.flag === "Migration" || r.flag === "PublicApi" || r.flag === "Security" ? "danger" : "warning"} title={r.items.join("\n")}>
            {t(`risks.${r.flag}`)}
          </Badge>
        ))}
      </div>
      {b.predictedConflicts.length > 0 && (
        <div className="mm-card__conflicts git-small">
          <AlertTriangle size={12} /> {t("branch.predicted", { paths: b.predictedConflicts.join(", ") })}
        </div>
      )}
      {(b.diff.files.length > 0 || b.diff.symbols.length > 0) && (
        <button type="button" className="mm-card__toggle git-small" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          {open ? t("branch.hideDetails") : t("branch.showDetails")}
        </button>
      )}
      {open && (
        <div className="mm-card__details">
          {b.diff.truncated && <p className="git-muted git-small">{t("branch.truncated")}</p>}
          <h4 className="mm-subhead">{t("branch.filesTitle")}</h4>
          <ul className="mm-files">
            {b.diff.files.map((f) => (
              <li key={f.path} title={f.hunkHeaders.join("\n")}>
                <span className="mm-mono mm-files__path">{f.oldPath ? `${f.oldPath} → ${f.path}` : f.path}</span>
                <span className="git-muted">{f.binary ? t("branch.binary") : t("branch.hunks", { count: f.hunks })}</span>
                <span className="mm-add">+{f.additions}</span>
                <span className="mm-del">−{f.deletions}</span>
              </li>
            ))}
          </ul>
          <h4 className="mm-subhead">{t("branch.symbolsTitle")}</h4>
          {b.diff.symbols.length === 0 ? (
            <p className="git-muted git-small">{t("branch.noSymbols")}</p>
          ) : (
            <ul className="mm-symbols">
              {b.diff.symbols.map((s) => (
                <li key={`${s.path}:${s.kind}:${s.name}:${s.change}`}>
                  <span className={clsx("mm-sym", `mm-sym--${s.change}`)}>{t(`symbolChange.${s.change}`)}</span>
                  <span className="git-muted">{s.kind}</span>
                  <span className="mm-mono">{s.name}</span>
                  {s.public && <Badge tone="info">{t("branch.public")}</Badge>}
                  {s.signatureChanged && <Badge tone="warning">{t("branch.signature")}</Badge>}
                  <span className="git-muted mm-symbols__path">{s.path}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </li>
  );
}
