/** Messages between the page and the voice AudioWorklet (`worklet.ts`). */

export type WorkletInMessage =
  | { type: "record"; on: boolean }
  | { type: "segmenting"; on: boolean }
  | { type: "budget"; cpuBudgetPct: number }
  | { type: "stop" };

export type WorkletOutMessage =
  | { type: "level"; db: number; speaking: boolean; floorDb: number }
  | { type: "segment"; data: Float32Array; frames: number; early: boolean; peakDb: number }
  | { type: "pcm"; data: Float32Array }
  | { type: "load"; pct: number; economy: boolean };
