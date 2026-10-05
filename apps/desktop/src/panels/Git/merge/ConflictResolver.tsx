/**
 * Conflict view of a stopped merge: conflicted files, a 3-way-ish view
 * (ours ↔ theirs, base ↔ result, editable result) and resolutions — ours,
 * theirs, edited content, or an AI suggestion the user reviews and saves.
 */
import type { MergeFileConflict, MergeManagerProposal, MergeResolution } from "@orkestr/protocol";
import { Badge, Button, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { CheckCircle2, FileWarning, Save, Sparkles } from "lucide-react";
import { Suspense, lazy, useEffect, useMemo, useState } from "react";

import { getClient } from "../../../core/client";
import { useT } from "../../../i18n";
import { Notice } from "../components";
import { languageForPath } from "../model";
import { errorMessage } from "../state";
import { openConflicts } from "./model";

const MonacoDiff = lazy(() => import("../MonacoDiff"));
const MonacoResult = lazy(() => import("./MonacoResult"));

type View = "diff" | "base" | "result";

const MARKERS = /^(<{7}|={7}|>{7})( |$)/m;

/** Whether text still contains git conflict markers. */
export function hasConflictMarkers(text: string): boolean {
  return MARKERS.test(text);
}

interface Props {
  proposal: MergeManagerProposal;
  onUpdated: (p: MergeManagerProposal) => void;
}

export function ConflictResolver({ proposal, onUpdated }: Props) {
  const { t } = useT("merge");
  const conflicts = useMemo(() => proposal.execution?.conflicts ?? [], [proposal.execution]);
  const theirsBranch = proposal.execution?.conflictBranch ?? "";
  const [selected, setSelected] = useState<string | null>(conflicts[0]?.path ?? null);
  useEffect(() => {
    if (!selected || !conflicts.some((c) => c.path === selected)) setSelected(conflicts.find((c) => !c.resolved)?.path ?? conflicts[0]?.path ?? null);
  }, [conflicts, selected]);
  const current = conflicts.find((c) => c.path === selected) ?? null;
  const open = openConflicts(proposal);

  return (
    <section className="mm-section">
      <h3 className="mm-heading">
        {t("resolve.title")}
        <Badge tone={open ? "warning" : "success"}>{open ? t("resolve.open", { count: open }) : t("resolve.allResolved")}</Badge>
      </h3>
      <div className="mm-conflicts">
        <ul className="git-list mm-conflicts__files">
          {conflicts.map((c) => (
            <li key={c.path} className={clsx("git-row", c.path === selected && "git-row--selected")}>
              <button type="button" className="git-row__main mm-conflicts__file" onClick={() => setSelected(c.path)} title={c.path}>
                {c.resolved ? <CheckCircle2 size={13} className="mm-ok" /> : <FileWarning size={13} className="mm-warn" />}
                <span className="git-row__name mm-mono">{c.path}</span>
                <span className="git-small git-muted">{c.resolved ? t("resolve.resolved") : t("resolve.unresolved")}</span>
              </button>
            </li>
          ))}
        </ul>
        {current ? (
          <FileResolver key={current.path} proposal={proposal} conflict={current} theirsBranch={theirsBranch} onUpdated={onUpdated} />
        ) : (
          <p className="git-muted git-pad">{t("resolve.selectFile")}</p>
        )}
      </div>
    </section>
  );
}

interface FileProps {
  proposal: MergeManagerProposal;
  conflict: MergeFileConflict;
  theirsBranch: string;
  onUpdated: (p: MergeManagerProposal) => void;
}

function FileResolver({ proposal, conflict, theirsBranch, onUpdated }: FileProps) {
  const { t } = useT("merge");
  const [view, setView] = useState<View>("diff");
  const [draft, setDraft] = useState(conflict.result ?? "");
  const [version, setVersion] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const language = languageForPath(conflict.path);
  const missing = t("resolve.missing");

  const replace = (text: string) => {
    setDraft(text);
    setVersion((v) => v + 1);
    setView("result");
  };

  const resolve = async (resolution: MergeResolution, tag: string) => {
    setBusy(tag);
    setError(null);
    try {
      const r = await getClient().call("merge.resolveConflict", { proposalId: proposal.id, path: conflict.path, resolution });
      onUpdated(r.proposal);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const markers = hasConflictMarkers(draft);
  const views = [
    { id: "diff" as const, label: t("resolve.viewDiff") },
    { id: "base" as const, label: t("resolve.viewBase") },
    { id: "result" as const, label: t("resolve.viewResult") },
  ];
  const loading = (
    <div className="git-center">
      <Spinner size={16} /> {t("resolve.editorLoading")}
    </div>
  );

  return (
    <div className="mm-resolver">
      {conflict.binary ? (
        <Notice tone="warning">{t("resolve.binary")}</Notice>
      ) : (
        <>
          <div className="mm-resolver__bar">
            <Tabs aria-label={t("resolve.views")} items={views} value={view} onChange={setView} />
            <span className="git-small git-muted mm-resolver__sides">
              {view === "diff"
                ? `${t("resolve.ours")} ↔ ${t("resolve.theirs", { branch: theirsBranch })}`
                : view === "base"
                  ? `${t("resolve.base")} ↔ ${t("resolve.result")}`
                  : t("resolve.result")}
            </span>
          </div>
          <div className="mm-resolver__editor">
            <Suspense fallback={loading}>
              {view === "result" ? (
                <MonacoResult value={draft} version={version} language={language} ariaLabel={t("resolve.result")} onChange={setDraft} />
              ) : (
                <MonacoDiff
                  original={view === "diff" ? (conflict.ours ?? missing) : (conflict.base ?? missing)}
                  modified={view === "diff" ? (conflict.theirs ?? missing) : draft}
                  language={language}
                  sideBySide
                />
              )}
            </Suspense>
          </div>
          {conflict.suggestion != null && (
            <Notice
              tone="info"
              action={
                <Button size="sm" variant="secondary" onClick={() => replace(conflict.suggestion ?? "")}>
                  {t("resolve.useSuggestion")}
                </Button>
              }
            >
              {t("resolve.aiReady")}
              {conflict.suggestionNote ? ` ${t("resolve.aiNote", { note: conflict.suggestionNote })}` : ""}
            </Notice>
          )}
          {markers && <Notice tone="warning">{t("resolve.markers")}</Notice>}
        </>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
      <div className="mm-resolver__actions">
        <Button size="sm" variant="secondary" loading={busy === "ours"} disabled={busy !== null} onClick={() => void resolve({ kind: "Ours" }, "ours")}>
          {t("resolve.useOurs")}
        </Button>
        <Button size="sm" variant="secondary" loading={busy === "theirs"} disabled={busy !== null} onClick={() => void resolve({ kind: "Theirs" }, "theirs")}>
          {t("resolve.useTheirs")}
        </Button>
        {!conflict.binary && (
          <>
            <Button
              size="sm"
              variant="secondary"
              leftIcon={<Sparkles size={13} />}
              loading={busy === "ai"}
              disabled={busy !== null}
              onClick={() => void resolve({ kind: "Ai" }, "ai")}
            >
              {t("resolve.askAi")}
            </Button>
            <div className="git-spacer" />
            <Button
              size="sm"
              variant="primary"
              leftIcon={<Save size={13} />}
              loading={busy === "content"}
              disabled={busy !== null || markers}
              onClick={() => void resolve({ kind: "Content", content: draft }, "content")}
            >
              {t("resolve.save")}
            </Button>
          </>
        )}
      </div>
    </div>
  );
}
