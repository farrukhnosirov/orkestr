import { useState } from "react";
import { useTranslation } from "react-i18next";

import { BiometricDeniedError, callWithStepUp } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useAsync, useClient, useDeps } from "../state/deps";
import { Badge, Button, Card, ConfirmDialog, ErrorText, H1, ListItem, Muted, Screen } from "../ui/components";

export default function DevicesScreen({ route }: ScreenProps<"Devices">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const { coreId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const self = useAsync(() => core.store.get(coreId), [coreId]);
  const { data, error, reload } = useAsync(client ? () => client.call("devices.list", {}) : null, [client]);
  const [target, setTarget] = useState<{ id: string; name: string } | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const revoke = async (deviceId: string) => {
    setTarget(null);
    if (!client) return;
    setActionError(null);
    try {
      await callWithStepUp(client, "devices.revoke", { deviceId }, core.stepUp(coreId));
      reload();
    } catch (e) {
      setActionError(e instanceof BiometricDeniedError ? "biometric_denied" : errorCode(e));
    }
  };

  const isAdmin = self.data?.role === "admin";
  const e = actionError ?? connError ?? error;
  return (
    <Screen testID="screen-devices">
      <H1>{t("devices.title")}</H1>
      <ErrorText testID="devices-error">{e ? t(`errors.${e}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      {!isAdmin && self.data ? <Muted>{t("devices.adminOnly")}</Muted> : null}
      <Card>
        {data?.devices.map((d) => {
          const me = d.deviceId === self.data?.deviceId;
          return (
            <ListItem
              key={d.deviceId}
              testID={`device-${d.deviceId}`}
              title={`${d.name}${me ? ` (${t("devices.thisDevice")})` : ""}`}
              subtitle={`${t(`roles.${d.role}`, { defaultValue: d.role })} · ${d.platform}${d.stepUpRegistered ? ` · ${t("devices.stepUp")}` : ""}`}
              right={
                d.revoked ? (
                  <Badge label={t("devices.revoked")} tone="danger" />
                ) : isAdmin && !me ? (
                  <Button label={t("devices.revoke")} variant="secondary" onPress={() => setTarget({ id: d.deviceId, name: d.name })} testID={`device-revoke-${d.deviceId}`} />
                ) : undefined
              }
            />
          );
        })}
      </Card>
      <ConfirmDialog
        visible={target !== null}
        testID="revoke-confirm"
        danger
        title={t("devices.revokeTitle")}
        message={t("devices.revokeConfirm", { name: target?.name ?? "" })}
        confirmLabel={t("devices.revoke")}
        cancelLabel={t("common.cancel")}
        onCancel={() => setTarget(null)}
        onConfirm={() => target && void revoke(target.id)}
      />
    </Screen>
  );
}
