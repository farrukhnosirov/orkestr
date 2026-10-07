/** Pure helpers of the VS Code feature (unit-tested). */

/** Numeric, segment-wise version comparison (mirrors the core's `compare_versions`). */
export function compareVersions(a: string, b: string): number {
  const seg = (v: string) => v.split(".").map((s) => Number.parseInt(s, 10) || 0);
  const x = seg(a);
  const y = seg(b);
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d > 0 ? 1 : -1;
  }
  return 0;
}

/** Whether a state means a server process exists (or is about to). */
export function isServerActive(state: string): boolean {
  return state === "running" || state === "starting" || state === "restarting";
}
