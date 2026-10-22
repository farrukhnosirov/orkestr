import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { View } from "react-native";

import { PUSH_CATEGORIES, type PushPrefs, registerStepUpAfterPairing } from "../core";
import { setLanguage } from "../i18n";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useClient, useDeps } from "../state/deps";
import { Body, Button, Card, ConfirmDialog, ErrorText, H1, Muted, Screen, Toggle } from "../ui/components";

export default function SettingsScreen({ navigation, route }: ScreenProps<"Settings">) {
  const { t, i18n } = useTranslation();
  const { core, pushConfigured } = useDeps();
  const coreId = route.params?.coreId;
  const { client } = useClient(coreId);
  const [prefs, setPrefs] = useState<PushPrefs | null>(null);
  const [pushStatus, setPushStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmUnpair, setConfirmUnpair] = useState(false);
  const [hasStepUp, setHasStepUp] = useState<boolean | null>(null);

  useEffect(() => {
    void core.push.prefs().then(setPrefs);
    if (coreId) void core.stepUpKeys.meta(coreId).then((m) => setHasStepUp(m !== null));
  }, [core, coreId]);

  const update = async (p: PushPrefs) => {
    setPrefs(p);
    await core.push.setPrefs(p);
  };

  const registerPush = async () => {
    if (!client || !coreId) return;
    setError(null);
    try {
      const r = await core.push.registerNative(client, coreId);
      setPushStatus(r.status === "registered" ? t(r.deliverable ? "settings.pushRegistered" : "settings.pushNotDeliverable") : t(r.reason === "permission" ? "settings.pushPermission" : "settings.pushUnavailable"));
    } catch (e) {
      setError(errorCode(e));
    }
  };

  const setupStepUp = async () => {
    if (!client || !coreId) return;
    const rec = await core.store.get(coreId);
    if (!rec) return;
    const r = await registerStepUpAfterPairing(client, rec, core.stepUpKeys);
    setHasStepUp(r.registered);
    if (!r.registered) setError(r.error ?? "internal");
  };

  return (
    <Screen testID="screen-settings">
      <H1>{t("settings.title")}</H1>
      <ErrorText testID="settings-error">{error ? t(`errors.${error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>

      <Card title={t("settings.language")}>
        <View style={{ flexDirection: "row", gap: 8 }}>
          {(["en", "ru"] as const).map((l) => (
            <View key={l} style={{ flex: 1 }}>
              <Button label={t(`settings.lang.${l}`)} variant={i18n.language === l ? "primary" : "secondary"} onPress={() => void setLanguage(l, core.o.kv)} testID={`settings-lang-${l}`} />
            </View>
          ))}
        </View>
      </Card>

      <Card title={t("settings.notifications")}>
        {!pushConfigured ? <Muted testID="settings-push-unavailable">{t("settings.pushUnavailable")}</Muted> : null}
        {prefs
          ? PUSH_CATEGORIES.map((c) => (
              <Toggle
                key={c}
                label={t(`settings.category.${c}`)}
                value={prefs.categories.includes(c)}
                onChange={(on) => void update({ ...prefs, categories: on ? [...prefs.categories, c] : prefs.categories.filter((x) => x !== c) })}
                testID={`settings-cat-${c}`}
              />
            ))
          : null}
        {prefs ? (
          <Toggle
            label={t("settings.lockScreenDetails")}
            hint={t("settings.lockScreenHint")}
            value={prefs.lockScreen === "detailed"}
            onChange={(on) => void update({ ...prefs, lockScreen: on ? "detailed" : "generic" })}
            testID="settings-lockscreen"
          />
        ) : null}
        {coreId ? <Button label={t("settings.registerPush")} variant="secondary" onPress={() => void registerPush()} disabled={!client} testID="settings-register-push" /> : null}
        {pushStatus ? <Body testID="settings-push-status">{pushStatus}</Body> : null}
      </Card>

      {coreId ? (
        <Card title={t("settings.security")}>
          <Body>{hasStepUp ? t("settings.stepUpOn") : t("pairing.stepUpMissing")}</Body>
          {hasStepUp === false ? <Button label={t("settings.setupStepUp")} onPress={() => void setupStepUp()} disabled={!client} testID="settings-setup-stepup" /> : null}
          <View style={{ height: 8 }} />
          <Button label={t("settings.unpair")} variant="danger" onPress={() => setConfirmUnpair(true)} testID="settings-unpair" />
        </Card>
      ) : null}

      <Button
        label={t("settings.logout")}
        variant="ghost"
        onPress={() =>
          void core.logout().then(() => navigation.reset({ index: 0, routes: [{ name: "Login" }] }))
        }
        testID="settings-logout"
      />

      <ConfirmDialog
        visible={confirmUnpair}
        testID="unpair-confirm"
        danger
        title={t("settings.unpair")}
        message={t("settings.unpairConfirm")}
        confirmLabel={t("settings.unpair")}
        cancelLabel={t("common.cancel")}
        onCancel={() => setConfirmUnpair(false)}
        onConfirm={() => {
          setConfirmUnpair(false);
          if (coreId) void core.unpair(coreId).then(() => navigation.reset({ index: 0, routes: [{ name: "Cores" }] }));
        }}
      />
    </Screen>
  );
}
