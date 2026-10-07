/**
 * Global search providers (palette `/` mode, §54): projects, tasks, memory,
 * the knowledge base and files. Memory and knowledge use the core's FTS5
 * search (`memory.search`, `knowledge.search`), files the quick-open matcher
 * (`files.find`); every provider runs in parallel and a failing one is
 * skipped.
 */
import type { Task } from "@orkestr/protocol";
import { BookOpen, BrainCircuit, FileText, FolderOpen, ListTodo, type LucideIcon } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { maybeClient, openProject } from "../../core/client";
import { t } from "../../i18n";
import { useMemoryFocus } from "../../panels/Memory/focus";
import { kindLabel } from "../../panels/Memory/kinds";
import { useTasksUi } from "../../panels/Tasks/store";
import { useAppStore } from "../../store/app";
import { openInEditor } from "../files/actions";
import { fuzzyMatch } from "./fuzzy";

export interface SearchResult {
  /** Unique across providers, e.g. `project:<id>`. */
  id: string;
  title: string;
  subtitle?: string;
  icon?: LucideIcon;
  score: number;
  /** Highlight positions in `title`. */
  indices: number[];
  run: () => void | Promise<void>;
}

export interface SearchProvider {
  id: string;
  /** Group heading in the palette (localized; define it as a getter over `t()`). */
  readonly title: string;
  /** Results for a non-empty query, best first. May be async (RPC-backed providers). */
  search: (query: string, signal: AbortSignal) => SearchResult[] | Promise<SearchResult[]>;
}

export const MAX_RESULTS_PER_PROVIDER = 8;

export const projectsProvider: SearchProvider = {
  id: "projects",
  get title() {
    return t("search:providers.projects");
  },
  search: (query) => {
    const { projects } = useAppStore.getState();
    const results: SearchResult[] = [];
    for (const p of projects) {
      const byName = fuzzyMatch(query, p.name);
      const byOther = fuzzyMatch(query, `${p.slug} ${p.description} ${p.tags.join(" ")} ${p.rootPath ?? ""}`);
      if (!byName && !byOther) continue;
      const subtitle = [p.rootPath, p.description].filter((x): x is string => Boolean(x)).join(" · ");
      const result: SearchResult = {
        id: `project:${p.id}`,
        title: p.name,
        icon: FolderOpen,
        score: byName ? byName.score : (byOther?.score ?? 0) - 200,
        indices: byName?.indices ?? [],
        run: () => openProject(p.id),
      };
      if (subtitle) result.subtitle = subtitle;
      results.push(result);
    }
    return results.sort((a, b) => b.score - a.score).slice(0, MAX_RESULTS_PER_PROVIDER);
  },
};

/** RPC-backed providers need a connection and at least this many characters. */
const MIN_REMOTE_QUERY = 2;

function remoteReady(query: string): { client: NonNullable<ReturnType<typeof maybeClient>>; projectId: string | null } | null {
  const client = maybeClient();
  if (!client || client.state !== "open" || query.trim().length < MIN_REMOTE_QUERY) return null;
  return { client, projectId: useAppStore.getState().activeProjectId };
}

/** Tasks of the active project cached briefly: one `tasks.list` per palette session, not per keystroke. */
let taskCache: { projectId: string; at: number; tasks: Task[] } | null = null;
const TASK_CACHE_MS = 15_000;

/** Test seam. */
export function resetSearchCaches(): void {
  taskCache = null;
}

export const tasksProvider: SearchProvider = {
  id: "tasks",
  get title() {
    return t("search:providers.tasks");
  },
  search: async (query) => {
    const ready = remoteReady(query);
    if (!ready?.projectId) return [];
    const { client, projectId } = ready;
    if (!taskCache || taskCache.projectId !== projectId || Date.now() - taskCache.at > TASK_CACHE_MS) {
      const { tasks } = await client.call("tasks.list", { projectId });
      taskCache = { projectId, at: Date.now(), tasks };
    }
    const results: SearchResult[] = [];
    for (const task of taskCache.tasks) {
      const byTitle = fuzzyMatch(query, task.title);
      const byOther = byTitle ? null : fuzzyMatch(query, `${task.description} ${task.tags.join(" ")}`);
      if (!byTitle && !byOther) continue;
      results.push({
        id: `task:${task.id}`,
        title: task.title,
        subtitle: t(`common:status.task.${task.status}`),
        icon: ListTodo,
        score: byTitle ? byTitle.score : (byOther?.score ?? 0) - 200,
        indices: byTitle?.indices ?? [],
        run: () => {
          workspaceActions.openPanel("tasks");
          useTasksUi.getState().select(task.id);
        },
      });
    }
    return results.sort((a, b) => b.score - a.score).slice(0, MAX_RESULTS_PER_PROVIDER);
  },
};

