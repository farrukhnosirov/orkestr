/**
 * Session Resume (§57) presentation logic: turns `system.resumeSummary`
 * counts into readable, localized lines. Generic — unknown kinds get a
 * readable label derived from the kind name, so new engines need no changes
 * here (add a `resume:kinds.<Kind>` plural set to give them a proper label).
 */
import type { EventKindCount, ResumeSummary } from "@orkestr/protocol";
import i18n from "i18next";

import { t } from "../../i18n";

/** "BuildSucceeded" → "build succeeded". */
export function humanizeKind(kind: string): string {
  return kind
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .trim();
}

export function describeCount(c: EventKindCount): string {
  // Plural keys exist as `<Kind>_one` / `_other` (+ `_few` / `_many` in Russian).
  if (i18n.exists(`resume:kinds.${c.kind}_other`)) {
    return t(`resume:kinds.${c.kind}` as "resume:kinds.Notification", { count: c.count });
  }
  return t("resume:other", { count: c.count, kind: humanizeKind(c.kind) });
}

/** Up to `max` lines, most frequent first; the rest folded into "+N more". */
export function summaryLines(summary: ResumeSummary, max = 6): string[] {
  const lines = summary.counts.slice(0, max).map(describeCount);
  const rest = summary.counts.slice(max).reduce((n, c) => n + c.count, 0);
  if (rest > 0) lines.push(t("resume:more", { count: rest }));
  return lines;
}

/** Headline for the card. */
export function headline(summary: ResumeSummary, projectName: string | null): string {
  const project = projectName ?? undefined;
  if (summary.totalEvents === 0) {
    if (summary.since) return project ? t("resume:headline.quietIn", { project }) : t("resume:headline.quiet");
    return project ? t("resume:headline.welcomeIn", { project }) : t("resume:headline.welcome");
  }
  const count = summary.totalEvents;
  if (summary.since) {
    return project ? t("resume:headline.sinceLastIn", { count, project }) : t("resume:headline.sinceLast", { count });
  }
  return project ? t("resume:headline.sinceStartIn", { count, project }) : t("resume:headline.sinceStart", { count });
}
