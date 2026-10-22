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
//   unlock [pin]             wake + dismiss keyguard (+ PIN)
//   enrollFinger [pin]       set PIN + enroll emulator fingerprint 1 via Settings
// Env: ADB (path to adb), ANDROID_SERIAL (device).
// Also importable: import { tap, typeText, waitFor, screenshot, … } from "./android-e2e.mjs".
import { spawnSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const PACKAGE = "dev.orkestr.mobile";

function adbPath() {
  if (process.env.ADB) return process.env.ADB;
  const home =
    process.env.ANDROID_HOME ||
    process.env.ANDROID_SDK_ROOT ||
    (process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, "Android", "Sdk") : path.join(process.env.HOME ?? "", "Android", "Sdk"));
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
  return String(adb(["shell", "pidof", PACKAGE], { allowFail: true }) ?? "").trim() !== "";
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

export function key(...codes) {
  return adb(["shell", "input", "keyevent", ...codes.map(String)]);
}
export const back = () => key(4);

/** Clears a text field: focus, move to end, delete. */
export async function clearField(testID, n = 60) {
  await tap(testID);
  await sleep(200);
  key(123, ...Array(n).fill(67));
}

/** Swipes the content up until `testID` is visible (RN ScrollView omits off-screen nodes). */
export async function scrollTo(testID, swipes = 6) {
  for (let i = 0; i <= swipes; i++) {
    const n = find(testID);
    if (n && n.cy < 2150) return n;
    adb(["shell", "input", "swipe", "540", "1700", "540", "900", "300"]);
    await sleep(600);
  }
  throw new Error(`"${testID}" not found after scrolling`);
}

/** True when the system BiometricPrompt is on screen. */
export function biometricPromptVisible(nodes = dump()) {
  return nodes.some((n) => /systemui:id\/(biometric|auth_|fingerprint)/.test(n.id) || /fingerprint sensor|Use (your )?fingerprint/i.test(`${n.text} ${n.desc}`));
}

/** Simulated fingerprint touch (emulator only): touch then lift. */
export async function fingerTap(id = 1) {
  finger(id);
  await sleep(500);
  adb(["emu", "finger", "remove", String(id)], { allowFail: true });
  await sleep(500);
}

export function fingerprintEnrolled() {
  return /"prints":\[\{[^\]]*"count":[1-9]/.test(String(adb(["shell", "dumpsys", "fingerprint"], { allowFail: true }) ?? ""));
}

export async function unlock(pin = "1111") {
  adb(["shell", "svc", "power", "stayon", "true"], { allowFail: true });
  key(224);
  await sleep(400);
  adb(["shell", "wm", "dismiss-keyguard"], { allowFail: true });
  await sleep(800);
  if (dump().some((n) => n.id === "com.android.systemui:id/keyguard_root_view" || /PIN area/.test(n.desc))) {
    adb(["shell", "input", "swipe", "540", "2000", "540", "600", "200"]);
    await sleep(800);
    adb(["shell", "input", "text", pin]);
    key(66);
    await sleep(1200);
  }
}

/**
 * Sets a PIN and enrolls fingerprint 1 through the Settings UI (Pixel AVD,
 * Android 16 strings). Idempotent; returns true when a fingerprint is enrolled.
 */
export async function enrollFingerprint(pin = "1111") {
  if (fingerprintEnrolled()) return true;
  adb(["shell", "locksettings", "set-pin", pin], { allowFail: true });
  await unlock(pin);
  adb(["shell", "am", "start", "-a", "android.settings.SECURITY_SETTINGS"]);
  await sleep(2500);
  const step = async (re, timeoutMs = 10000) => {
    const end = Date.now() + timeoutMs;
    for (;;) {
      const n = dump().find((x) => re.test(x.text));
      if (n) {
        adb(["shell", "input", "tap", String(n.cx), String(n.cy)]);
        await sleep(2000);
        return true;
      }
      if (Date.now() > end) return false;
      await sleep(700);
    }
  };
  if (!(await step(/^Device unlock$/))) return false;
  if (!(await step(/^(Pixel Imprint|Fingerprint.*)$/))) return false;
  if (dump().some((n) => /PIN/.test(n.text) || /PIN area/.test(n.desc))) {
    adb(["shell", "input", "text", pin]);
    key(66);
    await sleep(2500);
  }
  for (let i = 0; i < 3 && !dump().some((n) => /I AGREE/i.test(n.text)); i++) await step(/^MORE$/i, 3000);
  await step(/^I AGREE$/i);
  for (let i = 0; i < 20 && !fingerprintEnrolled(); i++) await fingerTap(1);
  await step(/^DONE$/i, 3000);
  key(3);
  return fingerprintEnrolled();
}

/** The app's posted notifications parsed from `dumpsys notification`: [{tag, title, text}]. */
export function appNotifications() {
  const out = String(adb(["shell", "dumpsys", "notification", "--noredact"], { allowFail: true }) ?? "");
  return out
    .split(/(?=\n\s*NotificationRecord\()/)
    .filter((b) => b.includes(`pkg=${PACKAGE} `))
    .map((b) => ({
      tag: /\btag=(\S+)/.exec(b)?.[1] ?? "",
      title: /android\.title=\S+ \(([^)]*)\)/.exec(b)?.[1] ?? "",
      text: /android\.text=\S+ \(([^)]*)\)/.exec(b)?.[1] ?? "",
    }));
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
      return console.log(JSON.stringify(appNotifications(), null, 1));
    case "unlock":
      return unlock(args[0]);
    case "enrollFinger":
      return console.log((await enrollFingerprint(args[0])) ? "enrolled" : "not enrolled");
    default:
      console.log("usage: node scripts/android-e2e.mjs install|launch|deeplink|reverse|dump|tap|tapText|type|wait|screenshot|finger|running|notifications|unlock|enrollFinger …");
      process.exitCode = cmd ? 2 : 0;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
}

