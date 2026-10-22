import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Text, View } from "react-native";

import { type MonitoringData, type RemoteClient, disableRemote, emergencyStop, loadMonitoring } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useAsync, useClient, useDeps } from "../state/deps";
import { Badge, Body, Button, Card, ConfirmDialog, ErrorText, ListItem, Muted, Screen, s } from "../ui/components";
import { colors } from "../ui/theme";

/** Clients already feeding local notifications (one subscription per connection). */
const watched = new WeakSet<object>();

const gb = (b: number) => (b / 1024 ** 3).toFixed(1);

type Dialog = null | { kind: "stopAll" } | { kind: "killSwitch" } | { kind: "stopAgent"; agentId: string; name: string };

export function MonitoringView({ data, onStopAgent }: { data: MonitoringData; onStopAgent: (id: string, name: string) => void }) {
  const { t } = useTranslation();
  const err = (e: string) => t(`errors.${e}`, { defaultValue: t("errors.internal") });
  const d = data.dashboard;
  return (
    <>
      {data.emergency.ok && data.emergency.value.flags.allStopped ? (
        <Card testID="emergency-engaged">
          <Badge label={t("home.stopped")} tone="danger" />
        </Card>
      ) : null}
      <Card title={t("home.overview")} testID="monitoring-overview">
        {d.ok ? (
          <View style={s.grid}>
            {(
              [
                ["activeAgents", d.value.totals.activeAgents],
                ["runningTasks", d.value.totals.runningTasks],
                ["pendingApprovals", d.value.totals.pendingApprovals],
                ["failedTasks", d.value.totals.failedTasks],
              ] as const
            ).map(([k, v]) => (
              <View key={k} style={{ minWidth: "45%" }}>
                <Text style={s.stat} testID={`stat-${k}`}>
                  {v}
                </Text>
                <Muted>{t(`home.${k}`)}</Muted>
              </View>
            ))}
          </View>
        ) : (
          <ErrorText>{err(d.error)}</ErrorText>
        )}
      </Card>
      <Card title={t("home.node")} testID="monitoring-node">
        {data.resources.ok ? (
          <Body>
            {t("home.cpu", { value: data.resources.value.sample.cpuPercent.toFixed(0) })} ·{" "}
            {t("home.memory", { used: gb(data.resources.value.sample.memoryUsedBytes), total: gb(data.resources.value.sample.memoryTotalBytes) })}
          </Body>
        ) : (
          <ErrorText>{err(data.resources.error)}</ErrorText>
        )}
        {data.nodes.ok ? <Muted>{data.nodes.value.map((n) => n.name).join(", ")}</Muted> : null}
      </Card>
      <Card title={t("home.agents")} testID="monitoring-agents">
        {data.agents.ok ? (
          data.agents.value.length === 0 ? (
            <Muted>{t("home.noAgents")}</Muted>
          ) : (
            data.agents.value.map((a) => (
              <ListItem
                key={a.id}
                testID={`agent-${a.id}`}
                title={a.name}
                subtitle={`${a.state}${a.activity ? ` · ${a.activity}` : ""}`}
                right={<Button label={t("home.stopAgent")} variant="secondary" onPress={() => onStopAgent(a.id, a.name)} testID={`agent-stop-${a.id}`} />}
              />
            ))
          )
        ) : (
          <ErrorText>{err(data.agents.error)}</ErrorText>
        )}
      </Card>
    </>
  );
}

