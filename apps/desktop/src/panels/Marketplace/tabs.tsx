/** Installed, Sources and Keys tabs of the Marketplace panel. */
import { Badge, Button, EmptyState, Input } from "@orkestr/ui";
import { KeyRound, Package, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import {
  type InstalledPackage,
  type PublisherKey,
  type RegistrySource,
  marketplaceApi,
} from "./api";
import { TrustKeyDialog } from "./dialogs";
import type { InstallFlow } from "./flow";
import { fingerprintRows, trustTone } from "./model";

/** Loads `load()` on mount and whenever `version` changes. */
function useLoad<T>(load: () => Promise<T>, version: number, initial: T) {
  const [data, setData] = useState<T>(initial);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let cancelled = false;
    load()
      .then((d) => {
        if (cancelled) return;
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => !cancelled && setError(describeError(e)));
    return () => {
      cancelled = true;
    };
  }, [version, nonce]);
  return { data, error, reload: useCallback(() => setNonce((n) => n + 1), []) };
}

export function InstalledTab({
  flow,
  version,
  onChanged,
}: {
  flow: InstallFlow;
  version: number;
  onChanged: () => void;
}) {
  const { t } = useT("marketplace");
  const { data, error } = useLoad(
    marketplaceApi.installed,
    version,
    [] as InstalledPackage[],
  );
  const [removing, setRemoving] = useState<string | null>(null);

  const uninstall = async (p: InstalledPackage) => {
    setRemoving(p.id);
    try {
      await marketplaceApi.uninstall(p.id);
      flow.setNotice({
        tone: "success",
        text: t("notice.uninstalled", { name: p.name }),
      });
      onChanged();
    } catch (e) {
      flow.setNotice({ tone: "danger", text: describeError(e) });
    } finally {
      setRemoving(null);
    }
  };

  if (error) {
    return (
      <p className="mp-error" role="alert">
        {error}
      </p>
    );
  }
  if (data.length === 0) {
    return (
      <EmptyState
        icon={<Package size={24} strokeWidth={1.6} />}
        title={t("installed.empty")}
        description={t("installed.emptyHint")}
      />
    );
  }
  return (
    <ul className="mp-rows" aria-label={t("tabs.installed")}>
      {data.map((p) => (
        <li key={p.id} className="mp-row">
          <div className="mp-row__main">
            <strong>{p.name}</strong>{" "}
            <span className="mp-muted">{p.version}</span>
            <div className="mp-item__meta">
              <Badge tone="violet">{t(`kind.${p.kind}`)}</Badge>
              <span className="mp-muted">
                {t("details.source", { source: p.sourceId })}
              </span>
              {p.revoked && <Badge tone="danger">{t("status.revoked")}</Badge>}
              {p.updateAvailable && (
                <Badge tone="warning">
                  {t("installed.update", { version: p.updateAvailable })}
                </Badge>
              )}
            </div>
          </div>
          <div className="mp-row__actions">
            {p.updateAvailable && !p.revoked && (
              <Button
                size="sm"
                variant="primary"
                loading={flow.busy === p.id}
                onClick={() =>
                  void flow.start(p.id, p.sourceId, p.updateAvailable)
                }
              >
                {t("action.update")}
              </Button>
            )}
            <Button
              size="sm"
              variant="ghost"
              leftIcon={<Trash2 size={14} />}
              loading={removing === p.id}
              onClick={() => void uninstall(p)}
            >
              {t("action.uninstall")}
            </Button>
          </div>
        </li>
      ))}
    </ul>
  );
}

