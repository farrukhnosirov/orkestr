/**
 * Codex CLI setup blocks, shared by the Codex panel and the «Codex (ChatGPT
 * subscription)» settings card:
 *
 * - `CodexInstall`: «Install Codex CLI» (`codex.install`, needs npm) with a
 *   spinner and the npm log tail on failure; «Update» when a newer version
 *   exists.
 * - `CodexSignIn`: «Sign in with ChatGPT» (the CLI's browser flow; the sign-in
 *   link is shown too and the state follows `CodexAccount` events), device
 *   code (URL + one-time code with Copy), sign-in in a terminal, cancel.
 * - `CodexAccountLine`: e-mail + plan of the signed-in account, sign out.
 */
import { Badge, Button, Spinner } from "@orkestr/ui";
import { Check, Copy, Download, ExternalLink, KeyRound, LogIn, LogOut, RefreshCw, SquareTerminal, X } from "lucide-react";
import { useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { useT } from "../../i18n";
import { useTerminalUi } from "../../panels/Terminal/store";
import { codexErrorKey } from "./model";
import { useCodexStore } from "./store";
import "./codexSetup.css";

/** Last lines of a long log. */
export function logTail(log: string, lines = 12): string {
  const all = log.replace(/\s+$/, "").split(/\r?\n/);
  return all.slice(Math.max(0, all.length - lines)).join("\n");
}

export function CodexInstall({ compact = false }: { compact?: boolean }) {
  const { t } = useT("codex");
  const status = useCodexStore((s) => s.status);
  const installing = useCodexStore((s) => s.installing);
  const install = useCodexStore((s) => s.install);
  const installCli = useCodexStore((s) => s.installCli);
  const loadStatus = useCodexStore((s) => s.loadStatus);
  const loading = useCodexStore((s) => s.statusLoading);
  const installed = status?.binary != null;
  const npm = status?.npmAvailable !== false;

  if (installed && !status?.updateAvailable && !installing && !(install && !install.ok)) return null;

  return (
    <div className={compact ? "cx-setup cx-setup--compact" : "cx-setup"} data-testid="codex-install">
      {!installed && !compact && <p className="cx-setup__text">{t("install.description")}</p>}
      {!npm && !installed && <p className="cx-setup__warn">{t("install.noNpm")}</p>}
      <div className="cx-setup__row">
        {!installed ? (
          <Button
            variant="primary"
            leftIcon={<Download size={14} />}
            loading={installing}
            disabled={!npm || installing}
            onClick={() => void installCli(false)}
            data-testid="codex-install-button"
          >
            {installing ? t("install.installing") : t("install.button")}
          </Button>
        ) : (
          status?.updateAvailable && (
            <Button
              leftIcon={<RefreshCw size={14} />}
              loading={installing}
              disabled={!npm || installing}
              onClick={() => void installCli(true)}
              data-testid="codex-update-button"
            >
              {installing
                ? t("install.updating")
                : status.latestVersion
                  ? t("install.updateTo", { version: status.latestVersion })
                  : t("install.update")}
            </Button>
          )
        )}
        {!installed && (
          <Button variant="ghost" leftIcon={<RefreshCw size={14} />} loading={loading} onClick={() => void loadStatus(true)}>
            {t("actions.checkAgain")}
          </Button>
        )}
      </div>
      {installing && <p className="cx-setup__muted">{t("install.wait")}</p>}
      {install && !install.ok && (
        <div className="cx-setup__fail" role="alert">
          <strong>{t("install.failed")}</strong>
          <pre className="cx-setup__log">{logTail(install.log)}</pre>
        </div>
      )}
      {install?.ok && <p className="cx-setup__ok" role="status">{t("install.done")}</p>}
    </div>
  );
}

function CopyCode({ code }: { code: string }) {
  const { t } = useT("codex");
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };
  return (
    <span className="cx-code">
      <code className="cx-code__value" data-testid="codex-user-code">
        {code}
      </code>
      <Button size="sm" leftIcon={copied ? <Check size={13} /> : <Copy size={13} />} onClick={() => void copy()}>
        {copied ? t("signIn.copied") : t("signIn.copy")}
      </Button>
    </span>
  );
}