export const memoryProvider: SearchProvider = {
  id: "memory",
  get title() {
    return t("search:providers.memory");
  },
  search: async (query, signal) => {
    const ready = remoteReady(query);
    if (!ready) return [];
    const { client, projectId } = ready;
    const { hits } = await client.call("memory.search", {
      query: query.trim(),
      ...(projectId ? { projectId, includeGlobal: true } : {}),
      limit: MAX_RESULTS_PER_PROVIDER,
    });
    if (signal.aborted) return [];
    return hits.map((h, i) => {
      const global = h.item.scope.scope === "Global";
      return {
        id: `memory:${h.item.id}`,
        title: h.item.title,
        subtitle: `${kindLabel(h.item.kind)} · ${global ? t("search:scope.global") : t("search:scope.project")}`,
        icon: BrainCircuit,
        // FTS order is the ranking.
        score: 1000 - i,
        indices: fuzzyMatch(query, h.item.title)?.indices ?? [],
        run: () => {
          useMemoryFocus.getState().request({ tab: global ? "global" : "project", query: h.item.title, expand: h.item.id });
          workspaceActions.openPanel("memory");
        },
      };
    });
  },
};

export const knowledgeProvider: SearchProvider = {
  id: "knowledge",
  get title() {
    return t("search:providers.knowledge");
  },
  search: async (query, signal) => {
    const ready = remoteReady(query);
    if (!ready) return [];
    const { client, projectId } = ready;
    const { hits } = await client.call("knowledge.search", {
      query: query.trim(),
      ...(projectId ? { projectId, includeGlobal: true } : {}),
      limit: MAX_RESULTS_PER_PROVIDER,
    });
    if (signal.aborted) return [];
    return hits.map((h, i) => ({
      id: `knowledge:${h.sourceId}:${h.path}:${h.ordinal}`,
      title: h.headingPath ? `${h.path} — ${h.headingPath}` : h.path,
      subtitle: h.sourceName,
      icon: BookOpen,
      score: 1000 - i,
      indices: [],
      run: () => {
        useMemoryFocus.getState().request({ tab: "knowledge", query: query.trim(), expand: null });
        workspaceActions.openPanel("memory");
      },
    }));
  },
};

export const filesProvider: SearchProvider = {
  id: "files",
  get title() {
    return t("search:providers.files");
  },
  search: async (query, signal) => {
    const ready = remoteReady(query);
    if (!ready?.projectId) return [];
    const { client, projectId } = ready;
    const { matches } = await client.call("files.find", { projectId, query: query.trim(), maxResults: MAX_RESULTS_PER_PROVIDER });
    if (signal.aborted) return [];
    return matches.map((m, i) => {
      const slash = Math.max(m.path.lastIndexOf("/"), m.path.lastIndexOf("\\"));
      const result: SearchResult = {
        id: `file:${m.path}`,
        title: m.path,
        icon: FileText,
        score: 1000 - i,
        indices: m.positions,
        run: () => openInEditor(m.path),
      };
      if (slash > 0) result.subtitle = m.path.slice(0, slash);
      return result;
    });
  },
};

/**
 * Registered providers (append new ones here): projects (local), tasks of
 * the active project, memory (FTS5), knowledge base (FTS5) and project files.
 * Logs are searched in the Logs panel.
 */
export const searchProviders: SearchProvider[] = [projectsProvider, tasksProvider, memoryProvider, knowledgeProvider, filesProvider];

export interface SearchGroup {
  provider: SearchProvider;
  results: SearchResult[];
}

/** Runs every provider; failing providers are skipped (logged). */
export async function searchAll(
  query: string,
  signal: AbortSignal,
  providers: readonly SearchProvider[] = searchProviders,
): Promise<SearchGroup[]> {
  const q = query.trim();
  if (!q) return [];
  const settled = await Promise.allSettled(providers.map(async (p) => ({ provider: p, results: await p.search(q, signal) })));
  const groups: SearchGroup[] = [];
  for (const s of settled) {
    if (s.status === "fulfilled") {
      if (s.value.results.length > 0) groups.push(s.value);
    } else {
      console.warn("search provider failed", s.reason);
    }
  }
  return groups;
}
