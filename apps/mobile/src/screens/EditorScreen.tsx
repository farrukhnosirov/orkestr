import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TextInput, View } from "react-native";

import { readFile, saveFile } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useClient } from "../state/deps";
import { Body, Button, Card, ErrorText, Field, Muted, Screen, s } from "../ui/components";
import { ProjectPicker } from "./ProjectPicker";

/** Light editing: `files.read` → textarea → `files.write` with the hash guard. */
export default function EditorScreen({ route }: ScreenProps<"Editor">) {
  const { t } = useTranslation();
  const { coreId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const [projectId, setProjectId] = useState<string | null>(route.params.projectId ?? null);
  const [path, setPath] = useState(route.params.path ?? "");
  const [content, setContent] = useState<string | null>(null);
  const [hash, setHash] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const open = async () => {
    if (!client || !projectId || !path.trim()) return;
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      const f = await readFile(client, projectId, path.trim());
      if (f.binary || f.content === null) {
        setError("binary_file");
        setContent(null);
      } else {
        setContent(f.content);
        setHash(f.hash);
        setDirty(false);
        if (f.readonly) setStatus(t("editor.readonly"));
      }
    } catch (e) {
      setError(errorCode(e));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!client || !projectId || content === null || !hash) return;
    setBusy(true);
    setError(null);
    try {
      const r = await saveFile(client, projectId, path.trim(), content, hash);
      if (r.status === "saved") {
        setHash(r.hash);
        setDirty(false);
        setStatus(t("editor.saved"));
      } else {
        setError("conflict");
      }
    } catch (e) {
      setError(errorCode(e));
    } finally {
      setBusy(false);
    }
  };

  const e = connError ?? error;
  return (
    <Screen testID="screen-editor">
      <ProjectPicker client={client} value={projectId} onChange={setProjectId} />
      <Field label={t("editor.path")} value={path} onChangeText={setPath} placeholder="src/main.rs" testID="editor-path" />
      <Button label={t("editor.open")} onPress={() => void open()} busy={busy && content === null} disabled={!client || !projectId || !path.trim()} variant="secondary" testID="editor-open" />
      <ErrorText testID="editor-error">{e ? t(`errors.${e}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      {e === "conflict" ? <Button label={t("editor.reload")} onPress={() => void open()} variant="ghost" testID="editor-reload" /> : null}
      {status ? <Body testID="editor-status">{status}</Body> : null}
      {content !== null ? (
        <Card>
          <TextInput
            testID="editor-content"
            value={content}
            onChangeText={(v) => {
              setContent(v);
              setDirty(true);
            }}
            multiline
            autoCapitalize="none"
            autoCorrect={false}
            textAlignVertical="top"
            style={[s.input, s.mono, { minHeight: 320 }]}
          />
          <View style={{ height: 12 }} />
          <Button label={t("editor.save")} onPress={() => void save()} busy={busy} disabled={!dirty} testID="editor-save" />
          <Muted>{t("editor.hashGuard")}</Muted>
        </Card>
      ) : null}
    </Screen>
  );
}
