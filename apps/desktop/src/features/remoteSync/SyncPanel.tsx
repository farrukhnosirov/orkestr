/**
 * Settings → Remote & Sync → encrypted sync of the active project:
 * status, configure (gateway URL, token, remote key), Sync now, Seamless
 * Continue (publish / resume) and the conflict list (ADR-0031).
 */
import type { SyncConflictInfo, SyncResolution, SyncStatusInfo } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { formatDate } from "../../i18n/format";
import { useAppStore } from "../../store/app";
import { SettingGroup, SettingRow } from "../settings/controls";
import { captureUiState, isUnimplemented, restoreUiState, syncErrorKind } from "./logic";

type Notice = { tone: "ok" | "error"; text: string } | null;

export function SyncPanel({ projectId }: { projectId: string | null }) {
  const { t } = useT("sync");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const [status, setStatus] = useState<SyncStatusInfo | null>(null);
  const [conflicts, setConflicts] = useState<SyncConflictInfo[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const [generatedKey, setGeneratedKey] = useState<string | null>(null);
  const [form, setForm] = useState({ serverUrl: "", accessToken: "", remoteKey: "", remoteProject: "" });

  const refresh = useCallback(async () => {
    if (!projectId) return;
    try {
      const c = getClient();
      const s = await c.call("sync.status", { projectId });
      setStatus(s);
      const l = await c.call("sync.listConflicts", { projectId });
      setConflicts(l.conflicts);
    } catch (e) {
      if (isUnimplemented(e)) setUnavailable(true);
      else setNotice({ tone: "error", text: describeError(e) });
    }
  }, [projectId]);

  useEffect(() => {
    setStatus(null);
    setConflicts([]);
    setUnavailable(false);
    setNotice(null);
    setGeneratedKey(null);
    void refresh();
  }, [refresh]);

  if (!projectId || !project) return <p className="rs-muted">{t("sync.noProject")}</p>;
  if (unavailable) return <p className="rs-muted" role="status">{t("sync.unavailable")}</p>;

  const policyDenied = project.privacy.sync === "NoRemoteSync" || project.classification === "Secret";

  const run = async (key: string, fn: () => Promise<string | null>) => {
    setBusy(key);
    setNotice(null);
    try {
      const text = await fn();
      if (text) setNotice({ tone: "ok", text });
    } catch (e) {
      if (isUnimplemented(e)) setUnavailable(true);
      else if (syncErrorKind(e) === "policyDenied") setNotice({ tone: "error", text: t("sync.policyDenied") });
      else setNotice({ tone: "error", text: describeError(e) });
    } finally {
      setBusy(null);
      await refresh();
    }
  };

  const configure = () =>
    run("configure", async () => {
      const r = await getClient().call("sync.configure", {
        projectId,
        serverUrl: form.serverUrl.trim(),
        accessToken: form.accessToken,
        remoteKey: form.remoteKey.trim() || undefined,
        remoteProject: form.remoteProject.trim() || undefined,
      });
      setGeneratedKey(r.generatedRemoteKey ?? null);
      setForm((f) => ({ ...f, accessToken: "", remoteKey: "" }));
      return null;
    });
  const disable = () =>
    run("disable", async () => {
      await getClient().call("sync.configure", { projectId, disable: true });
      return null;
    });
  const syncNow = () =>
    run("now", async () => {
      const r = await getClient().call("sync.now", { projectId });
      return t("sync.synced", { pushed: r.pushed, pulled: r.pulled, conflicts: r.conflicts });
    });
  const continueOn = () =>
    run("continue", async () => {
      const r = await getClient().call("sync.continueOn", { projectId, uiState: captureUiState() });
      return t("sync.continued", { summary: r.summary ?? "" });
    });
  const resume = () =>
    run("resume", async () => {
      const r = await getClient().call("sync.resume", { projectId });
      restoreUiState(r.uiState);
      return t("sync.resumed", { summary: r.summary ?? "" });
    });
  const resolve = (id: string, resolution: SyncResolution) =>
    run(`resolve:${id}`, async () => {
      await getClient().call("sync.resolveConflict", { projectId, conflictId: id, resolution });
      return null;
    });

  const configured = status?.configured ?? false;
  return (
    <div className="rs-panel">
      <SettingGroup title={t("sync.group")}>
        <p className="rs-muted">{t("sync.intro", { name: project.name })}</p>
        {policyDenied && (
          <p className="rs-error" role="alert">
            {t("sync.policyDenied")}
          </p>
        )}
        {!project.rootPath && <p className="rs-muted">{t("sync.noRoot")}</p>}
        <SettingRow title={t("sync.status")}>
          <span className="rs-status">
            <Badge tone={configured ? "success" : "neutral"}>{configured ? t("sync.configured") : t("sync.notConfigured")}</Badge>
            {configured && (
              <span className="rs-muted">
                {status?.lastSyncAt ? t("sync.lastSync", { time: formatDate(status.lastSyncAt) }) : t("sync.never")}
              </span>
            )}
            {status && status.pending > 0 && <span className="rs-muted">{t("sync.pending", { count: status.pending })}</span>}
          </span>
        </SettingRow>
        {status?.lastError && <p className="rs-error">{t("sync.lastError", { error: status.lastError })}</p>}
        {!policyDenied && project.rootPath && (
          <div className="rs-form">
            <Input
              label={t("sync.serverUrl")}
              hint={t("sync.serverUrlHint")}
              value={form.serverUrl}
              onChange={(e) => setForm({ ...form, serverUrl: e.target.value })}
            />
            <Input
              label={t("sync.token")}
              type="password"
              value={form.accessToken}
              onChange={(e) => setForm({ ...form, accessToken: e.target.value })}
            />
            <Input
              label={t("sync.remoteKey")}
              hint={t("sync.remoteKeyHint")}
              type="password"
              value={form.remoteKey}
              onChange={(e) => setForm({ ...form, remoteKey: e.target.value })}
            />
            <Input
              label={t("sync.remoteProject")}
              hint={t("sync.remoteProjectHint")}
              value={form.remoteProject}
              placeholder={project.slug}
              onChange={(e) => setForm({ ...form, remoteProject: e.target.value })}
            />
            <div className="rs-actions">
              <Button
                variant="primary"
                loading={busy === "configure"}
                disabled={busy !== null || !form.serverUrl.trim() || !form.accessToken}
                onClick={() => void configure()}
              >
                {t("sync.save")}
              </Button>
              {configured && (
                <Button variant="ghost" loading={busy === "disable"} disabled={busy !== null} onClick={() => void disable()}>
                  {t("sync.disable")}
                </Button>
              )}
            </div>
          </div>
        )}
        {generatedKey && (
          <div className="rs-key" role="status">
            <span>{t("sync.generatedKey")}</span>
            <code className="rs-code">{generatedKey}</code>
          </div>
        )}
        {configured && !policyDenied && (
          <div className="rs-actions">
            <Button loading={busy === "now"} disabled={busy !== null} onClick={() => void syncNow()}>
              {t("sync.syncNow")}
            </Button>
            <Button variant="primary" loading={busy === "continue"} disabled={busy !== null} onClick={() => void continueOn()} title={t("sync.continueHint")}>
              {t("sync.continueOn")}
            </Button>
            <Button loading={busy === "resume"} disabled={busy !== null} onClick={() => void resume()}>
              {t("sync.resume")}
            </Button>
          </div>
        )}
        {notice && (
          <p className={notice.tone === "ok" ? "rs-ok" : "rs-error"} role={notice.tone === "ok" ? "status" : "alert"}>
            {notice.text}
          </p>
        )}
      </SettingGroup>
      {configured && (
        <SettingGroup title={t("conflicts.group")}>
          {conflicts.length === 0 && <p className="rs-muted">{t("conflicts.none")}</p>}
          <ul className="rs-list">
            {conflicts.map((c) => (
              <li key={c.id} className="rs-list__item">
                <div className="rs-list__main">
                  <code>{c.path}</code>
                  {c.conflictCopy && <span className="rs-muted">{t("conflicts.copy", { copy: c.conflictCopy })}</span>}
                </div>
                <div className="rs-actions">
                  <Button size="sm" disabled={busy !== null} onClick={() => void resolve(c.id, "keepLocal")}>
                    {t("conflicts.keepLocal")}
                  </Button>
                  <Button size="sm" disabled={busy !== null} onClick={() => void resolve(c.id, "keepRemote")}>
                    {t("conflicts.keepRemote")}
                  </Button>
                  <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void resolve(c.id, "keepBoth")}>
                    {t("conflicts.keepBoth")}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </SettingGroup>
      )}
    </div>
  );
}
