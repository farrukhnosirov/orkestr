import type { Approval } from "@orkestr/protocol";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Text, View } from "react-native";

import { BiometricDeniedError, isDangerous, listPendingApprovals, resolveWithStepUp } from "../core";
import type { ScreenProps } from "../navigation/types";
import { errorCode, useAsync, useClient, useDeps } from "../state/deps";
import { Badge, Body, Button, Card, ErrorText, H1, ListItem, Muted, Screen, s } from "../ui/components";

const riskTone = (a: Approval) => (isDangerous(a) ? "danger" : a.risk === "Medium" ? "warn" : "muted");

export function ApprovalsScreen({ navigation, route }: ScreenProps<"Approvals">) {
  const { t } = useTranslation();
  const { coreId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const { data, error, loading, reload } = useAsync(client ? () => listPendingApprovals(client) : null, [client]);

  useEffect(() => {
    if (!client) return;
    return client.subscribe(["*"], (ev) => {
      if (ev.kind === "ApprovalRequested" || ev.kind === "ApprovalResolved") reload();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  const e = connError ?? error;
  return (
    <Screen testID="screen-approvals">
      <H1>{t("approvals.title")}</H1>
      <ErrorText>{e ? t(`errors.${e}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      <Card>
        {data && data.length === 0 ? <Muted testID="approvals-empty">{t("approvals.empty")}</Muted> : null}
        {data?.map((a) => (
          <ListItem
            key={a.id}
            testID={`approval-${a.id}`}
            title={a.actionSummary}
            subtitle={a.reason}
            right={<Badge label={t(`risk.${a.risk}`, { defaultValue: a.risk })} tone={riskTone(a)} />}
            onPress={() => navigation.navigate("ApprovalDetail", { coreId, approvalId: a.id })}
          />
        ))}
      </Card>
      <Button label={t("common.refresh")} onPress={reload} busy={loading} variant="secondary" testID="approvals-refresh" />
    </Screen>
  );
}

export function ApprovalDetailScreen({ navigation, route }: ScreenProps<"ApprovalDetail">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const { coreId, approvalId } = route.params;
  const { client, error: connError } = useClient(coreId);
  const { data, error } = useAsync(client ? async () => (await client.call("approvals.get", { approvalId })).approval : null, [client, approvalId]);
  const [busy, setBusy] = useState<"Approve" | "Reject" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [result, setResult] = useState<Approval | null>(null);

  const decide = async (decision: "Approve" | "Reject") => {
    if (!client) return;
    setBusy(decision);
    setActionError(null);
    try {
      setResult(await resolveWithStepUp(client, { approvalId, decision }, core.stepUp(coreId)));
    } catch (e) {
      setActionError(e instanceof BiometricDeniedError ? "biometric_denied" : errorCode(e));
    } finally {
      setBusy(null);
    }
  };

  const a = result ?? data;
  const e = actionError ?? connError ?? error;
  return (
    <Screen testID="screen-approval-detail">
      <H1>{t("approvals.detail")}</H1>
      <ErrorText testID="approval-error">{e ? t(`errors.${e}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      {a ? (
        <>
          <Card>
            <View style={{ flexDirection: "row", gap: 8, marginBottom: 8 }}>
              <Badge label={t(`risk.${a.risk}`, { defaultValue: a.risk })} tone={riskTone(a)} />
              <Badge label={t(`approvals.status.${a.status}`, { defaultValue: a.status })} tone={a.status === "Pending" ? "accent" : "muted"} />
            </View>
            <Body testID="approval-summary">{a.actionSummary}</Body>
            <View style={{ height: 8 }} />
            <Muted>{a.reason}</Muted>
            {a.commandPreview ? (
              <View style={{ marginTop: 12, padding: 10, borderRadius: 8, backgroundColor: "#000" }}>
                <Text style={s.mono} testID="approval-command">
                  {a.commandPreview}
                </Text>
              </View>
            ) : null}
            {isDangerous(a) ? (
              <>
                <View style={{ height: 8 }} />
                <Muted>{t("approvals.stepUpNote")}</Muted>
              </>
            ) : null}
          </Card>
          {a.status === "Pending" ? (
            <>
              <Button label={t("approvals.approve")} onPress={() => void decide("Approve")} busy={busy === "Approve"} disabled={!!busy} testID="approval-approve" />
              <View style={{ height: 8 }} />
              <Button label={t("approvals.reject")} variant="danger" onPress={() => void decide("Reject")} busy={busy === "Reject"} disabled={!!busy} testID="approval-reject" />
            </>
          ) : (
            <Button label={t("common.back")} variant="secondary" onPress={() => navigation.goBack()} testID="approval-back" />
          )}
        </>
      ) : null}
    </Screen>
  );
}
