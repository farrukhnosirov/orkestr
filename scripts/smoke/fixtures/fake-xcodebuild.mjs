#!/usr/bin/env node
// Fake `xcodebuild` for the simulated macOS node (scripts/smoke/fixtures/node-mac.json,
// `toolShims.xcodebuild`). Prints build-log lines like the real tool and writes a
// fake `<Scheme>.app.zip` (a valid, stored zip with Info.plist + a binary) into
// the output directory, then prints `ARTIFACT <path> sha256:<hex>` so the
// acceptance script can compare it with the blob returned to the coordinator.
//
// Usage (subset of the real flags):
//   xcodebuild -version
//   xcodebuild [-scheme App] [-configuration Release] [-derivedDataPath <dir> | -archivePath <dir> | --output <dir>] [build|archive]
// Output dir fallback: $ORKESTR_JOB_OUTPUT_DIR, else ./build. `--fail` exits 65.
// `--steps N` / `--step-ms M` stretch the log (for migration tests).
// `--side-effect <action>` prints a side-effect marker line before the steps:
//   ::orkestr-side-effect {"action":"deploy","kind":"nonIdempotent","detail":"..."}
// (the worker records it in the job's side-effect log → needs_user_decision on
// unplanned migration; see node-protocol.md §7).

import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const argv = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = argv.indexOf(name);
  return i >= 0 && i + 1 < argv.length ? argv[i + 1] : fallback;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (argv.includes("-version")) {
  console.log("Xcode 16.0 (orkestr fake)\nBuild version 16A242d");
  process.exit(0);
}

const scheme = opt("-scheme", "App");
const configuration = opt("-configuration", "Release");
const outDir = resolve(opt("-derivedDataPath", opt("-archivePath", opt("--output", process.env.ORKESTR_JOB_OUTPUT_DIR ?? "build"))));
const steps = Math.max(1, Number(opt("--steps", "4")));
const stepMs = Math.max(0, Number(opt("--step-ms", "150")));
const action = argv.includes("archive") ? "archive" : "build";

// ── minimal zip writer (stored entries, CRC-32) ──────────────────────────
const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, data } of entries) {
    const n = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(n.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(n.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, n, data);
    centrals.push(central, n);
    offset += local.length + n.length + data.length;
  }
  const cd = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

console.log(`Command line invocation:\n    /usr/bin/xcodebuild ${argv.join(" ")}`);
console.log(`User defaults from command line:\n    IDEPackageSupportUseBuiltinSCM = YES`);
console.log(`=== ${action.toUpperCase()} TARGET ${scheme} OF PROJECT ${scheme} WITH CONFIGURATION ${configuration} ===`);
const sideEffect = opt("--side-effect", null);
if (sideEffect) {
  console.log(`::orkestr-side-effect ${JSON.stringify({ action: sideEffect, kind: "nonIdempotent", detail: `fake ${sideEffect} of ${scheme}` })}`);
}
for (let i = 1; i <= steps; i++) {
  console.log(`CompileSwift normal arm64 ${scheme}/Sources/File${i}.swift (in target '${scheme}' from project '${scheme}')`);
  await sleep(stepMs);
}
console.log(`Ld ${outDir}/Build/Products/${configuration}/${scheme}.app/Contents/MacOS/${scheme} normal (in target '${scheme}')`);

if (argv.includes("--fail")) {
  console.error(`error: fake build failure requested (--fail)`);
  console.log("** BUILD FAILED **");
  process.exit(65);
}

const plist = Buffer.from(
  `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleName</key><string>${scheme}</string><key>CFBundleIdentifier</key><string>dev.orkestr.fake.${scheme.toLowerCase()}</string><key>CFBundleShortVersionString</key><string>1.0</string></dict></plist>\n`,
);
const binary = createHash("sha256").update(`${scheme}:${configuration}`).digest();
const app = zip([
  { name: `${scheme}.app/Contents/Info.plist`, data: plist },
  { name: `${scheme}.app/Contents/MacOS/${scheme}`, data: binary },
]);
mkdirSync(outDir, { recursive: true });
const artifact = join(outDir, `${scheme}.app.zip`);
writeFileSync(artifact, app);
console.log(`Touch ${outDir}/Build/Products/${configuration}/${scheme}.app (in target '${scheme}')`);
console.log(`ARTIFACT ${artifact} sha256:${createHash("sha256").update(app).digest("hex")}`);
console.log(`** ${action.toUpperCase()} SUCCEEDED **`);
