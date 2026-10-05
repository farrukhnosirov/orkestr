/** Route of a detached panel window: `#/panel/<panelId>?projectId=<id>` (ADR-0013). */
export interface DetachedRoute {
  panelId: string;
  projectId: string | null;
}

export function parseDetachedRoute(hash: string): DetachedRoute | null {
  const m = /^#\/panel\/([A-Za-z][A-Za-z0-9_-]{0,47})(?:\?(.*))?$/.exec(hash);
  if (!m || !m[1]) return null;
  const params = new URLSearchParams(m[2] ?? "");
  const projectId = params.get("projectId");
  return { panelId: m[1], projectId: projectId && /^[A-Za-z0-9-]{1,64}$/.test(projectId) ? projectId : null };
}
