/**
 * Local voice intent parser for Russian and English (§33, ADR-0017 "small
 * local grammar with fuzzy project and agent name matching"). Anything it
 * does not recognize goes to the AI Manager as text. Pure, table-tested.
 */
import { bestMatch, type EntityMatch, type Named, normalize, stripWakePhrase } from "./text";

export interface IntentContext {
  projects: readonly Named[];
  agents: readonly (Named & { projectId: string })[];
  activeProjectId: string | null;
  /** Enrolled wake phrase(s), stripped from the start. */
  wakePhrases?: readonly string[];
}

/** Panels a voice command can open (ids from the panel registry, plus settings). */
export type PanelTarget =
  | "manager"
  | "code"
  | "files"
  | "terminal"
  | "git"
  | "agents"
  | "tasks"
  | "logs"
  | "memory"
  | "costs"
  | "monitoring"
  | "missionControl"
  | "settings";

export interface ProjectRef {
  id: string;
  name: string;
  /** How the project was chosen. */
  via: "spoken" | "active";
}

export type Intent =
  | { kind: "empty" }
  | { kind: "stopAll" }
  | { kind: "emergencyStop" }
  | { kind: "stopProject"; project: ProjectRef | null; query: string }
  | { kind: "runQa"; project: ProjectRef | null; query: string }
  | { kind: "agentStatus"; agent: (Named & { projectId: string }) | null; query: string }
  | { kind: "projectStatus"; project: ProjectRef | null; query: string }
  | { kind: "showBuildErrors"; project: ProjectRef | null }
  | { kind: "openPanel"; panel: PanelTarget }
  | { kind: "switchProject"; project: ProjectRef | null; query: string }
  /**
   * `approvalId` is bound when the confirmation is asked (`bindApproval`):
   * the "yes" resolves exactly the approval that was read out, never a newer one.
   */
  | { kind: "approve"; approvalId?: string }
  | { kind: "reject"; approvalId?: string }
  | { kind: "dnd"; on: boolean }
  | { kind: "askManager"; text: string };

export type IntentKind = Intent["kind"];

/**
 * Intents that change state and need a confirmation first (ADR-0017, §70):
 * stopping work and resolving approvals. Emergency stop is the exception —
 * it runs at once (§52).
 */
export function needsConfirmation(intent: Intent): boolean {
  return intent.kind === "stopProject" || intent.kind === "stopAll" || intent.kind === "approve" || intent.kind === "reject";
}

const YES = /^(?:yes|yeah|yep|confirm|confirmed|do it|go ahead|ok|okay|да|ага|подтверждаю|подтвердить|давай|делай|выполняй|конечно)$/;
const NO = /^(?:no|nope|cancel|stop|never mind|nevermind|нет|отмена|отмени|не надо|не нужно|стоп)$/;

/** A spoken answer to a pending confirmation, or `null` when it is not one. */
export function parseConfirmation(utterance: string, wakePhrases: readonly string[] = []): "yes" | "no" | null {
  const text = stripWakePhrase(utterance, wakePhrases).replace(/^(please|пожалуйста)\s+/, "").replace(/\s+(please|пожалуйста)$/, "");
  if (YES.test(text)) return "yes";
  if (NO.test(text)) return "no";
  return null;
}

const PANEL_WORDS: ReadonlyArray<readonly [PanelTarget, readonly string[]]> = [
  ["missionControl", ["mission control", "центр управления", "миссия контроль", "dashboard", "дашборд"]],
  ["monitoring", ["мониторинг", "monitoring"]],
  ["manager", ["manager", "менеджер", "менеджера", "чат", "chat", "ai manager"]],
  ["code", ["code", "editor", "код", "редактор"]],
  ["files", ["files", "file tree", "файлы", "файл", "проводник"]],
  ["terminal", ["terminal", "console", "терминал", "консоль"]],
  ["git", ["git", "гит", "branches", "ветки", "commits", "коммиты"]],
  ["agents", ["agents", "agent list", "агенты", "агентов", "список агентов"]],
  ["tasks", ["tasks", "task board", "задачи", "задач", "доску задач"]],
  ["logs", ["logs", "log", "events", "журнал", "логи", "события", "журнал событий"]],
  ["memory", ["memory", "память", "knowledge base", "базу знаний"]],
  ["costs", ["costs", "cost", "spend", "spending", "budget", "расходы", "бюджет", "траты"]],
  ["settings", ["settings", "preferences", "настройки", "параметры"]],
];

