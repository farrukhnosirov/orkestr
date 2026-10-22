import type { DeviceRecord } from "@orkestr/client";
import { Button } from "@orkestr/ui";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDeps } from "../deps";
import { currentSubscription, disablePush, enablePush, pushSupported } from "../push";
import { type RemoteClient, errorCode } from "../remote";

/** Device settings: Web Push toggle, step-up key, forget device. */
export function SettingsPanel({
  client,
  device,
  onForget,
}: {
  client: RemoteClient;
  device: DeviceRecord;
  onForget: () => void;
}) {
  const { t } = useTranslation();
  const { gateway, registerStepUp } = useDeps();
  const [push, setPush] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const supported = pushSupported();

  useEffect(() => {
    void currentSubscription().then((s) => setPush(s !== null));
  }, []);

  const act = async (fn: () => Promise<string>) => {
    setBusy(true);
    setMsg(null);
    try {
      setMsg(await fn());
    } catch (e) {
      const c = e instanceof Error && !("code" in e) ? e.message : errorCode(e);
      setMsg(t(`settings.error.${c}`, { defaultValue: t(`errors.${c}`, { defaultValue: t("errors.internal") }) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-label={t("tabs.settings")} className="web-stack">
      <p className="web-muted">
        {t("settings.device", { name: device.deviceName, role: device.role ?? "?" })}
      </p>
      <label className="web-switch">
        <input
          type="checkbox"
          role="switch"
          checked={push}
          disabled={!supported || busy}
          onChange={(e) => {
            const on = e.target.checked;
            void act(async () => {
              if (on) await enablePush(client, gateway);
              else await disablePush(client);
              setPush(on);
              return on ? t("settings.pushOn") : t("settings.pushOff");
            });
          }}
        />
        <span>{supported ? t("settings.push") : t("settings.pushUnsupported")}</span>
      </label>
      {push && (
        <Button size="sm" disabled={busy} onClick={() => void act(async () => {
          const r = await client.call("push.test", {});
          return t("settings.testSent", { count: r.sent });
        })}>
          {t("settings.pushTest")}
        </Button>
      )}
      <Button size="sm" disabled={busy} onClick={() => void act(async () => {
        const credential = await registerStepUp(device);
        await client.call("devices.registerStepUp", { credential } as never);
        return t("settings.stepUpDone");
      })}>
        {t("settings.stepUp")}
      </Button>
      <Button size="sm" variant="danger" onClick={onForget}>
        {t("settings.forget")}
      </Button>
      {msg && <p role="status">{msg}</p>}
    </section>
  );
}
