/**
 * Pure helpers of the Product view (unit-tested): immutable edits of an AI
 * Product Manager breakdown (epic → features → stories → acceptance
 * criteria → tasks), unique keys, and validation mirroring the core
 * (`orchestrator::pm::validate_epic`) so problems show before saving.
 */
import type { AgentRole, Event, PmDraft, PmEpic, PmFeature, PmStory, PmTask } from "@orkestr/protocol";

import { t } from "../../../i18n";

/** Bounds (same numbers as `orkestr_protocol::methods::pm` / tasks). */
export const PM_LIMITS = { features: 12, stories: 12, tasks: 40, criteria: 20, title: 200, idea: 20_000 } as const;

export type Path = { f: number; s?: number; t?: number };

function mapAt<T>(list: readonly T[], i: number, fn: (x: T) => T): T[] {
  return list.map((x, j) => (j === i ? fn(x) : x));
}

/** Every key used in the epic (features, stories, tasks). */
export function allKeys(epic: PmEpic): Set<string> {
  const keys = new Set<string>();
  for (const f of epic.features) {
    keys.add(f.key);
    for (const s of f.stories) {
      keys.add(s.key);
      for (const x of s.tasks) keys.add(x.key);
    }
  }
  return keys;
}

/** All executable tasks in order. */
export function allTasks(epic: PmEpic): PmTask[] {
  return epic.features.flatMap((f) => f.stories.flatMap((s) => s.tasks));
}

function freshKey(keys: Set<string>, prefix: "f" | "s" | "t"): string {
  let n = 1;
  while (keys.has(`${prefix}${n}`)) n += 1;
  const key = `${prefix}${n}`;
  keys.add(key);
  return key;
}

/** A fresh key `<prefix><n>` not used in the epic. */
export function nextKey(epic: PmEpic, prefix: "f" | "s" | "t"): string {
  return freshKey(allKeys(epic), prefix);
}

export function counts(epic: PmEpic): { features: number; stories: number; tasks: number } {
  const stories = epic.features.reduce((n, f) => n + f.stories.length, 0);
  return { features: epic.features.length, stories, tasks: allTasks(epic).length };
}

function taskWith(keys: Set<string>, role: AgentRole = "Backend"): PmTask {
  return { key: freshKey(keys, "t"), title: "", description: "", role, priority: "Normal", dependsOn: [], estimateMinutes: null, taskId: null };
}

function storyWith(keys: Set<string>): PmStory {
  const key = freshKey(keys, "s");
  return { key, title: "", description: "", acceptanceCriteria: [""], tasks: [taskWith(keys)], taskId: null };
}

export function newTask(epic: PmEpic, role: AgentRole = "Backend"): PmTask {
  return taskWith(allKeys(epic), role);
}

/** An empty story with one criterion and one task (fresh keys). */
export function newStory(epic: PmEpic): PmStory {
  return storyWith(allKeys(epic));
}

/** An empty feature with one story (fresh keys). */
export function newFeature(epic: PmEpic): PmFeature {
  const keys = allKeys(epic);
  const key = freshKey(keys, "f");
  return { key, title: "", description: "", stories: [storyWith(keys)], taskId: null };
}

export const edit = {
  epic: (epic: PmEpic, patch: Partial<Pick<PmEpic, "title" | "description">>): PmEpic => ({ ...epic, ...patch }),
  feature: (epic: PmEpic, f: number, patch: Partial<Pick<PmFeature, "title" | "description">>): PmEpic => ({
    ...epic,
    features: mapAt(epic.features, f, (x) => ({ ...x, ...patch })),
  }),
  addFeature: (epic: PmEpic): PmEpic => ({ ...epic, features: [...epic.features, newFeature(epic)] }),
  removeFeature: (epic: PmEpic, f: number): PmEpic => {
    const gone = epic.features[f]?.stories.flatMap((s) => s.tasks.map((x) => x.key)) ?? [];
    return dropDeps({ ...epic, features: epic.features.filter((_, i) => i !== f) }, gone);
  },
  story: (epic: PmEpic, f: number, s: number, patch: Partial<Pick<PmStory, "title" | "description">>): PmEpic => ({
    ...epic,
    features: mapAt(epic.features, f, (x) => ({ ...x, stories: mapAt(x.stories, s, (y) => ({ ...y, ...patch })) })),
  }),
  addStory: (epic: PmEpic, f: number): PmEpic => {
    const story = newStory(epic);
    return { ...epic, features: mapAt(epic.features, f, (x) => ({ ...x, stories: [...x.stories, story] })) };
  },
  removeStory: (epic: PmEpic, f: number, s: number): PmEpic => {
    const gone = epic.features[f]?.stories[s]?.tasks.map((x) => x.key) ?? [];
    return dropDeps(
      { ...epic, features: mapAt(epic.features, f, (x) => ({ ...x, stories: x.stories.filter((_, i) => i !== s) })) },
      gone,
    );
  },
  criterion: (epic: PmEpic, f: number, s: number, c: number, text: string): PmEpic =>
    onStory(epic, f, s, (y) => ({ ...y, acceptanceCriteria: mapAt(y.acceptanceCriteria, c, () => text) })),
  addCriterion: (epic: PmEpic, f: number, s: number): PmEpic =>
    onStory(epic, f, s, (y) => ({ ...y, acceptanceCriteria: [...y.acceptanceCriteria, ""] })),
  removeCriterion: (epic: PmEpic, f: number, s: number, c: number): PmEpic =>
    onStory(epic, f, s, (y) => ({ ...y, acceptanceCriteria: y.acceptanceCriteria.filter((_, i) => i !== c) })),
  task: (epic: PmEpic, f: number, s: number, ti: number, patch: Partial<Omit<PmTask, "key" | "taskId">>): PmEpic =>
    onStory(epic, f, s, (y) => ({ ...y, tasks: mapAt(y.tasks, ti, (x) => ({ ...x, ...patch })) })),
  addTask: (epic: PmEpic, f: number, s: number): PmEpic => {
    const task = newTask(epic);
    return onStory(epic, f, s, (y) => ({ ...y, tasks: [...y.tasks, task] }));
  },
  removeTask: (epic: PmEpic, f: number, s: number, ti: number): PmEpic => {
    const key = epic.features[f]?.stories[s]?.tasks[ti]?.key;
    const next = onStory(epic, f, s, (y) => ({ ...y, tasks: y.tasks.filter((_, i) => i !== ti) }));
    return key ? dropDeps(next, [key]) : next;
  },
  /** Adds or removes `dep` from the prerequisites of task `key`. */
  toggleDependency: (epic: PmEpic, key: string, dep: string): PmEpic => mapTasks(epic, (x) =>
    x.key !== key || dep === key ? x : { ...x, dependsOn: x.dependsOn.includes(dep) ? x.dependsOn.filter((d) => d !== dep) : [...x.dependsOn, dep] },
  ),
};

