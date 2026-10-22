import { b64uDecode, b64uEncode, encryptNativePush, nobleCrypto, sas, setRemoteCrypto } from "@orkestr/client";

const cat = (...p: Uint8Array[]) => {
  const out = new Uint8Array(p.reduce((n, x) => n + x.length, 0));
  let o = 0;
  for (const x of p) {
    out.set(x, o);
    o += x.length;
  }
  return out;
};
import type { Event } from "@orkestr/protocol";

import { approval, fakeClient, stepUpRequired } from "../test/fakeClient";
import { BiometricDeniedError, MemoryBiometricGate, MemoryNotifier, MemorySecureKV } from "./adapters";
import { createMemoryCore } from "./app";
import { callWithStepUp, resolveWithStepUp } from "./approvals";
import { GatewaySession, bearerFetch } from "./gatewaySession";
import { SecureKVKeyStore, StepUpKeys } from "./keys";
import { loadMonitoring, saveFile } from "./monitoring";
import { pairingDeepLink, parsePairingInput, sasMatches } from "./pairing";
import { PushManager, extractSealed, routeForNotification } from "./push";
import { executeTranscript } from "./voice/executor";

beforeAll(() => setRemoteCrypto(nobleCrypto()));

const offer = { gatewayUrl: "http://gw.test", coreId: "core-1", pairingId: "p-1", code: "ABCD2345", coreKeyFingerprint: "fp" };

describe("pairing input", () => {
  it("parses QR text, bare JSON, deep links and base64url payloads", () => {
    expect(parsePairingInput("orkestr-pair:" + JSON.stringify(offer))).toEqual(offer);
    expect(parsePairingInput(JSON.stringify(offer))).toEqual(offer);
    expect(parsePairingInput(pairingDeepLink(offer))).toEqual(offer);
    const b64 = b64uEncode(new TextEncoder().encode("orkestr-pair:" + JSON.stringify(offer)) as Uint8Array<ArrayBuffer>);
    expect(parsePairingInput(`orkestr://pair?offer=${b64}`)).toEqual(offer);
    expect(parsePairingInput("hello")).toBeNull();
    expect(parsePairingInput("")).toBeNull();
  });

  it("compares SAS codes", () => {
    expect(sasMatches("123456", "123 456")).toBe(true);
    expect(sasMatches("123456", "123457")).toBe(false);
  });
});

describe("gateway session", () => {
  it("logs in with TOTP, keeps the bearer and sends it", async () => {
    const seen: (string | null)[] = [];
    const f = jest.fn(async (url: string, init: RequestInit) => {
      seen.push(new Headers(init.headers).get("authorization"));
      if (url.endsWith("/v1/auth/totp/login")) return new Response(JSON.stringify({ token: "tok", accountId: "acc" }));
      if (url.endsWith("/v1/cores")) return new Response(JSON.stringify({ cores: [{ coreId: "c", name: "PC", online: true }] }));
      return new Response("nope", { status: 404 });
    }) as unknown as typeof fetch;
    const kv = new MemorySecureKV();
    const s = new GatewaySession("http://gw", { kv, fetch: f });
    await s.totpLogin(" 123456 ");
    expect(s.token).toBe("tok");
    expect(await s.cores()).toEqual([{ coreId: "c", name: "PC", online: true }]);
    expect(seen).toEqual([null, "Bearer tok"]);
    const restored = await GatewaySession.restore(kv, f);
    expect(restored?.token).toBe("tok");
    await expect(new GatewaySession("http://gw", { fetch: f }).state()).rejects.toMatchObject({ status: 404 });
  });

  it("bearerFetch adds the header", async () => {
    const f = jest.fn(async () => new Response("{}")) as unknown as typeof fetch;
    await bearerFetch("x", f)("http://a", { headers: { a: "1" } });
    const init = (f as unknown as jest.Mock).mock.calls[0][1] as RequestInit;
    expect(new Headers(init.headers).get("authorization")).toBe("Bearer x");
  });
});

