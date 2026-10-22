/**
 * Settings → Remote & Sync → remote access: enable / disable, pairing code +
 * SAS confirmation + approve, paired devices with revoke, and the kill switch
 * (revoke all sessions + disable). Calls `remote.*` / `devices.*` (ADR-0030);
 * while the core answers `unimplementedFeature` a notice is shown instead.
 */
import type { DeviceInfo, DevicePairingStart, RemoteStatus, Role } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import QRCode from "qrcode";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { formatDate } from "../../i18n/format";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import { isUnimplemented, PAIRING_QR_OPTIONS, pairingDeepLink } from "./logic";

const ROLES: Role[] = ["viewer", "operator", "admin"];
type Notice = { tone: "ok" | "error"; text: string } | null;
/**
 * The pairing start result; the SAS (and the joining device's name) appear
 * once the other device has claimed the code — the core lists it under
 * `devices.list.pending`, which the panel polls while a pairing is open.
 */
type Pairing = DevicePairingStart & { sas?: string; deviceName?: string };
const PAIRING_POLL_MS = 1500;

/**
 * QR of the pairing deep link (what the phone scans) next to the plain pairing
 * text (what the web client accepts) and a copy button.
 */
function PairingQr({ text }: { text: string }) {
  const { t } = useT("remote");
  const [src, setSrc] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    let live = true;
    setSrc(null);
    QRCode.toString(pairingDeepLink(text), PAIRING_QR_OPTIONS)
      .then((svg) => {
        if (live) setSrc(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`);
      })
      .catch(() => {
        // too long for a QR: the text + copy button still work
      });
    return () => {
      live = false;
    };
  }, [text]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="rs-qr-block">
      {src && <img className="rs-qr-img" src={src} alt={t("pairing.qrAlt")} data-testid="pairing-qr" />}
      <div className="rs-qr-text">
        <p className="rs-muted">{t("pairing.qrLabel")}</p>
        <p className="rs-muted rs-qr" title={t("pairing.qrHint")}>
          <code>{text}</code>
        </p>
        <div className="rs-actions">
          <Button size="sm" onClick={() => void copy()}>
            {t("pairing.copy")}
          </Button>
          {copied && (
            <span className="rs-ok" role="status">
              {t("pairing.copied")}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

export function RemotePanel() {
  const { t } = useT("remote");
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const [gatewayUrl, setGatewayUrl] = useState("");
  const [enrollmentToken, setEnrollmentToken] = useState("");
  const [pairing, setPairing] = useState<Pairing | null>(null);
  const [sasOk, setSasOk] = useState(false);
  const [role, setRole] = useState<Role>("operator");
  const [terminal, setTerminal] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const c = getClient();
      setStatus(await c.call("remote.status", {}));
      setDevices((await c.call("devices.list", {})).devices);
    } catch (e) {
      if (isUnimplemented(e)) setUnavailable(true);
      else setNotice({ tone: "error", text: describeError(e) });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const pairingId = pairing?.pairingId ?? null;
  const pairingClaimed = Boolean(pairing?.sas);
  useEffect(() => {
    if (!pairingId || pairingClaimed) return;
    let stopped = false;
    const poll = async () => {
      try {
        const list = await getClient().call("devices.list", {});
        const p = list.pending.find((x) => x.pairingId === pairingId);
        if (!stopped && p) {
          setPairing((cur) => (cur && cur.pairingId === pairingId ? { ...cur, sas: p.sas, deviceName: p.deviceName } : cur));
        }
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
  }, [pairingId, pairingClaimed]);

  const run = async (key: string, fn: () => Promise<string | null>) => {
    setBusy(key);
    setNotice(null);
    try {
      const text = await fn();
      if (text) setNotice({ tone: "ok", text });
    } catch (e) {
      if (isUnimplemented(e)) setUnavailable(true);
      else setNotice({ tone: "error", text: describeError(e) });
    } finally {
      setBusy(null);
      if (key !== "pair") await refresh();
    }
  };

  if (unavailable) {
    return (
      <SettingGroup title={t("group")}>
        <p className="rs-muted">{t("intro")}</p>
        <p className="rs-muted" role="status">
          {t("unavailable")}
        </p>
      </SettingGroup>
    );
  }

  const enable = () =>
    run("enable", async () => {
      setStatus(
        await getClient().call("remote.enable", { gatewayUrl: gatewayUrl.trim(), enrollmentToken: enrollmentToken || undefined }),
      );
      setEnrollmentToken("");
      return null;
    });
  const disable = () =>
    run("disable", async () => {
      setStatus(await getClient().call("remote.disable", {}));
      return null;
    });
  const killSwitch = () =>
    run("kill", async () => {
      const c = getClient();
      const r = await c.call("remote.revokeAllSessions", {});
      setStatus(await c.call("remote.disable", {}));
      return t("killed", { count: r.revoked });
    });
  const startPairing = () =>
    run("pair", async () => {
      setSasOk(false);
      setPairing((await getClient().call("devices.startPairing", {})) as Pairing);
      return null;
    });
  const approve = () =>
    run("approve", async () => {
      if (!pairing) return null;
      const d = await getClient().call("devices.approve", {
        pairingId: pairing.pairingId,
        sasConfirmed: sasOk,
        role,
        remoteTerminal: terminal,
      });
      setPairing(null);
      return t("pairing.approved", { name: d.name });
    });
  const revoke = (deviceId: string) =>
    run(`revoke:${deviceId}`, async () => {
      await getClient().call("devices.revoke", { deviceId });
      return null;
    });

  const enabled = status?.enabled ?? false;
  return (
    <div className="rs-panel">
      <SettingGroup title={t("group")}>
        <p className="rs-muted">{t("intro")}</p>
        {status && (
          <SettingRow title={enabled ? t("enabled") : t("disabled")}>
            <span className="rs-status">
              <Badge tone={status.connected ? "success" : "neutral"}>{status.connected ? t("connected") : t("disconnected")}</Badge>
              <span className="rs-muted">{t("sessions", { count: status.sessions })}</span>
            </span>
          </SettingRow>
        )}
        {!enabled && (
          <div className="rs-form">
            <Input label={t("gatewayUrl")} value={gatewayUrl} onChange={(e) => setGatewayUrl(e.target.value)} />
            <Input
              label={t("enrollmentToken")}
              type="password"
              value={enrollmentToken}
              onChange={(e) => setEnrollmentToken(e.target.value)}
            />
            <div className="rs-actions">
              <Button variant="primary" loading={busy === "enable"} disabled={busy !== null || !gatewayUrl.trim()} onClick={() => void enable()}>
                {t("enable")}
              </Button>
            </div>
          </div>
        )}
        {enabled && (
          <div className="rs-actions">
            <Button loading={busy === "disable"} disabled={busy !== null} onClick={() => void disable()}>
              {t("disable")}
            </Button>
            <Button variant="danger" loading={busy === "kill"} disabled={busy !== null} onClick={() => void killSwitch()} title={t("killSwitchHint")}>
              {t("killSwitch")}
            </Button>
          </div>
        )}
        {notice && (
          <p className={notice.tone === "ok" ? "rs-ok" : "rs-error"} role={notice.tone === "ok" ? "status" : "alert"}>
            {notice.text}
          </p>
        )}
      </SettingGroup>
      {enabled && (
        <SettingGroup title={t("pairing.group")}>
          <div className="rs-actions">
            <Button loading={busy === "pair"} disabled={busy !== null} onClick={() => void startPairing()}>
              {t("pairing.start")}
            </Button>
          </div>
          {pairing && (
            <div className="rs-pairing">
              <p className="rs-code">{t("pairing.code", { code: pairing.code })}</p>
              <p className="rs-muted">{t("pairing.expires", { time: formatDate(pairing.expiresAt) })}</p>
              {pairing.qr && <PairingQr text={pairing.qr} />}
              {pairing.sas ? (
                <>
                  {pairing.deviceName && <p>{t("pairing.joined", { name: pairing.deviceName })}</p>}
                  <p className="rs-code">{t("pairing.sas", { sas: pairing.sas })}</p>
                  <p className="rs-muted">{t("pairing.sasHint")}</p>
                </>
              ) : (
                <p className="rs-muted" role="status">
                  {t("pairing.waiting")}
                </p>
              )}
              <SettingRow title={t("pairing.sasConfirmed")}>
                <Switch checked={sasOk} label={t("pairing.sasConfirmed")} onChange={setSasOk} />
              </SettingRow>
              <SettingRow title={t("pairing.role")}>
                <Segmented<Role>
                  label={t("pairing.role")}
                  value={role}
                  options={ROLES.map((r) => ({ value: r, label: t(`pairing.roles.${r}`) }))}
                  onChange={setRole}
                />
              </SettingRow>
              <SettingRow title={t("pairing.remoteTerminal")}>
                <Switch checked={terminal} label={t("pairing.remoteTerminal")} onChange={setTerminal} />
              </SettingRow>
              <div className="rs-actions">
                <Button variant="primary" loading={busy === "approve"} disabled={busy !== null || !sasOk || !pairing.sas} onClick={() => void approve()}>
                  {t("pairing.approve")}
                </Button>
              </div>
            </div>
          )}
        </SettingGroup>
      )}
      <SettingGroup title={t("devices.group")}>
        {devices.length === 0 && <p className="rs-muted">{t("devices.none")}</p>}
        <ul className="rs-list">
          {devices.map((d) => (
            <li key={d.deviceId} className="rs-list__item">
              <div className="rs-list__main">
                <strong>{d.name}</strong>
                <Badge tone="info">{t(`pairing.roles.${d.role}`)}</Badge>
                {d.lastSeenAt && <span className="rs-muted">{t("devices.lastSeen", { time: formatDate(d.lastSeenAt) })}</span>}
                {d.revoked && <Badge tone="danger">{t("devices.revoked")}</Badge>}
              </div>
              {!d.revoked && (
                <Button size="sm" variant="danger" disabled={busy !== null} onClick={() => void revoke(d.deviceId)}>
                  {t("devices.revoke")}
                </Button>
              )}
            </li>
          ))}
        </ul>
      </SettingGroup>
    </div>
  );
}
