/**
 * The Agent Hub (ADR-0045): the compact flyout next to the orb. Every agent
 * and Claude Code session grouped by project with status and progress; per
 * item a quick message (to the session, or relayed by the project's
 * Manager), Pause / Resume, Stop, Restart and "open transcript"; pending
 * approvals with inline approve / deny (low-risk only — risky ones open the
 * main window, like the HUD); a two-step emergency stop; "Open Orkestr";
 * push-to-talk. Keyboard: ↑ / ↓ move between rows, Enter opens the message
 * box of the selected row, Esc closes the box, then the hub.
 */
import type { Approval } from "@orkestr/protocol";
import { clsx } from "clsx";
import {
  Check,
  CornerDownLeft,
  ExternalLink,
  FileText,
  Mic,
  MicOff,
  MessageSquare,
  OctagonX,
  Pause,
  Play,
  RotateCcw,
  Settings2,
  ShieldAlert,
  Square,
  X,
} from "lucide-react";
import { type FormEvent, type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";

import { Logo } from "../../components/Logo";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { agentActivity, agentName, approvalSummary } from "../../lib/coreText";
import { isQuickApprovable } from "../hud/hudModel";
import { resolveApproval, stopAllAi, useSecurityStore } from "../security/store";
import type { OrbVoiceState } from "./channel";
import { type HubAction, type HubGroup, type HubItem, hubCounts, moveSelection } from "./hubModel";

export interface OrbHubProps {
  groups: HubGroup[];
  items: HubItem[];
  loaded: boolean;
  voice: OrbVoiceState;
  /** Push-to-talk (press / release). */
  onPtt: (down: boolean) => void;
  onAction: (item: HubItem, action: Exclude<HubAction, "message">) => Promise<void>;
  onMessage: (item: HubItem, text: string) => Promise<void>;
  onMessageManager: (projectId: string, text: string) => Promise<void>;
  onOpenMain: () => void;
  onOpenSettings: () => void;
  onOpenApprovals: () => void;
  onClose: () => void;
}

type SendState = { kind: "idle" | "sending" | "sent" } | { kind: "error"; message: string };

function Composer({ placeholder, label, onSend, onCancel }: { placeholder: string; label: string; onSend: (text: string) => Promise<void>; onCancel: () => void }) {
  const { t } = useT("orb");
  const [text, setText] = useState("");
  const [state, setState] = useState<SendState>({ kind: "idle" });
  const ref = useRef<HTMLInputElement | null>(null);
  useEffect(() => ref.current?.focus(), []);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const value = text.trim();
    if (!value || state.kind === "sending") return;
    setState({ kind: "sending" });
    onSend(value).then(
      () => {
        setText("");
        setState({ kind: "sent" });
        window.setTimeout(() => setState((s) => (s.kind === "sent" ? { kind: "idle" } : s)), 2000);
      },
      (err: unknown) => setState({ kind: "error", message: describeError(err) }),
    );
  };
  return (
    <form className="orb-compose" onSubmit={submit}>
      <input
        ref={ref}
        className="orb-compose__field"
        value={text}
        placeholder={placeholder}
        aria-label={label}
        maxLength={20000}
        onChange={(e) => {
          setText(e.target.value);
          if (state.kind === "error" || state.kind === "sent") setState({ kind: "idle" });
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onCancel();
          }
        }}
      />
      <button type="submit" className="orb-icon-btn orb-icon-btn--accent" aria-label={t("hub.send")} title={t("hub.send")} disabled={!text.trim() || state.kind === "sending"}>
        <CornerDownLeft size={13} />
      </button>
      {state.kind === "sent" && <span className="orb-compose__note orb-compose__note--ok" role="status">{t("hub.sent")}</span>}
      {state.kind === "error" && (
        <span className="orb-compose__note orb-compose__note--error" role="alert" title={state.message}>
          {t("hub.sendFailed", { error: state.message })}
        </span>
      )}
    </form>
  );
}

const ACTION_ICONS = { pause: Pause, resume: Play, stop: Square, restart: RotateCcw, transcript: FileText } as const;

