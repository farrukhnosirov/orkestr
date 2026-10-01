/** Exponential backoff with jitter for reconnects. */
export interface BackoffOptions {
  /** First delay in ms (default 250). */
  initialDelayMs?: number;
  /** Max delay in ms (default 10 000). */
  maxDelayMs?: number;
  /** Growth factor (default 2). */
  factor?: number;
  /** Jitter ratio 0..1 (default 0.2 → ±20 %). */
  jitter?: number;
}

export function backoffDelay(attempt: number, opts: BackoffOptions = {}, random: () => number = Math.random): number {
  const initial = opts.initialDelayMs ?? 250;
  const max = opts.maxDelayMs ?? 10_000;
  const factor = opts.factor ?? 2;
  const jitter = Math.min(Math.max(opts.jitter ?? 0.2, 0), 1);
  const base = Math.min(max, initial * Math.pow(factor, Math.max(0, attempt)));
  const spread = base * jitter;
  return Math.max(0, Math.round(base - spread + random() * 2 * spread));
}