export function SourcesTab({
  flow,
  version,
  onChanged,
}: {
  flow: InstallFlow;
  version: number;
  onChanged: () => void;
}) {
  const { t } = useT("marketplace");
  const { data, error } = useLoad(
    marketplaceApi.sources,
    version,
    [] as RegistrySource[],
  );
  const [kind, setKind] = useState<"Directory" | "Http">("Http");
  const [location, setLocation] = useState("");
  const [name, setName] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  const run = async (key: string, fn: () => Promise<unknown>, ok: string) => {
    setBusy(key);
    try {
      await fn();
      flow.setNotice({ tone: "success", text: ok });
      onChanged();
      return true;
    } catch (e) {
      flow.setNotice({ tone: "danger", text: describeError(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  const add = async () => {
    const done = await run(
      "add",
      () =>
        marketplaceApi.addSource({
          kind,
          location: location.trim(),
          name: name.trim() || undefined,
          token: token.trim() || undefined,
        }),
      t("notice.sourceAdded"),
    );
    if (done) {
      setLocation("");
      setName("");
      setToken("");
    }
  };

  return (
    <div className="mp-sources">
      {error && (
        <p className="mp-error" role="alert">
          {error}
        </p>
      )}
      <div className="mp-toolbar">
        <Button
          size="sm"
          variant="secondary"
          leftIcon={<RefreshCw size={14} />}
          loading={busy === "all"}
          onClick={() =>
            void run(
              "all",
              () => marketplaceApi.refresh(),
              t("notice.refreshed"),
            )
          }
        >
          {t("sources.refreshAll")}
        </Button>
      </div>
      <ul className="mp-rows" aria-label={t("tabs.sources")}>
        {data.map((s) => (
          <li key={s.id} className="mp-row">
            <div className="mp-row__main">
              <strong>{s.name}</strong>{" "}
              <Badge tone="neutral">{t(`sourceKind.${s.kind}`)}</Badge>
              <div className="mp-muted mp-mono">{s.location}</div>
              <div className="mp-muted">
                {t("sources.entries", { count: s.entries })}
                {s.lastRefresh
                  ? ` · ${t("sources.lastRefresh", { at: new Date(s.lastRefresh).toLocaleString() })}`
                  : ""}
              </div>
              {s.lastError && (
                <p className="mp-error" role="alert">
                  {t("sources.lastError", { error: s.lastError })}
                </p>
              )}
            </div>
            <div className="mp-row__actions">
              <Button
                size="sm"
                variant="ghost"
                aria-label={t("sources.refreshOne", { name: s.name })}
                loading={busy === `r:${s.id}`}
                onClick={() =>
                  void run(
                    `r:${s.id}`,
                    () => marketplaceApi.refresh(s.id),
                    t("notice.refreshed"),
                  )
                }
              >
                <RefreshCw size={14} />
              </Button>
              {s.kind !== "Official" && (
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={t("sources.remove", { name: s.name })}
                  loading={busy === `d:${s.id}`}
                  onClick={() =>
                    void run(
                      `d:${s.id}`,
                      () => marketplaceApi.removeSource(s.id),
                      t("notice.sourceRemoved"),
                    )
                  }
                >
                  <Trash2 size={14} />
                </Button>
              )}
            </div>
          </li>
        ))}
      </ul>
      <form
        className="mp-add"
        aria-label={t("sources.add")}
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <h4>{t("sources.add")}</h4>
        <select
          className="mp-select"
          aria-label={t("sources.kind")}
          value={kind}
          onChange={(e) => setKind(e.target.value as "Directory" | "Http")}
        >
          <option value="Http">{t("sourceKind.Http")}</option>
          <option value="Directory">{t("sourceKind.Directory")}</option>
        </select>
        <Input
          label={t("sources.location")}
          placeholder={
            kind === "Http"
              ? t("sources.urlPlaceholder")
              : t("sources.dirPlaceholder")
          }
          value={location}
          onChange={(e) => setLocation(e.target.value)}
        />
        <Input
          label={t("sources.name")}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        {kind === "Http" && (
          <Input
            label={t("sources.token")}
            hint={t("sources.tokenHint")}
            type="password"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
        )}
        <Button
          type="submit"
          variant="primary"
          leftIcon={<Plus size={14} />}
          loading={busy === "add"}
          disabled={!location.trim()}
        >
          {t("sources.addButton")}
        </Button>
      </form>
    </div>
  );
}

export function KeysTab({
  flow,
  version,
  onChanged,
}: {
  flow: InstallFlow;
  version: number;
  onChanged: () => void;
}) {
  const { t } = useT("marketplace");
  const { data, error } = useLoad(marketplaceApi.keys, version, {
    keys: [] as PublisherKey[],
  } as Awaited<ReturnType<typeof marketplaceApi.keys>>);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<PublisherKey | null>(null);

  const act = async (k: PublisherKey, trust: boolean) => {
    setBusy(k.keyId);
    try {
      if (trust) await marketplaceApi.trustKey(k.keyId, k.fingerprint);
      else await marketplaceApi.untrustKey(k.keyId);
      flow.setNotice({
        tone: "success",
        text: t(trust ? "notice.keyTrusted" : "notice.keyUntrusted", {
          publisher: k.publisher,
        }),
      });
      onChanged();
    } catch (e) {
      flow.setNotice({ tone: "danger", text: describeError(e) });
    } finally {
      setBusy(null);
      setConfirm(null);
    }
  };

  const local = data.localPublisher ?? null;
  return (
    <div className="mp-keys">
      {error && (
        <p className="mp-error" role="alert">
          {error}
        </p>
      )}
      <section className="mp-local" aria-label={t("keys.local")}>
        <h4>{t("keys.local")}</h4>
        {local ? (
          <>
            <p>
              {t("keys.localPublisher", {
                publisher: local.publisher,
                keyId: local.keyId,
              })}
            </p>
            <div
              className="mp-fingerprint mp-fingerprint--small"
              aria-label={t("keys.fingerprint")}
            >
              {fingerprintRows(local.fingerprint).map((r) => (
                <code key={r}>{r}</code>
              ))}
            </div>
          </>
        ) : (
          <p className="mp-muted">{t("keys.noLocal")}</p>
        )}
      </section>
      {data.keys.length === 0 ? (
        <EmptyState
          icon={<KeyRound size={24} strokeWidth={1.6} />}
          title={t("keys.empty")}
        />
      ) : (
        <ul className="mp-rows" aria-label={t("tabs.keys")}>
          {data.keys.map((k) => (
            <li key={k.keyId} className="mp-row">
              <div className="mp-row__main">
                <strong>{k.publisher}</strong>{" "}
                <Badge tone={trustTone(k.trust)}>
                  {t(`trustState.${k.trust}`)}
                </Badge>
                <div
                  className="mp-fingerprint mp-fingerprint--small"
                  aria-label={t("keys.fingerprintOf", {
                    publisher: k.publisher,
                  })}
                >
                  {fingerprintRows(k.fingerprint).map((r) => (
                    <code key={r}>{r}</code>
                  ))}
                </div>
              </div>
              <div className="mp-row__actions">
                {k.trust === "Untrusted" && (
                  <Button
                    size="sm"
                    variant="primary"
                    loading={busy === k.keyId}
                    onClick={() => setConfirm(k)}
                  >
                    {t("keys.trust")}
                  </Button>
                )}
                {k.trust === "Trusted" && k.keyId !== local?.keyId && (
                  <Button
                    size="sm"
                    variant="ghost"
                    loading={busy === k.keyId}
                    onClick={() => void act(k, false)}
                  >
                    {t("keys.untrust")}
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      {confirm && (
        <TrustKeyDialog
          pkey={confirm}
          busy={busy !== null}
          onTrust={() => void act(confirm, true)}
          onCancel={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
