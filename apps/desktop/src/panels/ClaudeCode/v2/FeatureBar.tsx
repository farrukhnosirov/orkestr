/**
 * «Возможности» bar: the Claude Code features that are hidden behind slash
 * commands, flags or the control protocol, as buttons and toggles — plan
 * mode, accept edits, extended thinking, effort, compact, context meter, MCP
 * servers, extra working folder (`--add-dir`), export to Markdown, copy the
 * last answer, retry / edit the last message, continue in the Orkestr
 * terminal or in VS Code.
 */
import type { ClaudeContextUsage, ClaudeMcpServer } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import {
  Archive,
  Brain,
  ClipboardCopy,
  ClipboardList,
  Code2,
  Download,
  FilePen,
  FolderPlus,
  Gauge,
  Layers,
  Pencil,
  Plug,
  RotateCcw,
  SquareTerminal,
} from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useState } from "react";

import { chatMarkdown, exportFileName, lastAnswer, lastPrompt } from "../../../features/claudeCode/chatTools";
import { useClaudeStore, type SessionView } from "../../../features/claudeCode/store";
import { downloadText } from "../../../features/theme/i18nText";
import { workspaceActions } from "../../../components/workspaceActions";
import { useLauncherStore } from "../../../features/vscode/launcher";
import { useFormat, useT } from "../../../i18n";
import { useTerminalUi } from "../../Terminal/store";
import { Popover } from "./Popover";

const EFFORT = ["low", "medium", "high", "xhigh", "max"] as const;

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

function Chip({
  icon,
  label,
  hint,
  pressed,
  disabled,
  onClick,
  children,
  tone,
}: {
  icon: ReactNode;
  label: string;
  hint: string;
  pressed?: boolean;
  disabled?: boolean;
  onClick: () => void;
  children?: ReactNode;
  tone?: "accent" | "warn";
}) {
  return (
    <button
      type="button"
      className={clsx("cc2-feat", pressed && "cc2-feat--on", tone && `cc2-feat--${tone}`)}
      aria-pressed={pressed}
      title={hint}
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
      <span>{label}</span>
      {children}
    </button>
  );
}

function ContextBody({ ctx, onCompact }: { ctx: ClaudeContextUsage | null; onCompact: () => void }) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  if (!ctx) return <Spinner size={14} label={t("mcp.loading")} />;
  const used = ctx.categories.filter((c) => c.kind === "used" && c.tokens > 0);
  return (
    <div className="cc2-ctx">
      <h3 className="cc2-pop__title">{t("context.title")}</h3>
      <div className="cc2-ctx__total">
        {t("context.used", { used: fmt.integer(ctx.totalTokens), max: fmt.integer(ctx.maxTokens) })} · {fmt.percent(ctx.percentage / 100)}
      </div>
      <div className="cc2-ctx__stack" aria-hidden="true">
        {used.map((c, i) => (
          <span key={c.name} className={`cc2-ctx__seg cc2-ctx__seg--${i % 6}`} style={{ width: `${(c.tokens / Math.max(1, ctx.maxTokens)) * 100}%` }} />
        ))}
      </div>
      <ul className="cc2-ctx__list">
        {used.map((c, i) => (
          <li key={c.name}>
            <span className={`cc2-ctx__dot cc2-ctx__seg--${i % 6}`} aria-hidden="true" />
            <span className="cc2-ctx__name">{c.name}</span>
            <span>{fmt.integer(c.tokens)}</span>
          </li>
        ))}
      </ul>
      <Button size="sm" leftIcon={<Archive size={12} />} onClick={onCompact}>
        {t("context.compactNow")}
      </Button>
    </div>
  );
}

