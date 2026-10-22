/** Renders a screen with an in-memory MobileCore whose connections return a fake client. */
import "../i18n";

import { render } from "@testing-library/react-native";
import type { ComponentType } from "react";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { MemoryRecorder, MemorySpeaker, createMemoryCore } from "../core";
import { type Deps, DepsProvider } from "../state/deps";
import type { FakeClient } from "./fakeClient";

const metrics = { frame: { x: 0, y: 0, width: 400, height: 800 }, insets: { top: 0, left: 0, right: 0, bottom: 0 } };

export function makeDeps(client: FakeClient | null) {
  const core = createMemoryCore();
  if (client) jest.spyOn(core.connections, "get").mockResolvedValue(client as never);
  const recorder = new MemoryRecorder();
  const deps: Deps = { core, speaker: new MemorySpeaker(), useRecorder: () => recorder, pushConfigured: false, platform: "android" };
  return { core, deps, recorder };
}

export function fakeNav() {
  return { navigate: jest.fn(), reset: jest.fn(), goBack: jest.fn(), addListener: jest.fn(() => () => undefined), setOptions: jest.fn() };
}

export async function renderScreen<P extends object>(Screen: ComponentType<P>, props: { params?: object }, deps: Deps) {
  const navigation = fakeNav();
  const all = { navigation, route: { key: "k", name: "X", params: props.params } } as unknown as P;
  const r = await render(
    <SafeAreaProvider initialMetrics={metrics}>
      <DepsProvider deps={deps}>
        <Screen {...all} />
      </DepsProvider>
    </SafeAreaProvider>,
  );
  return { ...r, navigation };
}
