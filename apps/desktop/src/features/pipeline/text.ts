/**
 * The quality pipeline stores its stage details, reasons and analyses in
 * English (they are logged and sent to agents). The UI shows the common,
 * machine-written forms in the UI language; anything else — agent and
 * reviewer text, error messages — is shown as written. Pure and unit-tested.
 */
import { t as translate } from "../../i18n";

/** A translate function (the hook's `t` re-renders on a language change). */
export type PipelineT = (key: string, opts?: Record<string, unknown>) => string;
type T = PipelineT;

/** Whole-string forms. */
const WHOLE: [RegExp, (m: RegExpExecArray, t: T) => string][] = [
  [/^(\d+) check\(s\) passed$/, (m, t) => t("pipeline:text.checksPassed", { count: Number(m[1]) })],
  [/^(\d+) passed$/, (m, t) => t("pipeline:text.testsPassed", { count: Number(m[1]) })],
  [/^(\d+) failed, (\d+) passed$/, (m, t) => t("pipeline:text.testsFailed", { failed: m[1], passed: m[2] })],
  [/^(\d+) finding\(s\), none blocking$/, (m, t) => t("pipeline:text.findingsNone", { count: Number(m[1]) })],
  [/^(\d+) unresolved critical finding\(s\)$/, (m, t) => t("pipeline:text.findingsBlocking", { count: Number(m[1]) })],
  [/^merged `(.+?)` into `(.+?)`(?: \(proposal [^)]+\))?$/, (m, t) => t("pipeline:text.mergedInto", { branch: m[1], target: m[2] })],
  [/^proposed — awaiting approval$/, (_m, t) => t("pipeline:text.mergeProposed")],
  [
    /^merge proposal ready \((\w+)\) — waiting for your approval in the Merge Manager$/,
    (m, t) => t("pipeline:text.mergeReady", { recommendation: t(`merge:recommendation.${m[1]}`, { defaultValue: m[1] }) }),
  ],
  [/^merge proposal ready — merge it in the Merge Manager$/, (_m, t) => t("pipeline:text.mergeReadyManual")],
  [/^(already merged|nothing to merge — the branch is already in the target)$/, (_m, t) => t("pipeline:text.alreadyMerged")],
  [/^no build \/ type check check is configured$/, (_m, t) => t("pipeline:text.noBuildCheck")],
  [/^no test check is configured$/, (_m, t) => t("pipeline:text.noTestCheck")],
  [/^the checks were skipped$/, (_m, t) => t("pipeline:text.checksSkipped")],
  [/^accepted by the user$/, (_m, t) => t("pipeline:text.accepted")],
  [/^more attempts granted by the user$/, (_m, t) => t("pipeline:text.moreAttempts")],
  [/^fix attempt (\d+) submitted — checking it$/, (m, t) => t("pipeline:text.fixSubmitted", { n: m[1] })],
  [/^no agent branch to merge$/, (_m, t) => t("pipeline:text.noBranch")],
  [/^the task has no agent branch — there is no change to attack$/, (_m, t) => t("pipeline:text.noChangeToAttack")],
  [
    /^self-healing stopped \((.+)\); the user declined further attempts$/,
    (m, t) => t("pipeline:text.declined", { reason: localizeReason(m[1] ?? "", t) }),
  ],
];

/** Escalation reasons inside other forms. */
function localizeReason(reason: string, t: T): string {
  const repeat = /^no progress: the last fix produced the same failure$/.exec(reason);
  if (repeat) return t("pipeline:text.noProgress");
  const attempts = /^(\d+) of (\d+) fix attempt\(s\) used$/.exec(reason);
  if (attempts) return t("pipeline:text.attemptsUsed", { used: attempts[1], allowed: attempts[2] });
  return reason;
}

/** In-string pieces (healing analysis summaries). */
const PARTS: [RegExp, (m: RegExpExecArray, t: T) => string][] = [
  [/^The tests failed: /, (_m, t) => `${t("pipeline:text.testsFailedPrefix")} `],
  [/^The build failed: /, (_m, t) => `${t("pipeline:text.buildFailedPrefix")} `],
  [/(\d+) passed, (\d+) failed, (\d+) skipped/g, (m, t) => t("pipeline:text.counts", { passed: m[1], failed: m[2], skipped: m[3] })],
  [/ (\d+) failing test\(s\)\./g, (m, t) => ` ${t("pipeline:text.failingTests", { count: Number(m[1]) })}`],
];

/** The UI-language form of a machine-written pipeline text (others unchanged). */
export function localizePipelineText(text: string, t: T = translate as unknown as T): string {
  const s = text.trim();
  for (const [re, f] of WHOLE) {
    const m = re.exec(s);
    if (m) return f(m, t);
  }
  let out = s;
  for (const [re, f] of PARTS) {
    out = out.replace(re, (...args: unknown[]) => {
      const groups = args.slice(0, -2) as string[];
      const m = Object.assign([...groups], { index: 0, input: out }) as unknown as RegExpExecArray;
      return f(m, t);
    });
  }
  return out;
}
