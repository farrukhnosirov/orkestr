import type { Intent } from "@orkestr/client";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Text, View } from "react-native";

import { executeTranscript, loadIntentContext, transcribe } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useAsync, useClient, useDeps } from "../state/deps";
import { Button, Card, ConfirmDialog, ErrorText, Field, Muted, Screen } from "../ui/components";
import { colors } from "../ui/theme";
import { ProjectPicker } from "./ProjectPicker";

export default function ManagerScreen({ route }: ScreenProps<"Manager">) {
  const { t, i18n } = useTranslation();
  const { core, speaker, useRecorder } = useDeps();
  const recorder = useRecorder();
  const { coreId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [recording, setRecording] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [voiceReply, setVoiceReply] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ intent: Intent; resolve: (ok: boolean) => void } | null>(null);
  const history = useAsync(client && projectId ? async () => (await client.call("manager.history", { projectId, limit: 50 })).messages : null, [client, projectId]);
  const reloadRef = useRef(history.reload);
  reloadRef.current = history.reload;

  useEffect(() => {
    if (!client || !projectId) return;
    return client.subscribe([`project:${projectId}`], (ev) => {
      if (ev.kind === "ManagerMessageAdded") reloadRef.current();
    });
  }, [client, projectId]);

  const send = async () => {
    if (!client || !projectId || !text.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await client.call("manager.chat", { projectId, text: text.trim() });
      setText("");
      history.reload();
    } catch (e) {
      setError(errorCode(e));
    } finally {
      setBusy(false);
    }
  };

  const reply = (s: string) => {
    setVoiceReply(s);
    if (s) speaker.speak(s, i18n.language);
  };

  /** Voice pipeline: record → voice.transcribe → shared intent parser → RPC. */
  const toggleMic = async () => {
    if (!client) return;
    setError(null);
    if (!recording) {
      try {
        await recorder.start();
        setRecording(true);
      } catch (e) {
        setError(errorCode(e) === "permission" ? "mic_permission" : errorCode(e));
      }
      return;
    }
    setRecording(false);
    setBusy(true);
    try {
      const rec = await recorder.stop();
      const said = await transcribe(client, rec, i18n.language);
      setText(said);
      const ctx = await loadIntentContext(client, projectId);
      const out = await executeTranscript(client, said, ctx, {
        confirm: (intent) => new Promise((resolve) => setConfirm({ intent, resolve })),
        stepUp: core.stepUp(coreId),
      });
      if (out.status === "ok") {
        if (out.intent.kind === "askManager") {
          setText("");
          history.reload();
        } else reply(out.reply);
      } else if (out.status === "failed") setError(out.error);
      else if (out.status === "unsupported" || out.status === "navigate") reply(t("manager.voiceUnsupported"));
      else if (out.status === "cancelled") reply(t("common.cancelled"));
    } catch (e) {
      setError(errorCode(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen testID="screen-manager">
      <ProjectPicker client={client} value={projectId} onChange={setProjectId} />
      <ErrorText testID="manager-error">{(connError ?? error ?? history.error) ? t(`errors.${connError ?? error ?? history.error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      <Card title={t("manager.title")}>
        {history.data?.length === 0 ? <Muted>{t("manager.empty")}</Muted> : null}
        {history.data?.map((m) => (
          <View key={m.id} style={{ alignSelf: m.role === "User" ? "flex-end" : "flex-start", maxWidth: "88%", marginVertical: 4 }}>
            <Text
              style={{
                color: m.role === "User" ? colors.inverse : colors.text,
                backgroundColor: m.role === "User" ? colors.accent : colors.surface2,
                padding: 10,
                borderRadius: 12,
                overflow: "hidden",
              }}
            >
              {m.text}
            </Text>
          </View>
        ))}
      </Card>
      {voiceReply ? (
        <Card>
          <Text style={{ color: colors.violet }} testID="manager-voice-reply">
            {voiceReply}
          </Text>
        </Card>
      ) : null}
      <Field label={t("manager.input")} value={text} onChangeText={setText} multiline testID="manager-input" placeholder={t("manager.placeholder")} />
      <View style={{ flexDirection: "row", gap: 8 }}>
        <View style={{ flex: 1 }}>
          <Button label={t("manager.send")} onPress={() => void send()} busy={busy && !recording} disabled={!text.trim() || !projectId} testID="manager-send" />
        </View>
        <View style={{ flex: 1 }}>
          <Button
            label={recording ? t("manager.stopRecording") : t("manager.mic")}
            variant={recording ? "danger" : "secondary"}
            onPress={() => void toggleMic()}
            disabled={!client || (busy && !recording)}
            testID="manager-mic"
          />
        </View>
      </View>
      <Muted>{t("manager.typeInstead")}</Muted>
      <ConfirmDialog
        visible={confirm !== null}
        testID="voice-confirm"
        danger
        title={t("manager.confirmTitle")}
        message={confirm ? t(`manager.intent.${confirm.intent.kind}`, { defaultValue: confirm.intent.kind }) : ""}
        confirmLabel={t("common.confirm")}
        cancelLabel={t("common.cancel")}
        onConfirm={() => {
          confirm?.resolve(true);
          setConfirm(null);
        }}
        onCancel={() => {
          confirm?.resolve(false);
          setConfirm(null);
        }}
      />
    </Screen>
  );
}