function onStory(epic: PmEpic, f: number, s: number, fn: (s: PmStory) => PmStory): PmEpic {
  return { ...epic, features: mapAt(epic.features, f, (x) => ({ ...x, stories: mapAt(x.stories, s, fn) })) };
}

function mapTasks(epic: PmEpic, fn: (t: PmTask) => PmTask): PmEpic {
  return {
    ...epic,
    features: epic.features.map((f) => ({ ...f, stories: f.stories.map((s) => ({ ...s, tasks: s.tasks.map(fn) })) })),
  };
}

/** Removes dependencies on deleted task keys. */
function dropDeps(epic: PmEpic, gone: readonly string[]): PmEpic {
  if (gone.length === 0) return epic;
  return mapTasks(epic, (x) => (x.dependsOn.some((d) => gone.includes(d)) ? { ...x, dependsOn: x.dependsOn.filter((d) => !gone.includes(d)) } : x));
}

/** Whether `deps` (key → prerequisites) contain a cycle. */
export function hasCycle(deps: ReadonlyMap<string, readonly string[]>): boolean {
  const done = new Set<string>();
  for (;;) {
    const ready = [...deps.entries()].filter(([k, ds]) => !done.has(k) && ds.every((d) => done.has(d))).map(([k]) => k);
    if (ready.length === 0) return done.size !== deps.size;
    for (const k of ready) done.add(k);
  }
}

const blank = (s: string) => s.trim().length === 0;

/** Localized problem of a breakdown, `null` when it can be saved. */
export function validateEpic(epic: PmEpic): string | null {
  if (blank(epic.title) || epic.title.length > PM_LIMITS.title) return t("product:validation.epicTitle");
  if (epic.features.length === 0 || epic.features.length > PM_LIMITS.features) return t("product:validation.features", { max: PM_LIMITS.features });
  const keys = new Set<string>();
  const deps = new Map<string, readonly string[]>();
  for (const f of epic.features) {
    if (blank(f.title)) return t("product:validation.featureTitle");
    if (blank(f.key) || keys.has(f.key)) return t("product:validation.key", { key: f.key });
    keys.add(f.key);
    if (f.stories.length === 0 || f.stories.length > PM_LIMITS.stories) return t("product:validation.stories", { title: f.title, max: PM_LIMITS.stories });
    for (const s of f.stories) {
      if (blank(s.title)) return t("product:validation.storyTitle");
      if (blank(s.key) || keys.has(s.key)) return t("product:validation.key", { key: s.key });
      keys.add(s.key);
      if (s.acceptanceCriteria.length === 0 || s.acceptanceCriteria.length > PM_LIMITS.criteria || s.acceptanceCriteria.some(blank)) {
        return t("product:validation.criteria", { title: s.title });
      }
      if (s.tasks.length === 0) return t("product:validation.tasks", { title: s.title });
      for (const x of s.tasks) {
        if (blank(x.title)) return t("product:validation.taskTitle");
        if (blank(x.key) || keys.has(x.key)) return t("product:validation.key", { key: x.key });
        keys.add(x.key);
        deps.set(x.key, x.dependsOn);
      }
    }
  }
  if (deps.size > PM_LIMITS.tasks) return t("product:validation.tooManyTasks", { max: PM_LIMITS.tasks });
  for (const [k, ds] of deps) {
    const bad = ds.find((d) => d === k || !deps.has(d));
    if (bad !== undefined) return t("product:validation.unknownDep", { key: k, dep: bad });
  }
  if (hasCycle(deps)) return t("product:validation.cycle");
  return null;
}

/** Completion percent of an approved draft (0 when unknown). */
export function progressPercent(d: PmDraft): number {
  const p = d.progress;
  if (!p || p.total === 0) return 0;
  return Math.round((p.completed / p.total) * 100);
}

/** Inserts or replaces a draft (newest first) on `PmDraftUpdated`. */
export function applyPmEvent(list: PmDraft[], ev: Event): PmDraft[] {
  if (ev.kind !== "PmDraftUpdated") return list;
  const d = ev.payload.draft;
  const i = list.findIndex((x) => x.id === d.id);
  if (i < 0) return [d, ...list];
  const next = [...list];
  // Events carry no progress; keep the last known one.
  next[i] = { ...d, progress: d.progress ?? list[i]?.progress ?? null };
  return next;
}
