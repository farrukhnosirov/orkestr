import type { SecretReference, SecretScope, VaultStatusResult } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { Eye, EyeOff, KeyRound, Lock, LockOpen, Plus, Trash2 } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useState } from "react";

import { maybeClient } from "../../core/client";
import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import type { SettingsSectionProps } from "../settings/sections";
import { secretScopeLabel, validateSecretName } from "./logic";
import { errorMessage, refreshFlags } from "./store";

function AddSecretForm({ projectId, disabled, onSaved }: { projectId: string | null; disabled: boolean; onSaved: () => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [reveal, setReveal] = useState(false);
  const [scopeKind, setScopeKind] = useState<"Global" | "Project">("Global");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT("security");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const nameError = validateSecretName(name);
    if (nameError) {
      setError(nameError);
      return;
    }
    if (!value) {
      setError(t("secrets.valueRequired"));
      return;
    }
    const c = maybeClient();
    if (!c) return;
    const scope: SecretScope = scopeKind === "Project" && projectId ? { scope: "Project", projectId } : { scope: "Global" };
    setBusy(true);
    setError(null);
    try {
      await c.call("secrets.set", { name: name.trim(), scope, value });
      setName("");
      setValue("");
      setReveal(false);
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="sec-rule-form" onSubmit={(e) => void submit(e)} aria-label={t("secrets.formAria")} autoComplete="off">
      <Input
        className="sec-rule-form__grow"
        label={t("secrets.name")}
        placeholder={t("secrets.namePlaceholder")}
        value={name}
        maxLength={128}
        onChange={(e) => setName(e.target.value)}
        spellCheck={false}
      />
      <Input
        className="sec-rule-form__grow"
        label={t("secrets.value")}
        type={reveal ? "text" : "password"}
        autoComplete="new-password"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        spellCheck={false}
        hint={t("secrets.valueHint")}
        rightSlot={
          <button
            type="button"
            className="sec-icon-btn"
            aria-label={reveal ? t("secrets.hideValue") : t("secrets.showValue")}
            onClick={() => setReveal((r) => !r)}
          >
            {reveal ? <EyeOff size={14} /> : <Eye size={14} />}
          </button>
        }
      />
      <label className="sec-select">
        <span>{t("permissions.scope")}</span>
        <select value={scopeKind} onChange={(e) => setScopeKind(e.target.value as "Global" | "Project")}>
          <option value="Global">{t("scope.global")}</option>
          <option value="Project" disabled={!projectId}>
            {t("scope.thisProject")}
          </option>
        </select>
      </label>
      <Button type="submit" variant="primary" leftIcon={<Plus size={14} />} loading={busy} disabled={disabled}>
        {t("secrets.save")}
      </Button>
      {error && (
        <p className="sec-error sec-rule-form__full" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** Settings section "Secrets Vault". Values are write-only. */
export function SecretsSettings({ projectId }: SettingsSectionProps) {
  const ready = useAppStore((s) => s.connection.state === "open");
  const projects = useAppStore((s) => s.projects);
  const [secrets, setSecrets] = useState<SecretReference[] | null>(null);
  const [status, setStatus] = useState<VaultStatusResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const { t } = useT(["security", "common"]);
  const fmt = useFormat();

  const load = useCallback(async () => {
    const c = maybeClient();
    if (!c) return;
    try {
      const [list, st] = await Promise.all([c.call("secrets.list", {}), c.call("secrets.status", {})]);
      setSecrets(list.secrets);
      setStatus(st);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  const lockToggle = async () => {
    const c = maybeClient();
    if (!c || !status) return;
    setBusy("lock");
    try {
      setStatus(await c.call(status.locked ? "secrets.unlock" : "secrets.lock", {}));
      void refreshFlags();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (s: SecretReference) => {
    const c = maybeClient();
    if (!c) return;
    if (!window.confirm(t("secrets.confirmDelete", { name: s.name }))) return;
    setBusy(s.id);
    try {
      await c.call("secrets.delete", { secretId: s.id });
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const projectName = (id: string) => projects.find((p) => p.id === id)?.name;

  return (
    <div className="sec-section">
      <section className="sec-vault-head" aria-label={t("secrets.statusAria")}>
        <div className="dialog__icon">
          <KeyRound size={18} />
        </div>
        <div>
          <h3 className="sec-h">{t("secrets.title")}</h3>
          <p className="sec-muted">
            {status
              ? t("secrets.summary", { count: status.secretCount, backend: t(`secrets.preference.${status.backend}`) })
              : t("common:state.loading")}
          </p>
        </div>
        {status && (
          <Badge tone={status.locked ? "danger" : "success"} icon={status.locked ? <Lock size={12} /> : <LockOpen size={12} />}>
            {status.locked ? t("secrets.locked") : t("secrets.unlocked")}
          </Badge>
        )}
        <Button
          className="sec-push"
          variant={status?.locked ? "primary" : "secondary"}
          leftIcon={status?.locked ? <LockOpen size={14} /> : <Lock size={14} />}
          loading={busy === "lock"}
          disabled={!status}
          onClick={() => void lockToggle()}
        >
          {status?.locked ? t("secrets.unlock") : t("secrets.lock")}
        </Button>
      </section>
      <AddSecretForm projectId={projectId} disabled={!status || status.locked} onSaved={() => void load()} />
      {error && (
        <p className="sec-error" role="alert">
          {error}
        </p>
      )}
      {secrets === null && !error ? (
        <Spinner label={t("secrets.loading")} />
      ) : secrets && secrets.length === 0 ? (
        <EmptyState
          icon={<KeyRound size={22} />}
          title={t("secrets.emptyTitle")}
          description={t("secrets.emptyDescription")}
        />
      ) : secrets ? (
        <table className="sec-table">
          <thead>
            <tr>
              <th scope="col">{t("secrets.name")}</th>
              <th scope="col">{t("permissions.scope")}</th>
              <th scope="col">{t("secrets.storedIn")}</th>
              <th scope="col">{t("secrets.created")}</th>
              <th scope="col">{t("secrets.lastUsed")}</th>
              <th scope="col">
                <span className="sr-only">{t("permissions.actions")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {secrets.map((s) => (
              <tr key={s.id}>
                <td>
                  <code>{s.name}</code>
                </td>
                <td>{secretScopeLabel(s.scope, projectName)}</td>
                <td>
                  <Badge tone={s.backend === "OsKeyring" ? "accent" : "violet"}>
                    {t(`secrets.backend.${s.backend}`)}
                  </Badge>
                </td>
                <td className="sec-muted" title={fmt.date(s.rotatedAt ?? s.createdAt, "dateTime")}>
                  {fmt.relative(s.rotatedAt ?? s.createdAt)}
                </td>
                <td className="sec-muted">{s.lastAccessedAt ? fmt.relative(s.lastAccessedAt) : t("common:time.never")}</td>
                <td>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={t("secrets.delete", { name: s.name })}
                    leftIcon={<Trash2 size={13} />}
                    loading={busy === s.id}
                    onClick={() => void remove(s)}
                  >
                    {t("common:actions.delete")}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
