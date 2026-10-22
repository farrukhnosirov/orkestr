import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Text, View } from "react-native";

import { loadDiff, loadTasks, loadTranscript } from "../core";
import type { ScreenProps } from "../navigation/types";
import { useAsync, useClient } from "../state/deps";
import { Badge, Button, Card, ErrorText, ListItem, Muted, Screen, s } from "../ui/components";
import { colors } from "../ui/theme";
import { ProjectPicker } from "./ProjectPicker";

type Tab = "tasks" | "diff" | "logs";

function lineColor(l: string) {
  if (l.startsWith("+")) return colors.success;
  if (l.startsWith("-")) return colors.danger;
  if (l.startsWith("@@")) return colors.violet;
  return colors.muted;
}

export default function ReviewScreen({ navigation, route }: ScreenProps<"Review">) {
  const { t } = useTranslation();
  const { coreId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("tasks");
  const [agentId, setAgentId] = useState<string | null>(null);

  const tasks = useAsync(client && projectId && tab === "tasks" ? () => loadTasks(client, projectId) : null, [client, projectId, tab]);
  const diff = useAsync(client && projectId && tab === "diff" ? () => loadDiff(client, projectId) : null, [client, projectId, tab]);
  const agents = useAsync(
    client && projectId && tab === "logs" ? async () => (await client.call("agents.list", { projectId })).agents : null,
    [client, projectId, tab],
  );
  const transcript = useAsync(client && agentId ? () => loadTranscript(client, agentId) : null, [client, agentId]);

  const err = connError ?? tasks.error ?? diff.error ?? agents.error ?? transcript.error;
  return (
    <Screen testID="screen-review">
      <ProjectPicker client={client} value={projectId} onChange={setProjectId} />
      <View style={{ flexDirection: "row", gap: 8, marginBottom: 12 }}>
        {(["tasks", "diff", "logs"] as const).map((k) => (
          <View key={k} style={{ flex: 1 }}>
            <Button label={t(`review.${k}`)} variant={tab === k ? "primary" : "secondary"} onPress={() => setTab(k)} testID={`review-tab-${k}`} />
          </View>
        ))}
      </View>
      <ErrorText>{err ? t(`errors.${err}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>

      {tab === "tasks" ? (
        <Card>
          {tasks.data?.length === 0 ? <Muted>{t("review.noTasks")}</Muted> : null}
          {tasks.data?.map((task) => (
            <ListItem key={task.id} testID={`task-${task.id}`} title={task.title} subtitle={task.description} right={<Badge label={task.status} tone={task.status === "Failed" ? "danger" : task.status === "Completed" ? "ok" : "accent"} />} />
          ))}
        </Card>
      ) : null}

      {tab === "diff" && diff.data ? (
        <>
          <Muted>{t("review.diffSummary", { files: diff.data.files.length, add: diff.data.additions, del: diff.data.deletions })}</Muted>
          {diff.data.files.map((f, i) => (
            <Card key={`${f.path}-${i}`} title={f.path}>
              <Text style={s.mono}>
                {(f.patch ?? "")
                  .split("\n")
                  .slice(0, 400)
                  .map((l, j) => (
                    <Text key={j} style={{ color: lineColor(l) }}>
                      {l + "\n"}
                    </Text>
                  ))}
              </Text>
              <Button label={t("review.openInEditor")} variant="ghost" onPress={() => projectId && navigation.navigate("Editor", { coreId, projectId, path: f.path })} testID={`diff-edit-${i}`} />
            </Card>
          ))}
        </>
      ) : null}

      {tab === "logs" ? (
        <Card>
          {agents.data?.length === 0 ? <Muted>{t("home.noAgents")}</Muted> : null}
          {agents.data?.map((a) => (
            <ListItem key={a.id} testID={`log-agent-${a.id}`} title={a.name} subtitle={a.state} onPress={() => setAgentId(a.id)} />
          ))}
          {transcript.data
            ? transcript.data.messages.slice(-80).map((m, i) => (
                <Text key={i} style={[s.mono, { marginTop: 6 }]}>
                  <Text style={{ color: colors.accent }}>{m.role}: </Text>
                  {m.text}
                </Text>
              ))
            : null}
        </Card>
      ) : null}
    </Screen>
  );
}