function panelFor(words: string): PanelTarget | null {
  const w = normalize(words).replace(/^(the|a|panel|панель|вкладку|окно)\s+/, "").replace(/\s+(panel|панель|tab)$/, "");
  for (const [panel, names] of PANEL_WORDS) if (names.includes(w)) return panel;
  return null;
}

function projectRef(query: string, ctx: IntentContext): ProjectRef | null {
  const q = query.replace(/^(project|проект|проекта|проекту|проектом|the)\s+/, "").trim();
  if (q) {
    const m: EntityMatch<Named> | null = bestMatch(q, ctx.projects);
    return m ? { id: m.item.id, name: m.item.name, via: "spoken" } : null;
  }
  const active = ctx.projects.find((p) => p.id === ctx.activeProjectId);
  return active ? { id: active.id, name: active.name, via: "active" } : null;
}

/** Words that mean "the current project" ("this project", "здесь"). */
const HERE = /^(this project|current project|here|этот проект|этому проекту|текущий проект|текущему проекту|здесь|тут)$/;

function projectQuery(raw: string): string {
  const q = raw.trim();
  return HERE.test(q) ? "" : q;
}

interface Rule {
  re: RegExp;
  build: (m: RegExpMatchArray, ctx: IntentContext, text: string) => Intent | null;
}

const PREP_EN = "(?:on|for|in|with|at|of)";
const PREP_RU = "(?:по|для|в|во|на|у|с|со|о|об)";

