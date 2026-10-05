/**
 * Results of one quality run: a row per check (status, kind, summary from the
 * structured counts) that expands into failures, diagnostics (jump to file in
 * the Code panel) and the output tail.
 */
import type { CheckResult, QualityRun } from "@orkestr/protocol";
import { Badge, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { ChevronDown, ChevronRight, FileCode2 } from "lucide-react";
import { useState } from "react";

import { useFormat, useT } from "../../i18n";
import { openInEditor } from "../files/actions";
import { checkTone, isFinal, locationOf, resultReason } from "./logic";
import "./quality.css";

const MAX_DIAGNOSTICS = 50;

function ResultSummary({ result }: { result: CheckResult }) {
  const { t } = useT("quality");
  const reason = resultReason(result);
  if (reason) {
    return <span className="q-result__summary">{t(`result.why.${reason}`, { message: result.summary })}</span>;
  }
  if (!isFinal(result.status)) return null;
  const tests = result.passed + result.failed + result.skipped;
  if (tests > 0) {
    return (
      <span className="q-result__summary">
        {t("result.tests", { passed: result.passed, failed: result.failed, skipped: result.skipped })}
      </span>
    );
  }
  const errors = result.diagnostics.filter((d) => d.severity === "Error").length;
  const warnings = result.diagnostics.filter((d) => d.severity === "Warning").length;
  if (errors + warnings > 0) {
    return <span className="q-result__summary">{t("result.diagnostics", { errors, warnings })}</span>;
  }
  if (result.status === "Passed") return <span className="q-result__summary">{t("result.clean")}</span>;
  if (result.exitCode != null) return <span className="q-result__summary">{t("result.exitCode", { code: result.exitCode })}</span>;
  return null;
}

function Location({ file, line, column, result, run }: { file?: string | undefined; line?: number | undefined; column?: number | undefined; result: CheckResult; run: QualityRun }) {
  const { t } = useT("quality");
  const loc = locationOf({ file, line, column }, result, run);
  if (!file) return null;
  const text = `${file}${line != null ? `:${line}` : ""}${column != null ? `:${column}` : ""}`;
  if (!loc) return <span className="q-loc q-mono">{text}</span>;
  return (
    <button
      type="button"
      className="q-loc q-loc--link q-mono"
      title={t("result.openAt", { path: loc.path, line: loc.line })}
      onClick={() => openInEditor(loc.path, loc.column != null ? { line: loc.line, column: loc.column } : { line: loc.line })}
    >
      <FileCode2 size={11} aria-hidden="true" />
      {text}
    </button>
  );
}

function ResultDetails({ result, run }: { result: CheckResult; run: QualityRun }) {
  const { t } = useT("quality");
  const diagnostics = result.diagnostics.slice(0, MAX_DIAGNOSTICS);
  const more = result.diagnostics.length - diagnostics.length;
  return (
    <div className="q-result__details">
      <div className="q-muted q-mono">{t("result.command", { command: result.command })}</div>
      {result.failures.length > 0 && (
        <>
          <h6 className="q-subtitle">{t("result.failures")}</h6>
          <ul className="q-list">
            {result.failures.map((f, i) => (
              <li key={`${f.name}-${i}`} className="q-failure">
                <div className="q-failure__head">
                  <span className="q-mono q-failure__name">{f.name}</span>
                  <Location file={f.file} line={f.line} result={result} run={run} />
                </div>
                {f.message && <pre className="q-pre">{f.message}</pre>}
              </li>
            ))}
          </ul>
        </>
      )}
      {diagnostics.length > 0 && (
        <>
          <h6 className="q-subtitle">{t("result.diagnosticsTitle")}</h6>
          <ul className="q-list">
            {diagnostics.map((d, i) => (
              <li key={i} className={clsx("q-diag", `q-diag--${d.severity.toLowerCase()}`)}>
                <Location file={d.file} line={d.line} column={d.column} result={result} run={run} />
                {d.code && <span className="q-mono q-muted">{d.code}</span>}
                <span className="q-diag__msg">{d.message}</span>
              </li>
            ))}
          </ul>
          {more > 0 && <p className="q-muted">{t("result.moreDiagnostics", { count: more })}</p>}
        </>
      )}
      {result.rawTail && (
        <>
          <h6 className="q-subtitle">{t("result.output")}</h6>
          <pre className="q-pre q-pre--output">{result.rawTail}</pre>
        </>
      )}
    </div>
  );
}

function ResultRow({ result, run, defaultOpen }: { result: CheckResult; run: QualityRun; defaultOpen: boolean }) {
  const { t } = useT("quality");
  const fmt = useFormat();
  const [open, setOpen] = useState(defaultOpen);
  const expandable = isFinal(result.status) && (result.failures.length > 0 || result.diagnostics.length > 0 || result.rawTail.length > 0);
  return (
    <li className={clsx("q-result", `q-result--${result.status.toLowerCase()}`)}>
      <div className="q-result__row">
        {expandable ? (
          <button
            type="button"
            className="q-result__toggle"
            aria-expanded={open}
            aria-label={open ? t("result.hideDetails") : t("result.showDetails")}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          </button>
        ) : (
          <span className="q-result__toggle q-result__toggle--empty" aria-hidden="true">
            {result.status === "Running" ? <Spinner size={11} /> : null}
          </span>
        )}
        <span className="q-result__label" title={result.label}>
          {t(`kind.${result.kind}`)}
          {result.label && result.label !== result.command ? <span className="q-muted"> · {result.label}</span> : null}
        </span>
        <Badge tone={checkTone(result.status)}>{t(`status.${result.status}`)}</Badge>
        {result.durationMs > 0 && <span className="q-muted q-result__time">{fmt.duration(result.durationMs)}</span>}
      </div>
      <ResultSummary result={result} />
      {open && expandable && <ResultDetails result={result} run={run} />}
    </li>
  );
}

/** Results of a run (expands the first failing check). */
export function RunResults({ run }: { run: QualityRun }) {
  const firstFailing = run.results.findIndex((r) => r.status === "Failed" || r.status === "TimedOut" || r.status === "Error");
  return (
    <ul className="q-results">
      {run.results.map((r, i) => (
        <ResultRow key={r.checkId} result={r} run={run} defaultOpen={i === firstFailing} />
      ))}
    </ul>
  );
}
