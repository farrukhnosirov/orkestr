import type { NativeStackScreenProps } from "@react-navigation/native-stack";

import { type NotificationRoute, routeForNotification } from "../core";

export type RootStackParamList = {
  Login: undefined;
  Cores: undefined;
  Pairing: { offer?: string } | undefined;
  Home: { coreId: string };
  Approvals: { coreId: string };
  ApprovalDetail: { coreId: string; approvalId: string };
  Manager: { coreId: string };
  Review: { coreId: string };
  Editor: { coreId: string; projectId?: string; path?: string };
  Devices: { coreId: string };
  Settings: { coreId?: string } | undefined;
};

export type ScreenProps<K extends keyof RootStackParamList> = NativeStackScreenProps<RootStackParamList, K>;

/** Minimal navigation surface used to route a tapped notification. */
export interface Navigator {
  navigate(name: string, params?: object): void;
}

/** Opens the screen a notification points at. */
export function openRoute(nav: Navigator, r: NotificationRoute): void {
  switch (r.screen) {
    case "ApprovalDetail":
      nav.navigate("ApprovalDetail", { coreId: r.coreId, approvalId: r.approvalId });
      return;
    case "Approvals":
      nav.navigate("Approvals", { coreId: r.coreId });
      return;
    case "Home":
      if (r.coreId) nav.navigate("Home", { coreId: r.coreId });
      else nav.navigate("Cores");
  }
}

/** Routes a tapped notification (local or decrypted push). */
export function handleNotificationTap(nav: Navigator, data: Record<string, unknown> | undefined): void {
  openRoute(nav, routeForNotification(data));
}
