/**
 * HUD / Ambient mode (§34): a compact, translucent, always-on-top strip with
 * the run status, the agent strip, approvals with quick approve, a voice
 * slot and a mini Manager input. Rendered in its own window (`hud`), or
 * inside the page with `?hud=1` in a browser.
 */
import type { AgentInstance } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Check, CornerDownLeft, GripVertical, Maximize2, MicOff, ShieldAlert, X } from "lucide-react";
import { useState, type FormEvent } from "react";

import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Logo } from "../../components/Logo";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { agentName } from "../../lib/coreText";
import { managerActions } from "../../panels/Manager/data";
import { useProjectAgents } from "../../panels/Agents/data";
import { roleInitials } from "../../panels/Agents/model";
import { useActiveProject, useAppStore } from "../../store/app";
import { useUiPrefs } from "../aiui/aiuiStore";
import { resolveApproval, useSecurityStore } from "../security/store";
import { useHudQuality, useHudRuns, useHudVoiceSlot } from "./hudData";
import { agentStrip, isQuickApprovable, isWorking, nextApproval, runLine } from "./hudModel";
import { hudControls } from "./hudWindow";
import "./hud.css";

const AVATAR_TONES = ["accent", "violet", "magenta", "info", "success"] as const;

function toneOf(id: string): (typeof AVATAR_TONES)[number] {
  let h = 0;
  for (let i = 0; i < id.length; i += 1) h = (h * 31 + id.charCodeAt(i)) | 0;
  return AVATAR_TONES[Math.abs(h) % AVATAR_TONES.length] ?? "accent";
}

function AgentAvatar({ agent }: { agent: AgentInstance }) {
  const { t } = useT(["hud", "common"]);
  const state = t(`common:status.agent.${agent.state}`);
  const name = agentName(agent.name);
  return (
    <span
      className={clsx("hud-agent", `hud-agent--${toneOf(agent.id)}`, isWorking(agent) && "hud-agent--working", agent.state === "Error" && "hud-agent--error")}
      title={t("hud:agentTitle", { name, state })}
      role="img"
      aria-label={t("hud:agentTitle", { name, state })}
    >
      <svg viewBox="0 0 32 32" aria-hidden="true">
        <circle className="hud-agent__ring" cx="16" cy="16" r="14.5" />
        <circle className="hud-agent__core" cx="16" cy="16" r="11" />
      </svg>
      <span className="hud-agent__initials">{roleInitials(agent.role)}</span>
    </span>
  );
}

function ManagerInput({ projectId, projectName }: { projectId: string | null; projectName: string | null }) {
  const { t } = useT("hud");
  const [text, setText] = useState("");
  const [state, setState] = useState<{ kind: "idle" | "sending" | "sent" } | { kind: "error"; message: string }>({ kind: "idle" });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const value = text.trim();
    if (!value || !projectId || state.kind === "sending") return;
    setState({ kind: "sending" });
    managerActions.chat(projectId, value).then(
      () => {
        setText("");
        setState({ kind: "sent" });
        window.setTimeout(() => setState((s) => (s.kind === "sent" ? { kind: "idle" } : s)), 2500);
      },
      (err: unknown) => setState({ kind: "error", message: describeError(err) }),
    );
  };

  return (
    <form className="hud-input" onSubmit={submit}>
      <input
        className="hud-input__field"
        value={text}
        disabled={!projectId}
        onChange={(e) => {
          setText(e.target.value);
          if (state.kind === "error" || state.kind === "sent") setState({ kind: "idle" });
        }}
        placeholder={projectName ? t("managerPlaceholderProject", { project: projectName }) : t("noProject")}
        aria-label={t("managerPlaceholder")}
      />
      {state.kind === "sent" && <span className="hud-input__note hud-input__note--ok">{t("sent")}</span>}
      {state.kind === "error" && (
        <span className="hud-input__note hud-input__note--error" title={state.message}>
          {t("sendFailed", { error: state.message })}
        </span>
      )}
      <button type="submit" className="hud-input__send" aria-label={t("send")} title={t("send")} disabled={!projectId || !text.trim() || state.kind === "sending"}>
        <CornerDownLeft size={14} />
      </button>
    </form>
  );
}