describe("pairing flow", () => {
  it("claims with a raw device key, shows the SAS, stores the record and registers step-up", async () => {
    const coreKey = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const corePub = new Uint8Array(await crypto.subtle.exportKey("raw", coreKey.publicKey));
    let devicePub: Uint8Array = new Uint8Array();
    const f = (async (url: string, init: RequestInit = {}) => {
      expect(new Headers(init.headers).get("authorization")).toBe("Bearer tok");
      if (url.endsWith("/claim")) {
        const body = JSON.parse(String(init.body)) as { devicePubKey: string };
        devicePub = b64uDecode(body.devicePubKey);
        return new Response(JSON.stringify({ status: "pending", coreId: "core-1", corePublicKey: b64uEncode(corePub) }));
      }
      const enc = new TextEncoder();
      const msg = await crypto.subtle.digest(
        "SHA-256",
        cat(enc.encode("orkestr-pairing-v1"), enc.encode("p-1"), enc.encode("dev-1"), devicePub, enc.encode("admin")),
      );
      const sig = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, coreKey.privateKey, msg));
      return new Response(JSON.stringify({ status: "approved", result: { status: "approved", deviceId: "dev-1", role: "admin", sig: b64uEncode(sig) } }));
    }) as unknown as typeof fetch;

    const core = createMemoryCore({ fetch: f });
    core.session = new GatewaySession("http://gw.test", { fetch: f, token: "tok" });
    const { coreKeyFingerprint: _fp, ...noFp } = offer;
    const session = await core.pair(noFp);
    expect(session.sas).toMatch(/^\d{6}$/);
    expect(session.sas).toBe(await sas(corePub, devicePub, "p-1"));

    const client = fakeClient({ "devices.registerStepUp": {} });
    jest.spyOn(core.connections, "get").mockResolvedValue(client as never);
    const done = await core.completePairing(session, { intervalMs: 1 });
    expect(done.record.deviceId).toBe("dev-1");
    expect(done.record.rawKey?.d).toBeTruthy();
    expect(done.stepUpRegistered).toBe(true);
    const cred = client.calls[0]?.params.credential as { type: string; keyStorage: string; publicKey: string };
    expect(cred.type).toBe("orkestr-native-v1");
    // the step-up key is a different key than the device key
    expect(cred.publicKey).not.toBe(done.record.rawKey?.pub);
    expect((await core.store.list()).map((r) => r.coreId)).toEqual(["core-1"]);
    // device key secret is in plain SecureKV, step-up key only in the biometric gate
    expect([...core.o.gate.data.keys()]).toEqual(["stepup.core-1.key"]);
  });
});

describe("approvals step-up", () => {
  const params = { approvalId: "a-1", decision: "Approve" as const };

  it("retries once with the assertion for the core's challenge", async () => {
    let n = 0;
    const client = fakeClient({
      "approvals.resolve": (p: Record<string, unknown>) => {
        n += 1;
        if (!p.stepUp) throw stepUpRequired("ch-9");
        return { approval: approval({ status: "Approved" }) };
      },
    });
    const stepUp = jest.fn(async (challenge: string) => ({ credentialId: "c", authenticatorData: "a", clientDataJSON: challenge, signature: "s" }));
    const a = await resolveWithStepUp(client, params, stepUp);
    expect(a.status).toBe("Approved");
    expect(stepUp).toHaveBeenCalledTimes(1);
    expect(stepUp).toHaveBeenCalledWith("ch-9");
    expect(n).toBe(2);
    expect(client.calls[1]?.params.stepUp).toMatchObject({ clientDataJSON: "ch-9" });
  });

  it("biometric denied → no retry, approval stays pending", async () => {
    const client = fakeClient({
      "approvals.resolve": () => {
        throw stepUpRequired();
      },
    });
    const kv = new MemorySecureKV();
    const gate = new MemoryBiometricGate();
    const keys = new StepUpKeys(kv, gate);
    await keys.create("core-1");
    gate.allow = false;
    await expect(resolveWithStepUp(client, params, (c) => keys.assert("core-1", "dev-1", c, "x"))).rejects.toBeInstanceOf(BiometricDeniedError);
    expect(client.call).toHaveBeenCalledTimes(1);
    expect(gate.prompts).toBe(1);
    expect((await keys.meta("core-1"))?.counter).toBe(0);
  });

  it("non-step-up errors propagate untouched", async () => {
    const client = fakeClient({});
    const stepUp = jest.fn();
    await expect(callWithStepUp(client, "devices.revoke", { deviceId: "d" }, stepUp)).rejects.toMatchObject({ code: "not_found" });
    expect(stepUp).not.toHaveBeenCalled();
  });

  it("step-up counter strictly increases and persists", async () => {
    const kv = new MemorySecureKV();
    const keys = new StepUpKeys(kv, new MemoryBiometricGate());
    await keys.create("c");
    const a1 = await keys.assert("c", "d", "x", "p");
    const a2 = await new StepUpKeys(kv, new MemoryBiometricGate()).meta("c");
    expect(a1.credentialId).toBe(a2?.credentialId);
    // authenticatorData = rpIdHash(32) ‖ flags ‖ counterBE32
    expect(Array.from(b64uDecode(a1.authenticatorData).slice(33))).toEqual([0, 0, 0, 1]);
    expect(JSON.parse(new TextDecoder().decode(b64uDecode(a1.clientDataJSON)))).toMatchObject({ type: "webauthn.get", challenge: "x", origin: "orkestr-native:d" });
    expect(a2?.counter).toBe(1);
    await keys.assert("c", "d", "y", "p");
    expect((await keys.meta("c"))?.counter).toBe(2);
  });
});

describe("key store", () => {
  it("round-trips records without CryptoKeyPair", async () => {
    const s = new SecureKVKeyStore(new MemorySecureKV());
    const rec = { id: "c/1", gatewayUrl: "g", coreId: "c/1", deviceId: "d", corePubKey: "k", deviceName: "n", pairedAt: 1, rawKey: { d: "x", pub: "y" } };
    await s.put(rec);
    expect(await s.get("c/1")).toEqual(rec);
    await s.delete("c/1");
    expect(await s.list()).toEqual([]);
  });
});

