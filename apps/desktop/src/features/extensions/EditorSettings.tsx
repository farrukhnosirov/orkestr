/**
 * Settings → Editor (ADR-0025): editor color theme (separate from the
 * Orkestr UI theme), file icon theme, and language servers (status,
 * pre-approval, revoke, stop).
 */
import "./extensions.css";

import type { EditorPrefs, LspServerInfo, LspSessionInfo, ThemeRef } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { SettingGroup, SettingRow } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { ensureExtensionsLoaded, useExtensionsStore } from "./store";

const NONE = "";
const refKey = (r: ThemeRef | null): string => (r ? `${r.extensionId}\u0000${r.themeId}` : NONE);
const parseRef = (key: string): ThemeRef | null => {
  if (!key) return null;
  const [extensionId, themeId] = key.split("\u0000");
  return extensionId && themeId ? { extensionId, themeId } : null;
};

function ServerRow({
  server,
  sessions,
  projectId,
  onChanged,
}: {
  server: LspServerInfo;
  sessions: LspSessionInfo[];
  projectId: string | null;
  onChanged: () => void;
}) {
  const { t } = useT("extensions");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [approvalId, setApprovalId] = useState<string | null>(null);

  const act = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      onChanged();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  /** Pre-approve: request the approval, approve it, verify by starting once, then stop. */
  const allow = () =>
    act(async () => {
      if (!projectId) return;
      const client = getClient();
      let id = approvalId;
      if (!id) {
        try {
          const res = await client.call("lsp.start", { projectId, serverId: server.id });
          await client.call("lsp.stop", { sessionId: res.session.sessionId });
          return;
        } catch (e) {
          const data = (e as { data?: { approvalId?: string } }).data;
          if (!data?.approvalId) throw e;
          id = data.approvalId;
          setApprovalId(id);
        }
      }
      await client.call("approvals.resolve", { approvalId: id, decision: "Approve" });
      const res = await client.call("lsp.start", { projectId, serverId: server.id, approvalId: id });
      await client.call("lsp.stop", { sessionId: res.session.sessionId });
      setApprovalId(null);
    });

  return (
    <SettingRow
      title={server.name}
      description={
        server.available
          ? server.command
            ? t("lsp.foundAt", { command: server.command })
            : t("lsp.found")
          : t("lsp.notFound", { hint: server.installHint })
      }
    >
      <div className="ext-lsp-row">
        <span className="ext-lsp-row__langs">{server.languages.join(", ")}</span>
        {server.source?.kind === "extension" && <Badge tone="info">{t("lsp.fromExtension", { id: server.source.extensionId })}</Badge>}
        {server.available && (server.trusted ? <Badge tone="success">{t("lsp.trusted")}</Badge> : <Badge tone="warning">{t("lsp.untrusted")}</Badge>)}
        {sessions.map((s) => (
          <span key={s.sessionId} className="ext-lsp-row__session">
            <Badge tone={s.state === "running" ? "success" : s.state === "failed" ? "danger" : "neutral"}>{t(`lsp.state.${s.state}`)}</Badge>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => getClient().call("lsp.stop", { sessionId: s.sessionId }).then(() => undefined))}>
              {t("lsp.stop")}
            </Button>
          </span>
        ))}
        {server.available && !server.trusted && (
          <Button size="sm" variant="primary" disabled={busy || !projectId} title={projectId ? undefined : t("lsp.needsProject")} onClick={() => void allow()}>
            {t("lsp.allow")}
          </Button>
        )}
        {server.trusted && (
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => getClient().call("lsp.revoke", { serverId: server.id }).then(() => undefined))}>
            {t("lsp.revoke")}
          </Button>
        )}
        {busy && <Spinner size={12} />}
        {error && <span className="ext-error">{error}</span>}
      </div>
    </SettingRow>
  );
}

export function EditorSettings({ projectId }: SettingsSectionProps) {
  const { t } = useT("extensions");
  const connected = useAppStore((s) => s.connection.state === "open");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const installed = useExtensionsStore((s) => s.installed);
  const prefs = useExtensionsStore((s) => s.prefs);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [servers, setServers] = useState<LspServerInfo[] | null>(null);
  const [sessions, setSessions] = useState<LspSessionInfo[]>([]);

  useEffect(() => {
    if (connected) ensureExtensionsLoaded();
  }, [connected]);

  const loadServers = useCallback(() => {
    if (!connected) return;
    getClient()
      .call("lsp.servers", { refresh: true })
      .then((r) => {
        setServers(r.servers);
        setSessions(r.sessions);
      })
      .catch((e: unknown) => setError(describeError(e)));
  }, [connected]);
  useEffect(loadServers, [loadServers, installed]);

  const enabled = installed.filter((e) => e.enabled);
  const themes = enabled.flatMap((e) => e.contributions.themes.map((th) => ({ ext: e, th })));
  const iconThemes = enabled.flatMap((e) => e.contributions.iconThemes.map((th) => ({ ext: e, th })));

  const save = async (next: EditorPrefs) => {
    setSaving(true);
    setError(null);
    try {
      const saved = await getClient().call("extensions.setPrefs", next);
      useExtensionsStore.setState({ prefs: saved, version: useExtensionsStore.getState().version + 1 });
    } catch (e) {
      setError(describeError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="ext-settings">
      <SettingGroup title={t("settings.appearance")}>
        <SettingRow title={t("settings.colorTheme")} description={t("settings.colorThemeHint")}>
          <select
            className="set-select"
            aria-label={t("settings.colorTheme")}
            value={refKey(prefs.editorTheme)}
            disabled={saving}
            onChange={(e) => void save({ ...prefs, editorTheme: parseRef(e.target.value) })}
          >
            <option value={NONE}>{t("settings.followUiTheme")}</option>
            {themes.map(({ ext, th }) => (
              <option key={`${ext.id}/${th.id}`} value={refKey({ extensionId: ext.id, themeId: th.id })}>
                {t("settings.themeOption", { theme: th.label, extension: ext.displayName })}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow title={t("settings.iconTheme")} description={t("settings.iconThemeHint")}>
          <select
            className="set-select"
            aria-label={t("settings.iconTheme")}
            value={refKey(prefs.iconTheme)}
            disabled={saving}
            onChange={(e) => void save({ ...prefs, iconTheme: parseRef(e.target.value) })}
          >
            <option value={NONE}>{t("settings.builtinIcons")}</option>
            {iconThemes.map(({ ext, th }) => (
              <option key={`${ext.id}/${th.id}`} value={refKey({ extensionId: ext.id, themeId: th.id })}>
                {t("settings.themeOption", { theme: th.label, extension: ext.displayName })}
              </option>
            ))}
          </select>
        </SettingRow>
        {themes.length === 0 && iconThemes.length === 0 && <p className="ext-muted">{t("settings.noThemes")}</p>}
      </SettingGroup>
      <SettingGroup title={t("settings.languageServers")}>
        <p className="ext-muted">{t("settings.languageServersHint")}</p>
        {!project?.rootPath && <p className="ext-muted">{t("lsp.needsProject")}</p>}
        {servers === null ? (
          <Spinner size={14} label={t("settings.loading")} />
        ) : (
          servers.map((s) => (
            <ServerRow
              key={s.id}
              server={s}
              sessions={sessions.filter((x) => x.serverId === s.id)}
              projectId={project?.rootPath ? project.id : null}
              onChanged={loadServers}
            />
          ))
        )}
      </SettingGroup>
      {error && <p className="ext-error">{error}</p>}
    </div>
  );
}
