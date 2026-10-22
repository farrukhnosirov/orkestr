// Builds signed `.orkp` plugin packages in Node (ADR-0034 package format, as
// crates/plugins/src/{package,signing}.rs): a zip (stored entries) with
// `orkestr-plugin.json`, payload files and `signature.json` = ed25519 over
// "orkestr-package-signature-v1\n" + content digest, where the content digest
// is sha256("orkestr-plugin-content-v1\n" + Σ(path "\n" sha256hex(bytes) "\n"))
// over every file except `signature.json`, in byte order of the paths.
//
// Used by the Phase 7 acceptance run for packages the Rust fixture generator
// does not ship (widgets with forbidden data bindings, deferred plugin kinds,
// a JS entry). They are signed with the same public, deterministic dev key as
// `cargo run -p orkestr-plugins --example make-fixtures` (never trust it
// outside tests).

import crypto from "node:crypto";

const CONTENT_PREFIX = "orkestr-plugin-content-v1\n";
const SIGNATURE_PREFIX = "orkestr-package-signature-v1\n";
const PKCS8_ED25519 = Buffer.from("302e020100300506032b657004220420", "hex");

const sha256 = (b) => crypto.createHash("sha256").update(b).digest();
const hex = (b) => Buffer.from(b).toString("hex");

/** An ed25519 key from a 32-byte seed: {privateKey, publicRaw (32 bytes), publicB64, keyId, fingerprint}. */
export function keyFromSeed(seed) {
  const privateKey = crypto.createPrivateKey({ key: Buffer.concat([PKCS8_ED25519, Buffer.from(seed)]), format: "der", type: "pkcs8" });
  const spki = crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" });
  const publicRaw = Buffer.from(spki.subarray(spki.length - 32));
  const fp = hex(sha256(publicRaw));
  return { privateKey, publicRaw, publicB64: publicRaw.toString("base64"), keyId: fp.slice(0, 16), fingerprint: fp };
}

/** The public dev publisher key of the plugin fixtures (seed = sha256 of a public string). */
export const devKey = () => keyFromSeed(sha256(Buffer.from("orkestr-dev-plugin-publisher-key-v1")));

/** A fresh random ed25519 key (marketplace publisher tests). */
export const randomKey = () => keyFromSeed(crypto.randomBytes(32));

/** Detached ed25519 signature (base64). */
export const sign = (key, message) => crypto.sign(null, Buffer.from(message), key.privateKey).toString("base64");

// ── zip (stored) ─────────────────────────────────────────────────────────
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** A zip archive with stored (uncompressed) entries: [[name, Buffer|string], …]. */
export function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, content] of entries) {
    const data = Buffer.isBuffer(content) ? content : Buffer.from(content);
    const fname = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4); // version needed
    local.writeUInt16LE(0x0800, 6); // utf-8 names
    local.writeUInt16LE(0, 8); // stored
    local.writeUInt32LE(0, 10); // time/date
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(fname.length, 26);
    local.writeUInt16LE(0, 28);
    locals.push(local, fname, data);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4); // made by
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt32LE(0, 12);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(fname.length, 28);
    central.writeUInt16LE(0, 30); // extra
    central.writeUInt16LE(0, 32); // comment
    central.writeUInt16LE(0, 34); // disk
    central.writeUInt16LE(0, 36); // internal attrs
    central.writeUInt32LE(0, 38); // external attrs (regular file)
    central.writeUInt32LE(offset, 42);
    centrals.push(central, fname);
    offset += local.length + fname.length + data.length;
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

/** Content digest (hex) of the package files (map path → Buffer). */
export function contentDigest(files) {
  const h = crypto.createHash("sha256");
  h.update(CONTENT_PREFIX);
  const paths = Object.keys(files)
    .filter((p) => p !== "signature.json")
    .sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  for (const p of paths) {
    h.update(p);
    h.update("\n");
    h.update(hex(sha256(files[p])));
    h.update("\n");
  }
  return h.digest("hex");
}

/**
 * A signed `.orkp`: `manifest` (PluginManifest JSON, camelCase), `files`
 * (path → string|Buffer payload), `key` (default: the dev key; null = unsigned).
 */
export function buildPackage(manifest, files = {}, key = devKey()) {
  const all = { "orkestr-plugin.json": Buffer.from(JSON.stringify(manifest, null, 2)) };
  for (const [p, c] of Object.entries(files)) all[p] = Buffer.isBuffer(c) ? c : Buffer.from(c);
  if (key) {
    const digest = contentDigest(all);
    all["signature.json"] = Buffer.from(JSON.stringify({ keyId: key.keyId, publicKey: key.publicB64, signature: sign(key, SIGNATURE_PREFIX + digest) }, null, 2));
  }
  return zip(Object.entries(all));
}

/** A declarative widget-only manifest (no code, no grants) bound to `methods`. */
export function widgetManifest(id, title, methods, events = []) {
  return {
    id,
    name: title,
    version: "1.0.0",
    publisher: "Orkestr Dev",
    description: `Widget bound to ${methods.join(", ")}.`,
    abi: 1,
    kinds: ["Widget"],
    permissions: [],
    tools: [],
    widgets: [
      {
        id: "w",
        title,
        entryHtml: "widgets/w.html",
        sizes: [{ w: 3, h: 2 }],
        dataBinding: { methods, events },
        configSchema: { type: "object", properties: {} },
      },
    ],
    deployProviders: [],
    agentTemplates: [],
    subscribes: [],
    limits: { memoryMb: 64, fuelPerCall: 1000000000, timeoutMs: 5000 },
  };
}

export const WIDGET_HTML = `<!doctype html><html><body><p id="t">widget</p><script>parent.postMessage({type:"ready"},"*");</script></body></html>`;