describe("push", () => {
  const ev = (a = approval()) => ({ kind: "ApprovalRequested", payload: { approval: a } }) as unknown as Event;

  it("registers the native token with a fresh 32-byte key", async () => {
    const kv = new MemorySecureKV();
    const n = new MemoryNotifier();
    const pm = new PushManager(kv, n);
    const client = fakeClient({ "push.registerNative": { id: "r1", deliverable: true } });
    expect(await pm.registerNative(client, "c")).toEqual({ status: "unavailable", reason: "no_token" });
    n.token = { transport: "fcm", token: "fcm-token" };
    expect(await pm.registerNative(client, "c")).toMatchObject({ status: "registered", deliverable: true });
    const p = client.calls[0]?.params as { key: string; transport: string; lockScreen: string };
    expect(b64uDecode(p.key)).toHaveLength(32);
    expect(p).toMatchObject({ transport: "fcm", lockScreen: "generic" });
    expect(b64uEncode((await pm.payloadKey("c")) as Uint8Array<ArrayBuffer>)).toBe(p.key);
    const payload = { v: 1, eventId: "evt-1", category: "approval", ts: "now", title: "Orkestr", body: "Action needed" };
    const sealed = await encryptNativePush(b64uDecode(p.key), payload);
    await expect(pm.decrypt("c", { p: JSON.stringify(sealed), k: "approval" })).resolves.toEqual(payload);
  });

  it("raises a generic local notification for ApprovalRequested", async () => {
    const n = new MemoryNotifier();
    const pm = new PushManager(new MemorySecureKV(), n);
    const client = fakeClient({});
    pm.attach(client, "core-1");
    client.emit(ev());
    client.emit({ kind: "TaskUpdated", payload: {} } as unknown as Event);
    await new Promise((r) => setTimeout(r, 0));
    expect(n.shown).toEqual([
      { id: "approval-a-1", title: "Orkestr", body: "Action needed", data: { kind: "approval", coreId: "core-1", approvalId: "a-1" } },
    ]);
    await pm.setPrefs({ categories: [], lockScreen: "detailed" });
    expect(await pm.onEvent("core-1", ev())).toBeNull();
  });

  it("routes taps", () => {
    expect(routeForNotification({ kind: "approval", coreId: "c", approvalId: "a" })).toEqual({ screen: "ApprovalDetail", coreId: "c", approvalId: "a" });
    expect(routeForNotification({ k: "approval" }, "c")).toEqual({ screen: "Approvals", coreId: "c" });
    expect(routeForNotification({})).toEqual({ screen: "Home" });
    expect(() => extractSealed({ p: "{" })).toThrow("malformed");
  });
});

describe("voice executor", () => {
  const ctx = { projects: [{ id: "p1", name: "Backend" }], agents: [], activeProjectId: "p1" };

  it('"stop all agents" → emergency.stopAll after confirmation', async () => {
    const client = fakeClient({ "emergency.stopAll": { agentsStopped: 3, otherStopped: 0, approvalsCancelled: 0, flags: {} } });
    expect((await executeTranscript(client, "stop all agents", ctx)).status).toBe("needsConfirmation");
    expect(client.call).not.toHaveBeenCalled();
    const r = await executeTranscript(client, "stop all agents", ctx, { confirm: async () => true });
    expect(r).toMatchObject({ status: "ok", intent: { kind: "stopAll" } });
    expect(client.calls.map((c) => c.method)).toEqual(["emergency.stopAll"]);
    expect((await executeTranscript(client, "stop all agents", ctx, { confirm: async () => false })).status).toBe("cancelled");
  });

  it("unknown text goes to the manager", async () => {
    const client = fakeClient({ "manager.chat": { userMessage: {}, replyMessageId: "m" } });
    const r = await executeTranscript(client, "what changed in the auth module", ctx);
    expect(r.status).toBe("ok");
    expect(client.calls[0]).toMatchObject({ method: "manager.chat", params: { projectId: "p1" } });
  });
});

describe("monitoring & editor", () => {
  it("loads sections independently", async () => {
    const client = fakeClient({
      "dashboard.snapshot": { projects: [] },
      "nodes.list": { nodes: [{ id: "n" }] },
      "agents.list": { agents: [] },
      "approvals.list": { approvals: [approval()] },
      "emergency.status": { flags: {}, activeAgents: 0 },
    });
    const m = await loadMonitoring(client);
    expect(m.nodes).toEqual({ ok: true, value: [{ id: "n" }] });
    expect(m.resources).toEqual({ ok: false, error: "not_found" });
    expect(m.approvals.ok && m.approvals.value).toHaveLength(1);
  });

  it("save reports hash conflicts", async () => {
    const { OrkestrError } = jest.requireActual("@orkestr/client");
    const client = fakeClient({
      "files.write": () => {
        throw new OrkestrError("conflict", "changed", { actualHash: "h2" });
      },
    });
    expect(await saveFile(client, "p", "a.txt", "x", "h1")).toEqual({ status: "conflict", actualHash: "h2" });
    expect(client.calls[0]?.params.expectedHash).toBe("h1");
  });
});