const RULES: readonly Rule[] = [
  // Emergency stop runs at once (§52); "stop all" goes through the confirmation dialog.
  { re: /^(?:emergency stop|panic stop|экстренная остановка|экстренно останови(?:ть)?(?: всё| все)?)$/, build: () => ({ kind: "emergencyStop" }) },
  {
    re: /^(?:stop|halt|kill|pause) (?:all|everything)(?: agents| ai| work)?$|^(?:останови|остановить|стоп|прекрати|выключи)(?:те)? (?:всё|все|всех)(?: агентов| агенты| ai| работу)?$|^(?:всем стоп|стоп всё|стоп все)$/,
    build: () => ({ kind: "stopAll" }),
  },
  {
    re: /^(?:show|display|open)(?: me)?(?: the)? (?:build errors|build failures|failed builds?|compile errors)(?: (?:on|for|in) (.+))?$|^(?:покажи|показать|открой)(?: мне)? (?:ошибки сборки|ошибки билда|ошибки компиляции|упавшие сборки)(?: (?:по|для|в|во|у) (.+))?$/,
    build: (m, ctx) => {
      const q = projectQuery(m[1] ?? m[2] ?? "");
      return { kind: "showBuildErrors", project: projectRef(q, ctx) };
    },
  },
  {
    re: new RegExp(
      `^(?:run|start|launch|kick off)(?: the)? (?:qa|tests?|test suite|quality checks?)(?: ${PREP_EN} (.+))?$|^(?:test|qa) (.+)$|^(?:запусти|запустить|запускай|начни|проведи|прогони)(?:те)? (?:qa|ку эй|тесты|тестирование|проверку качества|проверки)(?: ${PREP_RU} (.+))?$|^(?:протестируй|проверь)(?:те)? (.+)$`,
    ),
    build: (m, ctx) => {
      const raw = m[1] ?? m[2] ?? m[3] ?? m[4] ?? "";
      const q = projectQuery(raw);
      return { kind: "runQa", project: projectRef(q, ctx), query: q };
    },
  },
  {
    re: /^(?:what(?: is|'s| s)?|whats) (.+?) (?:doing|working on|up to)$|^(?:status of agent|agent status(?: of)?) (.+)$|^(?:что|чем) (?:делает|занят|занимается) (.+)$|^(?:статус агента|что с агентом) (.+)$/,
    build: (m, ctx) => {
      const q = (m[1] ?? m[2] ?? m[3] ?? m[4] ?? "").replace(/^the\s+/, "");
      const agent = bestMatch(q, ctx.agents);
      return { kind: "agentStatus", agent: agent?.item ?? null, query: q };
    },
  },
  {
    re: new RegExp(
      `^(?:what(?: is|'s| s)?|whats) (?:happening|going on|up|new) ${PREP_EN} (.+)$|^(?:status|state|progress)(?: of| for| on)? (.+)$|^(?:how is|how's|hows) (.+?)(?: going| doing)?$|^(?:что|чё) (?:происходит|нового|там|сейчас) ${PREP_RU} (.+)$|^(?:как|как там) (?:дела|успехи|продвигается|идёт|идет)(?: ${PREP_RU})? (.+)$|^(?:статус|состояние|прогресс)(?: проекта| по)? (.+)$|^что с (.+)$`,
    ),
    build: (m, ctx) => {
      const q = projectQuery(m.slice(1).find((x) => x !== undefined) ?? "");
      return { kind: "projectStatus", project: projectRef(q, ctx), query: q };
    },
  },
  {
    re: /^(?:what(?: is|'s| s)?|whats) (?:happening|going on)$|^(?:status|status report)$|^(?:что|чё) (?:происходит|нового)$|^(?:статус|сводка|как дела)$/,
    build: (_m, ctx) => ({ kind: "projectStatus", project: projectRef("", ctx), query: "" }),
  },
  {
    re: /^(?:stop|halt|pause) (?:project )?(.+)$|^(?:останови|остановить|приостанови|стоп)(?:те)? (?:проект |работу (?:над|по|в) )?(.+)$/,
    build: (m, ctx) => {
      const q = projectQuery(m[1] ?? m[2] ?? "");
      return { kind: "stopProject", project: projectRef(q, ctx), query: q };
    },
  },
  {
    re: /^(?:approve|accept|allow|confirm)(?: it| that| this| the request| request| all)?$|^(?:подтверди|подтвердить|одобри|разреши|согласовано|да подтверждаю)(?:те)?(?: это| запрос)?$/,
    build: () => ({ kind: "approve" }),
  },
  {
    re: /^(?:reject|deny|decline|refuse)(?: it| that| this| the request| request)?$|^(?:отклони|отклонить|запрети|откажи)(?:те)?(?: это| запрос)?$/,
    build: () => ({ kind: "reject" }),
  },
  {
    re: /^(?:(turn on|enable|start)|(turn off|disable|stop)) (?:do not disturb|dnd)$|^(?:(включи)|(выключи|отключи))(?:те)? (?:режим )?не беспокоить$|^(do not disturb|не беспокоить)$/,
    build: (m) => ({ kind: "dnd", on: Boolean(m[1] ?? m[3] ?? m[5]) }),
  },
  {
    re: /^(?:switch|go|change|jump)(?: over)? to(?: project)? (.+)$|^(?:open project) (.+)$|^(?:переключись|переключи|перейди|переключиться|перейти)(?:те)? (?:на|в|во|к) (?:проект )?(.+)$|^(?:открой проект) (.+)$/,
    build: (m, ctx) => {
      const q = m[1] ?? m[2] ?? m[3] ?? m[4] ?? "";
      const panel = panelFor(q);
      if (panel) return { kind: "openPanel", panel };
      return { kind: "switchProject", project: projectRef(q, ctx), query: q };
    },
  },
  {
    re: /^(?:open|show|display|go to)(?: me)?(?: the)? (.+)$|^(?:открой|покажи|показать|открыть|перейди в|перейди к)(?:те)?(?: мне)? (.+)$/,
    build: (m, ctx) => {
      const q = m[1] ?? m[2] ?? "";
      const panel = panelFor(q);
      if (panel) return { kind: "openPanel", panel };
      const project = projectRef(q, ctx);
      if (project && project.via === "spoken") return { kind: "switchProject", project, query: q };
      return null;
    },
  },
];

/** Parses an utterance into an intent. */
export function parseIntent(utterance: string, ctx: IntentContext): Intent {
  const text = stripWakePhrase(utterance, ctx.wakePhrases ?? []).replace(/^(please|пожалуйста)\s+/, "").replace(/\s+(please|пожалуйста)$/, "");
  if (!text) return { kind: "empty" };
  for (const rule of RULES) {
    const m = text.match(rule.re);
    if (!m) continue;
    const intent = rule.build(m, ctx, text);
    if (intent) return intent;
  }
  return { kind: "askManager", text: utterance.trim() };
}

/** Stable intent id for the command log (`voice.recordCommand`). */
export function intentId(intent: Intent): string {
  return intent.kind;
}
