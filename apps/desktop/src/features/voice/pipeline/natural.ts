/**
 * Natural spoken replies. Status facts become conversational text, the way a
 * human assistant would say them:
 *
 * (a) `phraseNaturally`: the Manager's AI model phrases the facts
 *     (`voice.phrase`) under a strict time limit, falling back to
 * (b) `naturalStatus`: local templates — no "slug:" prefix, facts merged into
 *     flowing sentences with proper plurals, what needs attention last.
 *
 * The HUD shows and TTS speaks the same text.
 */
import { t as typedT } from "../../../i18n";
import { agentActivity, agentName } from "../../../lib/coreText";
import type { ProjectFacts } from "./summary";

/** Dynamic keys under `voice:natural.*` (variants, counted phrases). */
const t = typedT as unknown as (key: string, options?: Record<string, unknown>) => string;

/** A project name as people say it (`ai_office` → `ai office`). */
export function humanProjectName(name: string): string {
  return name.replace(/[_]+/g, " ").replace(/\s+/g, " ").trim();
}

export function lowerFirst(s: string): string {
  return s ? s.charAt(0).toLocaleLowerCase() + s.slice(1) : s;
}

function upperFirst(s: string): string {
  return s ? s.charAt(0).toLocaleUpperCase() + s.slice(1) : s;
}

/** Count phrase: "one task" for 1 (a word, not a digit), plural form otherwise. */
function counted(key: string, count: number): string {
  return count === 1 ? t(`voice:natural.${key}Single`) : t(`voice:natural.${key}`, { count });
}

/** "a, b and c" in the UI language. */
function joinClauses(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")}${t("voice:natural.lastJoin")}${items[items.length - 1]}`;
}

export interface NaturalOptions {
  /** Human project name to mention (only with several projects or when asked), else null. */
  project: string | null;
  /** Phrasing variant (random by default; fixed in tests). */
  variant?: number;
}

/** Whether the facts warrant offering to show the build / test errors. */
export function offersErrors(f: ProjectFacts): boolean {
  return (f.quality.lastBuild !== null && !f.quality.lastBuild.ok) || (f.quality.lastTests?.failed ?? 0) > 0;
}

/** Deterministic natural status reply (template fallback). */
export function naturalStatus(f: ProjectFacts, o: NaturalOptions): string {
  const v = (o.variant ?? Math.floor(Math.random() * 2)) % 2;
  const sentences: string[] = [];

  // 1. What is going on.
  let opening: string;
  let quiet = false;
  if (f.total === 0 && f.activeAgents === 0) {
    opening = t(`voice:natural.noTasks_v${v}`);
    quiet = true;
  } else if (f.running === 0 && f.activeAgents === 0) {
    opening = t(`voice:natural.idle_v${v}`);
    quiet = true;
  } else if (f.running > 0 && f.activeAgents > 0) {
    opening = t(`voice:natural.working_v${v}`, { tasks: counted("tasks", f.running), agents: counted("agentsBusy", f.activeAgents) });
  } else if (f.activeAgents > 0) {
    opening = t("voice:natural.agentsOnly", { agents: counted("agentsBusy", f.activeAgents) });
  } else {
    opening = t("voice:natural.tasksOnly", { tasks: counted("tasks", f.running) });
  }
  if (o.project) opening = t("voice:natural.inProject", { name: o.project, rest: lowerFirst(opening) });
  sentences.push(opening);

  if (f.activeAgents === 1 && f.focus?.activity) {
    sentences.push(t("voice:natural.focus", { agent: agentName(f.focus.agent), activity: lowerFirst(agentActivity(f.focus.activity)) }));
  }
  if (f.completedToday > 0) sentences.push(counted("doneToday", f.completedToday));

  // 2. What needs attention, merged into one sentence.
  const attention: string[] = [];
  if (f.quality.lastBuild && !f.quality.lastBuild.ok) attention.push(t("voice:natural.buildFailed"));
  const failingTests = f.quality.lastTests?.failed ?? 0;
  if (failingTests > 0) attention.push(counted("testsFailing", failingTests));
  if (f.failed > 0) attention.push(counted("tasksFailed", f.failed));
  if (f.blocked > 0) attention.push(counted("tasksBlocked", f.blocked));
  if (f.approvals > 0) attention.push(counted("approvals", f.approvals));
  if (attention.length > 0) {
    const lead = t(quiet ? `voice:natural.butQuiet_v${v}` : "voice:natural.butBusy");
    sentences.push(`${lead}${joinClauses(attention)}.`);
    if (offersErrors(f)) sentences.push(t("voice:natural.offerErrors"));
  } else if (f.quality.lastBuild?.ok) {
    sentences.push(t("voice:natural.buildOk"));
  }
  return sentences.map(upperFirst).join(" ");
}

/** The facts as plain lines for the AI phrasing prompt (no IDs). */
export function factsForModel(f: ProjectFacts, project: string | null): string {
  const lines: string[] = [];
  if (project) lines.push(`Project (mention it by this name): ${project}`);
  else lines.push("Project: the current one (do not name it)");
  if (f.total === 0) lines.push("No tasks yet");
  lines.push(`Tasks in progress: ${f.running}`, `Agents working: ${f.activeAgents}`);
  if (f.activeAgents === 1 && f.focus?.activity) lines.push(`The working agent: ${agentName(f.focus.agent)} — ${agentActivity(f.focus.activity)}`);
  if (f.completedToday > 0) lines.push(`Tasks completed today: ${f.completedToday}`);
  if (f.waiting > 0) lines.push(`Tasks waiting: ${f.waiting}`);
  if (f.failed > 0) lines.push(`Failed tasks: ${f.failed}`);
  if (f.blocked > 0) lines.push(`Blocked tasks: ${f.blocked}`);
  if (f.approvals > 0) lines.push(`Requests waiting for the user's approval: ${f.approvals}`);
  if (f.quality.lastBuild) lines.push(`Last build: ${f.quality.lastBuild.ok ? "passed" : "failed"}`);
  if (f.quality.lastTests && f.quality.lastTests.failed > 0) lines.push(`Failing tests: ${f.quality.lastTests.failed}`);
  if (f.quality.openReviews) lines.push(`Open code reviews: ${f.quality.openReviews}`);
  if (offersErrors(f)) lines.push("Next step to offer: show the build errors (end with that short question)");
  return lines.join("\n");
}

/** Time limit for AI phrasing before the template is used. */
export const PHRASE_TIMEOUT_MS = 5000;

/**
 * AI phrasing with a strict time limit; the template `fallback` on any
 * failure, timeout or an empty answer.
 */
export async function phraseNaturally(
  phrase: ((facts: string) => Promise<string>) | undefined,
  facts: string,
  fallback: string,
  timeoutMs = PHRASE_TIMEOUT_MS,
): Promise<string> {
  if (!phrase) return fallback;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), timeoutMs);
  });
  try {
    const text = await Promise.race([phrase(facts).catch(() => null), timeout]);
    const clean = text?.trim() ?? "";
    return clean ? clean : fallback;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
