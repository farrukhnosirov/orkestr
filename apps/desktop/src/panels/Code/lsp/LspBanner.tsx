/**
 * Prompt shown above the editor when a language server needs the user's
 * approval to run (once per exact program), or failed.
 */
import { Button, IconButton } from "@orkestr/ui";
import { ShieldQuestion, TriangleAlert, X } from "lucide-react";
import { useState } from "react";

import { useT } from "../../../i18n";
import { describeError } from "../../../i18n/errors";
import { type LspEntry, lspCoordinator, useLspStore } from "./bridge";

export function LspBanner({ projectId }: { projectId: string }) {
  const { t } = useT("extensions");
  const entries = useLspStore((s) => s.entries);
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const visible = Object.values(entries).filter(
    (e) => e.projectId === projectId && (e.state === "needsApproval" || e.state === "failed" || e.state === "error") && !dismissed.has(`${e.serverId}:${e.state}`),
  );
  if (visible.length === 0) return null;

  const act = async (e: LspEntry, allow: boolean) => {
    setBusy(e.serverId);
    setError(null);
    try {
      if (allow) await lspCoordinator.approve(e);
      else await lspCoordinator.decline(e);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="code-lsp-banners">
      {visible.map((e) =>
        e.state === "needsApproval" ? (
          <div key={e.serverId} className="code-lsp-banner" role="alert">
            <ShieldQuestion size={16} aria-hidden="true" />
            <div className="code-lsp-banner__text">
              <strong>{t("lsp.approveTitle", { name: e.serverName })}</strong>
              <span>{t("lsp.approveBody")}</span>
              {e.command && <code className="code-lsp-banner__cmd">{e.command}</code>}
              {error && <span className="code-lsp-banner__error">{error}</span>}
            </div>
            <Button size="sm" variant="primary" disabled={busy !== null} onClick={() => void act(e, true)}>
              {t("lsp.allow")}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void act(e, false)}>
              {t("lsp.notNow")}
            </Button>
          </div>
        ) : (
          <div key={e.serverId} className="code-lsp-banner code-lsp-banner--error" role="status">
            <TriangleAlert size={16} aria-hidden="true" />
            <div className="code-lsp-banner__text">
              <strong>{t("lsp.failedTitle", { name: e.serverName })}</strong>
              {e.error && <span className="code-lsp-banner__error">{e.error}</span>}
            </div>
            <IconButton
              label={t("lsp.dismiss")}
              icon={<X size={13} />}
              size="sm"
              onClick={() => setDismissed((d) => new Set(d).add(`${e.serverId}:${e.state}`))}
            />
          </div>
        ),
      )}
    </div>
  );
}
