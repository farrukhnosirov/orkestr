/**
 * Onboarding steps (§55, RR-G7). Steps 1–8 arrived in Phase 1 and voice
 * (step 9) in Phase 3, remote access and sync (10–11) in Phase 4; step 12 is
 * shown with the phase it arrives in and can only be acknowledged.
 */
import { t } from "../../i18n";

export type OnboardingStepId =
  | "profile"
  | "storage"
  | "providers"
  | "manager"
  | "permissions"
  | "project"
  | "git"
  | "terminal"
  | "voice"
  | "remote"
  | "sync"
  | "mobile";

export interface OnboardingStep {
  id: OnboardingStepId;
  /** Localized (getter over `t()`, follows language switches). */
  readonly title: string;
  readonly subtitle: string;
  /** Phase in which the step becomes interactive (1 = now). */
  phase: number;
  /** May be skipped without consequences. */
  optional: boolean;
}

/** The delivered phase: steps up to it are interactive (remote access and sync shipped in Phase 4). */
export const CURRENT_PHASE = 4;

function step(id: OnboardingStepId, phase: number, optional: boolean): OnboardingStep {
  return {
    id,
    phase,
    optional,
    get title() {
      return t(`onboarding:steps.${id}.title`);
    },
    get subtitle() {
      return t(`onboarding:steps.${id}.subtitle`);
    },
  };
}

export const ONBOARDING_STEPS: readonly OnboardingStep[] = [
  step("profile", 1, false),
  step("storage", 1, false),
  step("providers", 1, true),
  step("manager", 1, true),
  step("permissions", 1, false),
  step("project", 1, true),
  step("git", 1, false),
  step("terminal", 1, false),
  step("voice", 3, true),
  step("remote", 4, true),
  step("sync", 4, true),
  step("mobile", 5, true),
];

export type StepVisualState = "done" | "current" | "upcoming" | "later";

/** Visual state of step `index` in the stepper. */
export function stepState(index: number, current: number, visited: ReadonlySet<number>): StepVisualState {
  if (index === current) return "current";
  const step = ONBOARDING_STEPS[index];
  if (step && step.phase > CURRENT_PHASE) return "later";
  return visited.has(index) ? "done" : "upcoming";
}

export function clampStep(index: number): number {
  return Math.max(0, Math.min(ONBOARDING_STEPS.length - 1, index));
}

/** Index of the last step that is interactive in this phase (setup can be finished from there). */
export const LAST_ACTIVE_STEP = ONBOARDING_STEPS.reduce((last, s, i) => (s.phase <= CURRENT_PHASE ? i : last), 0);

/** Whether the "Finish" action is offered on step `index`. */
export function canFinishAt(index: number): boolean {
  return index >= LAST_ACTIVE_STEP;
}

/** Validates the local profile name (mirrors the core: ≤ 80 printable chars). */
export function validateProfileName(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed.length === 0) return t("onboarding:validation.nameRequired");
  if ([...trimmed].length > 80) return t("onboarding:validation.nameTooLong");
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(trimmed)) return t("onboarding:validation.nameControlChars");
  return null;
}

/** Validates an absolute folder path for importing a project (the core re-validates). */
export function validateFolderPath(path: string): string | null {
  const p = path.trim();
  if (!p) return null; // optional
  const absolute = /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("/") || p.startsWith("\\\\");
  return absolute ? null : t("onboarding:validation.pathNotAbsolute");
}