function Row({
  item,
  selected,
  composing,
  onSelect,
  onCompose,
  onAction,
  onMessage,
}: {
  item: HubItem;
  selected: boolean;
  composing: boolean;
  onSelect: () => void;
  onCompose: (open: boolean) => void;
  onAction: OrbHubProps["onAction"];
  onMessage: OrbHubProps["onMessage"];
}) {
  const { t } = useT(["orb", "common"]);
  const [busy, setBusy] = useState<HubAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const name = item.kind === "agent" ? agentName(item.name) : item.name;
  const activity = item.activity ? (item.kind === "agent" ? agentActivity(item.activity) : item.activity) : null;
  const status = t(`orb:status.${item.status}`);
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (selected && !composing && document.activeElement?.closest(".orb-row") !== ref.current) ref.current?.focus({ preventScroll: false });
  }, [selected, composing]);

  const run = (action: Exclude<HubAction, "message">) => {
    setBusy(action);
    setError(null);
    onAction(item, action).then(
      () => setBusy(null),
      (e: unknown) => {
        setBusy(null);
        setError(describeError(e));
      },
    );
  };

  return (
    <div
      ref={ref}
      className={clsx("orb-row", `orb-row--${item.status}`, selected && "orb-row--selected")}
      role="listitem"
      tabIndex={selected ? 0 : -1}
      aria-label={t("orb:hub.rowLabel", { name, status })}
      data-orb-row={item.id}
      onFocus={onSelect}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" && item.actions.includes("message")) {
          e.preventDefault();
          onCompose(!composing);
        }
      }}
    >
      <div className="orb-row__main">
        <span className={clsx("orb-dot", `orb-dot--${item.status}`)} aria-hidden="true" />
        <div className="orb-row__text">
          <span className="orb-row__name" title={name}>
            {name}
            {item.kind === "claude" && <span className="orb-row__kind">{t("orb:hub.claudeSession")}</span>}
          </span>
          <span className="orb-row__activity" title={activity ?? status}>
            {status}
            {activity ? ` · ${activity}` : ""}
          </span>
        </div>
        <div className="orb-row__actions">
          {item.actions.includes("message") && (
            <button type="button" className={clsx("orb-icon-btn", composing && "orb-icon-btn--on")} aria-label={t("orb:actions.message")} title={t("orb:actions.message")} aria-expanded={composing} onClick={() => onCompose(!composing)}>
              <MessageSquare size={13} />
            </button>
          )}
          {item.actions
            .filter((a): a is Exclude<HubAction, "message"> => a !== "message")
            .map((a) => {
              const Icon = ACTION_ICONS[a];
              return (
                <button
                  key={a}
                  type="button"
                  className={clsx("orb-icon-btn", a === "stop" && "orb-icon-btn--danger")}
                  aria-label={t(`orb:actions.${a}`)}
                  title={t(`orb:actions.${a}`)}
                  disabled={busy !== null}
                  onClick={() => run(a)}
                >
                  <Icon size={13} />
                </button>
              );
            })}
        </div>
      </div>
      {item.progress !== null && item.live && (
        <div className="orb-progress" role="progressbar" aria-label={t("orb:hub.progress")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(item.progress * 100)}>
          <span style={{ width: `${Math.round(item.progress * 100)}%` }} />
        </div>
      )}
      {error && (
        <div className="orb-row__error" role="alert">
          {error}
        </div>
      )}
      {composing && (
        <Composer
          placeholder={item.kind === "claude" ? t("orb:hub.messageSession", { name }) : t("orb:hub.messageAgent", { name })}
          label={t("orb:actions.message")}
          onSend={(text) => onMessage(item, text)}
          onCancel={() => {
            onCompose(false);
            ref.current?.focus();
          }}
        />
      )}
    </div>
  );
}

