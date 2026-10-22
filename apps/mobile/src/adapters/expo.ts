/** Expo implementations of the core adapters (SecureKV, BiometricGate, Notifier, Speaker). */
import Constants from "expo-constants";
import * as LocalAuthentication from "expo-local-authentication";
import * as Notifications from "expo-notifications";
import * as SecureStore from "expo-secure-store";
import * as Speech from "expo-speech";
import { Platform } from "react-native";

import {
  type BiometricGate,
  BiometricDeniedError,
  type LocalNotification,
  type NativePushToken,
  type Notifier,
  type SecureKV,
  type Speaker,
} from "../core";

const OPTS: SecureStore.SecureStoreOptions = { keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY };

export class ExpoSecureKV implements SecureKV {
  get(key: string) {
    return SecureStore.getItemAsync(key, OPTS);
  }
  set(key: string, value: string) {
    return SecureStore.setItemAsync(key, value, OPTS);
  }
  delete(key: string) {
    return SecureStore.deleteItemAsync(key, OPTS);
  }
}

/**
 * Step-up key behind `requireAuthentication: true`: Android Keystore / iOS
 * Keychain release the item only after a biometric prompt.
 */
export class ExpoBiometricGate implements BiometricGate {
  async available() {
    return (await LocalAuthentication.hasHardwareAsync()) && (await LocalAuthentication.isEnrolledAsync());
  }
  async setSecret(key: string, value: string) {
    if (!(await this.available())) throw new BiometricDeniedError("no biometric enrolled on this device");
    await SecureStore.setItemAsync(key, value, { ...OPTS, requireAuthentication: true, authenticationPrompt: "Orkestr" });
  }
  async getSecret(key: string, prompt: string) {
    try {
      return await SecureStore.getItemAsync(key, { ...OPTS, requireAuthentication: true, authenticationPrompt: prompt });
    } catch (e) {
      throw new BiometricDeniedError(e instanceof Error ? e.message : String(e));
    }
  }
  async deleteSecret(key: string) {
    await SecureStore.deleteItemAsync(key, { ...OPTS, requireAuthentication: true }).catch(() => undefined);
  }
}

/** Whether this build carries push credentials (google-services.json on Android). */
export function nativePushConfigured(): boolean {
  if (Platform.OS === "android") return !!Constants.expoConfig?.android?.googleServicesFile;
  return Platform.OS === "ios";
}

export class ExpoNotifier implements Notifier {
  async requestPermission() {
    const cur = await Notifications.getPermissionsAsync();
    if (cur.granted) return true;
    return (await Notifications.requestPermissionsAsync()).granted;
  }
  async show(n: LocalNotification) {
    await Notifications.scheduleNotificationAsync({
      identifier: n.id,
      content: { title: n.title, body: n.body, data: n.data },
      trigger: null,
    });
  }
  async nativeToken(): Promise<NativePushToken | null> {
    if (!nativePushConfigured()) return null;
    try {
      const t = await Notifications.getDevicePushTokenAsync();
      return { transport: Platform.OS === "ios" ? "apns" : "fcm", token: String(t.data) };
    } catch {
      return null;
    }
  }
}

export class ExpoSpeaker implements Speaker {
  speak(text: string, language: string) {
    Speech.stop();
    Speech.speak(text, { language: language === "ru" ? "ru-RU" : "en-US" });
  }
}

export function configureNotificationChannel(): void {
  Notifications.setNotificationHandler({
    handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: true, shouldSetBadge: false }),
  });
  if (Platform.OS === "android") {
    void Notifications.setNotificationChannelAsync("default", {
      name: "Orkestr",
      importance: Notifications.AndroidImportance.HIGH,
      lockscreenVisibility: Notifications.AndroidNotificationVisibility.PRIVATE,
    }).catch(() => undefined);
  }
}
