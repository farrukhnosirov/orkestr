/**
 * Install / update flow shared by Browse and Installed: details → (trust the
 * publisher key on first use) → install → (permission prompt for plugin and
 * widget packages).
 */
import { type ReactNode, useCallback, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { type EntryDetails, type GrantDecision, marketplaceApi } from "./api";
import { PermissionDialog, TrustKeyDialog } from "./dialogs";
import { installGate } from "./model";

export interface Notice {
  tone: "success" | "danger" | "info";
  text: string;
}

export interface InstallFlow {
  /** Starts installing `id` (optionally a version / source). */
  start: (id: string, sourceId?: string, version?: string) => Promise<void>;
  busy: string | null;
  notice: Notice | null;
  setNotice: (n: Notice | null) => void;
  dialogs: ReactNode;
}

export function useInstallFlow(onChanged: () => void): InstallFlow {
  const { t } = useT("marketplace");
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [trustFor, setTrustFor] = useState<{
    d: EntryDetails;
    version?: string;
  } | null>(null);
  const [grantsFor, setGrantsFor] = useState<{
    pluginId: string;
    name: string;
    pending: EntryDetails["permissions"];
  } | null>(null);

  const install = useCallback(
    async (d: EntryDetails, version?: string) => {
      setBusy(d.entry.id);
      try {
        const r = await marketplaceApi.install({
          id: d.entry.id,
          sourceId: d.entry.sourceId,
          version,
        });
        onChanged();
        if (r.plugin && r.pendingGrants.length > 0) {
          setGrantsFor({
            pluginId: r.plugin.plugin.id,
            name: d.entry.name,
            pending: r.pendingGrants,
          });
        } else {
          setNotice({
            tone: "success",
            text: t("notice.installed", {
              name: d.entry.name,
              version: r.version,
            }),
          });
        }
      } catch (e) {
        setNotice({ tone: "danger", text: describeError(e) });
      } finally {
        setBusy(null);
      }
    },
    [onChanged, t],
  );

  const start = useCallback(
    async (id: string, sourceId?: string, version?: string) => {
      setNotice(null);
      setBusy(id);
      let d: EntryDetails;
      try {
        d = await marketplaceApi.get(id, sourceId);
      } catch (e) {
        setNotice({ tone: "danger", text: describeError(e) });
        setBusy(null);
        return;
      }
      setBusy(null);
      const gate = installGate(d);
      if (gate === "revoked" || gate === "keyRevoked") {
        setNotice({
          tone: "danger",
          text: t(gate === "revoked" ? "notice.revoked" : "notice.keyRevoked", {
            name: d.entry.name,
          }),
        });
        return;
      }
      if (gate === "untrusted") {
        setTrustFor({ d, version });
        return;
      }
      await install(d, version);
    },
    [install, t],
  );

  const trustAndInstall = async () => {
    if (!trustFor) return;
    const { d, version } = trustFor;
    setBusy(d.entry.id);
    try {
      await marketplaceApi.trustKey(d.key.keyId, d.key.fingerprint);
    } catch (e) {
      setNotice({ tone: "danger", text: describeError(e) });
      setBusy(null);
      setTrustFor(null);
      return;
    }
    setTrustFor(null);
    await install(d, version);
  };

  const decide = async (decisions: GrantDecision[]) => {
    if (!grantsFor) return;
    setBusy(grantsFor.pluginId);
    try {
      await marketplaceApi.grant(grantsFor.pluginId, decisions, true);
      setNotice({
        tone: "success",
        text: t("notice.granted", { name: grantsFor.name }),
      });
      setGrantsFor(null);
      onChanged();
    } catch (e) {
      setNotice({ tone: "danger", text: describeError(e) });
    } finally {
      setBusy(null);
    }
  };

  const dialogs = (
    <>
      {trustFor && (
        <TrustKeyDialog
          pkey={trustFor.d.key}
          busy={busy !== null}
          onTrust={() => void trustAndInstall()}
          onCancel={() => setTrustFor(null)}
        />
      )}
      {grantsFor && (
        <PermissionDialog
          name={grantsFor.name}
          pending={grantsFor.pending}
          busy={busy !== null}
          onDecide={(ds) => void decide(ds)}
          onCancel={() => {
            setNotice({
              tone: "info",
              text: t("notice.grantsPending", { name: grantsFor.name }),
            });
            setGrantsFor(null);
          }}
        />
      )}
    </>
  );

  return { start, busy, notice, setNotice, dialogs };
}