function ApprovalRow({ approval, projectName, onReview }: { approval: Approval; projectName: string | null; onReview: () => void }) {
  const { t } = useT("orb");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const summary = approvalSummary(approval.actionSummary);
  const decide = (decision: "Approve" | "Reject") => {
    setBusy(true);
    setError(null);
    resolveApproval(approval.id, decision).then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        setError(describeError(e));
      },
    );
  };
  return (
    <div className="orb-approval" role="listitem">
      <ShieldAlert size={14} className="orb-approval__icon" aria-hidden="true" />
      <div className="orb-approval__text">
        <span className="orb-approval__summary" title={summary}>
          {summary}
        </span>
        <span className="orb-approval__meta">
          {projectName ? `${projectName} · ` : ""}
          {t(`risk.${approval.risk}`)}
        </span>
        {error && <span className="orb-row__error">{error}</span>}
      </div>
      {isQuickApprovable(approval) ? (
        <>
          <button type="button" className="orb-icon-btn orb-icon-btn--ok" disabled={busy} aria-label={t("hub.approveTitle", { summary })} title={t("hub.approve")} onClick={() => decide("Approve")}>
            <Check size={13} />
          </button>
          <button type="button" className="orb-icon-btn" disabled={busy} aria-label={t("hub.denyTitle", { summary })} title={t("hub.deny")} onClick={() => decide("Reject")}>
            <X size={13} />
          </button>
        </>
      ) : (
        <button type="button" className="orb-text-btn" title={t("hub.reviewHint")} onClick={onReview}>
          {t("hub.review")}
        </button>
      )}
    </div>
  );
}

function EmergencyStop() {
  const { t } = useT("orb");
  const stopped = useSecurityStore((s) => s.flags.allStopped);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!confirm) return undefined;
    const timer = window.setTimeout(() => setConfirm(false), 5000);
    return () => window.clearTimeout(timer);
  }, [confirm]);
  if (stopped) {
    return (
      <span className="orb-chip orb-chip--danger" role="status">
        <OctagonX size={13} /> {t("hub.allStopped")}
      </span>
    );
  }
  const run = () => {
    setBusy(true);
    stopAllAi().then(
      (r) => {
        setBusy(false);
        setConfirm(false);
        setResult(t("hub.stoppedResult", { count: r.agentsStopped }));
      },
      (e: unknown) => {
        setBusy(false);
        setResult(describeError(e));
      },
    );
  };
  return (
    <>
      <button
        type="button"
        className={clsx("orb-stop", confirm && "orb-stop--confirm")}
        disabled={busy}
        aria-label={confirm ? t("hub.emergencyConfirm") : t("hub.emergency")}
        title={confirm ? t("hub.emergencyConfirm") : t("hub.emergencyHint")}
        onClick={() => (confirm ? run() : setConfirm(true))}
      >
        <OctagonX size={14} />
        <span>{confirm ? t("hub.emergencyConfirm") : t("hub.emergency")}</span>
      </button>
      {result && (
        <span className="orb-footer__note" role="status">
          {result}
        </span>
      )}
    </>
  );
}

function PushToTalk({ voice, onPtt }: { voice: OrbVoiceState; onPtt: (down: boolean) => void }) {
  const { t } = useT("orb");
  const down = useRef(false);
  const press = (on: boolean) => {
    if (down.current === on) return;
    down.current = on;
    onPtt(on);
  };
  const listening = voice.phase === "listening" || voice.phase === "transcribing";
  const label = !voice.enabled ? t("hub.voiceOff") : listening ? t("hub.voiceListening") : t("hub.voiceHold");
  return (
    <button
      type="button"
      className={clsx("orb-ptt", listening && "orb-ptt--on")}
      disabled={!voice.enabled}
      aria-label={label}
      title={label}
      aria-pressed={listening}
      style={{ ["--orb-level" as string]: String(voice.level) }}
      onPointerDown={(e) => {
        if (e.button === 0) press(true);
      }}
      onPointerUp={() => press(false)}
      onPointerLeave={() => press(false)}
      onKeyDown={(e) => {
        if ((e.key === " " || e.key === "Enter") && !e.repeat) {
          e.preventDefault();
          press(true);
        }
      }}
      onKeyUp={(e) => {
        if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          press(false);
        }
      }}
    >
      {voice.enabled ? <Mic size={15} /> : <MicOff size={15} />}
    </button>
  );
}

