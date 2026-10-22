import { useState } from "react";
import { useTranslation } from "react-i18next";
import { View } from "react-native";

import { useDeps, errorCode } from "../state/deps";
import type { ScreenProps } from "../navigation/types";
import { Button, Card, ErrorText, Field, H1, Muted, Screen } from "../ui/components";

export default function LoginScreen({ navigation }: ScreenProps<"Login">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const [url, setUrl] = useState(core.session?.url ?? "http://10.0.2.2:8787");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await core.login(url, code);
      navigation.reset({ index: 0, routes: [{ name: "Cores" }] });
    } catch (e) {
      setError(errorCode(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen testID="screen-login" top>
      <H1>{t("login.title")}</H1>
      <Muted>{t("login.subtitle")}</Muted>
      <View style={{ height: 16 }} />
      <Card>
        <Field label={t("login.gatewayUrl")} value={url} onChangeText={setUrl} keyboardType="url" testID="login-gateway-url" />
        <Field
          label={t("login.totp")}
          value={code}
          onChangeText={setCode}
          keyboardType="number-pad"
          maxLength={8}
          placeholder="123456"
          testID="login-totp"
        />
        <ErrorText testID="login-error">{error ? t(`errors.${error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
        <Button label={t("login.signIn")} onPress={submit} busy={busy} disabled={!url || code.trim().length < 6} testID="login-submit" />
      </Card>
      <Card>
        <Button label={t("login.passkey")} onPress={() => undefined} disabled variant="secondary" testID="login-passkey" />
        <View style={{ height: 8 }} />
        <Muted testID="login-passkey-note">{t("login.passkeyUnimplemented")}</Muted>
      </Card>
    </Screen>
  );
}
