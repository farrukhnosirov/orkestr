#!/usr/bin/env node
// Renders the Orkestr app icon (1024×1024 PNG) without external dependencies:
// a deep-space rounded square with a glowing cyan→violet orbit and a core.
// `npm run icons` then runs `tauri icon` to derive every platform size.
//
// Output: apps/desktop/src-tauri/icons/source.png

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const SIZE = 1024;
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "apps", "desktop", "src-tauri", "icons", "source.png");

const clamp = (v, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
const mix = (a, b, t) => a + (b - a) * t;
const smooth = (e0, e1, x) => {
  const t = clamp((x - e0) / (e1 - e0));
  return t * t * (3 - 2 * t);
};

const CYAN = [34, 211, 238];
const VIOLET = [167, 139, 250];
const BLUE = [109, 139, 255];

function sdRoundedSquare(x, y, half, r) {
  const qx = Math.abs(x) - half + r;
  const qy = Math.abs(y) - half + r;
  const ox = Math.max(qx, 0);
  const oy = Math.max(qy, 0);
  return Math.hypot(ox, oy) + Math.min(Math.max(qx, qy), 0) - r;
}

function shade(px, py) {
  // Normalized coordinates centered at 0, range ~[-1, 1].
  const x = (px + 0.5 - SIZE / 2) / (SIZE / 2);
  const y = (py + 0.5 - SIZE / 2) / (SIZE / 2);
  const aa = 2 / SIZE;

  // Rounded-square mask.
  const d = sdRoundedSquare(x, y, 0.9, 0.38);
  const mask = 1 - smooth(-aa, aa, d);
  if (mask <= 0) return [0, 0, 0, 0];

  // Background: deep space with two nebula glows.
  const g1 = Math.exp(-((x + 0.55) ** 2 + (y + 0.6) ** 2) * 2.2);
  const g2 = Math.exp(-((x - 0.6) ** 2 + (y - 0.45) ** 2) * 2.6);
  let r = 6 + 70 * g1 + 10 * g2;
  let g = 8 + 40 * g1 + 90 * g2;
  let b = 22 + 150 * g1 + 120 * g2;

  // Stars (deterministic hash).
  const h = Math.sin(px * 12.9898 + py * 78.233) * 43758.5453;
  const star = h - Math.floor(h);
  if (star > 0.9965) {
    const s = (star - 0.9965) / 0.0035;
    r += 140 * s;
    g += 150 * s;
    b += 170 * s;
  }

  // Orbit ring (tilted ellipse) with angular cyan→violet gradient.
  const angle = -0.5;
  const rx = x * Math.cos(angle) - y * Math.sin(angle);
  const ry = (x * Math.sin(angle) + y * Math.cos(angle)) / 0.42;
  const ringDist = Math.abs(Math.hypot(rx, ry) - 0.62);
  const ring = 1 - smooth(0.018, 0.04, ringDist);
  const ringGlow = Math.exp(-ringDist * ringDist * 220) * 0.55;
  // Periodic (seamless) cyan → blue → violet → blue → cyan gradient around the orbit.
  const t = (1 - Math.cos(Math.atan2(ry, rx) + 0.6)) / 2;
  const rc = t < 0.5 ? CYAN.map((c, i) => mix(c, BLUE[i], t * 2)) : BLUE.map((c, i) => mix(c, VIOLET[i], (t - 0.5) * 2));
  const ringA = clamp(ring + ringGlow);
  r = mix(r, rc[0], ringA);
  g = mix(g, rc[1], ringA);
  b = mix(b, rc[2], ringA);

  // Core: bright sphere with a cyan halo.
  const cd = Math.hypot(x, y);
  const core = 1 - smooth(0.17, 0.19, cd);
  const halo = Math.exp(-cd * cd * 18) * 0.85;
  const coreCol = [mix(225, 120, cd / 0.19), mix(250, 220, cd / 0.19), 255];
  r = mix(r, CYAN[0], halo * 0.6);
  g = mix(g, CYAN[1], halo * 0.6);
  b = mix(b, CYAN[2], halo * 0.6);
  r = mix(r, coreCol[0], core);
  g = mix(g, coreCol[1], core);
  b = mix(b, coreCol[2], core);

  // Satellite on the orbit.
  const sx = 0.62 * Math.cos(0.9);
  const sy = 0.62 * 0.42 * Math.sin(0.9);
  const satX = sx * Math.cos(-angle) - sy * Math.sin(-angle);
  const satY = sx * Math.sin(-angle) + sy * Math.cos(-angle);
  const sd = Math.hypot(x - satX, y - satY);
  const sat = 1 - smooth(0.05, 0.065, sd);
  r = mix(r, VIOLET[0] + 40, sat);
  g = mix(g, VIOLET[1] + 40, sat);
  b = mix(b, 255, sat);

  // Subtle inner border highlight.
  const edge = Math.exp(-((d + 0.012) ** 2) * 40000) * 0.35;
  r = mix(r, 150, edge);
  g = mix(g, 170, edge);
  b = mix(b, 255, edge);

  return [clamp(r, 0, 255), clamp(g, 0, 255), clamp(b, 0, 255), 255 * mask];
}

function crc32(buf) {
  let c;
  const table = crc32.table ?? (crc32.table = Array.from({ length: 256 }, (_, n) => {
    c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  }));
  let crc = 0xffffffff;
  for (const byte of buf) crc = table[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

const raw = Buffer.alloc((SIZE * 4 + 1) * SIZE);
for (let y = 0; y < SIZE; y++) {
  const row = y * (SIZE * 4 + 1);
  raw[row] = 0;
  for (let x = 0; x < SIZE; x++) {
    const [r, g, b, a] = shade(x, y);
    const i = row + 1 + x * 4;
    raw[i] = Math.round(r);
    raw[i + 1] = Math.round(g);
    raw[i + 2] = Math.round(b);
    raw[i + 3] = Math.round(a);
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, png);
process.stdout.write(`[gen-icon] wrote ${out} (${png.length} bytes)\n`);
