/**
 * Onboarding steps (§55, RR-G7). Phase 1 delivers steps 1–8; 9–12 are shown
 * with the phase they arrive in and can only be acknowledged.
 */

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
  title: string;
  subtitle: string;
  /** Phase in which the step becomes interactive (1 = now). */
  phase: number;
  /** May be skipped without consequences. */
  optional: boolean;
}

export const CURRENT_PHASE = 1;

export const ONBOARDING_STEPS: readonly OnboardingStep[] = [
  { id: "profile", title: "Your profile", subtitle: "A local profile — no account needed", phase: 1, optional: false },
  { id: "storage", title: "Storage", subtitle: "Where your projects and history live", phase: 1, optional: false },
  { id: "providers", title: "AI providers", subtitle: "Connect the models you already pay for", phase: 1, optional: true },
  { id: "manager", title: "Manager model", subtitle: "The model that plans and delegates", phase: 1, optional: true },
  { id: "permissions", title: "Permissions", subtitle: "How much autonomy agents get", phase: 1, optional: false },
  { id: "project", title: "First project", subtitle: "Import a folder or start fresh", phase: 1, optional: true },
  { id: "git", title: "Git", subtitle: "Version control for safe agent work", phase: 1, optional: false },
  { id: "terminal", title: "Terminal", subtitle: "Shells and system readiness", phase: 1, optional: false },
  { id: "voice", title: "Voice", subtitle: "Talk to your Manager", phase: 3, optional: true },
  { id: "remote", title: "Remote access", subtitle: "Reach this machine from anywhere", phase: 4, optional: true },
  { id: "sync", title: "Cloud sync", subtitle: "Sync projects across devices", phase: 4, optional: true },
  { id: "mobile", title: "Mobile pairing", subtitle: "Approve and monitor from your phone", phase: 5, optional: true },
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
  if (trimmed.length === 0) return "Enter a name (it stays on this machine).";
  if ([...trimmed].length > 80) return "Use at most 80 characters.";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(trimmed)) return "Control characters are not allowed.";
  return null;
}

/** Validates an absolute folder path for importing a project (the core re-validates). */
export function validateFolderPath(path: string): string | null {
  const p = path.trim();
  if (!p) return null; // optional
  const absolute = /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("/") || p.startsWith("\\\\");
  return absolute ? null : "Use an absolute path, e.g. C:\\Code\\my-app or /home/me/my-app.";
}
