/**
 * Per-instance widget config form generated from the widget's `configSchema`
 * (top-level properties; nested values edited as JSON). Validation uses the
 * same schema subset core enforces on `dashboards.save`.
 */
import { Button, Input } from "@orkestr/ui";
import { type FormEvent, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { type ConfigField, configFields, type SchemaIssue, validateConfig } from "./configSchema";

export interface ConfigFormProps {
  schema: unknown;
  value: unknown;
  title?: string;
  onSubmit: (config: Record<string, unknown>, title: string | undefined) => void;
  onCancel: () => void;
}

type Draft = Record<string, string | boolean>;

function toDraft(fields: readonly ConfigField[], value: Record<string, unknown>): Draft {
  const d: Draft = {};
  for (const f of fields) {
    const v = value[f.key] ?? f.default;
    if (f.kind === "boolean") d[f.key] = v === true;
    else if (f.kind === "json") d[f.key] = v === undefined ? "" : JSON.stringify(v, null, 2);
    else if (f.kind === "select") d[f.key] = v === undefined ? "" : JSON.stringify(v);
    else d[f.key] = v === undefined || v === null ? "" : String(v);
  }
  return d;
}

/** Converts the draft back to a config; `null` entries mark unparsable JSON. */
export function draftToConfig(fields: readonly ConfigField[], draft: Draft): { config: Record<string, unknown>; badJson: string[] } {
  const config: Record<string, unknown> = {};
  const badJson: string[] = [];
  for (const f of fields) {
    const raw = draft[f.key];
    if (f.kind === "boolean") {
      config[f.key] = raw === true;
      continue;
    }
    if (typeof raw !== "string" || raw.trim() === "") continue;
    switch (f.kind) {
      case "number":
      case "integer":
        config[f.key] = Number(raw);
        break;
      case "select":
      case "json":
        try {
          config[f.key] = JSON.parse(raw) as unknown;
        } catch {
          badJson.push(f.key);
        }
        break;
      default:
        config[f.key] = raw;
    }
  }
  return { config, badJson };
}

export function ConfigForm({ schema, value, title, onSubmit, onCancel }: ConfigFormProps) {
  const { t } = useT("dashboards");
  const fields = useMemo(() => configFields(schema), [schema]);
  const initial = typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  const [draft, setDraft] = useState<Draft>(() => toDraft(fields, initial));
  const [titleDraft, setTitleDraft] = useState(title ?? "");
  const [issues, setIssues] = useState<SchemaIssue[]>([]);

  const issueFor = (key: string): string | undefined => {
    const hit = issues.find((i) => i.path === `/${key}` || i.path.startsWith(`/${key}/`));
    return hit ? t(`config.errors.${hit.code}`, { detail: hit.detail ?? "" }) : undefined;
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const { config, badJson } = draftToConfig(fields, draft);
    const found: SchemaIssue[] = badJson.map((k) => ({ path: `/${k}`, code: "type", detail: "json" }));
    found.push(...validateConfig(schema, config));
    setIssues(found);
    if (found.length === 0) onSubmit(config, titleDraft.trim() || undefined);
  };

  const general = issues.filter((i) => i.path === "" || !fields.some((f) => i.path === `/${f.key}` || i.path.startsWith(`/${f.key}/`)));

  return (
    <form className="dsh-config" onSubmit={submit} aria-label={t("config.title")} noValidate>
      <Input label={t("config.instanceTitle")} value={titleDraft} maxLength={120} onChange={(e) => setTitleDraft(e.target.value)} />
      {fields.length === 0 && <p className="dsh-config__empty">{t("config.noOptions")}</p>}
      {fields.map((f) => {
        const err = issueFor(f.key);
        const label = f.required ? `${f.label} *` : f.label;
        const set = (v: string | boolean) => setDraft((d) => ({ ...d, [f.key]: v }));
        const raw = draft[f.key];
        switch (f.kind) {
          case "boolean":
            return (
              <label key={f.key} className="dsh-config__check">
                <input type="checkbox" checked={raw === true} onChange={(e) => set(e.target.checked)} />
                <span>{label}</span>
                {err && <span className="dsh-config__error" role="alert">{err}</span>}
              </label>
            );
          case "select":
            return (
              <label key={f.key} className="dsh-config__field">
                <span className="ok-field__label">{label}</span>
                <select value={typeof raw === "string" ? raw : ""} onChange={(e) => set(e.target.value)} aria-invalid={err ? true : undefined}>
                  {!f.required && <option value="">{t("config.unset")}</option>}
                  {(f.options ?? []).map((o) => (
                    <option key={JSON.stringify(o)} value={JSON.stringify(o)}>
                      {String(o)}
                    </option>
                  ))}
                </select>
                {err && <span className="dsh-config__error" role="alert">{err}</span>}
              </label>
            );
          case "textarea":
          case "json":
            return (
              <label key={f.key} className="dsh-config__field">
                <span className="ok-field__label">{label}</span>
                <textarea
                  value={typeof raw === "string" ? raw : ""}
                  rows={f.kind === "json" ? 4 : 3}
                  maxLength={f.maxLength}
                  spellCheck={f.kind !== "json"}
                  onChange={(e) => set(e.target.value)}
                  aria-invalid={err ? true : undefined}
                />
                {f.description && <span className="ok-field__hint">{f.description}</span>}
                {err && <span className="dsh-config__error" role="alert">{err}</span>}
              </label>
            );
          default:
            return (
              <Input
                key={f.key}
                label={label}
                type={f.kind === "number" || f.kind === "integer" ? "number" : "text"}
                step={f.kind === "integer" ? 1 : undefined}
                min={f.min}
                max={f.max}
                value={typeof raw === "string" ? raw : ""}
                hint={f.description}
                error={err}
                onChange={(e) => set(e.target.value)}
              />
            );
        }
      })}
      {general.map((i) => (
        <p key={`${i.path}:${i.code}`} className="dsh-config__error" role="alert">
          {t(`config.errors.${i.code}`, { detail: i.detail ?? "" })}
        </p>
      ))}
      <div className="dsh-config__actions">
        <Button type="button" variant="ghost" onClick={onCancel}>
          {t("actions.cancel")}
        </Button>
        <Button type="submit" variant="primary">
          {t("config.apply")}
        </Button>
      </div>
    </form>
  );
}
