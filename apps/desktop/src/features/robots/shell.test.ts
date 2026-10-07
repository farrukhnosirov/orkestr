import { describe, expect, it } from "vitest";

import { createLifecycleQueue } from "./shell";

function deferred() {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("robot window lifecycle queue", () => {
  it("re-opening a robot waits until its close finished (scale change / quick hide-unhide)", async () => {
    const q = createLifecycleQueue();
    const log: string[] = [];
    const close = deferred();
    const closing = q.run("a", async () => {
      log.push("close:start");
      await close.promise;
      log.push("close:end");
    });
    const opening = q.run("a", async () => {
      log.push("open");
    });
    await tick();
    expect(log).toEqual(["close:start"]);
    close.resolve();
    await Promise.all([closing, opening]);
    expect(log).toEqual(["close:start", "close:end", "open"]);
  });

  it("a failed close does not block the following open; other robots run concurrently", async () => {
    const q = createLifecycleQueue();
    const log: string[] = [];
    const hold = deferred();
    const failing = q.run("a", async () => {
      await hold.promise;
      throw new Error("boom");
    });
    const other = q.run("b", async () => {
      log.push("b");
    });
    const reopen = q.run("a", async () => {
      log.push("a");
    });
    await other;
    expect(log).toEqual(["b"]);
    hold.resolve();
    await expect(failing).rejects.toThrow("boom");
    await reopen;
    expect(log).toEqual(["b", "a"]);
  });

  it("closeAll runs after earlier calls and before later ones", async () => {
    const q = createLifecycleQueue();
    const log: string[] = [];
    const hold = deferred();
    void q.run("a", async () => {
      await hold.promise;
      log.push("open a");
    });
    const all = q.all(async () => {
      log.push("close all");
    });
    const later = q.run("a", async () => {
      log.push("open a again");
    });
    await tick();
    expect(log).toEqual([]);
    hold.resolve();
    await Promise.all([all, later]);
    expect(log).toEqual(["open a", "close all", "open a again"]);
  });
});
