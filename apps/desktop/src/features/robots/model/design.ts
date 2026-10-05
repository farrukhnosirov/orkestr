/**
 * Robot design data (§30 Robot Designer). Mirrors the Rust schema in
 * `crates/protocol/src/methods/robots.rs` (the core validates again on save):
 * skins, materials, accessories and sound presets are **data** understood by
 * the renderers, never code.
 */
import type { BubbleVerbosity, RobotDesign, RobotMaterial, RobotRenderMode, RobotsConfig } from "@orkestr/protocol";

export const SKIN_IDS = ["nebula", "ember", "mint", "chrome", "sakura", "obsidian", "sunrise", "glacier"] as const;
export type SkinId = (typeof SKIN_IDS)[number];
export const ACCESSORY_IDS = ["antenna", "headphones", "visor", "bowtie", "jetpack", "halo", "scarf"] as const;
export type AccessoryId = (typeof ACCESSORY_IDS)[number];
export const SOUND_PRESET_IDS = ["none", "soft", "retro", "scifi", "chime"] as const;
export const MATERIALS: readonly RobotMaterial[] = ["Matte", "Glossy", "Metal", "Neon"];
export const RENDER_MODES: readonly RobotRenderMode[] = ["TwoD", "ThreeD"];
export const BUBBLE_LEVELS: readonly BubbleVerbosity[] = ["Off", "Minimal", "Normal", "Chatty"];
export const MAX_ACCESSORIES = 4;
export const SCALE_RANGE = { min: 0.6, max: 1.6 } as const;

/** Colors of a skin (procedural renderers derive every shade from these). */
export interface SkinPalette {
  /** Body gradient: highlight → shade. */
  bodyLight: string;
  bodyDark: string;
  /** Trim (ears, joints, panel edges). */
  trim: string;
  /** Face screen. */
  screen: string;
  /** Default accent (eyes, antenna, glow) when the design has none. */
  accent: string;
}

export const SKINS: Readonly<Record<SkinId, SkinPalette>> = {
  nebula: { bodyLight: "#eef1ff", bodyDark: "#8e9bd6", trim: "#6d8bff", screen: "#0a0f24", accent: "#22d3ee" },
  ember: { bodyLight: "#ffe6d6", bodyDark: "#d9744a", trim: "#ff6b3d", screen: "#1a0b06", accent: "#ffb347" },
  mint: { bodyLight: "#e6fff6", bodyDark: "#6cc6a3", trim: "#2fbf8f", screen: "#06160f", accent: "#34d399" },
  chrome: { bodyLight: "#f7f9fc", bodyDark: "#87929f", trim: "#c0c8d4", screen: "#0b0d12", accent: "#93c5fd" },
  sakura: { bodyLight: "#fff2f7", bodyDark: "#e294b6", trim: "#f472b6", screen: "#1c0a14", accent: "#f9a8d4" },
  obsidian: { bodyLight: "#575d75", bodyDark: "#15172a", trim: "#7c3aed", screen: "#05050c", accent: "#a78bfa" },
  sunrise: { bodyLight: "#fff7db", bodyDark: "#efae62", trim: "#f59e0b", screen: "#1a1204", accent: "#fbbf24" },
  glacier: { bodyLight: "#f0faff", bodyDark: "#93c2e2", trim: "#38bdf8", screen: "#041420", accent: "#7dd3fc" },
};

export function isSkinId(v: string): v is SkinId {
  return (SKIN_IDS as readonly string[]).includes(v);
}

export function isAccessoryId(v: string): v is AccessoryId {
  return (ACCESSORY_IDS as readonly string[]).includes(v);
}

export function paletteOf(design: Pick<RobotDesign, "skin">): SkinPalette {
  return isSkinId(design.skin) ? SKINS[design.skin] : SKINS.nebula;
}

/** Accent of a design: its override, else the skin's. */
export function accentOf(design: Pick<RobotDesign, "skin" | "accent">): string {
  return design.accent && isHexColor(design.accent) ? design.accent : paletteOf(design).accent;
}

export function isHexColor(s: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(s);
}

export function defaultDesign(): RobotDesign {
  return {
    mode: "TwoD",
    skin: "nebula",
    material: "Glossy",
    accent: null,
    scale: 1,
    accessories: ["antenna"],
    animationIntensity: 0.8,
    voice: { enabled: false, voiceId: null },
    sounds: "soft",
    behaviour: { snap: true, autoHideIdle: false, clickThroughIdle: true, bubble: "Normal" },
  };
}

