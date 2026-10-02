/**
 * Global search providers (palette `/` mode, §54). Phase 1 searches projects;
 * files, tasks, memory and logs register providers here as their areas land
 * (Phase 2 adds a server-side `search.query` over FTS5, RR-G6).
 */
import { FolderOpen, type LucideIcon } from "lucide-react";

import { openProject } from "../../core/client";
import { useAppStore } from "../../store/app";
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
  /** Group heading in the palette. */
  title: string;
  /** Results for a non-empty query, best first. May be async (RPC-backed providers). */
  search: (query: string, signal: AbortSignal) => SearchResult[] | Promise<SearchResult[]>;
}

export const MAX_RESULTS_PER_PROVIDER = 8;

export const projectsProvider: SearchProvider = {
  id: "projects",
  title: "Projects",
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

/** Registered providers (append new ones here). */
export const searchProviders: SearchProvider[] = [projectsProvider];

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
