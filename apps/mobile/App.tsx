import "./src/i18n";

import * as Notifications from "expo-notifications";
import { StatusBar } from "expo-status-bar";
import { useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { configureNotificationChannel } from "./src/adapters/expo";
import { createAppDeps } from "./src/appDeps";
import { loadLanguage } from "./src/i18n";
import { RootNavigator, navigationRef } from "./src/navigation/RootNavigator";
import { type Navigator, type RootStackParamList, handleNotificationTap } from "./src/navigation/types";
import { type Deps, DepsProvider } from "./src/state/deps";
import { colors } from "./src/ui/theme";

export default function App({ deps: injected }: { deps?: Deps }) {
  const deps = useMemo(() => injected ?? createAppDeps(), [injected]);
  const [initial, setInitial] = useState<keyof RootStackParamList | null>(null);
  const pendingTap = useRef<Record<string, unknown> | null>(null);

  useEffect(() => {
    configureNotificationChannel();
    void (async () => {
      await loadLanguage(deps.core.o.kv, Intl.DateTimeFormat().resolvedOptions().locale).catch(() => undefined);
      const session = await deps.core.restore().catch(() => null);
      setInitial(session?.authenticated ? "Cores" : "Login");
    })();
    const sub = Notifications.addNotificationResponseReceivedListener((r) => {
      const data = r.notification.request.content.data as Record<string, unknown> | undefined;
      if (navigationRef.isReady()) handleNotificationTap(navigationRef as unknown as Navigator, data);
      else pendingTap.current = data ?? {};
    });
    void Notifications.getLastNotificationResponseAsync()
      .then((r) => {
        if (r) pendingTap.current = (r.notification.request.content.data as Record<string, unknown> | undefined) ?? {};
      })
      .catch(() => undefined);
    return () => sub.remove();
  }, [deps]);

  return (
    <SafeAreaProvider>
      <DepsProvider deps={deps}>
        <StatusBar style="light" />
        {initial ? (
          <RootNavigator
            initial={initial}
            onReady={() => {
              if (pendingTap.current && initial !== "Login") {
                handleNotificationTap(navigationRef as unknown as Navigator, pendingTap.current);
                pendingTap.current = null;
              }
            }}
          />
        ) : (
          <View style={{ flex: 1, backgroundColor: colors.bg, alignItems: "center", justifyContent: "center" }} testID="app-root">
            <ActivityIndicator color={colors.accent} />
          </View>
        )}
      </DepsProvider>
    </SafeAreaProvider>
  );
}
