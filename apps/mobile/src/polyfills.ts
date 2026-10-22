// Hermes gaps used by @orkestr/client: crypto.getRandomValues, TextDecoder.
import { getRandomValues } from "expo-crypto";

const g = globalThis as { crypto?: Partial<Crypto>; TextDecoder?: unknown };
if (!g.crypto) g.crypto = {};
if (typeof g.crypto.getRandomValues !== "function") {
  g.crypto.getRandomValues = getRandomValues as Crypto["getRandomValues"];
}
if (typeof g.TextDecoder === "undefined") {
  // Minimal UTF-8 decoder (Hermes ships TextEncoder only).
  class Utf8Decoder {
    readonly encoding = "utf-8";
    decode(input?: ArrayBuffer | ArrayBufferView): string {
      if (!input) return "";
      const b =
        input instanceof ArrayBuffer ? new Uint8Array(input) : new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      let s = "";
      for (let i = 0; i < b.length; i++) s += "%" + (b[i] as number).toString(16).padStart(2, "0");
      return decodeURIComponent(s);
    }
  }
  g.TextDecoder = Utf8Decoder;
}
