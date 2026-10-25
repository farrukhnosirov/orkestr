/**
 * «Приоритет ИИ-источников» (ADR-0051): one global, user-ordered list of AI
 * sources (subscriptions and API keys) with status chips, capabilities, a
 * test button, per-purpose overrides and the recent failover log. When a
 * source stops working every request switches to the next one at once.
 */
import type { AiSource, AiSourcesResult, RoutingConfig, SourceCapabilities } from "@orkestr/protocol";
import { Button, IconButton, Spinner } from "@orkestr/ui";
import { ArrowDown, ArrowUp, FlaskConical, GripVertical, RefreshCw, Save } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { formatDate } from "../../i18n/format";
import { Switch } from "./controls";
import { CAPABILITIES, reorder, sourceStatus, toggleCapability, withSources } from "./sources";
import { errorMessage, getRoutingConfig, saveRoutingConfig, testProvider, useProvidersLive } from "./store";
import "./providers.css";

interface Draft {
  config: RoutingConfig;
  sources: AiSource[];
  overrides: Record<string, string[] | undefined>;
  purposes: string[];
  log: AiSourcesResult["log"];
}

async function loadDraft(): Promise<Draft> {
  const [config, res] = await Promise.all([getRoutingConfig(), getClient().call("providers.sources", {})]);
  const overrides: Record<string, string[] | undefined> = {};
  for (const p of config.purposeSources ?? []) overrides[p.purpose] = p.sources;
  return { config, sources: res.sources, overrides, purposes: res.purposes, log: res.log };
}

export function SourcePrioritySettings() {
  const { t } = useT("providers");
  const { t: tn } = useT("notifications");
  const { providers } = useProvidersLive();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [tests, setTests] = useState<Record<string, string>>({});

  const load = useCallback(async () => {
    setError(null);
    try {
      setDraft(await loadDraft());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);
  useEffect(() => void load(), [load]);

  if (!draft) return error ? <p className="pv__error" role="alert">{error}</p> : <Spinner />;
  const view = (id: string) => providers.find((p) => p.provider.id === id);
  const name = (id: string) => view(id)?.provider.name ?? id;
  const setSources = (sources: AiSource[]) => setDraft({ ...draft, sources });
  const update = (i: number, s: AiSource) => setSources(draft.sources.map((x, j) => (j === i ? s : x)));

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await saveRoutingConfig(withSources(draft.config, draft.sources, draft.overrides, draft.config.failover ?? true));
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const runTest = async (id: string) => {
    setTests((m) => ({ ...m, [id]: t("sources.testing") }));
    try {
      const r = await testProvider(id);
      setTests((m) => ({ ...m, [id]: r.ok ? t("sources.testOk", { ms: r.latencyMs }) : t("sources.testFailed", { error: r.errorMessage ?? r.errorClass ?? "" }) }));
    } catch (e) {
      setTests((m) => ({ ...m, [id]: errorMessage(e) }));
    }
  };

  return (
    <div className="pv">
      <p className="pv__intro">{t("sources.intro")}</p>
      <Switch
        checked={draft.config.failover ?? true}
        label={t("sources.failover")}
        onChange={(failover) => setDraft({ ...draft, config: { ...draft.config, failover } })}
      />
      <ol className="pv__sources" aria-label={t("sources.title")}>
        {draft.sources.map((s, i) => {
          const st = sourceStatus(view(s.providerId), s);
          const until = st.until ? formatDate(st.until, "time") : "";
          return (
            <li
              key={s.providerId}
              className="pv__source"
              draggable
              onDragStart={() => setDragFrom(i)}
              onDragOver={(e) => e.preventDefault()}
              onDrop={() => {
                if (dragFrom !== null) setSources(reorder(draft.sources, dragFrom, i));
                setDragFrom(null);
              }}
            >
              <GripVertical size={14} aria-hidden />
              <span className="pv__source-rank">{i + 1}</span>
              <span className="pv__source-name">{name(s.providerId)}</span>
              <span className={`pv__chip pv__chip--${st.kind}`}>{t(`sources.status.${st.kind}`, { until })}</span>
              <span className="pv__source-caps">
                {CAPABILITIES.map((c: keyof SourceCapabilities) => (
                  <label key={c} className="pv__cap">
                    <input type="checkbox" checked={s.capabilities?.[c] ?? false} onChange={() => update(i, toggleCapability(s, c))} />
                    {t(`sources.cap.${c}`)}
                  </label>
                ))}
              </span>
              <Switch checked={s.enabled} label={t("sources.enabled")} onChange={(enabled) => update(i, { ...s, enabled })} />
              <IconButton label={t("sources.moveUp", { name: name(s.providerId) })} size="sm" icon={<ArrowUp size={14} />} disabled={i === 0} onClick={() => setSources(reorder(draft.sources, i, i - 1))} />
              <IconButton label={t("sources.moveDown", { name: name(s.providerId) })} size="sm" icon={<ArrowDown size={14} />} disabled={i === draft.sources.length - 1} onClick={() => setSources(reorder(draft.sources, i, i + 1))} />
              <IconButton label={t("sources.test", { name: name(s.providerId) })} size="sm" icon={<FlaskConical size={14} />} onClick={() => void runTest(s.providerId)} />
              {tests[s.providerId] ? <span className="pv__source-test">{tests[s.providerId]}</span> : null}
            </li>
          );
        })}
      </ol>

      <h3 className="pv__subtitle">{t("sources.overrides")}</h3>
      <p className="pv__hint">{t("sources.overridesHint")}</p>
      {draft.purposes.map((p) => {
        const list = draft.overrides[p];
        return (
          <details key={p} className="pv__override">
            <summary>
              {t(`sources.purpose.${p}`, { defaultValue: p })} — {list ? t("sources.custom") : t("sources.global")}
            </summary>
            <Switch
              checked={!!list}
              label={t("sources.useCustom")}
              onChange={(on) => setDraft({ ...draft, overrides: { ...draft.overrides, [p]: on ? draft.sources.map((s) => s.providerId) : undefined } })}
            />
            {list
              ? draft.sources.map((s) => (
                  <label key={s.providerId} className="pv__cap">
                    <input
                      type="checkbox"
                      checked={list.includes(s.providerId)}
                      onChange={(e) =>
                        setDraft({
                          ...draft,
                          overrides: {
                            ...draft.overrides,
                            [p]: e.target.checked ? [...list, s.providerId] : list.filter((x) => x !== s.providerId),
                          },
                        })
                      }
                    />
                    {name(s.providerId)}
                  </label>
                ))
              : null}
          </details>
        );
      })}

      <div className="pv__actions">
        <Button variant="primary" leftIcon={<Save size={14} />} disabled={busy} onClick={() => void save()}>
          {t("sources.save")}
        </Button>
        <Button leftIcon={<RefreshCw size={14} />} onClick={() => void load()}>
          {t("sources.reload")}
        </Button>
      </div>
      {error ? <p className="pv__error" role="alert">{error}</p> : null}

      <h3 className="pv__subtitle">{t("sources.log")}</h3>
      {draft.log.length === 0 ? (
        <p className="pv__hint">{t("sources.logEmpty")}</p>
      ) : (
        <ul className="pv__log">
          {draft.log.map((r, i) => (
            <li key={`${r.at}-${i}`}>
              {formatDate(r.at, "timeSeconds")} —{" "}
              {tn("events.sourceUnavailable", {
                from: r.fromName,
                to: r.toName,
                reason: tn(`fallbackReason.${r.reason}`, { defaultValue: r.reason }),
              })}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
