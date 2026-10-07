/**
 * Diff of an edit Claude made or proposes. A light text diff renders
 * immediately (and in tests); "Monaco" upgrades it to Monaco's inline diff
 * editor, loaded lazily so the chat never pulls the editor bundle up front.
 */
import { lazy, Suspense, useState } from "react";

import { useT } from "../../i18n";
import { lineDiff } from "../../features/claudeCode/model";

const MonacoDiff = lazy(() => import("./MonacoDiff"));

/** Lines shown before "show all". */
const PREVIEW_LINES = 40;

export function TextDiff({ before, after }: { before: string; after: string }) {
  const { t } = useT("claudeCode");
  const [all, setAll] = useState(false);
  const lines = lineDiff(before, after);
  const shown = all ? lines : lines.slice(0, PREVIEW_LINES);
  return (
    <div className="cc-diff" role="figure" aria-label={t("tool.diff")}>
      {shown.map((l, i) => (
        <div key={i} className={`cc-diff__line cc-diff__line--${l.type}`}>
          <span className="cc-diff__sign" aria-hidden="true">
            {l.type === "add" ? "+" : l.type === "del" ? "−" : " "}
          </span>
          <span className="cc-diff__text">{l.text}</span>
        </div>
      ))}
      {!all && lines.length > PREVIEW_LINES && (
        <button type="button" className="cc-link" onClick={() => setAll(true)}>
          {t("tool.showAllLines", { count: lines.length })}
        </button>
      )}
    </div>
  );
}

export function InlineDiff({ before, after, path }: { before: string; after: string; path: string }) {
  const { t } = useT("claudeCode");
  const [monaco, setMonaco] = useState(false);
  return (
    <div className="cc-diff-wrap">
      <div className="cc-diff-wrap__bar">
        <button type="button" className="cc-link" onClick={() => setMonaco((v) => !v)} aria-pressed={monaco}>
          {monaco ? t("tool.textDiff") : t("tool.editorDiff")}
        </button>
      </div>
      {monaco ? (
        <Suspense fallback={<TextDiff before={before} after={after} />}>
          <MonacoDiff before={before} after={after} path={path} />
        </Suspense>
      ) : (
        <TextDiff before={before} after={after} />
      )}
    </div>
  );
}
