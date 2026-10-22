/** Injectable dependencies (real browser implementations by default; fakes in tests). */
import {
  type DeviceKeyStore,
  type DeviceRecord,
  type PairDeviceOptions,
  type PairingSession,
  type StepUp,
  defaultKeyStore,
  pairDevice,
  registerStepUpCredential,
  stepUpAssertion,
} from "@orkestr/client";
import { type ReactNode, createContext, useContext } from "react";

import { type GatewayApi, HttpGateway } from "./gateway";
import { type RemoteClient, connectRemote } from "./remote";

export interface AppDeps {
  gateway: GatewayApi;
  store: DeviceKeyStore;
  connect: (rec: DeviceRecord, sessionToken?: string) => Promise<RemoteClient>;
  pair: (o: PairDeviceOptions) => Promise<PairingSession>;
  stepUp: (challenge: string) => Promise<StepUp>;
  registerStepUp: (rec: DeviceRecord) => Promise<Record<string, unknown>>;
}

let defaults: AppDeps | null = null;
export function defaultDeps(): AppDeps {
  defaults ??= {
    gateway: new HttpGateway(),
    store: defaultKeyStore(),
    connect: connectRemote,
    pair: pairDevice,
    stepUp: (challenge) => stepUpAssertion(challenge),
    registerStepUp: (rec) => registerStepUpCredential({ userName: rec.deviceName, deviceId: rec.deviceId }),
  };
  return defaults;
}

const Ctx = createContext<AppDeps | null>(null);

export function DepsProvider({ deps, children }: { deps: AppDeps; children: ReactNode }) {
  return <Ctx.Provider value={deps}>{children}</Ctx.Provider>;
}

export function useDeps(): AppDeps {
  return useContext(Ctx) ?? defaultDeps();
}
