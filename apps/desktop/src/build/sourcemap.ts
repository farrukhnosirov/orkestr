/**
 * Decides whether `vite build` emits source maps.
 *
 * Tauri embeds every file under `frontendDist` (../dist) into the executable,
 * so `.map` files written there would ship inside the release binary and
 * installer (~50 MB of maps, plus the full TypeScript source). Maps are
 * therefore emitted only for debug builds:
 *
 * - `TAURI_ENV_DEBUG === "true"`: set by the Tauri CLI for `tauri build --debug`
 *   (and `tauri dev`). Tauri sets it to the string `"false"` for release
 *   builds, so a plain truthiness check would be wrong.
 * - `ORKESTR_SOURCEMAP=1|true`: explicit opt-in for local debugging of a
 *   production bundle outside Tauri.
 */
export function shouldEmitSourcemap(env: Record<string, string | undefined>): boolean {
  const truthy = (v: string | undefined): boolean => {
    const s = v?.trim().toLowerCase();
    return s === "true" || s === "1";
  };
  return truthy(env.TAURI_ENV_DEBUG) || truthy(env.ORKESTR_SOURCEMAP);
}
