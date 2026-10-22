/**
 * Plugin management (ADR-0034): installed plugins with state, kinds,
 * publisher fingerprint and grants (revoke / allow), enable / disable,
 * uninstall, logs, install from a local `.orkp`, the global kill switch and
 * safe mode. Pending permission requests reuse the marketplace prompt.
 * Live via `PluginInstalled` / `PluginStateChanged`.
 */
import "../../panels/Marketplace/marketplace.css";
import "./plugins.css";

import type { Event } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState } from "@orkestr/ui";
import {
  FileDown,
  Power,
  PowerOff,
  Puzzle,
  ScrollText,
  ShieldAlert,
  ShieldCheck,
  Skull,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { PathInput } from "../../components/PathInput";
import { Overlay } from "../../components/Overlay";
import { maybeClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { PermissionDialog } from "../../panels/Marketplace/dialogs";
import { fingerprintRows } from "../../panels/Marketplace/model";
import { ActionDialog } from "../files/Dialog";
import { type PluginInfo, type PluginLogLine, pluginsApi } from "./api";
import {
  grantKey,
  groupFingerprint,
  sortPlugins,
  stateKey,
  stateTone,
  toggleAction,
} from "./model";

export interface PluginsNotice {
  tone: "success" | "danger" | "info";
  text: string;
}

type Confirm =
  | { kind: "killAll" }
  | { kind: "uninstall"; plugin: PluginInfo }
  | null;

interface Decide {
  pluginId: string;
  name: string;
  pending: PluginInfo["pendingGrants"];
}

export function PluginsManager({
  onNotice,
  version = 0,
}: {
  /** Where notices go (the marketplace banner); local banner when omitted. */
  onNotice?: (n: PluginsNotice) => void;
  /** External reload trigger. */
  version?: number;
}) {
  const { t } = useT(["marketplace", "security", "common"]);
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [safeMode, setSafeMode] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [decide, setDecide] = useState<Decide | null>(null);
  const [logsFor, setLogsFor] = useState<PluginInfo | null>(null);
  const [path, setPath] = useState("");
  const [localNotice, setLocalNotice] = useState<PluginsNotice | null>(null);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  const notify = useCallback(
    (n: PluginsNotice) => (onNotice ? onNotice(n) : setLocalNotice(n)),
    [onNotice],
  );

  useEffect(() => {
    let cancelled = false;
    pluginsApi
      .list()
      .then((r) => {
        if (cancelled) return;
        setPlugins(sortPlugins(r.plugins));
        setSafeMode(r.safeMode);
        setError(null);
        setLoaded(true);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(describeError(e));
        setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [nonce, version]);

  useEffect(() => {
    const client = maybeClient();
    if (!client) return undefined;
    return client.subscribe([topic.global], (ev: Event) => {
      if (ev.kind === "PluginInstalled" || ev.kind === "PluginStateChanged") {
        reload();
      }
    });
  }, [reload]);

  const run = async (
    key: string,
    fn: () => Promise<unknown>,
    ok?: string,
  ): Promise<boolean> => {
    setBusy(key);
    try {
      await fn();
      if (ok) notify({ tone: "success", text: ok });
      reload();
      return true;
    } catch (e) {
      notify({ tone: "danger", text: describeError(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  const installFile = async () => {
    const p = path.trim();
    if (!p) return;
    setBusy("install");
    try {
      const r = await pluginsApi.install({ path: p, source: "file" });
      setPath("");
      reload();
      if (r.pendingGrants.length > 0) {
        setDecide({
          pluginId: r.plugin.plugin.id,
          name: r.plugin.plugin.name,
          pending: r.pendingGrants,
        });
      } else {
        notify({
          tone: "success",
          text: t("notice.installed", {
            name: r.plugin.plugin.name,
            version: r.plugin.plugin.version,
          }),
        });
      }
    } catch (e) {
      notify({ tone: "danger", text: describeError(e) });
    } finally {
      setBusy(null);
    }
  };

  const toggleSafeMode = (next: boolean) =>
    void run(
      "safe",
      () => pluginsApi.setSafeMode(next),
      t(next ? "plugins.safeModeOn" : "plugins.safeModeOff"),
    );

  const killAll = async () => {
    setBusy("kill");
    try {
      const r = await pluginsApi.killAll();
      notify({
        tone: "info",
        text: t("plugins.killed", {
          count: r.killed,
          calls: r.callsInterrupted,
        }),
      });
      reload();
    } catch (e) {
      notify({ tone: "danger", text: describeError(e) });
    } finally {
      setBusy(null);
      setConfirm(null);
    }
  };

  const setGrant = (
    p: PluginInfo,
    g: { capability: PluginInfo["grants"][number]["capability"]; scope?: string },
    allow: boolean,
  ) =>
    void run(
      `g:${p.plugin.id}:${grantKey(g)}`,
      () =>
        pluginsApi.grant({
          pluginId: p.plugin.id,
          decisions: [{ capability: g.capability, scope: g.scope, allow }],
          enable: false,
        }),
      t(allow ? "plugins.grantAllowed" : "plugins.grantRevoked", {
        capability: t(`security:capability.${g.capability}`),
        name: p.plugin.name,
      }),
    );

  const applyDecisions = async (
    decisions: Parameters<typeof pluginsApi.grant>[0]["decisions"],
  ) => {
    if (!decide) return;
    const ok = await run(
      `d:${decide.pluginId}`,
      () =>
        pluginsApi.grant({
          pluginId: decide.pluginId,
          decisions,
          enable: true,
        }),
      t("notice.granted", { name: decide.name }),
    );
    if (ok) setDecide(null);
  };

  return (
    <div className="pl-manager">
      {localNotice && (
        <div className={`mp-notice mp-notice--${localNotice.tone}`} role="status">
          <span>{localNotice.text}</span>
          <button
            type="button"
            className="mp-notice__close"
            aria-label={t("notice.dismiss")}
            onClick={() => setLocalNotice(null)}
          >
            ×
          </button>
        </div>
      )}
      <section
        className={`pl-controls${safeMode ? " pl-controls--safe" : ""}`}
        aria-label={t("plugins.controls")}
      >
        <div className="pl-controls__safe">
          <label className="pl-switch">
            <input
              type="checkbox"
              role="switch"
              aria-checked={safeMode}
              checked={safeMode}
              disabled={busy === "safe"}
              onChange={(e) => toggleSafeMode(e.target.checked)}
            />
            <span className="pl-switch__label">
              <ShieldAlert size={14} /> {t("plugins.safeMode")}
            </span>
          </label>
          <p className="mp-muted">{t("plugins.safeModeHint")}</p>
          {safeMode && (
            <Badge tone="warning">{t("plugins.safeModeActive")}</Badge>
          )}
        </div>
        <div className="pl-controls__kill">
          <Button
            variant="danger"
            leftIcon={<Skull size={15} />}
            loading={busy === "kill"}
            onClick={() => setConfirm({ kind: "killAll" })}
          >
            {t("plugins.killAll")}
          </Button>
          <p className="mp-muted">{t("plugins.killAllHint")}</p>
        </div>
      </section>

      <form
        className="pl-install"
        aria-label={t("plugins.installFile")}
        onSubmit={(e) => {
          e.preventDefault();
          void installFile();
        }}
      >
        <PathInput
          kind="file"
          label={t("plugins.installFile")}
          placeholder={t("plugins.filePlaceholder")}
          dialogTitle={t("plugins.pickPackage")}
          value={path}
          onPathChange={setPath}
        />
        <Button
          type="submit"
          variant="primary"
          leftIcon={<FileDown size={14} />}
          loading={busy === "install"}
          disabled={!path.trim()}
        >
          {t("plugins.install")}
        </Button>
      </form>

      {error && (
        <p className="mp-error" role="alert">
          {error}
        </p>
      )}
      {loaded && !error && plugins.length === 0 && (
        <EmptyState
          icon={<Puzzle size={24} strokeWidth={1.6} />}
          title={t("plugins.empty")}
          description={t("plugins.emptyHint")}
        />
      )}
      {plugins.length > 0 && (
        <ul className="mp-rows" aria-label={t("plugins.list")}>
          {plugins.map((p) => {
            const id = p.plugin.id;
            const action = toggleAction(p);
            return (
              <li key={id} className="mp-row pl-row" data-plugin={p.plugin.packageId}>
                <div className="mp-row__main">
                  <div className="pl-row__title">
                    <strong>{p.plugin.name}</strong>{" "}
                    <span className="mp-muted">{p.plugin.version}</span>{" "}
                    <Badge tone={stateTone(p.plugin.state)}>
                      {t(`plugins.state.${stateKey(p)}`)}
                    </Badge>
                    {p.pendingGrants.length > 0 && (
                      <Badge tone="warning">
                        {t("plugins.pending", { count: p.pendingGrants.length })}
                      </Badge>
                    )}
                  </div>
                  <div className="mp-item__meta">
                    <span className="mp-muted mp-mono">{p.plugin.packageId}</span>
                    <span className="mp-muted">
                      {t("plugins.by", { publisher: p.plugin.publisher })}
                    </span>
                    {p.plugin.kinds.map((k) => (
                      <Badge key={k} tone="violet">
                        {t(`plugins.kind.${k}`)}
                      </Badge>
                    ))}
                  </div>
                  {p.manifest.description && (
                    <p className="mp-muted pl-row__desc">{p.manifest.description}</p>
                  )}
                  <div className="pl-row__fp">
                    <span className="mp-muted">{t("plugins.signer")}</span>
                    {p.signerFingerprint ? (
                      <div
                        className="mp-fingerprint mp-fingerprint--small"
                        aria-label={t("keys.fingerprintOf", {
                          publisher: p.plugin.publisher,
                        })}
                      >
                        {fingerprintRows(groupFingerprint(p.signerFingerprint)).map((r) => (
                          <code key={r}>{r}</code>
                        ))}
                      </div>
                    ) : (
                      <Badge tone="warning">{t("plugins.unsigned")}</Badge>
                    )}
                  </div>
                  {p.lastError && (
                    <p className="mp-error" role="alert">
                      {t("plugins.lastError", { error: p.lastError })}
                    </p>
                  )}
                  {p.grants.length > 0 && (
                    <ul
                      className="pl-grants"
                      aria-label={t("plugins.grantsOf", { name: p.plugin.name })}
                    >
                      {p.grants.map((g) => (
                        <li key={grantKey(g)} className="pl-grant">
                          <Badge tone={g.allowed ? "success" : "neutral"}>
                            {t(g.allowed ? "plugins.allowed" : "plugins.denied")}
                          </Badge>
                          <span>{t(`security:capability.${g.capability}`)}</span>
                          {g.scope && <code className="mp-mono">{g.scope}</code>}
                          <Button
                            size="sm"
                            variant="ghost"
                            loading={busy === `g:${id}:${grantKey(g)}`}
                            aria-label={t(
                              g.allowed ? "plugins.revokeAria" : "plugins.allowAria",
                              {
                                capability: t(`security:capability.${g.capability}`),
                                name: p.plugin.name,
                              },
                            )}
                            onClick={() => setGrant(p, g, !g.allowed)}
                          >
                            {t(g.allowed ? "plugins.revoke" : "plugins.allow")}
                          </Button>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
                <div className="mp-row__actions">
                  {action === "decide" && (
                    <Button
                      size="sm"
                      variant="primary"
                      leftIcon={<ShieldCheck size={14} />}
                      onClick={() =>
                        setDecide({
                          pluginId: id,
                          name: p.plugin.name,
                          pending: p.pendingGrants,
                        })
                      }
                    >
                      {t("plugins.decide")}
                    </Button>
                  )}
                  {action === "enable" && (
                    <Button
                      size="sm"
                      variant="secondary"
                      leftIcon={<Power size={14} />}
                      loading={busy === `e:${id}`}
                      disabled={safeMode}
                      title={safeMode ? t("plugins.safeModeActive") : undefined}
                      onClick={() =>
                        void run(
                          `e:${id}`,
                          () => pluginsApi.enable(id),
                          t("plugins.enabled", { name: p.plugin.name }),
                        )
                      }
                    >
                      {t("plugins.enable")}
                    </Button>
                  )}
                  {action === "disable" && (
                    <Button
                      size="sm"
                      variant="secondary"
                      leftIcon={<PowerOff size={14} />}
                      loading={busy === `e:${id}`}
                      onClick={() =>
                        void run(
                          `e:${id}`,
                          () => pluginsApi.disable(id),
                          t("plugins.disabled", { name: p.plugin.name }),
                        )
                      }
                    >
                      {t("plugins.disable")}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    leftIcon={<ScrollText size={14} />}
                    aria-label={t("plugins.logsOf", { name: p.plugin.name })}
                    onClick={() => setLogsFor(p)}
                  >
                    {t("plugins.logs")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    leftIcon={<Trash2 size={14} />}
                    aria-label={t("plugins.uninstallOf", { name: p.plugin.name })}
                    loading={busy === `u:${id}`}
                    onClick={() => setConfirm({ kind: "uninstall", plugin: p })}
                  >
                    {t("action.uninstall")}
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      {confirm?.kind === "killAll" && (
        <ActionDialog
          tone="danger"
          icon={<Skull size={18} />}
          title={t("plugins.killConfirmTitle")}
          subtitle={t("plugins.killConfirmText")}
          onCancel={() => setConfirm(null)}
          actions={[
            {
              id: "cancel",
              label: t("common:actions.cancel"),
              variant: "ghost",
              onSelect: () => setConfirm(null),
            },
            {
              id: "kill",
              label: t("plugins.killAll"),
              variant: "danger",
              busy: busy === "kill",
              autoFocus: true,
              onSelect: () => void killAll(),
            },
          ]}
        />
      )}
      {confirm?.kind === "uninstall" && (
        <ActionDialog
          tone="danger"
          icon={<Trash2 size={18} />}
          title={t("plugins.uninstallConfirmTitle", {
            name: confirm.plugin.plugin.name,
          })}
          subtitle={t("plugins.uninstallConfirmText")}
          onCancel={() => setConfirm(null)}
          actions={[
            {
              id: "cancel",
              label: t("common:actions.cancel"),
              variant: "ghost",
              autoFocus: true,
              onSelect: () => setConfirm(null),
            },
            {
              id: "uninstall",
              label: t("action.uninstall"),
              variant: "danger",
              busy: busy === `u:${confirm.plugin.plugin.id}`,
              onSelect: () => {
                const p = confirm.plugin;
                void run(
                  `u:${p.plugin.id}`,
                  () => pluginsApi.uninstall(p.plugin.id),
                  t("notice.uninstalled", { name: p.plugin.name }),
                ).then(() => setConfirm(null));
              },
            },
          ]}
        />
      )}
      {decide && (
        <PermissionDialog
          name={decide.name}
          pending={decide.pending}
          busy={busy !== null}
          onDecide={(ds) => void applyDecisions(ds)}
          onCancel={() => {
            notify({
              tone: "info",
              text: t("notice.grantsPending", { name: decide.name }),
            });
            setDecide(null);
          }}
        />
      )}
      {logsFor && (
        <PluginLogsDialog plugin={logsFor} onClose={() => setLogsFor(null)} />
      )}
    </div>
  );
}

function PluginLogsDialog({
  plugin,
  onClose,
}: {
  plugin: PluginInfo;
  onClose: () => void;
}) {
  const { t } = useT(["marketplace", "common"]);
  const [lines, setLines] = useState<PluginLogLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let cancelled = false;
    pluginsApi
      .logs(plugin.plugin.id)
      .then((l) => !cancelled && setLines(l))
      .catch((e: unknown) => !cancelled && setError(describeError(e)));
    return () => {
      cancelled = true;
    };
  }, [plugin.plugin.id, nonce]);
  return (
    <Overlay onDismiss={onClose}>
      <div
        className="dialog mp-dialog pl-logs-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pl-logs-title"
        onKeyDown={(e) => e.key === "Escape" && onClose()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <ScrollText size={18} />
          </div>
          <div>
            <h2 id="pl-logs-title" className="dialog__title">
              {t("plugins.logsTitle", { name: plugin.plugin.name })}
            </h2>
            <p className="dialog__subtitle">{plugin.plugin.packageId}</p>
          </div>
          <button
            type="button"
            className="dialog__close"
            aria-label={t("common:actions.close")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          {error && (
            <p className="mp-error" role="alert">
              {error}
            </p>
          )}
          {lines && lines.length === 0 && (
            <p className="mp-muted">{t("plugins.logsEmpty")}</p>
          )}
          {lines && lines.length > 0 && (
            <ol className="pl-logs" aria-label={t("plugins.logs")}>
              {lines.map((l, i) => (
                <li key={`${l.ts}-${i}`} className={`pl-log pl-log--${l.level}`}>
                  <time className="mp-muted">
                    {new Date(l.ts).toLocaleTimeString()}
                  </time>
                  <span className="pl-log__level">{l.level}</span>
                  <span className="pl-log__msg">{l.message}</span>
                </li>
              ))}
            </ol>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={() => setNonce((n) => n + 1)}>
            {t("plugins.logsRefresh")}
          </Button>
          <Button variant="primary" onClick={onClose}>
            {t("common:actions.close")}
          </Button>
        </footer>
      </div>
    </Overlay>
  );
}
