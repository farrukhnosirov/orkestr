/** Create / edit an environment: kind, protection, target + adapter settings, variables, vault credentials, URLs. */
import type { AdapterInfo, EnvironmentKind, EnvironmentRecord, SecretReference } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { useMemo, useState } from "react";

import { useT } from "../../i18n";
import { compactSettings, formatKv, KINDS, kindKey, lines, parseKv, schemaFields, targetKey } from "./logic";
import { AvailabilityBadge, useAdapterLabel } from "./parts";

export interface EnvironmentDraft {
  name: string;
  kind: EnvironmentKind;
  protected: boolean;
  targetKey: string;
  settings: Record<string, string>;
  variables: Record<string, string>;
  secretRefs: string[];
  urls: string[];
}

export function EnvironmentForm({
  initial,
  adapters,
  secrets,
  busy,
  onSave,
  onCancel,
}: {
  initial: EnvironmentRecord | null;
  adapters: readonly AdapterInfo[];
  secrets: readonly SecretReference[];
  busy: boolean;
  onSave: (draft: EnvironmentDraft) => void;
  onCancel: () => void;
}) {
  const { t } = useT("deploy");
  const adapterLabel = useAdapterLabel();
  const env = initial?.environment;
  const [name, setName] = useState(env?.name ?? "");
  const [kind, setKind] = useState<EnvironmentKind>(env?.kind ?? "Staging");
  const [prot, setProt] = useState(env?.protected ?? false);
  const [target, setTarget] = useState(initial?.target ? targetKey(initial.target) : "");
  const [settings, setSettings] = useState<Record<string, string>>(initial?.adapterSettings ?? {});
  const [vars, setVars] = useState(formatKv(env?.variables));
  const [refs, setRefs] = useState<string[]>(env?.secretRefs ?? []);
  const [urls, setUrls] = useState((env?.urls ?? []).join("\n"));
  const adapter = useMemo(() => adapters.find((a) => targetKey(a.target) === target), [adapters, target]);
  const fields = useMemo(() => schemaFields(adapter?.settingsSchema), [adapter]);
  const isProd = kindKey(kind) === "Production";
  const needed = adapter && adapter.availability.state === "NeedsCredential" ? adapter.availability.secret : null;
  const missingRequired = fields.some((f) => f.required && !(settings[f.key] ?? "").trim());
  const canSave = name.trim() !== "" && target !== "" && !missingRequired && !busy;

  const submit = () =>
    onSave({
      name: name.trim(),
      kind,
      protected: prot || isProd,
      targetKey: target,
      settings: compactSettings(Object.fromEntries(fields.map((f) => [f.key, settings[f.key] ?? ""]))),
      variables: parseKv(vars),
      secretRefs: refs,
      urls: lines(urls),
    });

  return (
    <form
      className="dp-form"
      data-testid="env-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (canSave) submit();
      }}
    >
      <h3 className="dp-h3">{initial ? t("form.editTitle", { name: env?.name ?? "" }) : t("form.createTitle")}</h3>
      <Input label={t("form.name")} value={name} onChange={(e) => setName(e.target.value)} data-testid="env-name" />
      <label className="dp-field">
        <span>{t("form.kind")}</span>
        <select className="dp-select" value={kindKey(kind)} onChange={(e) => setKind(e.target.value as EnvironmentKind)} data-testid="env-kind">
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {t(`kind.${k}`)}
            </option>
          ))}
        </select>
      </label>
      <label className="dp-check">
        <input type="checkbox" checked={prot || isProd} disabled={isProd} onChange={(e) => setProt(e.target.checked)} data-testid="env-protected" />
        <span>{isProd ? t("form.productionProtected") : t("form.protected")}</span>
      </label>
      <label className="dp-field">
        <span>{t("form.target")}</span>
        <select className="dp-select" value={target} onChange={(e) => setTarget(e.target.value)} data-testid="env-target">
          <option value="">{t("form.chooseTarget")}</option>
          {adapters.map((a) => (
            <option key={a.id} value={targetKey(a.target)}>
              {adapterLabel(a.id)}
            </option>
          ))}
        </select>
      </label>
      {adapter && (
        <p className="dp-muted">
          <AvailabilityBadge adapter={adapter} />
        </p>
      )}
      {needed && (
        <p className="dp-warn" role="note">
          {t("form.needsSecret", { secret: needed })}
        </p>
      )}
      {fields.length > 0 && (
        <fieldset className="dp-fieldset">
          <legend>{t("form.settings")}</legend>
          {fields.map((f) =>
            f.options ? (
              <label key={f.key} className="dp-field">
                <span>{f.key}</span>
                <select className="dp-select" value={settings[f.key] ?? f.defaultValue ?? ""} onChange={(e) => setSettings({ ...settings, [f.key]: e.target.value })}>
                  {f.options.map((o) => (
                    <option key={o} value={o}>
                      {o}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <Input
                key={f.key}
                label={f.required ? `${f.key} *` : f.key}
                hint={f.description ?? undefined}
                placeholder={f.defaultValue ?? ""}
                value={settings[f.key] ?? ""}
                onChange={(e) => setSettings({ ...settings, [f.key]: e.target.value })}
                data-testid={`env-setting-${f.key}`}
              />
            ),
          )}
        </fieldset>
      )}
      <label className="dp-field">
        <span>{t("form.variables")}</span>
        <textarea className="dp-textarea" rows={3} value={vars} onChange={(e) => setVars(e.target.value)} data-testid="env-vars" />
      </label>
      <fieldset className="dp-fieldset" data-testid="env-secrets">
        <legend>{t("form.secrets")}</legend>
        {secrets.length === 0 && <p className="dp-muted">{t("form.noSecrets")}</p>}
        {secrets.map((s) => (
          <label key={s.id} className="dp-check">
            <input
              type="checkbox"
              checked={refs.includes(s.id)}
              onChange={(e) => setRefs(e.target.checked ? [...refs, s.id] : refs.filter((r) => r !== s.id))}
              data-testid={`env-secret-${s.name}`}
            />
            <code>{s.name}</code>
            {s.name === needed && <Badge tone="warning">{t("form.required")}</Badge>}
          </label>
        ))}
      </fieldset>
      <label className="dp-field">
        <span>{t("form.urls")}</span>
        <textarea className="dp-textarea" rows={2} value={urls} onChange={(e) => setUrls(e.target.value)} />
      </label>
      <div className="dp-row">
        <Button type="submit" variant="primary" disabled={!canSave} loading={busy} data-testid="env-save">
          {t("actions.save")}
        </Button>
        <Button type="button" variant="ghost" onClick={onCancel}>
          {t("actions.discard")}
        </Button>
      </div>
    </form>
  );
}
