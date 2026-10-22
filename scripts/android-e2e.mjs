#!/usr/bin/env node
// Android UI helper for the optional Phase 5 Android lane (adb + uiautomator).
// React Native maps `testID` to the Android `resource-id`, so selectors are testIDs.
//
// CLI:  node scripts/android-e2e.mjs <command> [args]
//   install <apk>            adb install -r
//   launch                   start dev.orkestr.mobile
//   deeplink <url>           open orkestr://… (e.g. pairing offer)
//   reverse <port>[,port…]   adb reverse tcp:P tcp:P (gateway, metro 8081)
//   dump                     list visible nodes (id / text / bounds)
//   tap <testID>             tap the element with that testID
//   tapText <text>           tap the element whose text/content-desc matches
//   type <testID> <text>     focus element and type text
//   wait <testID> [ms]       wait until the element is visible
//   screenshot <file.png>    save a screenshot
//   finger [id]              emulator fingerprint touch (biometric prompt)
//   notifications            dumpsys notification lines for the app
// Env: ADB (path to adb), ANDROID_SERIAL (device).
// Also importable: import { tap, typeText, waitFor, screenshot, … } from "./android-e2e.mjs".
import { spawnSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const PACKAGE = "dev.orkestr.mobile";

function adbPath() {
  if (process.env.ADB) return process.env.ADB;
  const home = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT;
  if (home) {
    const p = path.join(home, "platform-tools", process.platform === "win32" ? "adb.exe" : "adb");
    if (existsSync(p)) return p;
  }
  return "adb";
}

export function adb(args, opts = {}) {
  const r = spawnSync(adbPath(), args, { encoding: opts.binary ? "buffer" : "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (r.status !== 0 && !opts.allowFail) {
    throw new Error(`adb ${args.join(" ")} failed: ${(r.stderr || "").toString().trim()}`);
  }
  return r.stdout;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function install(apk) {
  return adb(["install", "-r", "-t", apk]);
}
export function launch() {
  return adb(["shell", "monkey", "-p", PACKAGE, "-c", "android.intent.category.LAUNCHER", "1"]);
}
export function deeplink(url) {
  return adb(["shell", "am", "start", "-W", "-a", "android.intent.action.VIEW", "-d", `'${url}'`, PACKAGE]);
}
export function reverse(ports) {
  for (const p of String(ports).split(",")) adb(["reverse", `tcp:${p}`, `tcp:${p}`]);
}
export function isRunning() {
  return adb(["shell", "pidof", PACKAGE], { allowFail: true }).trim() !== "";
}

const attr = (s, name) => {
  const m = new RegExp(`\\s${name}="([^"]*)"`).exec(s);
  return m ? m[1].replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&#10;/g, "\n") : "";
};

/** Parses `uiautomator dump` into nodes {id, testID, text, desc, bounds:{x1,y1,x2,y2}, cx, cy}. */
export function dump() {
  adb(["shell", "uiautomator", "dump", "/sdcard/orkestr-ui.xml"], { allowFail: true });
  const xml = adb(["exec-out", "cat", "/sdcard/orkestr-ui.xml"]);
  const nodes = [];
  for (const m of xml.matchAll(/<node\b[^>]*>/g)) {
    const s = m[0];
    const b = /\[(\d+),(\d+)\]\[(\d+),(\d+)\]/.exec(attr(s, "bounds"));
    if (!b) continue;
    const [x1, y1, x2, y2] = b.slice(1).map(Number);
    const id = attr(s, "resource-id");
    nodes.push({
      id,
      testID: id.includes(":id/") ? id.split(":id/")[1] : id,
      text: attr(s, "text"),
      desc: attr(s, "content-desc"),
      cls: attr(s, "class"),
      bounds: { x1, y1, x2, y2 },
      cx: Math.round((x1 + x2) / 2),
      cy: Math.round((y1 + y2) / 2),
    });
  }
  return nodes;
}

export function find(testID, nodes = dump()) {
  return nodes.find((n) => n.testID === testID && n.bounds.x2 > n.bounds.x1);
}

export async function waitFor(testID, timeoutMs = 15000) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const n = find(testID);
    if (n) return n;
    if (Date.now() > end) throw new Error(`timeout waiting for testID "${testID}"`);
    await sleep(700);
  }
}

export async function tap(testID, timeoutMs) {
  const n = await waitFor(testID, timeoutMs);
  adb(["shell", "input", "tap", String(n.cx), String(n.cy)]);
  return n;
}

export function tapText(text) {
  const n = dump().find((x) => x.text === text || x.desc === text || x.text.includes(text));
  if (!n) throw new Error(`no element with text "${text}"`);
  adb(["shell", "input", "tap", String(n.cx), String(n.cy)]);
  return n;
}

/** `input text` needs spaces as %s and shell metacharacters escaped. */
export async function typeText(testID, text) {
  await tap(testID);
  await sleep(300);
  const esc = String(text).replace(/ /g, "%s").replace(/([\\'"`$&|;<>()*?!#~{}[\]])/g, "\\$1");
  adb(["shell", "input", "text", esc]);
}

export function screenshot(file) {
  writeFileSync(file, adb(["exec-out", "screencap", "-p"], { binary: true }));
  return file;
}

export function finger(id = 1) {
  return adb(["emu", "finger", "touch", String(id)], { allowFail: true });
}

export function notifications() {
  return adb(["shell", "dumpsys", "notification", "--noredact"], { allowFail: true })
    .split("\n")
    .filter((l) => l.includes(PACKAGE) || /android\.(title|text)=/.test(l))
    .join("\n");
}

async function main([cmd, ...args]) {
  switch (cmd) {
    case "install":
      return console.log(install(args[0]));
    case "launch":
      return console.log(launch());
    case "deeplink":
      return console.log(deeplink(args[0]));
    case "reverse":
      return reverse(args[0]);
    case "dump":
      for (const n of dump()) if (n.testID || n.text) console.log(`${n.testID || "-"}\t${JSON.stringify(n.text || n.desc)}\t${n.cx},${n.cy}`);
      return;
    case "tap":
      return void (await tap(args[0], Number(args[1]) || undefined));
    case "tapText":
      return void tapText(args.join(" "));
    case "type":
      return typeText(args[0], args.slice(1).join(" "));
    case "wait":
      return void (await waitFor(args[0], Number(args[1]) || undefined));
    case "screenshot":
      return console.log(screenshot(args[0] ?? "screen.png"));
    case "finger":
      return console.log(finger(args[0]));
    case "running":
      return console.log(isRunning() ? "running" : "not running");
    case "notifications":
      return console.log(notifications());
    default:
      console.log("usage: node scripts/android-e2e.mjs install|launch|deeplink|reverse|dump|tap|tapText|type|wait|screenshot|finger|running|notifications …");
      process.exitCode = cmd ? 2 : 0;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
}