function Approvals() {
  const { t } = useT("hud");
  const pending = useSecurityStore((s) => s.pending);
  const stopped = useSecurityStore((s) => s.flags.allStopped);
  const [busy, setBusy] = useState(false);
  const next = nextApproval(pending);
  if (stopped) {
    return (
      <span className="hud-chip hud-chip--danger">
        <ShieldAlert size={12} /> {t("stopped")}
      </span>
    );
  }
  if (!next) return <span className="hud-chip hud-chip--muted">{t("noApprovals")}</span>;
  const quick = isQuickApprovable(next);
  const decide = (decision: "Approve" | "Reject") => {
    setBusy(true);
    resolveApproval(next.id, decision).finally(() => setBusy(false));
  };
  return (
    <span className="hud-approvals">
      <span className="hud-chip hud-chip--warning" title={next.actionSummary}>
        <ShieldAlert size={12} /> {t("approvals", { count: pending.length })}
      </span>
      {quick ? (
        <>
          <button type="button" className="hud-icon-btn hud-icon-btn--ok" disabled={busy} title={t("approveTitle", { summary: next.actionSummary })} aria-label={t("approveTitle", { summary: next.actionSummary })} onClick={() => decide("Approve")}>
            <Check size={13} />
          </button>
          <button type="button" className="hud-icon-btn" disabled={busy} title={t("reject")} aria-label={t("reject")} onClick={() => decide("Reject")}>
            <X size={13} />
          </button>
        </>
      ) : (
        <button type="button" className="hud-text-btn" title={t("riskyOpensMain")} onClick={() => void hudControls.expand()}>
          {t("review")}
        </button>
      )}
    </span>
  );
}

export function Hud() {
  const { t } = useT(["hud", "common"]);
  const project = useActiveProject();
  const projectId = useAppStore((s) => s.activeProjectId);
  const prefs = useUiPrefs();
  const { agents } = useProjectAgents(projectId);
  const runs = useHudRuns(projectId);
  const quality = useHudQuality(projectId);
  const VoiceSlot = useHudVoiceSlot();
  const strip = agentStrip(agents);
  const line = runLine(runs);

  const status =
    line.kind === "idle"
      ? t("hud:run.idle")
      : line.status === "Running"
        ? t("hud:run.Running", { done: line.done, total: line.total })
        : t(`hud:run.${line.status}`);

  const qualityChip = quality ? (
    <span className={clsx("hud-chip", quality.state === "failing" ? "hud-chip--danger" : "hud-chip--muted")}>{quality.label}</span>
  ) : null;

  return (
    <section className={clsx("hud", line.kind === "run" && line.status === "Running" && "hud--live")} aria-label={t("hud:regionLabel")}>
      <header className="hud__bar" data-tauri-drag-region>
        <span
          className="hud__grip"
          title={t("hud:drag")}
          aria-hidden="true"
          onPointerDown={(e) => {
            if (e.button === 0) void hudControls.startDragging();
          }}
        >
          <GripVertical size={14} />
        </span>
        <span className="hud__orb" data-tauri-drag-region>
          <Logo size={18} />
        </span>
        <div className="hud__status" data-tauri-drag-region>
          <span className="hud__project" data-tauri-drag-region>
            {project?.name ?? t("hud:noProject")}
          </span>
          <span className="hud__run" data-tauri-drag-region title={line.kind === "run" ? line.goal : undefined}>
            {status}
          </span>
        </div>
        {/* The build/test chip lives in the agents row when it is shown, so the project name keeps its room. */}
        {!prefs.hud.showAgents && qualityChip}
        <ErrorBoundary name="HUD approvals" fallback={null}>
          <Approvals />
        </ErrorBoundary>
        <span className="hud__voice" title={VoiceSlot ? t("hud:voice") : t("hud:voiceUnavailable")}>
          {VoiceSlot ? (
            <ErrorBoundary name="HUD voice slot" fallback={<MicOff size={14} aria-label={t("hud:voiceUnavailable")} />}>
              <VoiceSlot size={18} />
            </ErrorBoundary>
          ) : (
            <MicOff size={14} aria-label={t("hud:voiceUnavailable")} />
          )}
        </span>
        <button type="button" className="hud-icon-btn" title={t("hud:expand")} aria-label={t("hud:expand")} onClick={() => void hudControls.expand()}>
          <Maximize2 size={13} />
        </button>
        <button type="button" className="hud-icon-btn" title={t("hud:hide")} aria-label={t("hud:hide")} onClick={() => void hudControls.hide()}>
          <X size={14} />
        </button>
      </header>

      {prefs.hud.showAgents && (
        <div className="hud__agents">
          {strip.shown.length === 0 ? (
            <span className="hud__muted">{t("hud:noAgents")}</span>
          ) : (
            <>
              <div className="hud__avatars">
                {strip.shown.map((a) => (
                  <AgentAvatar key={a.id} agent={a} />
                ))}
                {strip.more > 0 && <span className="hud-agent hud-agent--more">{t("hud:moreAgents", { count: strip.more })}</span>}
              </div>
              <span className="hud__muted">{t("hud:agents", { count: strip.working })}</span>
            </>
          )}
          {qualityChip && <span className="hud__agents-end">{qualityChip}</span>}
        </div>
      )}

      {prefs.hud.showManagerInput && <ManagerInput projectId={projectId} projectName={project?.name ?? null} />}
    </section>
  );
}
