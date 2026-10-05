/**
 * AudioWorklet processor of the on-device voice pipeline (ADR-0017).
 * Bundled by Vite as a separate script (`?worker&url`) and loaded with
 * `audioWorklet.addModule`. Everything here runs on the audio rendering
 * thread; it never sends audio anywhere — it posts levels, MFCC segments
 * (wake-word spotting / enrollment) and, only while the main thread asked
 * for a command recording, 16 kHz PCM back to the page.
 */
import { AudioFrontEnd } from "./frontend";
import type { WorkletInMessage, WorkletOutMessage } from "./workletProtocol";

// Minimal declarations of the AudioWorkletGlobalScope (not in lib.dom).
declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: unknown);
}
declare function registerProcessor(name: string, ctor: new (options?: unknown) => AudioWorkletProcessor): void;

const clock: () => number =
  typeof globalThis.performance?.now === "function" ? () => globalThis.performance.now() : () => Date.now();

class OrkestrVoiceProcessor extends AudioWorkletProcessor {
  private readonly fe: AudioFrontEnd;
  private running = true;
  private lastLoadPost = 0;

  constructor(options?: unknown) {
    super(options);
    const opts = (options as { processorOptions?: { cpuBudgetPct?: number } } | undefined)?.processorOptions;
    this.fe = new AudioFrontEnd({ inputRate: sampleRate, cpuBudgetPct: opts?.cpuBudgetPct ?? 2, now: clock });
    this.port.onmessage = (e: MessageEvent<WorkletInMessage>) => this.onMessage(e.data);
  }

  private post(msg: WorkletOutMessage, transfer: Transferable[] = []): void {
    this.port.postMessage(msg, transfer);
  }

  private onMessage(msg: WorkletInMessage): void {
    switch (msg.type) {
      case "record":
        this.fe.recording = msg.on;
        break;
      case "segmenting":
        this.fe.segmenting = msg.on;
        if (!msg.on) this.fe.reset();
        break;
      case "budget":
        this.fe.setBudget(msg.cpuBudgetPct);
        break;
      case "stop":
        this.running = false;
        break;
    }
  }

  process(inputs: Float32Array[][]): boolean {
    if (!this.running) return false;
    const ch = inputs[0]?.[0];
    if (!ch || ch.length === 0) return true;
    const out = this.fe.process(ch);
    for (const level of out.levels) this.post({ type: "level", ...level });
    for (const seg of out.segments) {
      this.post({ type: "segment", data: seg.data, frames: seg.frames, early: seg.early, peakDb: seg.peakDb }, [seg.data.buffer]);
    }
    if (out.pcm && out.pcm.length > 0) this.post({ type: "pcm", data: out.pcm }, [out.pcm.buffer]);
    const now = clock();
    if (now - this.lastLoadPost > 2000) {
      this.lastLoadPost = now;
      this.post({ type: "load", pct: this.fe.loadPct, economy: this.fe.economy });
    }
    return true;
  }
}

registerProcessor("orkestr-voice", OrkestrVoiceProcessor);
