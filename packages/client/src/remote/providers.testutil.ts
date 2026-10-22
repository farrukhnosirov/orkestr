/**
 * Test helper (not exported from the package): runs a suite under a given
 * `RemoteCrypto` provider. Under noble, `globalThis.crypto.subtle` is replaced
 * by a proxy that throws on any access, proving the pure-JS path never
 * touches WebCrypto (only `getRandomValues` remains, as on Hermes + expo-crypto).
 */
import { afterAll, beforeAll } from "vitest";

import { type RemoteCrypto, setRemoteCrypto } from "./crypto";
import { nobleCrypto } from "./noble";
import { webCrypto } from "./webcrypto";

export const PROVIDERS = [
  ["webcrypto", webCrypto],
  ["noble", nobleCrypto],
] as const;

export function useProvider(provider: () => RemoteCrypto): void {
  let saved: PropertyDescriptor | undefined;
  beforeAll(() => {
    const c = provider();
    setRemoteCrypto(c);
    if (c.name !== "noble") return;
    saved = Object.getOwnPropertyDescriptor(globalThis, "crypto");
    const real = globalThis.crypto;
    const poisoned = new Proxy(
      {},
      {
        get(_t, prop) {
          throw new Error(`crypto.subtle.${String(prop)} touched under the noble provider`);
        },
      },
    );
    Object.defineProperty(globalThis, "crypto", {
      configurable: true,
      writable: true,
      value: { getRandomValues: real.getRandomValues.bind(real), subtle: poisoned },
    });
  });
  afterAll(() => {
    if (saved) Object.defineProperty(globalThis, "crypto", saved);
    saved = undefined;
    setRemoteCrypto(webCrypto());
  });
}
