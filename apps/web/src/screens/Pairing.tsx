import { type DeviceRecord, PairingError, type PairingSession, parsePairingQr } from "@orkestr/client";
import { Button, Input, Panel } from "@orkestr/ui";
import { type FormEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDeps } from "../deps";

function platformName(): string {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/android/i.test(ua)) return "android";
  if (/iphone|ipad/i.test(ua)) return "ios";
  return "web";
}

/** Pairing: paste the desktop's pairing payload, compare SAS, wait for approval. */
export function Pairing({ onPaired }: { onPaired: (rec: DeviceRecord) => void }) {
  const { t } = useTranslation();
  const { pair, store, gateway } = useDeps();
  const [enrollToken, setEnrollToken] = useState<string | null>(null);
  const [payload, setPayload] = useState("");
  const [name, setName] = useState(() => (platformName() === "web" ? "Browser" : platformName()));
  const [session, setSession] = useState<PairingSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => () => abort.current?.abort(), []);

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    const offer = parsePairingQr(payload);
    if (!offer) {
      setError(t("pairing.badPayload"));
      return;
    }
    setBusy(true);
    try {
      const s = await pair({ ...offer, deviceName: name.trim() || "Browser", platform: platformName(), store });
      setSession(s);
      abort.current = new AbortController();
      onPaired(await s.waitForApproval({ signal: abort.current.signal }));
    } catch (err) {
      const code = err instanceof PairingError ? err.code : "claim_failed";
      if (code !== "aborted") setError(t(`pairing.error.${code}`));
      setSession(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="web-center">
      <Panel title={t("pairing.title")} className="web-card">
        {session ? (
          <div className="web-stack" aria-live="polite">
            <p>{t("pairing.compare")}</p>
            <output className="web-sas" aria-label={t("pairing.sas")}>
              {`${session.sas.slice(0, 3)} ${session.sas.slice(3)}`}
            </output>
            <p className="web-muted">{t("pairing.waiting")}</p>
          </div>
        ) : (
          <form className="web-stack" onSubmit={(e) => void onSubmit(e)}>
            <p>{t("pairing.intro")}</p>
            <label className="web-field">
              <span>{t("pairing.payload")}</span>
              <textarea rows={3} value={payload} onChange={(e) => setPayload(e.target.value)} />
            </label>
            <Input label={t("pairing.deviceName")} value={name} onChange={(e) => setName(e.target.value)} />
            <Button type="submit" variant="primary" loading={busy}>
              {t("pairing.submit")}
            </Button>
          </form>
        )}
        {!session && (
          <section className="web-stack" aria-label={t("pairing.enroll.title")}>
            <h3>{t("pairing.enroll.title")}</h3>
            <p className="web-muted">{t("pairing.enroll.intro")}</p>
            <Button
              onClick={() =>
                void gateway.enrollmentToken().then(
                  (r) => setEnrollToken(r.token),
                  () => setError(t("pairing.enroll.failed")),
                )
              }
            >
              {t("pairing.enroll.create")}
            </Button>
            {enrollToken && (
              <>
                <output className="web-token" aria-label={t("pairing.enroll.token")}>
                  {enrollToken}
                </output>
                <p className="web-muted">{t("pairing.enroll.hint")}</p>
              </>
            )}
          </section>
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
