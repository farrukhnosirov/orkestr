/**
 * Confirmation for "assign robot to project" (robot dropped on a project tile
 * or picked in its context menu). Assigning spawns the robot's template in
 * the target project — see `model/assign.ts`.
 */
import type { AgentInstance } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import { Bot, X } from "lucide-react";
import { useEffect, useState } from "react";

import { maybeClient } from "../../../core/client";
import { useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { planAssign } from "../model/assign";
import type { RobotSlot } from "../model/visibility";
import type { RobotDrop } from "../shell";
import { spawnInProject } from "./focus";
import "../designer/designer.css";

export interface AssignDialogProps {
  request: RobotDrop;
  slot: RobotSlot | null;
  onClose: () => void;
}

export function AssignDialog({ request, slot, onClose }: AssignDialogProps) {
  const { t } = useT(["robots", "common"]);
  const projects = useAppStore((s) => s.projects);
  const [agent, setAgent] = useState<AgentInstance | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const c = maybeClient();
    if (!c) {
      setLoaded(true);
      return undefined;
    }
    c.call("agents.get", { agentId: request.agentId })
      .then((r) => !cancelled && setAgent(r.agent))
      .catch((e: unknown) => console.warn("agents.get failed", e))
      .finally(() => !cancelled && setLoaded(true));
    return () => {
      cancelled = true;
    };
  }, [request.agentId]);

  const project = projects.find((p) => p.id === request.projectId) ?? null;
  const source = agent ?? (slot ? { projectId: slot.projectId, templateId: slot.templateId } : null);
  const plan = planAssign({ agent: source, projectId: request.projectId, projectIds: projects.map((p) => p.id) });
  const name = agent?.name ?? t("robots:robot.unknownAgent");
  const projectName = project?.name ?? "";

  const confirm = async () => {
    if (plan.kind !== "spawn") return;
    setBusy(true);
    await spawnInProject(plan.templateId, plan.projectId);
    setBusy(false);
    onClose();
  };

  let body: string;
  switch (plan.kind) {
    case "spawn":
      body = t("robots:assign.confirm", { agent: name, project: projectName });
      break;
    case "sameProject":
      body = t("robots:assign.sameProject", { agent: name, project: projectName });
      break;
    case "noTemplate":
      body = t("robots:assign.noTemplate", { agent: name });
      break;
    default:
      body = t("robots:assign.unavailable");
  }

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className="dialog robots-assign"
        role="dialog"
        aria-modal="true"
        aria-labelledby="robots-assign-title"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
        }}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Bot size={18} />
          </div>
          <div>
            <h2 id="robots-assign-title" className="dialog__title">
              {t("robots:assign.title")}
            </h2>
            <p className="dialog__subtitle">{projectName}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={onClose}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">{loaded ? <p className="robots-assign__text">{body}</p> : <Spinner size={18} />}</div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={onClose}>
            {plan.kind === "spawn" ? t("common:actions.cancel") : t("common:actions.close")}
          </Button>
          {plan.kind === "spawn" && (
            <Button variant="primary" autoFocus loading={busy} disabled={!loaded} onClick={() => void confirm()}>
              {t("robots:assign.action")}
            </Button>
          )}
        </footer>
      </div>
    </div>
  );
}