export function defaultRobotsConfig(): RobotsConfig {
  return {
    schemaVersion: 1,
    enabled: true,
    maxVisible: 6,
    lingerSeconds: 90,
    defaultDesign: defaultDesign(),
    designs: {},
    positions: {},
  };
}

/** One problem with a design (path as in the Rust validator). */
export interface DesignIssue {
  path: string;
  code: "skin" | "accent" | "scale" | "intensity" | "accessories" | "accessory" | "voice" | "sounds";
}

/** Validates a design exactly like `RobotDesign::validate_into` in Rust. */
export function validateDesign(d: RobotDesign, path = "design"): DesignIssue[] {
  const issues: DesignIssue[] = [];
  if (!isSkinId(d.skin)) issues.push({ path: `${path}.skin`, code: "skin" });
  if (d.accent !== null && !isHexColor(d.accent)) issues.push({ path: `${path}.accent`, code: "accent" });
  if (!(Number.isFinite(d.scale) && d.scale >= SCALE_RANGE.min && d.scale <= SCALE_RANGE.max)) {
    issues.push({ path: `${path}.scale`, code: "scale" });
  }
  if (!(Number.isFinite(d.animationIntensity) && d.animationIntensity >= 0 && d.animationIntensity <= 1)) {
    issues.push({ path: `${path}.animationIntensity`, code: "intensity" });
  }
  if (d.accessories.length > MAX_ACCESSORIES) issues.push({ path: `${path}.accessories`, code: "accessories" });
  d.accessories.forEach((a, i) => {
    if (!isAccessoryId(a)) issues.push({ path: `${path}.accessories.${i}`, code: "accessory" });
  });
  const v = d.voice.voiceId;
  // eslint-disable-next-line no-control-regex
  if (v !== null && (v.length === 0 || v.length > 120 || /[\u0000-\u001f]/.test(v))) {
    issues.push({ path: `${path}.voice.voiceId`, code: "voice" });
  }
  if (!(SOUND_PRESET_IDS as readonly string[]).includes(d.sounds)) issues.push({ path: `${path}.sounds`, code: "sounds" });
  return issues;
}

/**
 * Coerces untrusted / partial data into a valid design (used for previews
 * and before saving): unknown values fall back to the defaults, numbers are
 * clamped, accessories deduplicated and capped.
 */
export function normalizeDesign(input: Partial<RobotDesign> | null | undefined): RobotDesign {
  const base = defaultDesign();
  if (!input) return base;
  const clamp = (n: unknown, min: number, max: number, fallback: number) =>
    typeof n === "number" && Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
  const accessories = Array.from(new Set((input.accessories ?? base.accessories).filter(isAccessoryId))).slice(0, MAX_ACCESSORIES);
  const voiceId = input.voice?.voiceId?.trim() ?? null;
  return {
    mode: input.mode && RENDER_MODES.includes(input.mode) ? input.mode : base.mode,
    skin: input.skin && isSkinId(input.skin) ? input.skin : base.skin,
    material: input.material && MATERIALS.includes(input.material) ? input.material : base.material,
    accent: input.accent && isHexColor(input.accent) ? input.accent.toLowerCase() : null,
    scale: clamp(input.scale, SCALE_RANGE.min, SCALE_RANGE.max, base.scale),
    accessories,
    animationIntensity: clamp(input.animationIntensity, 0, 1, base.animationIntensity),
    voice: { enabled: input.voice?.enabled ?? false, voiceId: voiceId ? voiceId.slice(0, 120) : null },
    sounds: input.sounds && (SOUND_PRESET_IDS as readonly string[]).includes(input.sounds) ? input.sounds : base.sounds,
    behaviour: {
      snap: input.behaviour?.snap ?? base.behaviour.snap,
      autoHideIdle: input.behaviour?.autoHideIdle ?? base.behaviour.autoHideIdle,
      clickThroughIdle: input.behaviour?.clickThroughIdle ?? base.behaviour.clickThroughIdle,
      bubble:
        input.behaviour?.bubble && BUBBLE_LEVELS.includes(input.behaviour.bubble) ? input.behaviour.bubble : base.behaviour.bubble,
    },
  };
}

/** The design that applies to a template (its own, else the default). */
export function designForTemplate(config: RobotsConfig, templateId: string | null): RobotDesign {
  return (templateId ? config.designs[templateId] : undefined) ?? config.defaultDesign;
}

/** Toggles an accessory, respecting the cap. */
export function toggleAccessory(list: readonly string[], id: AccessoryId): string[] {
  if (list.includes(id)) return list.filter((a) => a !== id);
  if (list.length >= MAX_ACCESSORIES) return [...list];
  return [...list, id];
}
