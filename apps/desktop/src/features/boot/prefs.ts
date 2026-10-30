/**
 * Startup animation preferences and the environment it needs before the core
 * connects (ADR-0053). Everything here is per device in local storage: the
 * boot sequence starts before any RPC is possible, so it cannot ask the core.
 *
 * - `orkestr.boot.prefs`: the three user settings (animation / sound / voice).
 * - `orkestr.boot.env`: a mirror of what the core knows (sound settings, quiet
 *   hours, Do Not Disturb, performance mode, Jarvis form of address), written
 *   while the app runs and read on the next start.
 * - `orkestr.boot.lastVersion`: the version that last finished a start; a
 *   different (or missing) one means «first launch after install / update».
 */
import type { JarvisAddress, PerformanceMode, QuietHours } from "@orkestr/protocol";

/** When something plays: on every start, only after install / update, never. */
export type BootWhen = "always" | "firstLaunch" | "off";

export interface BootPrefs {
  animation: BootWhen;
  sound: BootWhen;
  /** Spoken greeting through the TTS (needs spoken replies on). */
  voice: boolean;
}

export const DEFAULT_BOOT_PREFS: BootPrefs = { animation: "always", sound: "firstLaunch", voice: false };

export interface BootEnv {
  soundEnabled: boolean;
  volumePct: number;
  muteInDnd: boolean;
  doNotDisturb: boolean;
  quietHours: QuietHours | null;
  performanceMode: PerformanceMode;
  address: JarvisAddress;
  userName: string;
  persona: boolean;
}

export const DEFAULT_BOOT_ENV: BootEnv = {
  soundEnabled: true,
  volumePct: 60,
  muteInDnd: true,
  doNotDisturb: false,
  quietHours: null,
  performanceMode: "Balanced",
  address: "Sir",
  userName: "",
  persona: true,
};

export const PREFS_KEY = "orkestr.boot.prefs";
export const ENV_KEY = "orkestr.boot.env";
export const LAST_VERSION_KEY = "orkestr.boot.lastVersion";

const WHEN: readonly BootWhen[] = ["always", "firstLaunch", "off"];

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function readJson(key: string): Record<string, unknown> | null {
  try {
    const raw = storage()?.getItem(key);
    if (!raw) return null;
    const v: unknown = JSON.parse(raw);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    storage()?.setItem(key, JSON.stringify(value));
  } catch {
    /* storage unavailable: lasts for this session */
  }
}

const when = (v: unknown, fallback: BootWhen): BootWhen => (WHEN.includes(v as BootWhen) ? (v as BootWhen) : fallback);

export function readBootPrefs(): BootPrefs {
  const raw = readJson(PREFS_KEY);
  if (!raw) return { ...DEFAULT_BOOT_PREFS };
  return {
    animation: when(raw.animation, DEFAULT_BOOT_PREFS.animation),
    sound: when(raw.sound, DEFAULT_BOOT_PREFS.sound),
    voice: typeof raw.voice === "boolean" ? raw.voice : DEFAULT_BOOT_PREFS.voice,
  };
}

export function writeBootPrefs(p: BootPrefs): void {
  writeJson(PREFS_KEY, p);
}

const PERF: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];
const ADDR: readonly JarvisAddress[] = ["Sir", "Boss", "Name", "None"];

export function readBootEnv(): BootEnv {
  const raw = readJson(ENV_KEY);
  if (!raw) return { ...DEFAULT_BOOT_ENV };
  const d = DEFAULT_BOOT_ENV;
  const q = raw.quietHours as QuietHours | null | undefined;
  return {
    soundEnabled: typeof raw.soundEnabled === "boolean" ? raw.soundEnabled : d.soundEnabled,
    volumePct: typeof raw.volumePct === "number" && Number.isFinite(raw.volumePct) ? Math.min(100, Math.max(0, raw.volumePct)) : d.volumePct,
    muteInDnd: typeof raw.muteInDnd === "boolean" ? raw.muteInDnd : d.muteInDnd,
    doNotDisturb: raw.doNotDisturb === true,
    quietHours: q && typeof q === "object" && typeof q.start === "string" && typeof q.end === "string" ? { enabled: q.enabled === true, start: q.start, end: q.end } : null,
    performanceMode: PERF.includes(raw.performanceMode as PerformanceMode) ? (raw.performanceMode as PerformanceMode) : d.performanceMode,
    address: ADDR.includes(raw.address as JarvisAddress) ? (raw.address as JarvisAddress) : d.address,
    userName: typeof raw.userName === "string" ? raw.userName.slice(0, 64) : "",
    persona: typeof raw.persona === "boolean" ? raw.persona : d.persona,
  };
}

/** Merges a partial snapshot into the stored environment. */
export function writeBootEnv(patch: Partial<BootEnv>): void {
  writeJson(ENV_KEY, { ...readBootEnv(), ...patch });
}

export function readLastVersion(): string | null {
  try {
    return storage()?.getItem(LAST_VERSION_KEY) ?? null;
  } catch {
    return null;
  }
}

export function writeLastVersion(version: string): void {
  try {
    storage()?.setItem(LAST_VERSION_KEY, version);
  } catch {
    /* storage unavailable */
  }
}

/** How the core's startup check (`app-version.json`) classified this start. */
export type StartKindName = "firstRun" | "same" | "upgraded" | "downgraded";

/**
 * First launch after install or update: no version seen on this device yet,
 * a different one, or the core's `app-version.json` check says so.
 */
export function isFirstLaunch(current: string, lastSeen: string | null, startKind: StartKindName | null = null): boolean {
  if (startKind && startKind !== "same") return true;
  return !lastSeen || lastSeen !== current;
}

/** `"1.3.0"` → `"1.3"` (shown as `v1.3` in the HUD). */
export function shortVersion(version: string): string {
  const m = /^(\d+)\.(\d+)/.exec(version);
  return m ? `${m[1]}.${m[2]}` : version;
}
