/**
 * <ProviderSetup/> — provider cards per kind (§55): connect API keys, detect
 * official CLIs, local endpoints, test connections, models, enable/disable,
 * delete. Reusable: the Settings section and onboarding both embed it.
 */
import type { Model, ProviderKind, ProviderKindInfo, ProviderView, TestProviderResult } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner, StatusDot } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  Cloud,
  Cpu,
  FlaskConical,
  HardDrive,
  KeyRound,
  Plug,
  Plus,
  RefreshCw,
  Server,
  Sparkles,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useState } from "react";
import { Trans } from "react-i18next";

import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { Switch } from "./controls";
import {
  authDetailText,
  formatContext,
  formatLatency,
  formatPricing,
  formatUsd,
  groupByKind,
  type HeaderRow,
  headersFromRows,
  instanceName,
  isCliKind,
  kindName,
  kindText,
  statusOf,
  validateBaseUrl,
} from "./logic";
import {
  checkHealth,
  clearCredential,
  createProvider,
  deleteProvider,
  errorMessage,
  listModels,
  loadProviders,
  setCredential,
  testProvider,
  updateProvider,
  useProvidersLive,
} from "./store";
import "./providers.css";

export interface ProviderSetupProps {
  /** Only show these kinds (onboarding may show a subset). */
  kinds?: readonly ProviderKind[];
  /** Denser grid for embedding. */
  compact?: boolean;
  /** Called after a provider was added. */
  onConfigured?: (view: ProviderView) => void;
}

const KIND_ICON: Record<ProviderKind, ReactNode> = {
  OpenAi: <Sparkles size={18} />,
  Anthropic: <Bot size={18} />,
  Gemini: <Sparkles size={18} />,
  DeepSeek: <Cloud size={18} />,
  Ollama: <HardDrive size={18} />,
  OpenAiCompatible: <Server size={18} />,
  ClaudeCode: <SquareTerminal size={18} />,
  Codex: <SquareTerminal size={18} />,
  Mock: <FlaskConical size={18} />,
  Custom: <Cpu size={18} />,
};

