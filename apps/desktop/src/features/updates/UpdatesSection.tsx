/**
 * Settings → Updates (ADR-0048): version and update status, release notes,
 * download progress, install now / when Orkestr closes, update source
 * (remote endpoint or a local release folder / file), channel, schedule,
 * data backups and rollback information.
 */
import "./updates.css";

import { Badge, Button, Input, Spinner } from "@orkestr/ui";
import { AlertTriangle, ArchiveRestore, CheckCircle2, Download, FileSearch, FolderOpen, RefreshCw, Rocket, X } from "lucide-react";
import { useEffect, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { canPickPaths, pickFile, pickFolder } from "../../lib/tauri";
import { SafeMarkdown } from "../extensions/markdown";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import type { BackupInfo, ErrorInfo, UpdateChannel, UpdateInfo, UpdateStatus } from "./api";
import { updatesAvailableHere } from "./api";
import { useUpdatesStore } from "./store";

const INTERVALS = [0, 6, 12, 24, 72, 168] as const;
const RETENTIONS = [1, 3, 5, 10, 20] as const;
const KNOWN_ERRORS = [
  "not_configured",
  "invalid_endpoint",
  "offline",
  "server_error",
  "bad_manifest",
  "no_platform",
  "bad_signature",
  "unsigned",
  "cancelled",
  "database_newer",
  "io",
] as const;
type KnownError = (typeof KNOWN_ERRORS)[number];

function ErrorLine({ error }: { error: ErrorInfo }) {
  const { t } = useT("updates");
  const known = (KNOWN_ERRORS as readonly string[]).includes(error.code);
  return (
    <div className="set-alert upd-error" role="alert">
      <AlertTriangle size={14} />
      <div>
        <div>{known ? t(`errors.${error.code as KnownError}`) : t("errors.unknown")}</div>
        {error.message && <div className="upd-error__detail">{error.message}</div>}
      </div>
    </div>
  );
}

function StatusLine({ status }: { status: UpdateStatus }) {
  const { t } = useT("updates");
  const fmt = useFormat();
  const checked = status.lastCheckedAt ? fmt.relative(status.lastCheckedAt) : null;
  switch (status.phase) {
    case "checking":
      return (
        <span className="upd-status">
          <Spinner size={12} /> {t("status.checking")}
        </span>
      );
    case "upToDate":
      return (
        <span className="upd-status upd-status--ok">
          <CheckCircle2 size={14} /> {t("status.upToDate")}
          {checked && <span className="upd-status__when">· {t("status.checkedAt", { when: checked })}</span>}
        </span>
      );
    case "available":
    case "downloading":
    case "ready":
      return (
        <span className="upd-status upd-status--new">
          <Download size={14} /> {t("status.available", { version: status.available?.version ?? "" })}
        </span>
      );
    case "installing":
      return (
        <span className="upd-status">
          <Spinner size={12} /> {t("status.installing")}
        </span>
      );
    case "error":
      return <span className="upd-status upd-status--error">{t("status.failed")}</span>;
    default:
      return <span className="upd-status">{checked ? t("status.checkedAt", { when: checked }) : t("status.neverChecked")}</span>;
  }
}

function sourceKey(info: UpdateInfo): "offer.sourceRemote" | "offer.sourceManifest" | "offer.sourceInstaller" {
  if (info.source === "remote") return "offer.sourceRemote";
  if (info.source === "localManifest") return "offer.sourceManifest";
  return "offer.sourceInstaller";
}

function Offer({ status }: { status: UpdateStatus }) {
  const { t, i18n } = useT("updates");
  const fmt = useFormat();
  const busy = useUpdatesStore((s) => s.busy);
  const download = useUpdatesStore((s) => s.download);
  const cancel = useUpdatesStore((s) => s.cancel);
  const install = useUpdatesStore((s) => s.install);
  const [acceptUnsigned, setAcceptUnsigned] = useState(false);
  const info = status.available;
  if (!info) return null;
  const unsigned = info.signature === null;
  const language = (i18n.language || "en").split("-")[0] ?? "en";
  const notes = info.notesI18n[language] ?? info.notes;
  const progress = status.progress;
  const pct = progress?.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null;

  return (
    <section className="upd-offer" aria-label={t("offer.title", { version: info.version })}>
      <header className="upd-offer__head">
        <Rocket size={18} />
        <div className="upd-offer__title">
          <strong>{t("offer.title", { version: info.version })}</strong>
          <span className="set-muted">
            {t("offer.from", { current: info.currentVersion })} · {t(sourceKey(info))}
            {info.size != null && ` · ${fmt.bytes(info.size)}`}
            {info.pubDate && ` · ${fmt.date(info.pubDate)}`}
          </span>
        </div>
        {info.prerelease && <Badge tone="violet">{t("offer.beta")}</Badge>}
        {unsigned ? <Badge tone="warning">{t("offer.unsigned")}</Badge> : <Badge tone="success">{t("offer.signed")}</Badge>}
      </header>

      <div className="upd-notes">
        <div className="upd-notes__title">{t("offer.notes")}</div>
        {notes ? <SafeMarkdown source={notes} maxBlocks={200} /> : <p className="set-muted">{t("offer.noNotes")}</p>}
      </div>

      {unsigned && (
        <label className="upd-unsigned">
          <input type="checkbox" checked={acceptUnsigned} onChange={(e) => setAcceptUnsigned(e.target.checked)} />
          <span>{t("offer.unsignedConsent")}</span>
        </label>
      )}

      {status.phase === "downloading" && (
        <div className="upd-progress" role="progressbar" aria-label={t("offer.downloading")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
          <div className="upd-progress__bar" style={{ width: pct == null ? "100%" : `${pct}%` }} data-indeterminate={pct == null || undefined} />
          <span className="upd-progress__text">
            {progress ? t("offer.progress", { done: fmt.bytes(progress.downloaded), total: progress.total ? fmt.bytes(progress.total) : "?" }) : t("offer.downloading")}
          </span>
        </div>
      )}

      <div className="upd-actions">
        {status.phase === "available" && (
          <Button variant="primary" leftIcon={<Download size={14} />} disabled={busy || (unsigned && !acceptUnsigned)} onClick={() => void download(unsigned && acceptUnsigned)}>
            {info.location.kind === "file" ? t("actions.prepare") : t("actions.download")}
          </Button>
        )}
        {status.phase === "downloading" && (
          <Button leftIcon={<X size={14} />} onClick={() => void cancel()}>
            {t("actions.cancelDownload")}
          </Button>
        )}
        {status.phase === "ready" && (
          <>
            <Button variant="primary" leftIcon={<Rocket size={14} />} disabled={busy} onClick={() => void install("now")}>
              {t("actions.installNow")}
            </Button>
            {status.installOnExit ? (
              <Button disabled={busy} onClick={() => void install("cancel")}>
                {t("actions.cancelOnExit")}
              </Button>
            ) : (
              <Button disabled={busy} onClick={() => void install("onExit")}>
                {t("actions.installOnExit")}
              </Button>
            )}
          </>
        )}
      </div>
      {status.phase === "ready" && (
        <p className="set-muted set-muted--small">{status.installOnExit ? t("offer.scheduled") : t("offer.installHint")}</p>
      )}
    </section>
  );
}

function reasonKey(b: BackupInfo): "backups.reasonPreUpdate" | "backups.reasonPreMigration" | "backups.reasonManual" {
  if (b.reason === "preUpdate") return "backups.reasonPreUpdate";
  if (b.reason === "preMigration") return "backups.reasonPreMigration";
  return "backups.reasonManual";
}

function Backups({ status }: { status: UpdateStatus }) {
  const { t } = useT("updates");
  const fmt = useFormat();
  const busy = useUpdatesStore((s) => s.busy);
  const backupNow = useUpdatesStore((s) => s.backupNow);
  const saveSettings = useUpdatesStore((s) => s.saveSettings);
  const startup = status.startup;
  return (
    <SettingGroup title={t("backups.title")}>
      {startup?.start.kind === "upgraded" && (
        <p className="upd-info">{t("rollback.updatedFrom", { from: startup.start.from, to: status.currentVersion })}</p>
      )}
      {startup?.start.kind === "downgraded" && (
        <p className="upd-info">{t("rollback.downgradedFrom", { from: startup.start.from, to: status.currentVersion })}</p>
      )}
      {startup?.backupError && <ErrorLine error={{ code: "io", message: startup.backupError }} />}
      <p className="set-muted">{t("rollback.howTo")}</p>
      <SettingRow title={t("backups.folder")} description={t("backups.folderHint")} stacked>
        <code className="upd-path">{status.backupsDir}</code>
      </SettingRow>
      <SettingRow title={t("backups.retention")} description={t("backups.retentionHint")}>
        <select
          className="set-select"
          aria-label={t("backups.retention")}
          value={status.settings.backupRetention}
          onChange={(e) => void saveSettings({ backupRetention: Number(e.target.value) })}
        >
          {RETENTIONS.map((n) => (
            <option key={n} value={n}>
              {t("backups.keep", { count: n })}
            </option>
          ))}
        </select>
      </SettingRow>
      {status.backups.length === 0 ? (
        <p className="set-muted">{t("backups.none")}</p>
      ) : (
        <ul className="upd-backups" aria-label={t("backups.list")}>
          {status.backups.map((b) => (
            <li key={b.path} className="upd-backups__row" title={b.path}>
              <ArchiveRestore size={14} />
              <span className="upd-backups__when">{fmt.date(b.createdAt, "dateTime")}</span>
              <Badge>{t(reasonKey(b))}</Badge>
              <span className="set-muted">
                {b.toVersion ? t("backups.versions", { from: b.fromVersion, to: b.toVersion }) : t("backups.version", { version: b.fromVersion })}
              </span>
              <span className="upd-backups__size">{fmt.bytes(b.sizeBytes)}</span>
            </li>
          ))}
        </ul>
      )}
      <div className="upd-actions">
        <Button leftIcon={<ArchiveRestore size={14} />} disabled={busy} onClick={() => void backupNow()}>
          {t("actions.backupNow")}
        </Button>
      </div>
    </SettingGroup>
  );
}

function Source({ status }: { status: UpdateStatus }) {
  const { t } = useT("updates");
  const saveSettings = useUpdatesStore((s) => s.saveSettings);
  const check = useUpdatesStore((s) => s.check);
  const busy = useUpdatesStore((s) => s.busy);
  const s = status.settings;
  const [endpoint, setEndpoint] = useState(s.endpoint);
  const [localPath, setLocalPath] = useState(s.localPath ?? "");
  useEffect(() => setEndpoint(s.endpoint), [s.endpoint]);
  useEffect(() => setLocalPath(s.localPath ?? ""), [s.localPath]);

  const pickAndCheck = async (folder: boolean) => {
    const path = folder ? await pickFolder({ title: t("source.pickFolder") }) : await pickFile({ title: t("source.pickFile") });
    if (!path) return;
    setLocalPath(path);
    await saveSettings({ localPath: path });
    await check(path);
  };

  return (
    <SettingGroup title={t("source.title")}>
      <SettingRow title={t("source.remote")} description={t("source.remoteHint")}>
        <Switch label={t("source.remote")} checked={s.remoteEnabled} onChange={(v) => void saveSettings({ remoteEnabled: v })} />
      </SettingRow>
      <SettingRow title={t("source.endpoint")} description={t("source.endpointHint")} stacked>
        <div className="upd-inline">
          <Input
            aria-label={t("source.endpoint")}
            value={endpoint}
            spellCheck={false}
            onChange={(e) => setEndpoint(e.target.value)}
            disabled={!s.remoteEnabled}
          />
          <Button disabled={busy || endpoint.trim() === s.endpoint} onClick={() => void saveSettings({ endpoint })}>
            {t("actions.save")}
          </Button>
        </div>
      </SettingRow>
      <SettingRow title={t("source.channel")} description={t("source.channelHint")}>
        <Segmented<UpdateChannel>
          label={t("source.channel")}
          value={s.channel}
          options={[
            { value: "stable", label: t("source.stable") },
            { value: "beta", label: t("source.beta") },
          ]}
          onChange={(channel) => void saveSettings({ channel })}
        />
      </SettingRow>
      <SettingRow title={t("source.local")} description={t("source.localHint")} stacked>
        <div className="upd-inline">
          <Input
            aria-label={t("source.local")}
            value={localPath}
            spellCheck={false}
            placeholder={t("source.localPlaceholder")}
            onChange={(e) => setLocalPath(e.target.value)}
          />
          <Button disabled={busy || localPath.trim() === (s.localPath ?? "")} onClick={() => void saveSettings({ localPath: localPath.trim() || null })}>
            {t("actions.save")}
          </Button>
        </div>
        {canPickPaths() && (
          <div className="upd-actions">
            <Button leftIcon={<FileSearch size={14} />} disabled={busy} onClick={() => void pickAndCheck(false)}>
              {t("actions.pickFile")}
            </Button>
            <Button leftIcon={<FolderOpen size={14} />} disabled={busy} onClick={() => void pickAndCheck(true)}>
              {t("actions.pickFolder")}
            </Button>
          </div>
        )}
      </SettingRow>
    </SettingGroup>
  );
}

function Schedule({ status }: { status: UpdateStatus }) {
  const { t } = useT("updates");
  const saveSettings = useUpdatesStore((s) => s.saveSettings);
  const s = status.settings;
  return (
    <SettingGroup title={t("schedule.title")}>
      <SettingRow title={t("schedule.onStartup")} description={t("schedule.onStartupHint")}>
        <Switch label={t("schedule.onStartup")} checked={s.checkOnStartup} onChange={(v) => void saveSettings({ checkOnStartup: v })} />
      </SettingRow>
      <SettingRow title={t("schedule.interval")} description={t("schedule.intervalHint")}>
        <select
          className="set-select"
          aria-label={t("schedule.interval")}
          value={s.checkIntervalHours}
          onChange={(e) => void saveSettings({ checkIntervalHours: Number(e.target.value) })}
        >
          {INTERVALS.map((h) => (
            <option key={h} value={h}>
              {h === 0 ? t("schedule.never") : t("schedule.everyHours", { count: h })}
            </option>
          ))}
        </select>
      </SettingRow>
      <SettingRow title={t("schedule.autoDownload")} description={t("schedule.autoDownloadHint")}>
        <Switch label={t("schedule.autoDownload")} checked={s.autoDownload} onChange={(v) => void saveSettings({ autoDownload: v })} />
      </SettingRow>
    </SettingGroup>
  );
}

/** Settings → Updates. */
export function UpdatesSection(_props: SettingsSectionProps) {
  const { t } = useT(["updates", "common"]);
  const status = useUpdatesStore((s) => s.status);
  const busy = useUpdatesStore((s) => s.busy);
  const error = useUpdatesStore((s) => s.error);
  const refresh = useUpdatesStore((s) => s.refresh);
  const check = useUpdatesStore((s) => s.check);
  const inShell = updatesAvailableHere();

  useEffect(() => {
    if (inShell && !useUpdatesStore.getState().status) void refresh();
  }, [inShell, refresh]);

  if (!inShell) {
    return (
      <div className="set-section">
        <p className="set-muted">{t("notInShell")}</p>
      </div>
    );
  }
  if (!status) {
    return (
      <div className="set-section">
        <div className="set-loading">
          <Spinner size={16} /> {t("common:state.loading")}
        </div>
        {error && <ErrorLine error={error} />}
      </div>
    );
  }
  const configured = status.settings.remoteEnabled || !!status.settings.localPath;
  const checking = status.phase === "checking";
  return (
    <div className="set-section upd">
      <SettingGroup title={t("version.title")}>
        <div className="upd-head">
          <div>
            <div className="upd-head__version">{t("version.current", { version: status.currentVersion })}</div>
            <StatusLine status={status} />
          </div>
          <Button variant="primary" leftIcon={<RefreshCw size={14} />} loading={checking} disabled={busy || checking} onClick={() => void check()}>
            {t("actions.check")}
          </Button>
        </div>
        {!configured && <p className="set-muted">{t("version.notConfigured")}</p>}
        {status.lastError && <ErrorLine error={status.lastError} />}
        {error && <ErrorLine error={error} />}
      </SettingGroup>
      <Offer status={status} />
      <Source status={status} />
      <Schedule status={status} />
      <Backups status={status} />
      <p className="set-muted set-muted--small">{t("dataSafety")}</p>
    </div>
  );
}
