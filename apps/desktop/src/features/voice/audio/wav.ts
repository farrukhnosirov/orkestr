/**
 * PCM helpers for cloud speech recognition (only used after push-to-talk or
 * the wake word, with consent): concatenation, trimming and 16-bit WAV
 * encoding. Pure, unit-tested.
 */

/** Concatenates PCM chunks, keeping at most `maxSamples` (the newest are dropped). */
export function concatPcm(chunks: readonly Float32Array[], maxSamples = Number.POSITIVE_INFINITY): Float32Array {
  const total = Math.min(
    maxSamples,
    chunks.reduce((n, c) => n + c.length, 0),
  );
  const out = new Float32Array(total);
  let off = 0;
  for (const c of chunks) {
    if (off >= total) break;
    const take = Math.min(c.length, total - off);
    out.set(c.subarray(0, take), off);
    off += take;
  }
  return out;
}

/** Peak absolute amplitude. */
export function peak(pcm: Float32Array): number {
  let p = 0;
  for (let i = 0; i < pcm.length; i++) p = Math.max(p, Math.abs(pcm[i] ?? 0));
  return p;
}

/** Encodes mono float PCM (−1..1) as a 16-bit little-endian WAV file. */
export function encodeWav(pcm: Float32Array, sampleRate: number): Uint8Array {
  const dataBytes = pcm.length * 2;
  const buf = new ArrayBuffer(44 + dataBytes);
  const v = new DataView(buf);
  const str = (off: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(off + i, s.charCodeAt(i));
  };
  str(0, "RIFF");
  v.setUint32(4, 36 + dataBytes, true);
  str(8, "WAVE");
  str(12, "fmt ");
  v.setUint32(16, 16, true); // fmt chunk size
  v.setUint16(20, 1, true); // PCM
  v.setUint16(22, 1, true); // mono
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * 2, true); // byte rate
  v.setUint16(32, 2, true); // block align
  v.setUint16(34, 16, true); // bits per sample
  str(36, "data");
  v.setUint32(40, dataBytes, true);
  for (let i = 0; i < pcm.length; i++) {
    const s = Math.max(-1, Math.min(1, pcm[i] ?? 0));
    v.setInt16(44 + i * 2, s < 0 ? Math.round(s * 0x8000) : Math.round(s * 0x7fff), true);
  }
  return new Uint8Array(buf);
}

/** Base64 of bytes (chunked; no stack blow-up on large buffers). */
export function toBase64(bytes: Uint8Array): string {
  let bin = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) bin += String.fromCharCode(...bytes.subarray(i, i + step));
  return btoa(bin);
}

/** Bytes of base64. */
export function fromBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