/** Provider cards for every supported kind. */
export function ProviderSetup({ kinds: only, compact = false, onConfigured }: ProviderSetupProps) {
  const connection = useAppStore((s) => s.connection.state);
  const { kinds, providers, loaded, loading, error } = useProvidersLive();
  const { t } = useT(["providers", "common"]);

  if (connection !== "open" && !loaded) {
    return <EmptyState icon={<Plug size={22} />} title={t("connecting")} description={t("connectingHint")} />;
  }
  if (error && !loaded) {
    return (
      <EmptyState
        icon={<X size={22} />}
        title={t("loadFailed")}
        description={error}
        action={
          <Button variant="primary" leftIcon={<RefreshCw size={14} />} onClick={() => void loadProviders()}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  }
  if (!loaded) {
    return (
      <div className="pv" aria-busy={loading}>
        <Spinner label={t("loading")} />
      </div>
    );
  }
  const visible = only ? kinds.filter((k) => only.includes(k.kind)) : kinds;
  const groups = groupByKind(visible, providers);
  if (groups.length === 0) {
    return <EmptyState icon={<Plug size={22} />} title={t("noKinds")} />;
  }
  return (
    <div className="pv">
      <div className={clsx("pv__grid", compact && "pv__grid--compact")}>
        {groups.map((g) => (
          <KindCard key={g.info.kind} info={g.info} instances={g.instances} onConfigured={onConfigured} />
        ))}
      </div>
    </div>
  );
}

interface KindCardProps {
  info: ProviderKindInfo;
  instances: ProviderView[];
  onConfigured?: (view: ProviderView) => void;
}

function KindCard({ info, instances, onConfigured }: KindCardProps) {
  const [adding, setAdding] = useState(false);
  const { t } = useT("providers");
  const cli = isCliKind(info.kind);
  const name = kindName(info);
  const signInNote = kindText(info.kind, "signInNote", info.signInNote);
  const loginInstructions = kindText(info.kind, "loginInstructions", info.loginInstructions);
  const titleId = `pv-kind-${info.kind}`;
  const canAddMore = info.kind === "OpenAiCompatible" || info.kind === "Ollama" || instances.length === 0;
  return (
    <section className="pv-card" aria-labelledby={titleId} data-configured={instances.length > 0 || undefined}>
      <header className="pv-card__head">
        <div className="pv-card__icon" aria-hidden="true">
          {KIND_ICON[info.kind]}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <h3 id={titleId} className="pv-card__title">
            {name}
          </h3>
          <p className="pv-card__desc">{kindText(info.kind, "description", info.description)}</p>
          <div className="pv-card__badges">
            {info.isLocal && <Badge tone="success">{t("badges.local")}</Badge>}
            {info.requiresApiKey && <Badge tone="info">{t("badges.apiKey")}</Badge>}
            {info.apiKeyOptional && <Badge tone="info">{t("badges.keyOptional")}</Badge>}
            {cli && <Badge tone="violet">{t("badges.officialCli")}</Badge>}
            {info.codingAgent && <Badge tone="violet">{t("badges.codingAgent")}</Badge>}
            {info.kind === "Mock" && <Badge tone="warning">{t("badges.offlineDemo")}</Badge>}
            {cli && info.detected != null && (
              <Badge tone={info.detected ? "success" : "neutral"}>{info.detected ? t("badges.detected") : t("badges.notInstalled")}</Badge>
            )}
          </div>
        </div>
      </header>

      {signInNote && <p className="pv-note">{signInNote}</p>}
      {cli && loginInstructions && (
        <p className={clsx("pv-note", info.detected === false && "pv-note--warn")}>
          <strong>{t("card.loginVia", { name })}</strong> {loginInstructions}
        </p>
      )}

      {instances.map((v) => (
        <ProviderInstance key={v.provider.id} view={v} info={info} />
      ))}

      {adding ? (
        <AddProviderForm
          info={info}
          existing={instances.length}
          onCancel={() => setAdding(false)}
          onDone={(v) => {
            setAdding(false);
            onConfigured?.(v);
          }}
        />
      ) : (
        canAddMore && (
          <div>
            <Button
              variant={instances.length === 0 ? "primary" : "ghost"}
              size="sm"
              leftIcon={<Plus size={14} />}
              onClick={() => setAdding(true)}
              disabled={cli && info.detected === false}
            >
              {instances.length === 0 ? (cli ? t("card.connectKind", { name }) : t("card.addKind", { name })) : t("card.addAnother")}
            </Button>
          </div>
        )
      )}
    </section>
  );
}

interface AddProviderFormProps {
  info: ProviderKindInfo;
  existing: number;
  onCancel: () => void;
  onDone: (view: ProviderView) => void;
}

function AddProviderForm({ info, existing, onCancel, onDone }: AddProviderFormProps) {
  const [name, setName] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState(info.kind === "OpenAiCompatible" ? "http://localhost:1234/v1" : "");
  const [model, setModel] = useState("");
  const [rows, setRows] = useState<HeaderRow[]>([{ name: "", value: "" }]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT(["providers", "common"]);
  const kindLabel = kindName(info);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (info.requiresApiKey && !apiKey.trim()) {
      setError(t("form.apiKeyRequired"));
      return;
    }
    const urlError = info.supportsBaseUrl ? validateBaseUrl(baseUrl) : null;
    if (urlError) {
      setError(urlError);
      return;
    }
    const parsed = headersFromRows(rows);
    if ("error" in parsed) {
      setError(parsed.error);
      return;
    }
    setBusy(true);
    try {
      const view = await createProvider({
        kind: info.kind,
        ...(name.trim() ? { name: name.trim() } : {}),
        ...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
        ...(info.supportsBaseUrl && baseUrl.trim() ? { baseUrl: baseUrl.trim() } : {}),
        ...(model.trim() ? { defaultModel: model.trim() } : {}),
        ...(info.supportsCustomHeaders && Object.keys(parsed.headers).length > 0 ? { headers: parsed.headers } : {}),
      });
      setApiKey("");
      onDone(view);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const simple = isCliKind(info.kind) || info.kind === "Mock";
  return (
    <form className="pv-form" onSubmit={(e) => void submit(e)} aria-label={t("card.addKind", { name: kindLabel })} onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <Input
        label={t("form.name")}
        placeholder={existing > 0 ? `${kindLabel} ${existing + 1}` : kindLabel}
        value={name}
        onChange={(e) => setName(e.target.value)}
        maxLength={80}
      />
      {(info.requiresApiKey || info.apiKeyOptional) && (
        <Input
          label={info.apiKeyOptional ? t("form.apiKeyOptional") : t("form.apiKey")}
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          hint={t("form.apiKeyHint")}
          leftIcon={<KeyRound size={14} />}
        />
      )}
      {info.supportsBaseUrl && (
        <Input
          label={t("form.baseUrl")}
          placeholder={info.defaultBaseUrl ?? t("form.baseUrlPlaceholder")}
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          hint={info.defaultBaseUrl ? t("form.baseUrlDefault", { url: info.defaultBaseUrl }) : undefined}
          spellCheck={false}
        />
      )}
      {!simple && (
        <Input
          label={t("form.defaultModel")}
          placeholder={info.defaultModel ?? t("form.modelPlaceholder")}
          value={model}
          onChange={(e) => setModel(e.target.value)}
          spellCheck={false}
        />
      )}
      {info.supportsCustomHeaders && <HeaderEditor rows={rows} onChange={setRows} />}
      {isCliKind(info.kind) && (
        <p className="pv-note">
          <Trans t={t} i18nKey="form.cliNote" values={{ binary: info.binary ?? "" }} components={{ code: <code /> }} />
        </p>
      )}
      {error && (
        <p className="pv-note pv-note--danger" role="alert">
          {error}
        </p>
      )}
      <div className="pv-form__actions">
        <Button variant="ghost" size="sm" onClick={onCancel} disabled={busy}>
          {t("common:actions.cancel")}
        </Button>
        <Button variant="primary" size="sm" type="submit" loading={busy}>
          {isCliKind(info.kind) ? t("form.connect") : t("form.addProvider")}
        </Button>
      </div>
    </form>
  );
}

function HeaderEditor({ rows, onChange }: { rows: HeaderRow[]; onChange: (rows: HeaderRow[]) => void }) {
  const set = (i: number, patch: Partial<HeaderRow>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const { t } = useT("providers");
  return (
    <fieldset className="pv-form" style={{ border: 0, padding: 0, margin: 0 }}>
      <legend className="ok-field__label">{t("form.headers")}</legend>
      {rows.map((r, i) => (
        <div className="pv-form__row" key={i}>
          <Input
            aria-label={t("form.headerName", { index: i + 1 })}
            placeholder={t("form.headerNamePlaceholder")}
            value={r.name}
            onChange={(e) => set(i, { name: e.target.value })}
            spellCheck={false}
          />
          <Input
            aria-label={t("form.headerValue", { index: i + 1 })}
            placeholder={t("form.headerValuePlaceholder")}
            value={r.value}
            onChange={(e) => set(i, { value: e.target.value })}
            spellCheck={false}
          />
          <IconButton label={t("form.removeHeader", { index: i + 1 })} icon={<X size={14} />} size="sm" onClick={() => onChange(rows.filter((_, j) => j !== i))} />
        </div>
      ))}
      <div>
        <Button variant="ghost" size="sm" leftIcon={<Plus size={14} />} onClick={() => onChange([...rows, { name: "", value: "" }])}>
          {t("form.addHeader")}
        </Button>
      </div>
    </fieldset>
  );
}

function ProviderInstance({ view, info }: { view: ProviderView; info: ProviderKindInfo }) {
  const p = view.provider;
  const st = statusOf(view);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<null | "test" | "health" | "toggle" | "delete">(null);
  const [test, setTest] = useState<TestProviderResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const { t } = useT(["providers", "common"]);

  const run = async (kind: NonNullable<typeof busy>, fn: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const status = view.status;
  const detailsId = `pv-inst-${p.id}`;
  return (
    <div className="pv-inst">
      <div className="pv-inst__row">
        <StatusDot status={st.dot} pulse={st.pulse} label={st.label} />
        <span className="pv-inst__name" title={instanceName(p)}>
          {instanceName(p)}
        </span>
        <Badge tone={st.dot === "online" ? "success" : st.dot === "error" ? "danger" : st.dot === "warning" ? "warning" : "neutral"}>{st.label}</Badge>
        <IconButton
          label={open ? t("instance.hideDetails") : t("instance.showDetails")}
          icon={open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          size="sm"
          aria-expanded={open}
          aria-controls={detailsId}
          onClick={() => setOpen((o) => !o)}
        />
      </div>
      <div className="pv-inst__meta">
        <span>
          <strong>{t("instance.auth")}</strong> {authDetailText(status)}
        </span>
        <span>
          <strong>{t("instance.latency")}</strong> {formatLatency(status.avgLatencyMs ?? status.latencyMs)}
        </span>
        {p.defaultModel && (
          <span>
            <strong>{t("instance.model")}</strong> {p.defaultModel}
          </span>
        )}
        {status.version && (
          <span>
            <strong>{t("instance.version")}</strong> {status.version}
          </span>
        )}
        {view.costModel === "Subscription" && <span>{t("instance.subscription")}</span>}
      </div>
      {status.healthDetail && st.dot !== "online" && <p className="pv-note pv-note--warn">{status.healthDetail}</p>}
      <div className="pv-inst__actions">
        <Button size="sm" variant="secondary" leftIcon={<Plug size={14} />} loading={busy === "test"} disabled={busy !== null} onClick={() => void run("test", async () => setTest(await testProvider(p.id)))}>
          {t("instance.test")}
        </Button>
        <IconButton label={t("instance.recheck")} tooltip icon={<RefreshCw size={14} />} size="sm" disabled={busy !== null} onClick={() => void run("health", () => checkHealth(p.id))} />
        <Switch checked={p.enabled} label={p.enabled ? t("instance.enabled") : t("instance.disabled")} disabled={busy !== null} onChange={(enabled) => void run("toggle", () => updateProvider(p.id, { enabled }))} />
        <span style={{ flex: 1 }} />
        {confirmDelete ? (
          <>
            <Button size="sm" variant="danger" loading={busy === "delete"} onClick={() => void run("delete", () => deleteProvider(p.id))}>
              {info.requiresApiKey || info.apiKeyOptional ? t("instance.deleteWithKey") : t("common:actions.delete")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>
              {t("instance.keep")}
            </Button>
          </>
        ) : (
          <IconButton label={t("instance.deleteNamed", { name: instanceName(p) })} tooltip variant="danger" icon={<Trash2 size={14} />} size="sm" disabled={busy !== null} onClick={() => setConfirmDelete(true)} />
        )}
      </div>
      {test && (
        <div className={clsx("pv-result", test.ok ? "pv-result--ok" : "pv-result--fail")} role="status">
          {test.ok ? (
            <>
              {t("instance.testOk", { model: test.model, latency: formatLatency(test.latencyMs) })}
              {test.sample ? t("instance.testSample", { sample: test.sample.slice(0, 120) }) : null}
              {test.costUsd != null ? t("instance.testCost", { cost: formatUsd(test.costUsd) }) : null}
            </>
          ) : (
            t("instance.testFailed", { errorClass: test.errorClass ?? t("instance.errorFallback"), message: test.errorMessage ?? "" })
          )}
        </div>
      )}
      {error && (
        <p className="pv-note pv-note--danger" role="alert">
          {error}
        </p>
      )}
      {open && (
        <div className="pv-inst__body" id={detailsId}>
          <ProviderSettings view={view} info={info} />
          <ModelList providerId={p.id} />
        </div>
      )}
    </div>
  );
}

function ProviderSettings({ view, info }: { view: ProviderView; info: ProviderKindInfo }) {
  const p = view.provider;
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState(p.baseUrl ?? "");
  const [model, setModel] = useState(p.defaultModel ?? "");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const { t } = useT(["providers", "common"]);

  const save = async (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true);
    setMsg(null);
    try {
      await fn();
      setMsg({ ok: true, text: ok });
    } catch (e) {
      setMsg({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const usesKey = info.requiresApiKey || info.apiKeyOptional;
  return (
    <div className="pv-form">
      {usesKey && (
        <form
          className="pv-form__row"
          onSubmit={(e) => {
            e.preventDefault();
            if (!key.trim()) return;
            void save(async () => {
              await setCredential(p.id, key.trim());
              setKey("");
            }, t("settings.keySaved"));
          }}
        >
          <Input
            label={view.hasCredential ? t("settings.replaceKey") : t("form.apiKey")}
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder={view.hasCredential ? t("settings.storedPlaceholder") : t("settings.pastePlaceholder")}
            value={key}
            onChange={(e) => setKey(e.target.value)}
            leftIcon={<KeyRound size={14} />}
          />
          <div style={{ flex: "none", display: "flex", gap: 8 }}>
            <Button size="sm" type="submit" variant="primary" disabled={busy || !key.trim()}>
              {t("settings.saveKey")}
            </Button>
            {view.hasCredential && (
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => void save(() => clearCredential(p.id), t("settings.keyRemoved"))}>
                {t("settings.removeKey")}
              </Button>
            )}
          </div>
        </form>
      )}
      {info.supportsBaseUrl && (
        <div className="pv-form__row">
          <Input label={t("form.baseUrl")} value={baseUrl} placeholder={info.defaultBaseUrl ?? ""} onChange={(e) => setBaseUrl(e.target.value)} spellCheck={false} />
          <div style={{ flex: "none" }}>
            <Button
              size="sm"
              disabled={busy || baseUrl === (p.baseUrl ?? "")}
              onClick={() => {
                const err = validateBaseUrl(baseUrl);
                if (err) setMsg({ ok: false, text: err });
                else void save(() => updateProvider(p.id, { baseUrl: baseUrl.trim() }), t("settings.baseUrlUpdated"));
              }}
            >
              {t("common:actions.save")}
            </Button>
          </div>
        </div>
      )}
      <div className="pv-form__row">
        <Input
          label={t("form.defaultModel")}
          value={model}
          placeholder={info.defaultModel ?? t("form.modelPlaceholder")}
          onChange={(e) => setModel(e.target.value)}
          spellCheck={false}
        />
        <div style={{ flex: "none" }}>
          <Button
            size="sm"
            disabled={busy || model === (p.defaultModel ?? "")}
            onClick={() => void save(() => updateProvider(p.id, { defaultModel: model.trim() }), t("settings.modelUpdated"))}
          >
            {t("common:actions.save")}
          </Button>
        </div>
      </div>
      {msg && (
        <p className={clsx("pv-note", !msg.ok && "pv-note--danger")} role={msg.ok ? "status" : "alert"}>
          {msg.text}
        </p>
      )}
    </div>
  );
}

function ModelList({ providerId }: { providerId: string }) {
  const [models, setModels] = useState<Model[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT("providers");
  const load = async (refresh: boolean) => {
    setLoading(true);
    setError(null);
    try {
      setModels(await listModels(providerId, refresh));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    let alive = true;
    setLoading(true);
    listModels(providerId, false)
      .then((m) => alive && setModels(m))
      .catch((e: unknown) => alive && setError(errorMessage(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [providerId]);
  return (
    <div className="pv-form">
      <div className="pv-inst__row">
        <span className="ok-field__label" style={{ flex: 1 }}>
          {t("models.title")}
        </span>
        <IconButton label={t("models.refresh")} tooltip size="sm" icon={<RefreshCw size={14} />} disabled={loading} onClick={() => void load(true)} />
      </div>
      {loading && <Spinner label={t("models.loading")} />}
      {error && (
        <p className="pv-note pv-note--danger" role="alert">
          {error}
        </p>
      )}
      {models && models.length === 0 && <p className="pv-note">{t("models.empty")}</p>}
      {models && models.length > 0 && (
        <ul className="pv-models" aria-label={t("models.listLabel")}>
          {models.map((m) => (
            <li key={m.id}>
              <span className="pv-models__id" title={m.displayName}>
                {m.id}
              </span>
              <span className="pv-models__meta">
                {t("models.meta", { window: formatContext(m.contextWindow), price: m.isLocal ? t("models.local") : formatPricing(m.pricing) })}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
