/**
 * Manager panel (§1, §5, §53, §57): the conversation with the AI Manager.
 * Streaming markdown replies, a virtualized message list, plan cards
 * (approve / edit / reject), live run cards (progress, agents, cost, pause /
 * cancel), the session resume banner, suggestion chips, the model
 * indicator, Natural vs Expert mode and "Ask Manager about selection" from
 * the Code panel. All data comes from the core over RPC (`manager.*`) and
 * live events.
 */
import "./manager.css";

import type { ChatAttachment, ManagerRequestOptions } from "@orkestr/protocol";
import { Button, EmptyState, IconButton, Spinner, Tabs } from "@orkestr/ui";
import { Eraser, FolderOpen, RefreshCw, Sparkles, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import type { PanelProps } from "../types";
import { Composer } from "./Composer";
import { errorMessage, managerActions, useManagerChat, useResumeSummary } from "./data";
import { type ChatItem, attachmentOf, chatItems, latestProposedPlan, pinnedRun, suggestions } from "./model";
import { MessageRow, ModelIndicator, ResumeBanner, StreamingRow, ThinkingRow, Toasts } from "./parts";
import { RunCard } from "./RunCard";
import { type ManagerMode, useManagerUi } from "./store";
import { VirtualList } from "./VirtualList";

const MODES: { id: ManagerMode; label: string }[] = [
  { id: "natural", label: "Natural" },
  { id: "expert", label: "Expert" },
];

type Row = ChatItem | { kind: "thinking"; id: string };

const rowKey = (r: Row) => r.id;

export default function ManagerPanel({ projectId }: PanelProps) {
  const { state, loading, error, reload, setConfig } = useManagerChat(projectId);
  const summary = useResumeSummary(projectId);
  const mode = useManagerUi((s) => s.mode);
  const setMode = useManagerUi((s) => s.setMode);
  const prefill = useManagerUi((s) => s.prefill);
  const setPrefill = useManagerUi((s) => s.setPrefill);
  const focusNonce = useManagerUi((s) => s.focusNonce);
  const requestFocus = useManagerUi((s) => s.requestFocus);
  const dismissed = useManagerUi((s) => s.dismissedResume);
  const setProposedPlan = useManagerUi((s) => s.setProposedPlan);
  const toast = useManagerUi((s) => s.toast);
  const [text, setText] = useState("");
  const [attachment, setAttachment] = useState<ChatAttachment | null>(null);
  const [pending, setPending] = useState(false);
  const [clearing, setClearing] = useState(false);

  // Consume a request from the Code panel ("Ask Manager about selection").
  useEffect(() => {
    if (!prefill || !projectId) return;
    if (prefill.projectId && prefill.projectId !== projectId) return;
    const a = attachmentOf(prefill);
    setAttachment(a);
    if (!a && prefill.text.trim()) setText((t) => (t.trim() ? `${t}\n${prefill.text}` : prefill.text));
    else if (a && !text.trim()) setText(`Explain this code from ${a.path}`);
    setPrefill(null);
    requestFocus();
  }, [prefill, projectId, setPrefill, requestFocus, text]);

  const proposed = latestProposedPlan(state);
  useEffect(() => {
    setProposedPlan(projectId && proposed ? { projectId, planId: proposed.id } : null);
  }, [projectId, proposed, setProposedPlan]);

  // "Thinking" placeholder until the first delta / stored reply arrives.
  useEffect(() => {
    if (state.streaming.length > 0 || !state.busy) setPending(false);
  }, [state.streaming.length, state.busy]);

  const rows: Row[] = useMemo(() => {
    const items: Row[] = chatItems(state);
    if ((pending || state.busy) && state.streaming.length === 0) items.push({ kind: "thinking", id: "thinking" });
    return items;
  }, [state, pending]);

  const tail = state.streaming.map((s) => s.text.length).join(",");
  const run = pinnedRun(state);

  const renderRow = useCallback(
    (r: Row) => {
      if (r.kind === "thinking") return <ThinkingRow />;
      if (r.kind === "streaming") return <StreamingRow text={r.text} />;
      return <MessageRow message={r.message} state={state} showRun={r.showRun} />;
    },
    [state],
  );

  const send = async (body: string, options: ManagerRequestOptions | undefined): Promise<boolean> => {
    if (!projectId) return false;
    setPending(true);
    try {
      await managerActions.chat(projectId, body, attachment, options);
      setText("");
      setAttachment(null);
      return true;
    } catch (e) {
      setPending(false);
      toast("danger", errorMessage(e));
      return false;
    }
  };

  const clear = async () => {
    if (!projectId) return;
    setClearing(true);
    try {
      await managerActions.clear(projectId);
      reload();
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setClearing(false);
    }
  };

  const resumeRun = async (runId: string) => {
    try {
      await managerActions.resume(runId);
      if (projectId) useManagerUi.getState().dismissResume(projectId);
    } catch (e) {
      toast("danger", errorMessage(e));
    }
  };

  if (!projectId) {
    return (
      <div className="mgr-panel mgr-panel--center">
        <EmptyState
          icon={<FolderOpen size={26} strokeWidth={1.6} />}
          title="No project open"
          description="Open or create a project, then tell the AI Manager what to build. It plans the work, creates tasks and runs agents for you."
        />
      </div>
    );
  }

  const showResume = summary && !summary.isEmpty && !dismissed.includes(projectId);
  const empty = state.messages.length === 0 && state.streaming.length === 0 && !state.busy && !pending;

  let body: JSX.Element;
  if (loading && state.messages.length === 0) {
    body = (
      <div className="mgr-loading">
        <Spinner size={18} /> Loading the conversation…
      </div>
    );
  } else if (error) {
    body = (
      <div className="mgr-panel--center">
        <EmptyState
          icon={<TriangleAlert size={26} strokeWidth={1.6} />}
          title="Could not load the conversation"
          description={error}
          action={
            <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
              Retry
            </Button>
          }
        />
      </div>
    );
  } else if (empty) {
    body = (
      <div className="mgr-panel--center mgr-welcome">
        <EmptyState
          icon={<Sparkles size={26} strokeWidth={1.6} />}
          title="What should we build?"
          description="Describe a goal in plain words. The Manager studies the project, proposes a plan with tasks and dependencies, and — once you approve — runs isolated agents in their own worktrees."
        />
      </div>
    );
  } else {
    body = <VirtualList className="mgr-list" aria-label="Conversation" items={rows} getKey={rowKey} render={renderRow} tailVersion={tail} />;
  }

  return (
    <div className="mgr-panel">
      <div className="mgr-toolbar">
        <Sparkles size={15} className="mgr-toolbar__icon" aria-hidden="true" />
        <strong className="mgr-toolbar__title">AI Manager</strong>
        <ModelIndicator config={state.config} onSaved={setConfig} />
        <div className="mgr-toolbar__spacer" />
        <Tabs aria-label="Manager mode" items={MODES} value={mode} onChange={setMode} />
        <IconButton
          label="New conversation (the current one is summarised)"
          icon={clearing ? <Spinner size={14} /> : <Eraser size={14} />}
          disabled={clearing || state.busy || state.messages.length === 0}
          onClick={() => void clear()}
          tooltip="bottom"
        />
        <IconButton label="Reload" icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>
      {showResume && summary && <ResumeBanner summary={summary} onResumeRun={(id) => void resumeRun(id)} />}
      <div className="mgr-body">{body}</div>
      {run && <RunCard run={run} compact />}
      {!state.busy && !pending && (
        <div className="mgr-chips" role="group" aria-label="Suggestions">
          {suggestions(state).map((s) => (
            <button
              key={s}
              type="button"
              className="mgr-chip"
              onClick={() => {
                setText(s);
                requestFocus();
              }}
            >
              {s}
            </button>
          ))}
        </div>
      )}
      <Composer
        disabled={false}
        busy={state.busy || pending}
        expert={mode === "expert"}
        focusNonce={focusNonce}
        text={text}
        onText={setText}
        attachment={attachment}
        onClearAttachment={() => setAttachment(null)}
        onSend={send}
      />
      <Toasts />
    </div>
  );
}
