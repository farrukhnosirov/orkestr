import { Button, Input, Panel } from "@orkestr/ui";
import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDeps } from "../deps";
import type { AuthState, LoginResult } from "../gateway";

/** Passkey register/login with a TOTP fallback. */
export function Login({ state, onLoggedIn }: { state: AuthState; onLoggedIn: (r: LoginResult) => void }) {
  const { t } = useTranslation();
  const { gateway } = useDeps();
  const [name, setName] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (fn: () => Promise<LoginResult>) => {
    setBusy(true);
    setError(null);
    try {
      onLoggedIn(await fn());
    } catch (e) {
      setError((e as { status?: number }).status === 401 ? t("login.rejected") : t("login.failed"));
    } finally {
      setBusy(false);
    }
  };

  const onTotp = (e: FormEvent) => {
    e.preventDefault();
    void run(() => gateway.totpLogin(code.trim()));
  };

  return (
    <main className="web-center">
      <Panel title={t("login.title")} className="web-card">
        {state.hasAccount ? (
          <Button variant="primary" loading={busy} onClick={() => void run(() => gateway.loginPasskey())}>
            {t("login.passkey")}
          </Button>
        ) : (
          <div className="web-stack">
            <p>{t("login.firstRun")}</p>
            <Input label={t("login.name")} value={name} onChange={(e) => setName(e.target.value)} />
            <Button variant="primary" loading={busy} onClick={() => void run(() => gateway.registerPasskey(name.trim() || "owner"))}>
              {t("login.register")}
            </Button>
          </div>
        )}
        {state.hasAccount && state.totpEnabled !== false && (
          <form className="web-stack web-sep" onSubmit={onTotp}>
            <Input
              label={t("login.totp")}
              inputMode="numeric"
              autoComplete="one-time-code"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
            <Button type="submit" disabled={busy || code.trim().length < 6}>
              {t("login.totpSubmit")}
            </Button>
          </form>
        )}
        {error && (
          <p role="alert" className="web-error">
            {error}
          </p>
        )}
      </Panel>
    </main>
  );
}
