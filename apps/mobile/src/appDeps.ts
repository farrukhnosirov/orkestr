/** Production dependencies: MobileCore over the Expo adapters. */
import * as Device from "expo-device";
import { Platform } from "react-native";

import { ExpoBiometricGate, ExpoNotifier, ExpoSecureKV, ExpoSpeaker, nativePushConfigured } from "./adapters/expo";
import { useExpoRecorder } from "./adapters/recorder";
import { MobileCore } from "./core";
import type { Deps } from "./state/deps";

export function createAppDeps(): Deps {
  const core = new MobileCore({
    kv: new ExpoSecureKV(),
    gate: new ExpoBiometricGate(),
    notifier: new ExpoNotifier(),
    platform: Platform.OS,
    deviceName: Device.deviceName ?? Device.modelName ?? `Orkestr ${Platform.OS}`,
    keyStorage: Platform.OS === "ios" ? "ios-keychain-wrapped" : "android-keystore-wrapped",
    stepUpPrompt: "Orkestr",
  });
  return { core, speaker: new ExpoSpeaker(), useRecorder: useExpoRecorder, pushConfigured: nativePushConfigured(), platform: Platform.OS };
}
