/**
 * Agent Teams data: `teams.list` plus live `TeamUpdated` / `TeamDeleted`
 * (global) and `TeamLeadRunUpdated` (project) events. Mutations go through
 * `teamActions`; their results also arrive as events.
 */
import type { AssignTeamParams, CreateTeamParams, Event, TeamView, UpdateTeamParams } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { applyTeamEvent } from "./teamsModel";

const TEAM_EVENTS = new Set(["TeamUpdated", "TeamDeleted", "TeamLeadRunUpdated"]);

export interface Teams {
  teams: TeamView[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Teams visible in a project (global teams, the project's own and assigned ones). */
export function useTeams(projectId: string | null): Teams {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [teams, setTeams] = useState<TeamView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setError(null);
    if (!connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    let buffer: Event[] | null = [];
    const topics = projectId ? [topic.global, topic.project(projectId)] : [topic.global];
    const off = client.subscribe(topics, (ev: Event) => {
      if (!TEAM_EVENTS.has(ev.kind)) return;
      if (buffer) buffer.push(ev);
      else setTeams((prev) => applyTeamEvent(prev, ev));
    });
    setLoading(true);
    client
      .call("teams.list", projectId ? { projectId } : {})
      .then((r) => {
        if (cancelled) return;
        let next = r.teams;
        for (const ev of buffer ?? []) next = applyTeamEvent(next, ev);
        buffer = null;
        setTeams(next);
      })
      .catch((e: unknown) => {
        buffer = null;
        if (!cancelled) setError(describeError(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected, nonce]);

  return { teams, loading, error, reload };
}

export const teamActions = {
  create: (p: CreateTeamParams) => getClient().call("teams.create", p),
  update: (p: UpdateTeamParams) => getClient().call("teams.update", p),
  remove: (teamId: string) => getClient().call("teams.delete", { teamId }),
  assign: (p: AssignTeamParams) => getClient().call("teams.assign", p),
};
