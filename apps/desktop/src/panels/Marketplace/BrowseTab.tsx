/** Browse: search the verified indexes, filter by kind / tag, details, install. */
import { Badge, Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { Search, ShieldAlert, Store } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import {
  type EntryDetails,
  type MarketEntry,
  type PackageKind,
  marketplaceApi,
} from "./api";
import type { InstallFlow } from "./flow";
import {
  PACKAGE_KINDS,
  collectTags,
  entryAction,
  fingerprintRows,
  trustTone,
} from "./model";

export function BrowseTab({
  flow,
  version,
}: {
  flow: InstallFlow;
  version: number;
}) {
  const { t } = useT(["marketplace", "security"]);
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<PackageKind | "">("");
  const [tag, setTag] = useState<string | null>(null);
  const [entries, setEntries] = useState<MarketEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<MarketEntry | null>(null);
  const [details, setDetails] = useState<EntryDetails | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const timer = setTimeout(() => {
      marketplaceApi
        .search({
          query: query.trim() || undefined,
          kind: kind || undefined,
          tags: tag ? [tag] : [],
          includeRevoked: true,
        })
        .then((r) => {
          if (cancelled) return;
          setEntries(r);
          setError(null);
        })
        .catch((e: unknown) => !cancelled && setError(describeError(e)))
        .finally(() => !cancelled && setLoading(false));
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, kind, tag, version]);

  useEffect(() => {
    setDetails(null);
    if (!selected) return undefined;
    let cancelled = false;
    marketplaceApi
      .get(selected.id, selected.sourceId)
      .then((d) => !cancelled && setDetails(d))
      .catch((e: unknown) => !cancelled && setError(describeError(e)));
    return () => {
      cancelled = true;
    };
  }, [selected, version]);

  const tags = useMemo(() => collectTags(entries), [entries]);
  const current = selected
    ? (entries.find(
        (e) => e.id === selected.id && e.sourceId === selected.sourceId,
      ) ?? selected)
    : null;

  return (
    <div className="mp-browse">
      <div className="mp-toolbar">
        <Input
          aria-label={t("browse.search")}
          placeholder={t("browse.searchPlaceholder")}
          leftIcon={<Search size={14} />}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <select
          className="mp-select"
          aria-label={t("browse.kind")}
          value={kind}
          onChange={(e) => setKind(e.target.value as PackageKind | "")}
        >
          <option value="">{t("browse.allKinds")}</option>
          {PACKAGE_KINDS.map((k) => (
            <option key={k} value={k}>
              {t(`kind.${k}`)}
            </option>
          ))}
        </select>
      </div>
      {tags.length > 0 && (
        <div className="mp-tags" role="group" aria-label={t("browse.tags")}>
          {tags.map((x) => (
            <button
              key={x}
              type="button"
              className="mp-tag"
              aria-pressed={tag === x}
              onClick={() => setTag(tag === x ? null : x)}
            >
              {x}
            </button>
          ))}
        </div>
      )}
      {error && (
        <p className="mp-error" role="alert">
          {error}
        </p>
      )}
      <div className="mp-split">
        <ul className="mp-list" aria-label={t("browse.results")}>
          {loading && entries.length === 0 && (
            <li className="mp-list__state">
              <Spinner size={16} label={t("browse.loading")} />
            </li>
          )}
          {!loading && entries.length === 0 && (
            <li className="mp-list__state">
              <EmptyState
                icon={<Store size={24} strokeWidth={1.6} />}
                title={t("browse.empty")}
                description={t("browse.emptyHint")}
              />
            </li>
          )}
          {entries.map((e) => {
            const action = entryAction(e);
            return (
              <li key={`${e.sourceId}/${e.id}`}>
                <button
                  type="button"
                  className="mp-item"
                  aria-current={
                    current?.id === e.id && current.sourceId === e.sourceId
                  }
                  onClick={() => setSelected(e)}
                >
                  <span className="mp-item__title">
                    {e.name} <span className="mp-muted">{e.version}</span>
                  </span>
                  <span className="mp-item__meta">
                    <Badge tone="violet">{t(`kind.${e.kind}`)}</Badge>
                    <span className="mp-muted">
                      {t("browse.by", { publisher: e.publisher })}
                    </span>
                    {action === "blocked" && (
                      <Badge tone="danger">{t("status.revoked")}</Badge>
                    )}
                    {action === "installed" && (
                      <Badge tone="success">{t("status.installed")}</Badge>
                    )}
                    {action === "update" && (
                      <Badge tone="warning">
                        {t("status.updateAvailable")}
                      </Badge>
                    )}
                  </span>
                  <span className="mp-item__desc">{e.description}</span>
                </button>
              </li>
            );
          })}
        </ul>
        <section className="mp-details" aria-label={t("details.label")}>
          {!current && <p className="mp-muted">{t("details.none")}</p>}
          {current && (
            <>
              <h3 className="mp-details__title">{current.name}</h3>
              <p className="mp-muted">
                {current.id} · {current.version} ·{" "}
                {t("details.source", { source: current.sourceId })}
              </p>
              <p>{current.description}</p>
              {(current.tags ?? []).length > 0 && (
                <p className="mp-muted">{(current.tags ?? []).join(", ")}</p>
              )}
              {details && (
                <>
                  <h4>{t("details.publisherKey")}</h4>
                  <div className="mp-keyline">
                    <Badge tone={trustTone(details.key.trust)}>
                      {t(`trustState.${details.key.trust}`)}
                    </Badge>
                    <span>{details.key.publisher}</span>
                  </div>
                  <div
                    className="mp-fingerprint mp-fingerprint--small"
                    aria-label={t("keys.fingerprint")}
                  >
                    {fingerprintRows(details.key.fingerprint).map((r) => (
                      <code key={r}>{r}</code>
                    ))}
                  </div>
                  {details.permissions.length > 0 && (
                    <>
                      <h4>{t("details.permissions")}</h4>
                      <ul className="mp-plain">
                        {details.permissions.map((p) => (
                          <li key={`${p.capability}:${p.scope ?? ""}`}>
                            <strong>
                              {t(`security:capability.${p.capability}`)}
                            </strong>
                            {p.scope ? ` (${p.scope})` : ""} — {p.reason}
                          </li>
                        ))}
                      </ul>
                    </>
                  )}
                  {details.versions.length > 1 && (
                    <p className="mp-muted">
                      {t("details.versions", {
                        versions: details.versions.join(", "),
                      })}
                    </p>
                  )}
                </>
              )}
              <div className="mp-actions">
                {entryAction(current) === "blocked" ? (
                  <p className="mp-error">
                    <ShieldAlert size={14} />{" "}
                    {t("notice.revoked", { name: current.name })}
                  </p>
                ) : (
                  <Button
                    variant="primary"
                    loading={flow.busy === current.id}
                    disabled={entryAction(current) === "installed"}
                    onClick={() =>
                      void flow.start(current.id, current.sourceId)
                    }
                  >
                    {t(`action.${entryAction(current)}`)}
                  </Button>
                )}
              </div>
            </>
          )}
        </section>
      </div>
    </div>
  );
}
