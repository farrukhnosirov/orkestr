/**
 * Enrollment (node-protocol.md §3, ADR-0033), both sides of the pairing:
 *
 * - **Enroll a node** (coordinator): enter the worker's core id, pairing id and
 *   code → `nodes.enroll` → compare the SAS with the worker's screen; a
 *   mismatch revokes the enrollment at once.
 * - **This computer as a node** (worker): `devices.startPairing` with
 *   `kind: "node"` + trust level → show core id / pairing id / code, then the
 *   SAS once the coordinator claimed it → confirm → `devices.approve`.
 */
import type { EnrollNodeResult, TrustLevel } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { formatDate } from "../../i18n/format";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import { approveNodePairing, type NodePairing, startNodePairing, TRUST_LEVELS } from "./logic";
import { NoticeLine, useAction } from "./useAction";

const PAIRING_POLL_MS = 1500;

export function EnrollNode({ onChanged }: { onChanged: () => Promise<void> }) {
  const { t } = useT("nodes");
  const { busy, notice, run } = useAction(onChanged);
  const [coreId, setCoreId] = useState("");
  const [pairingId, setPairingId] = useState("");
  const [code, setCode] = useState("");
  const [name, setName] = useState("");
  const [result, setResult] = useState<EnrollNodeResult | null>(null);

  const enroll = () =>
    run("enroll", async () => {
      const r = await getClient().call("nodes.enroll", {
        coreId: coreId.trim(),
        pairingId: pairingId.trim(),
        code: code.trim(),
        ...(name.trim() ? { name: name.trim() } : {}),
      });
      setResult(r);
      setCode("");
      return null;
    });
  const matches = () =>
    run("match", async () => {
      const nm = result?.enrollment.name ?? "";
      setResult(null);
      setCoreId("");
      setPairingId("");
      setName("");
      return t("enroll.done", { name: nm });
    });
  const mismatch = () =>
    run("mismatch", async () => {
      if (result) await getClient().call("nodes.revoke", { nodeId: result.enrollment.nodeId });
      setResult(null);
      return t("enroll.mismatchRevoked");
    });

  const ready = coreId.trim() && pairingId.trim() && code.trim();
  return (
    <SettingGroup title={t("enroll.group")}>
      <p className="nd-muted">{t("enroll.intro")}</p>
      {!result ? (
        <div className="nd-form" data-testid="enroll-form">
          <Input label={t("enroll.coreId")} value={coreId} onChange={(e) => setCoreId(e.target.value)} data-testid="enroll-core-id" />
          <Input label={t("enroll.pairingId")} value={pairingId} onChange={(e) => setPairingId(e.target.value)} data-testid="enroll-pairing-id" />
          <Input label={t("enroll.code")} value={code} onChange={(e) => setCode(e.target.value)} autoComplete="off" data-testid="enroll-code" />
          <Input label={t("enroll.name")} value={name} onChange={(e) => setName(e.target.value)} />
          <div className="nd-actions">
            <Button variant="primary" loading={busy === "enroll"} disabled={busy !== null || !ready} onClick={() => void enroll()} data-testid="enroll-submit">
              {t("enroll.submit")}
            </Button>
          </div>
        </div>
      ) : (
        <div className="nd-form" data-testid="enroll-sas">
          <p>{t("enroll.enrolled", { name: result.enrollment.name, trust: t(`trust.${result.enrollment.trustLevel}`) })}</p>
          <p className="nd-code" aria-label={t("enroll.sasLabel")}>
            {result.sas}
          </p>
          <p className="nd-muted">{t("enroll.sasHint")}</p>
          <div className="nd-actions">
            <Button variant="primary" disabled={busy !== null} onClick={() => void matches()} data-testid="enroll-sas-match">
              {t("enroll.sasMatches")}
            </Button>
            <Button variant="danger" loading={busy === "mismatch"} disabled={busy !== null} onClick={() => void mismatch()} data-testid="enroll-sas-mismatch">
              {t("enroll.sasMismatch")}
            </Button>
          </div>
        </div>
      )}
      <NoticeLine notice={notice} />
    </SettingGroup>
  );
}

