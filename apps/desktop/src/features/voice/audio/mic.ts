/**
 * Microphone capture for voice (§33, ADR-0017): one `getUserMedia` stream →
 * `AudioContext` → the voice AudioWorklet (levels, wake-word segments,
 * command PCM) + an `AnalyserNode` for the HUD waveform. Loaded lazily (only
 * when the user turns on voice input), so the worklet bundle and the audio
 * graph cost nothing until then. Audio stays in this process: nothing here
 * sends it anywhere.
 */
import workletUrl from "../wake/worklet.ts?worker&url";
import type { WorkletInMessage, WorkletOutMessage } from "../wake/workletProtocol";

export interface AudioInputDevice {
  id: string;
  label: string;
}

export type MicErrorCode = "unsupported" | "denied" | "notFound" | "failed";

export class MicError extends Error {
  constructor(
    readonly code: MicErrorCode,
    message: string,
  ) {
    super(message);
  }
}

/** Lists audio inputs (labels are empty until mic permission was granted once). */
export async function listInputDevices(): Promise<AudioInputDevice[]> {
  if (!navigator.mediaDevices?.enumerateDevices) return [];
  const all = await navigator.mediaDevices.enumerateDevices();
  return all.filter((d) => d.kind === "audioinput" && d.deviceId !== "").map((d) => ({ id: d.deviceId, label: d.label }));
}

type Listener<T> = (v: T) => void;

export class MicCapture {
  private readonly listeners = new Set<Listener<WorkletOutMessage>>();
  private readonly deadListeners = new Set<() => void>();
  private closed = false;
  private dead = false;

  private constructor(
    private readonly ctx: AudioContext,
    private readonly stream: MediaStream,
    private readonly node: AudioWorkletNode,
    private readonly source: MediaStreamAudioSourceNode,
    readonly analyser: AnalyserNode,
  ) {
    node.port.onmessage = (e: MessageEvent<WorkletOutMessage>) => {
      for (const l of this.listeners) l(e.data);
    };
    // The device went away (unplugged, driver reset, sleep) or the audio graph stopped (ADR-0056).
    for (const track of stream.getAudioTracks()) track.addEventListener("ended", () => this.markDead());
    ctx.addEventListener("statechange", () => {
      if (this.closed) return;
      if (ctx.state === "closed") this.markDead();
      else if (ctx.state !== "running") void ctx.resume().catch(() => this.markDead());
    });
  }

  private markDead(): void {
    if (this.closed || this.dead) return;
    this.dead = true;
    for (const l of [...this.deadListeners]) l();
  }

  /** Called once when the stream ends or the audio graph cannot run any more. */
  onDead(l: () => void): () => void {
    if (this.dead) queueMicrotask(l);
    else this.deadListeners.add(l);
    return () => this.deadListeners.delete(l);
  }

  /** The stream ended or the audio graph stopped: reopen the microphone. */
  get isDead(): boolean {
    return this.dead;
  }

  static async open(opts: { deviceId?: string | null; cpuBudgetPct?: number }): Promise<MicCapture> {
    if (typeof window === "undefined" || !navigator.mediaDevices?.getUserMedia || typeof AudioWorkletNode === "undefined") {
      throw new MicError("unsupported", "microphone capture is not available in this environment");
    }
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          ...(opts.deviceId ? { deviceId: { exact: opts.deviceId } } : {}),
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
    } catch (e) {
      const name = e instanceof DOMException ? e.name : "";
      if (name === "NotAllowedError" || name === "SecurityError") throw new MicError("denied", String(e));
      if (name === "NotFoundError" || name === "OverconstrainedError") throw new MicError("notFound", String(e));
      throw new MicError("failed", String(e));
    }
    const ctx = new AudioContext({ latencyHint: "interactive" });
    try {
      await ctx.audioWorklet.addModule(workletUrl);
      const source = ctx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(ctx, "orkestr-voice", {
        numberOfInputs: 1,
        numberOfOutputs: 0,
        channelCount: 1,
        channelCountMode: "explicit",
        processorOptions: { cpuBudgetPct: opts.cpuBudgetPct ?? 2 },
      });
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0.6;
      source.connect(node);
      source.connect(analyser);
      if (ctx.state === "suspended") await ctx.resume().catch(() => {});
      return new MicCapture(ctx, stream, node, source, analyser);
    } catch (e) {
      stream.getTracks().forEach((t) => t.stop());
      void ctx.close().catch(() => {});
      throw new MicError("failed", String(e));
    }
  }

  /** The device actually in use. */
  get deviceLabel(): string {
    return this.stream.getAudioTracks()[0]?.label ?? "";
  }

  get isClosed(): boolean {
    return this.closed;
  }

  subscribe(l: Listener<WorkletOutMessage>): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  send(msg: WorkletInMessage): void {
    if (!this.closed) this.node.port.postMessage(msg);
  }

  setRecording(on: boolean): void {
    this.send({ type: "record", on });
  }

  setSegmenting(on: boolean): void {
    this.send({ type: "segmenting", on });
  }

  setBudget(cpuBudgetPct: number): void {
    this.send({ type: "budget", cpuBudgetPct });
  }

  /** Stops the stream (the OS mic indicator goes off) and tears the graph down. */
  close(): void {
    if (this.closed) return;
    this.send({ type: "stop" });
    this.closed = true;
    this.listeners.clear();
    this.deadListeners.clear();
    try {
      this.source.disconnect();
      this.node.disconnect();
      this.analyser.disconnect();
    } catch {
      /* already disconnected */
    }
    this.stream.getTracks().forEach((t) => t.stop());
    void this.ctx.close().catch(() => {});
  }
}
