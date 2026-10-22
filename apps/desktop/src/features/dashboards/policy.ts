/**
 * Host-side widget binding allowlist (ADR-0034). A plugin widget may only call
 * methods that are (1) in its `dataBinding.methods` (validated by core in
 * `widgets.list` / `widgets.bundle`) and (2) Read-class and not local-only.
 *
 * (2) mirrors `orkestr_protocol::policy::policy_for` (READ_VERBS, local-only
 * areas, explicit read overrides). The mirror only ever narrows what core
 * already allows: core re-checks on `widgets.bundle`, so a drift here can only
 * cause a false rejection, never a leak.
 */

/** Areas whose methods are local-only / emergency / worker (never bindable). */
const NON_BINDABLE_AREAS: ReadonlySet<string> = new Set(["secrets", "vscode", "extensions", "lsp", "claudeEngine", "emergency", "node"]);

/** Methods with read verbs that are nonetheless not Read-class. */
const NON_READ_METHODS: ReadonlySet<string> = new Set<string>([]);

/** Read-class methods whose verbs are not read verbs (explicit policy overrides). */
const EXPLICIT_READ: ReadonlySet<string> = new Set([
  "marketplace.sources",
  "marketplace.keys",
  "widgets.bundle",
  "deploy.adapters",
  "deploy.plan",
  "monitor.overview",
  "monitor.metrics",
  "monitor.checksList",
  "monitor.logSources",
  "monitor.alerts",
  "audit.list",
]);

/** Mirror of `READ_VERBS` in `crates/protocol/src/policy.rs`. */
const READ_VERBS: ReadonlySet<string> = new Set([
  "list", "get", "status", "health", "info", "graph", "history", "diff", "log", "show", "branches", "worktrees", "stat",
  "read", "search", "find", "summary", "byDimension", "timeseries", "records", "budgetsList", "budgetsCheck",
  "effectiveRules", "decisionsList", "decisionsGet", "knowledgeList", "knowledgeSearch", "checks", "resumeSummary",
  "resources", "transcript", "handoffList", "stats", "config", "runs", "getRun", "gates", "getProfile", "detect",
  "evaluate", "map", "timeline", "step", "policy", "preview", "explain", "models", "agents", "getConfig", "themes",
  "device", "snapshot", "logs", "listExtensions", "commands", "tasks", "details", "icon", "readme", "theme", "iconTheme",
  "readAsset", "languageConfig", "snippets", "getPrefs", "servers", "sessions", "capabilities", "usage", "contextUsage",
  "mcpStatus", "kinds", "getManager", "getRouting", "shells", "mergePreview", "check", "getMode", "replay",
  "checkUpdate", "vapidKey", "listConflicts", "resume",
]);

const METHOD_RE = /^[a-zA-Z][a-zA-Z0-9]*\.[a-zA-Z][a-zA-Z0-9]*$/;

/** True when a method is Read-class and not local-only (see module doc). */
export function isBindableMethod(method: string): boolean {
  if (!METHOD_RE.test(method)) return false;
  const [area = "", verb = ""] = method.split(".");
  if (NON_BINDABLE_AREAS.has(area) || NON_READ_METHODS.has(method)) return false;
  return EXPLICIT_READ.has(method) || READ_VERBS.has(verb);
}

/** Why a widget query is refused, or `null` when allowed. */
export function bindingViolation(binding: { methods: readonly string[] }, method: string): string | null {
  if (!binding.methods.includes(method)) return `method ${method} is not in the widget's data binding`;
  if (!isBindableMethod(method)) return `method ${method} is not a Read-class, non-local method`;
  return null;
}
