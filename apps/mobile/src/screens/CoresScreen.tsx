import { useTranslation } from "react-i18next";
import { View } from "react-native";

import type { ScreenProps } from "../navigation/types";
import { useAsync, useDeps } from "../state/deps";
import { Badge, Button, Card, ErrorText, H1, ListItem, Muted, Screen } from "../ui/components";

export default function CoresScreen({ navigation }: ScreenProps<"Cores">) {
  const { t } = useTranslation();
  const { core } = useDeps();
  const { data, error, loading, reload } = useAsync(async () => {
    const paired = await core.store.list();
    const remote = core.session ? await core.session.cores().catch(() => null) : null;
    return { paired, remote };
  }, []);

  const pairedIds = new Set((data?.paired ?? []).map((r) => r.coreId));
  const online = new Map((data?.remote ?? []).map((c) => [c.coreId, c] as const));

  return (
    <Screen testID="screen-cores">
      <H1>{t("cores.title")}</H1>
      <ErrorText>{error ? t(`errors.${error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      <Card title={t("cores.paired")}>
        {data && data.paired.length === 0 ? <Muted testID="cores-empty">{t("cores.empty")}</Muted> : null}
        {data?.paired.map((r) => {
          const g = online.get(r.coreId);
          return (
            <ListItem
              key={r.coreId}
              testID={`core-${r.coreId}`}
              title={g?.name ?? r.coreId}
              subtitle={`${t(`roles.${r.role ?? "viewer"}`, { defaultValue: r.role ?? "" })} · ${r.gatewayUrl}`}
              right={<Badge label={g?.online ? t("cores.online") : t("cores.offline")} tone={g?.online ? "ok" : "muted"} />}
              onPress={() => navigation.navigate("Home", { coreId: r.coreId })}
            />
          );
        })}
      </Card>
      {data?.remote?.some((c) => !pairedIds.has(c.coreId)) ? (
        <Card title={t("cores.account")}>
          {data.remote
            .filter((c) => !pairedIds.has(c.coreId))
            .map((c) => (
              <ListItem
                key={c.coreId}
                testID={`gateway-core-${c.coreId}`}
                title={c.name}
                subtitle={t("cores.notPaired")}
                right={<Badge label={c.online ? t("cores.online") : t("cores.offline")} tone={c.online ? "ok" : "muted"} />}
              />
            ))}
        </Card>
      ) : null}
      <Button label={t("cores.pair")} onPress={() => navigation.navigate("Pairing")} testID="cores-pair" />
      <View style={{ height: 8 }} />
      <Button label={t("common.refresh")} onPress={reload} busy={loading} variant="secondary" testID="cores-refresh" />
      <View style={{ height: 8 }} />
      <Button label={t("settings.title")} onPress={() => navigation.navigate("Settings")} variant="ghost" testID="cores-settings" />
    </Screen>
  );
}
