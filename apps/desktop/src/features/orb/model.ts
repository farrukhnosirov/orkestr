/**
 * The floating orb's pure model (ADR-0045): which mood the sphere shows,
 * its badge, how much it may animate, its window box and the click-through
 * hit shape. Unit-tested in `orb.test.ts`; no React, no Tauri.
 */
import type { OrbAnimation, OrbPrefs, PerformanceMode } from "@orkestr/protocol";

import { resolvePolicy } from "../performance/policy";

/** What the sphere expresses, most urgent first. */
export type OrbMood = "offline" | "listening" | "speaking" | "stopped" | "approval" | "error" | "working" | "idle";

export interface OrbSignals {
  /** Connected to the core. */
  connected: boolean;
  /** Agents / sessions actively working. */
  working: number;
  /** Pending approvals. */
  approvals: number;
  /** Agents / sessions in error that the user has not seen in the hub yet. */
  errors: number;
  /** Voice phase of the main window (`idle`, `listening`, `transcribing`, `thinking`, …; null = unknown). */
  voicePhase: string | null;
  /** The assistant is speaking (TTS). */
  speaking: boolean;
  /** Emergency stop is engaged. */
  allStopped: boolean;
}

export const IDLE_SIGNALS: OrbSignals = {
  connected: true,
  working: 0,
  approvals: 0,
  errors: 0,
  voicePhase: null,
  speaking: false,
  allStopped: false,
};

/** The mood for the signals. Voice wins while it runs (the user is talking to Orkestr). */
export function orbMood(s: OrbSignals): OrbMood {
  if (!s.connected) return "offline";
  if (s.voicePhase === "listening" || s.voicePhase === "transcribing") return "listening";
  if (s.speaking) return "speaking";
  if (s.allStopped) return "stopped";
  if (s.approvals > 0) return "approval";
  if (s.errors > 0) return "error";
  if (s.working > 0) return "working";
  return "idle";
}

export type BadgeTone = "accent" | "warning" | "danger";

export interface OrbBadge {
  count: number;
  tone: BadgeTone;
}

/** The count badge: approvals first, then errors, then working agents (none when 0 or switched off). */
export function orbBadge(s: OrbSignals, enabled: boolean): OrbBadge | null {
  if (!enabled || !s.connected) return null;
  if (s.approvals > 0) return { count: s.approvals, tone: "warning" };
  if (s.errors > 0) return { count: s.errors, tone: "danger" };
  if (s.working > 0) return { count: s.working, tone: "accent" };
  return null;
}

/** Badge text (`99+` above 99). */
export function badgeText(count: number): string {
  return count > 99 ? "99+" : String(Math.max(0, Math.floor(count)));
}

/** How much the sphere animates. */
export type OrbMotion = "full" | "reduced" | "static";

export interface MotionInput {
  /** The user's animation level (orb settings). */
  pref: OrbAnimation;
  /** The effective Performance Mode (main window's, after auto-switching). */
  mode: PerformanceMode;
  /** `prefers-reduced-motion: reduce`. */
  reducedMotion: boolean;
  /** The orb window is hidden. */
  hidden: boolean;
  /** An Orkestr window has the focus (the user is working in Orkestr). */
  appActive: boolean;
  /** The cursor is over the orb. */
  hovered: boolean;
  /** The hub is open. */
  hubOpen: boolean;
  /** The sphere shows voice (listening / speaking): live feedback, never frozen while allowed to move. */
  voice: boolean;
}

/**
 * The effective motion level. Static in Battery Saver, when switched off,
 * when hidden, and while the user works in another app (decorative loops
 * pause when Orkestr is unfocused, ADR-0045) unless the orb is hovered, its
 * hub is open or voice is live. Performance mode and reduced motion cap it
 * at "reduced".
 */
export function orbMotion(i: MotionInput): OrbMotion {
  if (i.pref === "Static" || i.hidden) return "static";
  const policy = resolvePolicy(i.mode, { reducedMotion: i.reducedMotion, hidden: false });
  if (policy.motion === "off") return "static";
  if (!i.appActive && !i.hovered && !i.hubOpen && !i.voice) return "static";
  if (i.pref === "Reduced" || policy.motion === "reduced") return "reduced";
  return "full";
}

/** Frame-rate cap of the sphere for a motion level (0 = one still frame). */
export function orbFps(motion: OrbMotion, mode: PerformanceMode): number {
  if (motion === "static") return 0;
  const cap = Math.min(30, resolvePolicy(mode, { reducedMotion: false, hidden: false }).robotFps || 15);
  return motion === "reduced" ? Math.min(cap, 15) : cap;
}

/** Toggles the Agent Hub (main window; the orb window handles it while focused). */
export const ORB_HUB_SHORTCUT = "Ctrl+Alt+O";

/** Room around the sphere for its glow and badge, in px. */
export const ORB_PAD = 12;

export const DEFAULT_ORB_PREFS: OrbPrefs = { enabled: true, size: 64, showBadge: true, animation: "Full" };

/** Sphere size, clamped to the supported range (48..96 px). */
export function clampOrbSize(size: number): number {
  return Number.isFinite(size) ? Math.round(Math.min(96, Math.max(48, size))) : DEFAULT_ORB_PREFS.size;
}

/** Edge of the collapsed orb window (sphere + padding). */
export function orbBoxSize(size: number): number {
  return clampOrbSize(size) + ORB_PAD * 2;
}

/** Hub flyout size (logical px). */
export const HUB_SIZE = { width: 380, height: 540 } as const;

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface HitShape {
  circles: { cx: number; cy: number; r: number }[];
  rects: Box[];
}

/**
 * The interactive area reported to the shell: the sphere (a circle — the
 * transparent corners of its box pass clicks through) plus the open hub and
 * menu boxes.
 */
export function orbHitShape(orbAt: { x: number; y: number }, size: number, boxes: readonly (Box | null)[]): HitShape {
  const s = clampOrbSize(size);
  const c = { cx: orbAt.x + ORB_PAD + s / 2, cy: orbAt.y + ORB_PAD + s / 2, r: s / 2 + 3 };
  return { circles: [c], rects: boxes.filter((b): b is Box => b !== null && b.w > 0 && b.h > 0) };
}

/** A pointer move beyond this many px turns a press into a drag. */
export const DRAG_THRESHOLD = 4;

export function isDragGesture(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) > DRAG_THRESHOLD;
}

/** Orb preferences of a document, defaults filled in. */
export function orbPrefsOf(p: Partial<OrbPrefs> | null | undefined): OrbPrefs {
  const merged = { ...DEFAULT_ORB_PREFS, ...(p ?? {}) };
  return { ...merged, size: clampOrbSize(merged.size) };
}
