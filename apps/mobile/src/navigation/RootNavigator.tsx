import { DarkTheme, type LinkingOptions, NavigationContainer, createNavigationContainerRef } from "@react-navigation/native";
import { createNativeStackNavigator } from "@react-navigation/native-stack";
import { useTranslation } from "react-i18next";

import { ApprovalDetailScreen, ApprovalsScreen } from "../screens/ApprovalsScreen";
import CoresScreen from "../screens/CoresScreen";
import DevicesScreen from "../screens/DevicesScreen";
import EditorScreen from "../screens/EditorScreen";
import HomeScreen from "../screens/HomeScreen";
import LoginScreen from "../screens/LoginScreen";
import ManagerScreen from "../screens/ManagerScreen";
import PairingScreen from "../screens/PairingScreen";
import ReviewScreen from "../screens/ReviewScreen";
import SettingsScreen from "../screens/SettingsScreen";
import { colors } from "../ui/theme";
import type { RootStackParamList } from "./types";

const Stack = createNativeStackNavigator<RootStackParamList>();

export const navigationRef = createNavigationContainerRef<RootStackParamList>();

export const linking: LinkingOptions<RootStackParamList> = {
  prefixes: ["orkestr://"],
  config: {
    screens: {
      Pairing: "pair",
      ApprovalDetail: "approval/:coreId/:approvalId",
    },
  },
};

const theme = {
  ...DarkTheme,
  colors: { ...DarkTheme.colors, background: colors.bg, card: colors.surface, text: colors.text, border: colors.border, primary: colors.accent },
};

export function RootNavigator({ initial, onReady }: { initial: keyof RootStackParamList; onReady?: () => void }) {
  const { t } = useTranslation();
  return (
    <NavigationContainer ref={navigationRef} linking={linking} theme={theme} onReady={onReady}>
      <Stack.Navigator
        initialRouteName={initial}
        screenOptions={{ headerStyle: { backgroundColor: colors.surface }, headerTintColor: colors.text, contentStyle: { backgroundColor: colors.bg } }}
      >
        <Stack.Screen name="Login" component={LoginScreen} options={{ headerShown: false }} />
        <Stack.Screen name="Cores" component={CoresScreen} options={{ title: t("nav.cores") }} />
        <Stack.Screen name="Pairing" component={PairingScreen} options={{ title: t("nav.pairing") }} />
        <Stack.Screen name="Home" component={HomeScreen} options={{ title: t("nav.home") }} />
        <Stack.Screen name="Approvals" component={ApprovalsScreen} options={{ title: t("nav.approvals") }} />
        <Stack.Screen name="ApprovalDetail" component={ApprovalDetailScreen} options={{ title: t("nav.approvalDetail") }} />
        <Stack.Screen name="Manager" component={ManagerScreen} options={{ title: t("nav.manager") }} />
        <Stack.Screen name="Review" component={ReviewScreen} options={{ title: t("nav.review") }} />
        <Stack.Screen name="Editor" component={EditorScreen} options={{ title: t("nav.editor") }} />
        <Stack.Screen name="Devices" component={DevicesScreen} options={{ title: t("nav.devices") }} />
        <Stack.Screen name="Settings" component={SettingsScreen} options={{ title: t("nav.settings") }} />
      </Stack.Navigator>
    </NavigationContainer>
  );
}