export function OrbHub(props: OrbHubProps) {
  const { groups, items, loaded, voice } = props;
  const { t } = useT("orb");
  const pending = useSecurityStore((s) => s.pending);
  const [selected, setSelected] = useState<string | null>(null);
  const [composing, setComposing] = useState<string | null>(null);
  const [managerFor, setManagerFor] = useState<string | null>(null);
  const counts = hubCounts(items);
  const rowIds = useMemo(() => groups.flatMap((g) => g.items.map((i) => i.id)), [groups]);
  const projectNames = useMemo(() => new Map(groups.map((g) => [g.projectId, g.name])), [groups]);
  const open = pending.filter((a) => a.status === "Pending");

  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      if (composing || managerFor) {
        setComposing(null);
        setManagerFor(null);
      } else props.onClose();
      return;
    }
    const inField = e.target instanceof HTMLInputElement;
    if (inField) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((cur) => moveSelection(rowIds, cur, e.key === "ArrowDown" ? 1 : -1));
    }
  };

  const summary =
    counts.working + counts.waiting === 0 ? t("hub.summaryIdle") : t("hub.summary", { working: counts.working, waiting: counts.waiting });

  return (
    <section className="orb-hub" role="dialog" aria-label={t("hub.title")} onKeyDown={onKey}>
      <header className="orb-hub__head">
        <Logo size={18} />
        <div className="orb-hub__title">
          <span className="orb-hub__name">{t("hub.title")}</span>
          <span className="orb-hub__summary">{summary}</span>
        </div>
        <button type="button" className="orb-icon-btn" aria-label={t("hub.openMain")} title={t("hub.openMain")} onClick={props.onOpenMain}>
          <ExternalLink size={14} />
        </button>
        <button type="button" className="orb-icon-btn" aria-label={t("hub.settings")} title={t("hub.settings")} onClick={props.onOpenSettings}>
          <Settings2 size={14} />
        </button>
        <button type="button" className="orb-icon-btn" aria-label={t("hub.close")} title={t("hub.close")} onClick={props.onClose}>
          <X size={14} />
        </button>
      </header>

      {open.length > 0 && (
        <div className="orb-hub__approvals" role="list" aria-label={t("hub.approvals", { count: open.length })}>
          <div className="orb-hub__section-title">
            <span>{t("hub.approvals", { count: open.length })}</span>
            {open.length > 3 && (
              <button type="button" className="orb-text-btn" onClick={props.onOpenApprovals}>
                {t("hub.allApprovals")}
              </button>
            )}
          </div>
          {open.slice(0, 3).map((a) => (
            <ApprovalRow key={a.id} approval={a} projectName={a.projectId ? (projectNames.get(a.projectId) ?? null) : null} onReview={props.onOpenApprovals} />
          ))}
        </div>
      )}

      <div className="orb-hub__list">
        {!loaded && <div className="orb-hub__empty">{t("hub.loading")}</div>}
        {loaded && groups.length === 0 && (
          <div className="orb-hub__empty">
            <span>{t("hub.empty")}</span>
            <span className="orb-hub__empty-hint">{t("hub.emptyHint")}</span>
          </div>
        )}
        {groups.map((g) => (
          <section key={g.projectId} className="orb-group" aria-label={g.name}>
            <div className="orb-group__head">
              <span className="orb-group__swatch" style={{ background: g.color ?? "var(--ok-color-accent)" }} aria-hidden="true" />
              <span className="orb-group__name" title={g.name}>
                {g.name}
              </span>
              <span className="orb-group__count">{t("hub.groupCount", { count: g.items.length })}</span>
              <button
                type="button"
                className={clsx("orb-text-btn", managerFor === g.projectId && "orb-text-btn--on")}
                aria-expanded={managerFor === g.projectId}
                onClick={() => setManagerFor((cur) => (cur === g.projectId ? null : g.projectId))}
              >
                {t("hub.askManager")}
              </button>
            </div>
            {managerFor === g.projectId && (
              <Composer
                placeholder={t("hub.managerPlaceholder", { project: g.name })}
                label={t("hub.askManager")}
                onSend={(text) => props.onMessageManager(g.projectId, text)}
                onCancel={() => setManagerFor(null)}
              />
            )}
            <div role="list">
              {g.items.map((item) => (
                <Row
                  key={item.id}
                  item={item}
                  selected={selected === item.id || (selected === null && rowIds[0] === item.id)}
                  composing={composing === item.id}
                  onSelect={() => setSelected(item.id)}
                  onCompose={(on) => setComposing(on ? item.id : null)}
                  onAction={props.onAction}
                  onMessage={props.onMessage}
                />
              ))}
            </div>
          </section>
        ))}
      </div>

      <footer className="orb-hub__foot">
        <PushToTalk voice={voice} onPtt={props.onPtt} />
        <EmergencyStop />
      </footer>
    </section>
  );
}