function McpBody({ servers }: { servers: ClaudeMcpServer[] | null }) {
  const { t } = useT("claudeCodePanel");
  if (!servers) return <Spinner size={14} label={t("mcp.loading")} />;
  const known = ["connected", "needs-auth", "failed", "pending", "disabled"];
  return (
    <div className="cc2-mcp">
      <h3 className="cc2-pop__title">{t("mcp.title")}</h3>
      {servers.length === 0 ? (
        <p className="cc2-side__empty">{t("mcp.empty")}</p>
      ) : (
        <ul className="cc2-mcp__list">
          {servers.map((s) => (
            <li key={s.name} title={s.error ?? undefined}>
              <span className={clsx("cc2-mcp__dot", `cc2-mcp__dot--${s.status}`)} aria-hidden="true" />
              <span className="cc2-mcp__name">{s.name}</span>
              <span className="cc2-mcp__status">
                {known.includes(s.status) ? t(`mcp.status.${s.status}` as "mcp.status.connected") : s.status}
                {s.tools > 0 && ` · ${t("mcp.tools", { count: s.tools })}`}
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="cc2-pop__hint">{t("mcp.hint")}</p>
    </div>
  );
}

function AddDirForm({ current, onSubmit }: { current: string[]; onSubmit: (dir: string) => void }) {
  const { t } = useT("claudeCodePanel");
  const [dir, setDir] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (dir.trim()) onSubmit(dir.trim());
  };
  return (
    <form className="cc2-adddir" onSubmit={submit}>
      <h3 className="cc2-pop__title">{t("features.addDirTitle")}</h3>
      <p className="cc2-pop__hint">{t("features.addDirHint")}</p>
      {current.length > 0 && <p className="cc2-pop__hint">{t("features.addDirCurrent", { dirs: current.join(", ") })}</p>}
      <input
        className="cc2-input"
        value={dir}
        autoFocus
        placeholder={t("features.addDirPlaceholder")}
        aria-label={t("features.addDirLabel")}
        onChange={(e) => setDir(e.target.value)}
      />
      <Button type="submit" size="sm" variant="primary" disabled={!dir.trim()} leftIcon={<FolderPlus size={12} />}>
        {t("features.addDirSubmit")}
      </Button>
    </form>
  );
}

export interface FeatureBarProps {
  projectId: string;
  view: SessionView;
  title: string;
  /** Sends a message / slash command to the chat. */
  onSend: (text: string) => void;
}

export function FeatureBar({ projectId, view, title, onSend }: FeatureBarProps) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const { setMode, setThinking, loadContext, loadMcp, restartWithDirs, requestInsert, openInTerminal } = useClaudeStore.getState();
  const ctx = useClaudeStore((s) => s.context[view.info.sessionId] ?? null);
  const mcp = useClaudeStore((s) => s.mcp[view.info.sessionId] ?? null);
  const [pop, setPop] = useState<"context" | "mcp" | "addDir" | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const sid = view.info.sessionId;
  const ended = view.info.state === "exited" || view.info.state === "failed";
  const running = view.chat.running;
  const mode = view.info.permissionMode;
  const answer = lastAnswer(view.chat);
  const prompt = lastPrompt(view.chat);
  const claudeId = view.chat.claudeSessionId ?? view.info.claudeSessionId;
  const has = (name: string) => view.chat.commands.some((c) => c.name === name);
  const model = view.chat.models.find((m) => m.value === (view.info.model ?? "default")) ?? view.chat.models.find((m) => m.value === "default");
  const effortLevels = model?.effortLevels ?? (has("effort") ? [...EFFORT] : []);
  const close = useCallback(() => setPop(null), []);

  const say = (text: string) => {
    setFlash(text);
    setTimeout(() => setFlash((f) => (f === text ? null : f)), 2000);
  };

  const toggleMode = (target: "plan" | "acceptEdits") => void setMode(sid, mode === target ? "default" : target);

  const openPop = (p: "context" | "mcp" | "addDir") => {
    if (pop === p) {
      setPop(null);
      return;
    }
    setPop(p);
    if (p === "context") void loadContext(sid);
    if (p === "mcp") void loadMcp(sid);
  };

  const exportChat = async () => {
    const md = chatMarkdown(view.chat, title, {
      user: t("features.export_labels.user"),
      claude: t("features.export_labels.claude"),
      tool: t("features.export_labels.tool"),
    });
    const file = exportFileName(title, new Date());
    downloadText(file, md);
    await copy(md);
    say(t("features.exported", { file }));
  };

  const toTerminal = async () => {
    const id = await openInTerminal(projectId, claudeId, view.info.cwd);
    if (id) {
      useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
    }
  };

  // The same chat in the Claude Code extension of Orkestr's VS Code window:
  // `claude-vscode.editor.open(sessionId)` is the command the extension's own
  // session list and its `vscode://anthropic.claude-code/open?session=` URI
  // handler use; it runs through the Orkestr bridge (which opens the window
  // on the project first). The extension looks the transcript up by the
  // window's folder, so a chat started in a sub-folder / worktree opens as a
  // new conversation there; the id is copied for /resume in that case.
  const toVsCode = async () => {
    if (claudeId) await copy(claudeId);
    say(t("features.vscodeOpening"));
    const ok = await useLauncherStore.getState().launch({
      kind: "command",
      commandId: "claude-vscode.editor.open",
      title: t("features.vscode"),
      args: claudeId ? [claudeId] : [],
    });
    if (!ok) {
      say(useLauncherStore.getState().last?.message ?? t("features.vscodeFailed"));
      workspaceActions.openPanel("vscode");
    }
  };

  const ctxLabel = ctx ? fmt.percent(ctx.percentage / 100) : null;

  return (
    <div className="cc2-feats" role="toolbar" aria-label={t("features.label")}>
      <Chip
        icon={<ClipboardList size={13} aria-hidden />}
        label={t("features.plan")}
        hint={t("features.planHint")}
        pressed={mode === "plan"}
        disabled={ended}
        onClick={() => toggleMode("plan")}
      />
      <Chip
        icon={<FilePen size={13} aria-hidden />}
        label={t("features.acceptEdits")}
        hint={t("features.acceptEditsHint")}
        pressed={mode === "acceptEdits"}
        disabled={ended}
        onClick={() => toggleMode("acceptEdits")}
      />
      <Chip
        icon={<Brain size={13} aria-hidden />}
        label={t("features.thinking")}
        hint={`${view.thinking === false ? t("features.thinkingOff") : view.thinking ? t("features.thinkingOn") : t("features.thinking")} — ${t("features.thinkingHint")}`}
        pressed={view.thinking === true}
        disabled={ended}
        onClick={() => void setThinking(sid, view.thinking !== true)}
      />
      {effortLevels.length > 0 && (
        <label className="cc2-feat cc2-feat--select" title={t("features.effortHint")}>
          <Gauge size={13} aria-hidden />
          <span>{t("features.effort")}</span>
          <select
            aria-label={t("features.effort")}
            value=""
            disabled={ended || running}
            onChange={(e) => {
              if (e.target.value) onSend(`/effort ${e.target.value}`);
            }}
          >
            <option value="">—</option>
            {effortLevels.map((l) => (
              <option key={l} value={l}>
                {(EFFORT as readonly string[]).includes(l) ? t(`features.effortLevel.${l}` as "features.effortLevel.low") : l}
              </option>
            ))}
          </select>
        </label>
      )}
      <span className="cc2-feats__sep" aria-hidden="true" />
      <Chip
        icon={<Archive size={13} aria-hidden />}
        label={t("features.compact")}
        hint={t("features.compactHint")}
        disabled={ended || running}
        onClick={() => onSend("/compact")}
      />
      <div className="cc2-anchor">
        <Chip
          icon={<Layers size={13} aria-hidden />}
          label={t("features.context")}
          hint={t("features.contextHint")}
          pressed={pop === "context"}
          disabled={ended}
          onClick={() => openPop("context")}
        >
          {ctxLabel && <span className="cc2-feat__value">{t("features.contextValue", { percent: ctxLabel })}</span>}
        </Chip>
        <Popover open={pop === "context"} onClose={close} label={t("context.title")}>
          <ContextBody
            ctx={ctx}
            onCompact={() => {
              setPop(null);
              onSend("/compact");
            }}
          />
        </Popover>
      </div>
      <div className="cc2-anchor">
        <Chip icon={<Plug size={13} aria-hidden />} label={t("features.mcp")} hint={t("features.mcpHint")} pressed={pop === "mcp"} disabled={ended} onClick={() => openPop("mcp")} />
        <Popover open={pop === "mcp"} onClose={close} label={t("mcp.title")}>
          <McpBody servers={mcp} />
        </Popover>
      </div>
      <div className="cc2-anchor">
        <Chip
          icon={<FolderPlus size={13} aria-hidden />}
          label={t("features.addDir")}
          hint={t("features.addDirHint")}
          pressed={pop === "addDir"}
          disabled={running}
          onClick={() => openPop("addDir")}
        />
        <Popover open={pop === "addDir"} onClose={close} label={t("features.addDirTitle")}>
          <AddDirForm
            current={view.addDirs}
            onSubmit={(dir) => {
              setPop(null);
              void restartWithDirs(projectId, sid, [...view.addDirs.filter((d) => d !== dir), dir]);
            }}
          />
        </Popover>
      </div>
      <span className="cc2-feats__sep" aria-hidden="true" />
      <Chip icon={<Download size={13} aria-hidden />} label={t("features.export")} hint={t("features.exportHint")} disabled={view.chat.items.length === 0} onClick={() => void exportChat()} />
      <Chip
        icon={<ClipboardCopy size={13} aria-hidden />}
        label={t("features.copyAnswer")}
        hint={t("features.copyAnswerHint")}
        disabled={!answer}
        onClick={() => {
          if (answer) void copy(answer).then((ok) => say(ok ? t("features.copied") : t("errors.clipboard")));
        }}
      />
      <Chip
        icon={<RotateCcw size={13} aria-hidden />}
        label={t("features.retry")}
        hint={t("features.retryHint")}
        disabled={!prompt || ended || running}
        onClick={() => {
          if (prompt) onSend(prompt);
        }}
      />
      <Chip
        icon={<Pencil size={13} aria-hidden />}
        label={t("features.editLast")}
        hint={t("features.editLastHint")}
        disabled={!prompt || ended}
        onClick={() => {
          if (prompt) requestInsert(prompt, true);
        }}
      />
      <span className="cc2-feats__sep" aria-hidden="true" />
      <Chip icon={<SquareTerminal size={13} aria-hidden />} label={t("features.terminal")} hint={t("features.terminalHint")} onClick={() => void toTerminal()} />
      <Chip icon={<Code2 size={13} aria-hidden />} label={t("features.vscode")} hint={t("features.vscodeHint")} onClick={() => void toVsCode()} />
      {flash && (
        <span className="cc2-flash" role="status">
          {flash}
        </span>
      )}
    </div>
  );
}
