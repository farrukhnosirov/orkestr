import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ScrollView, View } from "react-native";

import type { RemoteClient } from "../core";
import { useAsync } from "../state/deps";
import { Button, ErrorText } from "../ui/components";

/** Horizontal project chips; selects the first project when none is chosen. */
export function ProjectPicker({ client, value, onChange }: { client: RemoteClient | null; value: string | null; onChange: (id: string) => void }) {
  const { t } = useTranslation();
  const { data, error } = useAsync(client ? async () => (await client.call("projects.list", {})).projects : null, [client]);
  useEffect(() => {
    const first = data?.[0];
    if (!value && first) onChange(first.id);
  }, [data, value, onChange]);
  return (
    <View style={{ marginBottom: 12 }}>
      <ErrorText>{error ? t(`errors.${error}`, { defaultValue: t("errors.internal") }) : null}</ErrorText>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        {data?.map((p) => (
          <View key={p.id} style={{ marginRight: 8 }}>
            <Button label={p.name} variant={p.id === value ? "primary" : "secondary"} onPress={() => onChange(p.id)} testID={`project-${p.id}`} />
          </View>
        ))}
      </ScrollView>
    </View>
  );
}
