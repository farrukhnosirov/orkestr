/**
 * Barge-in spotter client (ADR-0053): while the assistant speaks or works,
 * streams the shared microphone's 16 kHz PCM (the one `MicCapture`, echo
 * cancellation + noise suppression on) to the local core in ~100 ms chunks
 * (`voice.bargeFeed`), where a Vosk grammar recognizer restricted to the
 * stop phrases spots «стоп», «хватит», «замолчи»… without the wake word.
 * Audio goes only to the local core process and is dropped after each chunk.
 * Keeps the last 0.8 s of PCM as pre-roll for the follow-up command
 * («стоп, открой ютуб»).
 */
import type { BargeHit, BargeStartResult } from "@orkestr/protocol";

import type { MicCapture } from "../audio/mic";
import { toBase64 } from "../audio/wav";

/** Chunk sent per call: 100 ms at 16 kHz. */
export const BARGE_CHUNK_SAMPLES = 1600;
/** At most this much audio waits while a call is in flight (older audio is dropped). */
const MAX_BACKLOG_SAMPLES = 16_000;
/** Pre-roll kept for the follow-up command, samples (0.8 s: the stop phrase and what follows it). */
export const PREROLL_SAMPLES = 12_800;

/** The calls the spotter makes (the core client in the app; fakes in tests). */
export interface BargeTransport {
  start(): Promise<BargeStartResult>;
  feed(sessionId: string, pcmBase64: string, spoken: string | null): Promise<{ hit: BargeHit | null }>;
  stop(sessionId: string): Promise<void>;
}

/** Float32 [-1, 1] → 16-bit little-endian PCM bytes. */
export function floatToPcm16(f: Float32Array): Uint8Array {
  const out = new Uint8Array(f.length * 2);
  const view = new DataView(out.buffer);
  for (let i = 0; i < f.length; i++) {
    const v = Math.max(-1, Math.min(1, f[i] ?? 0));
    view.setInt16(i * 2, Math.round(v < 0 ? v * 0x8000 : v * 0x7fff), true);
  }
  return out;
}

export interface SpotterOptions {
  transport: BargeTransport;
  mic: MicCapture;
  /** What the assistant is saying right now (self-echo protection). */
  spoken: () => string | null;
  onHit: (hit: BargeHit, preroll: Float32Array[]) => void;
  /** Turns PCM recording off when the spotter stops (default: `mic.setRecording(false)`). */
  releaseRecording?: () => void;
}

/** One running spotter; `stop()` releases everything. */
export class BargeSpotterClient {
  private sessionId: string | null = null;
  private stopped = false;
  private pending: Float32Array[] = [];
  private pendingLen = 0;
  private inFlight = false;
  private readonly preroll: Float32Array[] = [];
  private prerollLen = 0;
  private off: (() => void) | null = null;
  engine: BargeStartResult["engine"] = null;

  private constructor(private readonly o: SpotterOptions) {}

  /** Starts a session; `null` when barge-in is off or no offline engine can spot phrases. */
  static async start(o: SpotterOptions): Promise<BargeSpotterClient | null> {
    const r = await o.transport.start();
    if (!r.sessionId) return null;
    const s = new BargeSpotterClient(o);
    s.sessionId = r.sessionId;
    s.engine = r.engine;
    s.off = o.mic.subscribe((m) => {
      if (m.type === "pcm") s.push(m.data);
    });
    o.mic.setRecording(true);
    return s;
  }

  get running(): boolean {
    return !this.stopped;
  }

  /** Exposed for tests: queue PCM as if it came from the worklet. */
  push(data: Float32Array): void {
    if (this.stopped) return;
    this.preroll.push(data);
    this.prerollLen += data.length;
    while (this.prerollLen - (this.preroll[0]?.length ?? 0) >= PREROLL_SAMPLES) this.prerollLen -= this.preroll.shift()?.length ?? 0;
    this.pending.push(data);
    this.pendingLen += data.length;
    while (this.pendingLen > MAX_BACKLOG_SAMPLES) this.pendingLen -= this.pending.shift()?.length ?? 0;
    void this.pump();
  }

  private take(): Float32Array | null {
    if (this.pendingLen < BARGE_CHUNK_SAMPLES) return null;
    const out = new Float32Array(this.pendingLen);
    let at = 0;
    for (const c of this.pending) {
      out.set(c, at);
      at += c.length;
    }
    this.pending = [];
    this.pendingLen = 0;
    return out;
  }

  private async pump(): Promise<void> {
    if (this.inFlight || this.stopped || !this.sessionId) return;
    const chunk = this.take();
    if (!chunk) return;
    this.inFlight = true;
    try {
      const r = await this.o.transport.feed(this.sessionId, toBase64(floatToPcm16(chunk)), this.o.spoken());
      if (r.hit && !this.stopped) {
        const pre = [...this.preroll];
        this.stop();
        this.o.onHit(r.hit, pre);
        return;
      }
    } catch {
      // Session expired / core restarted: stop quietly (the next speech starts a new one).
      this.stop();
      return;
    } finally {
      this.inFlight = false;
    }
    void this.pump();
  }

  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    this.off?.();
    this.off = null;
    if (this.o.releaseRecording) this.o.releaseRecording();
    else this.o.mic.setRecording(false);
    this.pending = [];
    this.preroll.length = 0;
    const id = this.sessionId;
    this.sessionId = null;
    if (id) void this.o.transport.stop(id).catch(() => undefined);
  }
}
