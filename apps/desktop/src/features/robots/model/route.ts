/**
 * Routes of the `robot.html` entry (set by src-tauri `robots.rs`):
 * `#/agent/<agentId>` — one robot; `#/cluster` — the overflow cluster;
 * `#/preview` — a robot cycling its states (browser dev / screenshots).
 */
export type RobotRoute = { kind: "agent"; agentId: string } | { kind: "cluster" } | { kind: "preview" } | { kind: "invalid" };

const SAFE_ID = /^[A-Za-z0-9-]{1,64}$/;

export function parseRobotRoute(hash: string): RobotRoute {
  const path = hash.replace(/^#/, "").replace(/^\/+/, "").split("?")[0] ?? "";
  const [head, arg, ...rest] = path.split("/");
  if (head === "cluster" && !arg) return { kind: "cluster" };
  if (head === "preview" && !arg) return { kind: "preview" };
  if (head === "agent" && arg && rest.length === 0 && SAFE_ID.test(arg)) return { kind: "agent", agentId: arg };
  return { kind: "invalid" };
}