export function CodexSignIn({ projectId, compact = false }: { projectId: string | null; compact?: boolean }) {
  const { t } = useT("codex");
  const login = useCodexStore((s) => s.login);
  const busy = useCodexStore((s) => s.loginBusy);
  const error = useCodexStore((s) => s.error);
  const startLogin = useCodexStore((s) => s.startLogin);
  const cancelLogin = useCodexStore((s) => s.cancelLogin);
  const openInTerminal = useCodexStore((s) => s.openInTerminal);
  const loadStatus = useCodexStore((s) => s.loadStatus);
  const statusLoading = useCodexStore((s) => s.statusLoading);
  const pending = login?.phase === "pending";
  const [terminalOpened, setTerminalOpened] = useState(false);

  const inTerminal = async () => {
    const id = await openInTerminal({ projectId, login: true });
    if (id) {
      if (projectId) useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
      setTerminalOpened(true);
    }
  };

  return (
    <div className={compact ? "cx-setup cx-setup--compact" : "cx-setup"} data-testid="codex-signin">
      {!compact && <p className="cx-setup__text">{t("signIn.description")}</p>}
      {pending && login.mode === "browser" && (
        <div className="cx-setup__pending" role="status" data-testid="codex-login-pending">
          <Spinner size={14} />
          <span>{t("signIn.waitingBrowser")}</span>
          {login.authUrl && (
            <a className="cx-setup__link" href={login.authUrl} target="_blank" rel="noreferrer" title={login.authUrl}>
              <ExternalLink size={12} aria-hidden /> {t("signIn.openLink")}
            </a>
          )}
        </div>
      )}
      {pending && login.mode === "deviceCode" && (
        <div className="cx-setup__device" role="status" data-testid="codex-login-device">
          <span>{t("signIn.deviceStep1")}</span>
          {login.verificationUrl ? (
            <a className="cx-setup__link" href={login.verificationUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={12} aria-hidden /> {login.verificationUrl}
            </a>
          ) : (
            <Spinner size={12} />
          )}
          <span>{t("signIn.deviceStep2")}</span>
          {login.userCode ? <CopyCode code={login.userCode} /> : <Spinner size={12} />}
          <span className="cx-setup__muted">{t("signIn.deviceWaiting")}</span>
        </div>
      )}
      <div className="cx-setup__row">
        {pending ? (
          <Button leftIcon={<X size={14} />} onClick={() => void cancelLogin()} data-testid="codex-login-cancel">
            {t("signIn.cancel")}
          </Button>
        ) : (
          <>
            <Button
              variant="primary"
              size={compact ? "sm" : "md"}
              leftIcon={<LogIn size={14} />}
              loading={busy}
              onClick={() => void startLogin("browser")}
              data-testid="codex-signin-button"
            >
              {t("signIn.button")}
            </Button>
            <Button size={compact ? "sm" : "md"} leftIcon={<KeyRound size={14} />} disabled={busy} onClick={() => void startLogin("deviceCode")} data-testid="codex-signin-device">
              {t("signIn.deviceCode")}
            </Button>
            <Button size={compact ? "sm" : "md"} variant="ghost" leftIcon={<SquareTerminal size={14} />} onClick={() => void inTerminal()} data-testid="codex-signin-terminal">
              {t("signIn.terminal")}
            </Button>
            {!compact && (
              <Button variant="ghost" leftIcon={<RefreshCw size={14} />} loading={statusLoading} onClick={() => void loadStatus(true)}>
                {t("actions.checkAgain")}
              </Button>
            )}
          </>
        )}
      </div>
      {terminalOpened && !pending && <p className="cx-setup__muted">{t("signIn.terminalHint")}</p>}
      {login?.phase === "failed" && (
        <p className="cx-setup__warn" role="alert">
          {login.error ? t("signIn.failedWith", { error: login.error }) : t("signIn.failed")}
        </p>
      )}
      {error && !pending && <p className="cx-setup__warn">{codexErrorKey(error) ? t("errors.appServer") : error}</p>}
    </div>
  );
}

export function CodexAccountLine({ showLogout = true }: { showLogout?: boolean }) {
  const { t } = useT("codex");
  const status = useCodexStore((s) => s.status);
  const logout = useCodexStore((s) => s.logout);
  if (!status?.loggedIn) return null;
  const acc = status.account;
  return (
    <div className="cx-account" data-testid="codex-account">
      <Badge tone="success" icon={<Check size={12} />}>
        {t("account.signedIn")}
      </Badge>
      {acc?.email && <span className="cx-account__email" title={acc.email}>{acc.email}</span>}
      {acc?.plan && <Badge tone="violet">{t("account.plan", { plan: acc.plan })}</Badge>}
      {acc && !acc.email && acc.kind !== "chatgpt" && <Badge tone="info">{t("account.apiKey")}</Badge>}
      {showLogout && (
        <Button size="sm" variant="ghost" leftIcon={<LogOut size={13} />} onClick={() => void logout()}>
          {t("account.signOut")}
        </Button>
      )}
    </div>
  );
}
