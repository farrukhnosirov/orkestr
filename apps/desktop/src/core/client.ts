/**
 * The desktop's single connection to the Orkestr core. All business data
 * flows through this client (RPC over WebSocket) — the same code path the
 * Web and Mobile clients use.
 */
import { OrkestrClient, OrkestrError } from "@orkestr/client";
import type { Event, Project } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";

import { t } from "../i18n";
import { getCoreConnection } from "../lib/tauri";
import { useAppStore } from "../store/app";

let client: OrkestrClient | null = null;
let booting: Promise<OrkestrClient> | null = null;

/** Returns the connected client (throws before `bootstrapCore` succeeded). */
export function getClient(): OrkestrClient {
  if (!client) throw new OrkestrError("disconnected", t("common:errors.notInitialised"));
  return client;
}

/** The client if it exists (for optional consumers). */
export function maybeClient(): OrkestrClient | null {
  return client;
}

function onGlobalEvent(event: Event): void {
  const store = useAppStore.getState();
  store.noteSeq(event.seq);
  switch (event.kind) {
    case "ProjectCreated":
    case "ProjectUpdated":
      store.upsertProject(event.payload.project);
      break;
    case "ProjectDeleted":
      store.removeProject(event.payload.projectId);
      break;
    default:
      break;
  }
}

async function refreshProjects(c: OrkestrClient): Promise<void> {
  const { projects } = await c.call("projects.list", {});
  useAppStore.getState().setProjects(projects);
}

/** Resolves the core connection, connects, loads projects and wires events. Idempotent. */
export function bootstrapCore(): Promise<OrkestrClient> {
  if (booting) return booting;
  booting = (async () => {
    const store = useAppStore.getState();
    store.setConnection({ state: "connecting", error: null });
    let conn;
    try {
      conn = await getCoreConnection();
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      store.setConnection({ state: "closed", error: message });
      booting = null;
      throw e;
    }
    store.setConnection({ mode: conn.mode, url: conn.url, startupMs: conn.startupMs });
    const c = new OrkestrClient(conn.url, conn.token, {
      clientInfo: { kind: "desktop", version: "0.1.0" },
      logger: (level, message, data) => {
        if (level !== "debug") console[level](`[orkestr-client] ${message}`, data ?? "");
      },
    });
    client = c;
    c.onStateChange((state, error) => {
      useAppStore.getState().setConnection({
        state,
        error: error?.message ?? (state === "open" ? null : useAppStore.getState().connection.error),
        serverVersion: c.server?.serverVersion ?? null,
      });
      // Re-sync the project list after every (re)connect.
      if (state === "open") {
        useAppStore.getState().noteSeq(c.server?.lastSeq ?? 0);
        refreshProjects(c).catch((err: unknown) => console.error("failed to load projects", err));
      }
    });
    c.subscribe([TOPIC_GLOBAL], onGlobalEvent);
    try {
      await c.connect();
    } catch (e) {
      booting = null;
      client = null;
      throw e;
    }
    return c;
  })();
  return booting;
}

/** Creates a project and makes it active. */
export async function createProject(input: { name: string; description?: string; color?: string }): Promise<Project> {
  const params: { name: string; description?: string; color?: string } = { name: input.name };
  if (input.description) params.description = input.description;
  if (input.color) params.color = input.color;
  const { project } = await getClient().call("projects.create", params);
  const store = useAppStore.getState();
  store.upsertProject(project);
  await openProject(project.id);
  return project;
}

/** Switches the active project and records it as opened in the core. */
export async function openProject(projectId: string): Promise<void> {
  useAppStore.getState().setActiveProject(projectId);
  try {
    const { project } = await getClient().call("projects.open", { projectId });
    useAppStore.getState().upsertProject(project);
  } catch (e) {
    console.warn("projects.open failed", e);
  }
}
