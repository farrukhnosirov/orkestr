import type { PairingOffer, PairingSession } from "@orkestr/client";
import { CameraView, useCameraPermissions } from "expo-camera";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Text, View } from "react-native";

import { parsePairingInput } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useDeps } from "../state/deps";
import { Body, Button, Card, ErrorText, Field, H1, Muted, Screen } from "../ui/components";
import { colors } from "../ui/theme";

type Phase = { k: "input" } | { k: "claiming" } | { k: "sas"; session: PairingSession; offer: PairingOffer } | { k: "waiting"; sas: string } | { k: "done"; stepUp: boolean };

function Scanner({ onData }: { onData: (s: string) => void }) {
  const { t } = useTranslation();
  const [perm, request] = useCameraPermissions();
  if (!perm?.granted) {
    return <Button label={t("pairing.allowCamera")} onPress={() => void request()} variant="secondary" testID="pairing-allow-camera" />;
  }
  return (
    <View style={{ height: 280, borderRadius: 12, overflow: "hidden", marginBottom: 12 }}>
      <CameraView style={{ flex: 1 }} facing="back" barcodeScannerSettings={{ barcodeTypes: ["qr"] }} onBarcodeScanned={(r) => onData(r.data)} testID="pairing-camera" />
    </View>
  );
}

export default function PairingScreen({ navigation, route }: ScreenProps<"Pairing">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const [text, setText] = useState(route.params?.offer ?? "");
  const [scan, setScan] = useState(false);
  const [phase, setPhase] = useState<Phase>({ k: "input" });
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<string | null>(null);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => () => abort.current?.abort(), []);

  const claim = async (input: string) => {
    const offer = parsePairingInput(input);
    if (!offer) {
      setError("invalid_offer");
      return;
    }
    setScan(false);
    setError(null);
    setPhase({ k: "claiming" });
    try {
      const session = await core.pair(offer);
      setPhase({ k: "sas", session, offer });
    } catch (e) {
      if (__DEV__) console.warn("pairing claim failed", e);
      setError(errorCode(e));
      setDetail(e instanceof Error ? e.message : String(e));
      setPhase({ k: "input" });
    }
  };

  // Deep link `orkestr://pair?offer=…` opens this screen with the offer filled in.
  useEffect(() => {
    if (route.params?.offer) {
      setText(route.params.offer);
      void claim(route.params.offer);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [route.params?.offer]);

  const confirmSas = async (session: PairingSession) => {
    abort.current = new AbortController();
    setPhase({ k: "waiting", sas: session.sas });
    try {
      const r = await core.completePairing(session, { signal: abort.current.signal });
      setPhase({ k: "done", stepUp: r.stepUpRegistered });
      navigation.reset({ index: 1, routes: [{ name: "Cores" }, { name: "Home", params: { coreId: r.record.coreId } }] });
    } catch (e) {
      setError(errorCode(e));
      setPhase({ k: "input" });
    }
  };

  return (
    <Screen testID="screen-pairing">
      <H1>{t("pairing.title")}</H1>
      <ErrorText testID="pairing-error">{error ? t(`errors.${error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      {error && detail ? <Muted testID="pairing-error-detail">{detail}</Muted> : null}

      {phase.k === "input" || phase.k === "claiming" ? (
        <Card>
          <Muted>{t("pairing.instructions")}</Muted>
          <View style={{ height: 12 }} />
          {scan ? <Scanner onData={(d) => void claim(d)} /> : null}
          <Button
            label={scan ? t("pairing.stopScan") : t("pairing.scan")}
            onPress={() => setScan((x) => !x)}
            variant="secondary"
            testID="pairing-scan"
          />
          <View style={{ height: 12 }} />
          <Field label={t("pairing.paste")} value={text} onChangeText={setText} multiline numberOfLines={3} testID="pairing-input" />
          <Button label={t("pairing.continue")} onPress={() => void claim(text)} busy={phase.k === "claiming"} disabled={!text.trim()} testID="pairing-submit" />
        </Card>
      ) : null}

      {phase.k === "sas" ? (
        <Card title={t("pairing.sasTitle")}>
          <Text testID="pairing-sas" style={{ color: colors.accent, fontSize: 44, fontWeight: "800", letterSpacing: 8, textAlign: "center", marginVertical: 16 }}>
            {phase.session.sas.slice(0, 3)} {phase.session.sas.slice(3)}
          </Text>
          <Body>{t("pairing.sasHelp")}</Body>
          <View style={{ height: 16 }} />
          <Button label={t("pairing.sasMatch")} onPress={() => void confirmSas(phase.session)} testID="pairing-sas-confirm" />
          <View style={{ height: 8 }} />
          <Button
            label={t("pairing.sasMismatch")}
            variant="danger"
            onPress={() => {
              setError("sas_mismatch");
              setPhase({ k: "input" });
            }}
            testID="pairing-sas-mismatch"
          />
        </Card>
      ) : null}

      {phase.k === "waiting" ? (
        <Card title={t("pairing.waitingTitle")}>
          <Body testID="pairing-waiting">{t("pairing.waiting", { sas: phase.sas })}</Body>
          <View style={{ height: 12 }} />
          <Button
            label={t("common.cancel")}
            variant="ghost"
            onPress={() => {
              abort.current?.abort();
              setPhase({ k: "input" });
            }}
            testID="pairing-cancel"
          />
        </Card>
      ) : null}

      {phase.k === "done" ? (
        <Card>
          <Body testID="pairing-done">{t("pairing.done")}</Body>
          {!phase.stepUp ? <Muted>{t("pairing.stepUpMissing")}</Muted> : null}
        </Card>
      ) : null}
    </Screen>
  );
}