export default function HomeScreen({ navigation, route }: ScreenProps<"Home">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const { coreId } = route.params;
  const { client, error: connError, retry } = useClient(coreId);
  const { data, error, loading, reload } = useAsync(client ? () => loadMonitoring(client) : null, [client]);
  const stepUp = useAsync(() => core.stepUpKeys.meta(coreId), [coreId]);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    if (!client || watched.has(client)) return;
    watched.add(client);
    core.push.attach(client, coreId);
    // Refresh the dashboard when approvals/agents change.
    const off = client.subscribe(["*"], (ev) => {
      if (ev.kind === "ApprovalRequested" || ev.kind === "ApprovalResolved" || ev.kind === "AgentStopped") reload();
    });
    return off;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  const run = async (c: RemoteClient, d: NonNullable<Dialog>) => {
    setDialog(null);
    setActionError(null);
    try {
      if (d.kind === "stopAll") {
        const r = await emergencyStop(c, { kind: "all" });
        setNotice(t("home.stoppedAll", { count: r?.agentsStopped ?? 0 }));
      } else if (d.kind === "stopAgent") {
        await emergencyStop(c, { kind: "agent", agentId: d.agentId });
        setNotice(t("home.stoppedAgent", { name: d.name }));
      } else {
        await disableRemote(c, core.stepUp(coreId));
        setNotice(t("home.remoteDisabled"));
        await core.connections.disconnect(coreId);
      }
      reload();
    } catch (e) {
      setActionError(errorCode(e));
    }
  };

  const err = (e: string | null) => (e ? t(`errors.${e}`, { defaultValue: t("errors.internal") }) : null);

  return (
    <Screen testID="screen-home">
      <Button big variant="danger" label={t("home.emergencyStop")} onPress={() => setDialog({ kind: "stopAll" })} disabled={!client} testID="emergency-stop" />
      <View style={{ height: 12 }} />
      {notice ? (
        <Card>
          <Body testID="home-notice">{notice}</Body>
        </Card>
      ) : null}
      <ErrorText testID="home-error">{err(actionError ?? connError ?? error)}</ErrorText>
      {connError ? <Button label={t("common.retry")} onPress={retry} variant="secondary" testID="home-retry" /> : null}
      {stepUp.data === null && !stepUp.loading ? (
        <Card>
          <Muted testID="home-stepup-missing">{t("pairing.stepUpMissing")}</Muted>
        </Card>
      ) : null}

      <View style={[s.grid, { marginBottom: 12 }]}>
        {(
          [
            ["Approvals", "nav.approvals", data?.approvals.ok ? data.approvals.value.length : null],
            ["Manager", "nav.manager", null],
            ["Review", "nav.review", null],
            ["Editor", "nav.editor", null],
            ["Devices", "nav.devices", null],
            ["Settings", "nav.settings", null],
          ] as const
        ).map(([screen, key, count]) => (
          <View key={screen} style={{ width: "48%" }}>
            <Button
              label={count ? `${t(key)} (${count})` : t(key)}
              variant="secondary"
              onPress={() => navigation.navigate(screen, { coreId })}
              testID={`nav-${screen.toLowerCase()}`}
            />
          </View>
        ))}
      </View>

      {data ? <MonitoringView data={data} onStopAgent={(agentId, name) => setDialog({ kind: "stopAgent", agentId, name })} /> : null}
      <Button label={t("common.refresh")} onPress={reload} busy={loading} variant="ghost" disabled={!client} testID="home-refresh" />

      <Card title={t("home.killSwitchTitle")}>
        <Muted>{t("home.killSwitchHelp")}</Muted>
        <View style={{ height: 8 }} />
        <Button label={t("home.killSwitch")} variant="secondary" onPress={() => setDialog({ kind: "killSwitch" })} disabled={!client} testID="kill-switch" />
      </Card>
      <Text style={{ color: colors.subtle, fontSize: 11, textAlign: "center" }}>{coreId}</Text>

      <ConfirmDialog
        visible={dialog !== null}
        testID="confirm"
        danger
        title={dialog?.kind === "killSwitch" ? t("home.killSwitchConfirmTitle") : t("home.stopConfirmTitle")}
        message={
          dialog?.kind === "killSwitch"
            ? t("home.killSwitchConfirm")
            : dialog?.kind === "stopAgent"
              ? t("home.stopAgentConfirm", { name: dialog.name })
              : t("home.stopAllConfirm")
        }
        confirmLabel={dialog?.kind === "killSwitch" ? t("home.killSwitch") : t("home.confirmStop")}
        cancelLabel={t("common.cancel")}
        onCancel={() => setDialog(null)}
        onConfirm={() => client && dialog && void run(client, dialog)}
      />
    </Screen>
  );
}