export function ThisComputerAsNode() {
  const { t } = useT("nodes");
  const { busy, notice, run } = useAction();
  const [trust, setTrust] = useState<TrustLevel>("standard");
  const [pairing, setPairing] = useState<NodePairing | null>(null);
  const [sasOk, setSasOk] = useState(false);

  const pairingId = pairing?.pairingId ?? null;
  const claimed = Boolean(pairing?.sas);
  useEffect(() => {
    if (!pairingId || claimed) return;
    let stopped = false;
    const poll = async () => {
      try {
        const list = await getClient().call("devices.list", {});
        const p = list.pending.find((x) => x.pairingId === pairingId);
        if (!stopped && p) setPairing((cur) => (cur && cur.pairingId === pairingId ? { ...cur, sas: p.sas, deviceName: p.deviceName } : cur));
      } catch {
        // transient: keep polling until the code expires or the panel closes
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), PAIRING_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [pairingId, claimed]);

  const start = () =>
    run("start", async () => {
      setSasOk(false);
      setPairing(await startNodePairing(trust));
      return null;
    });
  const approve = () =>
    run("approve", async () => {
      if (!pairing) return null;
      await approveNodePairing(pairing.pairingId, trust);
      const who = pairing.deviceName ?? "";
      setPairing(null);
      return t("worker.approved", { name: who });
    });

  return (
    <SettingGroup title={t("worker.group")}>
      <p className="nd-muted">{t("worker.intro")}</p>
      <SettingRow title={t("policy.trust")} description={t(`trustHint.${trust}`)}>
        <Segmented<TrustLevel>
          label={t("policy.trust")}
          value={trust}
          disabled={pairing !== null}
          options={TRUST_LEVELS.map((v) => ({ value: v, label: t(`trust.${v}`) }))}
          onChange={setTrust}
        />
      </SettingRow>
      <div className="nd-actions">
        <Button loading={busy === "start"} disabled={busy !== null} onClick={() => void start()} data-testid="worker-allow">
          {pairing ? t("worker.restart") : t("worker.allow")}
        </Button>
      </div>
      {pairing && (
        <div className="nd-form" data-testid="worker-pairing">
          <dl className="nd-facts">
            <dt>{t("enroll.coreId")}</dt>
            <dd className="nd-code" data-testid="worker-core-id">
              {pairing.coreId}
            </dd>
            <dt>{t("enroll.pairingId")}</dt>
            <dd className="nd-code" data-testid="worker-pairing-id">
              {pairing.pairingId}
            </dd>
            <dt>{t("enroll.code")}</dt>
            <dd className="nd-code" data-testid="worker-code">
              {pairing.code}
            </dd>
          </dl>
          <p className="nd-muted">{t("worker.expires", { time: formatDate(pairing.expiresAt) })}</p>
          {pairing.sas ? (
            <>
              {pairing.deviceName && <p>{t("worker.claimedBy", { name: pairing.deviceName })}</p>}
              <p className="nd-code" aria-label={t("enroll.sasLabel")} data-testid="worker-sas">
                {pairing.sas}
              </p>
              <p className="nd-muted">{t("worker.sasHint")}</p>
              <SettingRow title={t("worker.sasConfirmed")}>
                <Switch checked={sasOk} label={t("worker.sasConfirmed")} onChange={setSasOk} />
              </SettingRow>
              <div className="nd-actions">
                <Button variant="primary" loading={busy === "approve"} disabled={busy !== null || !sasOk} onClick={() => void approve()} data-testid="worker-approve">
                  {t("worker.approve")}
                </Button>
              </div>
            </>
          ) : (
            <p className="nd-muted" role="status">
              {t("worker.waiting")}
            </p>
          )}
        </div>
      )}
      <NoticeLine notice={notice} />
    </SettingGroup>
  );
}
