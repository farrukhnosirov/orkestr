/**
 * Preview card for a proposed UI change (§29): what the Manager wants to
 * change (diff summary), a live preview of it on the whole app, a compare
 * toggle and Keep / Revert. After keeping, the card turns into a short
 * "Interface updated — Undo" confirmation.
 */
import { Button, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Eye, EyeOff, Layers, Palette, PanelRight, ShieldCheck, Sparkles, SunDim, Undo2, X, type LucideIcon } from "lucide-react";
import { useEffect, useMemo } from "react";

import { useT } from "../../i18n";
import { MotionCard } from "../performance/motion";
import { useAiUiStore } from "./aiuiStore";
import { changeText } from "./changeText";
import { isColorValue, summarizeChanges, type ChangeSummary } from "./patch";
import "./aiui.css";

/** How long the "applied" confirmation stays. */
const APPLIED_TTL_MS = 9000;

const ICONS: Partial<Record<ChangeSummary["kind"], LucideIcon>> = {
  density: Layers,
  panel: PanelRight,
  panelReset: PanelRight,
  widgetsHidden: EyeOff,
  widgetsShown: Eye,
  backgroundDim: SunDim,
  token: Palette,
  effect: Sparkles,
  themeMode: Palette,
};

function ChangeLine({ c }: { c: ChangeSummary }) {
  const Icon = ICONS[c.kind] ?? Sparkles;
  return (
    <li className="aiui-change">
      <span className="aiui-change__icon" aria-hidden="true">
        <Icon size={13} />
      </span>
      <span className="aiui-change__text">{changeText(c)}</span>
      {c.kind === "token" && (isColorValue(c.from) || isColorValue(c.to)) && (
        <span className="aiui-change__swatches" aria-hidden="true">
          {c.from && isColorValue(c.from) && <span className="aiui-swatch" style={{ background: c.from }} />}
          <span className="aiui-change__arrow">→</span>
          {c.to && isColorValue(c.to) && <span className="aiui-swatch" style={{ background: c.to }} />}
        </span>
      )}
      {c.kind === "backgroundDim" && (
        <span className="aiui-dimbar" aria-hidden="true">
          <span style={{ width: `${Math.round(c.to * 100)}%` }} />
        </span>
      )}
    </li>
  );
}

export function UiPatchCard() {
  const proposal = useAiUiStore((s) => s.proposal);
  const previewVisible = useAiUiStore((s) => s.previewVisible);
  const busy = useAiUiStore((s) => s.busy);
  const { keepProposal, discardProposal, setPreviewVisible, undoLast, closeCard } = useAiUiStore.getState();
  const { t } = useT("aiui");

  const changes = useMemo(() => (proposal?.preview ? summarizeChanges(proposal.preview.changes) : []), [proposal?.preview]);

  // The confirmation closes by itself.
  useEffect(() => {
    if (proposal?.status !== "applied") return undefined;
    const timer = window.setTimeout(() => {
      if (useAiUiStore.getState().proposal?.id === proposal.id) closeCard();
    }, APPLIED_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [proposal?.status, proposal?.id, closeCard]);

  // Esc reverts a pending preview.
  useEffect(() => {
    if (proposal?.status !== "previewing") return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) discardProposal();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [proposal?.status, discardProposal]);

  const author =
    proposal?.author === "Manager" ? t("card.fromManager") : proposal?.author === "Agent" ? t("card.fromAgent") : t("card.fromUser");
  const status = proposal?.status;
  const loading = status === "previewing" && !proposal?.preview;

  return (
    <MotionCard
      show={proposal !== null}
      from="right"
      className={clsx("aiui-card", status === "invalid" && "aiui-card--invalid", status === "applied" && "aiui-card--applied")}
      role="dialog"
      aria-modal="false"
      aria-labelledby="aiui-card-title"
    >
      {proposal && status === "applied" ? (
        <div className="aiui-card__applied">
          <span className="aiui-card__orb aiui-card__orb--ok" aria-hidden="true">
            <Check size={16} />
          </span>
          <div className="aiui-card__applied-text">
            <div id="aiui-card-title" className="aiui-card__title">
              {t("card.applied")}
            </div>
            <div className="aiui-card__hint">{t("card.appliedHint")}</div>
          </div>
          <Button size="sm" variant="secondary" leftIcon={<Undo2 size={13} />} disabled={busy} onClick={() => void undoLast().then(() => closeCard())}>
            {t("card.undo")}
          </Button>
          <button type="button" className="aiui-card__close" aria-label={t("card.close")} onClick={closeCard}>
            <X size={14} />
          </button>
        </div>
      ) : proposal ? (
        <>
          <header className="aiui-card__head">
            <span className="aiui-card__orb" aria-hidden="true">
              <Sparkles size={16} />
            </span>
            <div className="aiui-card__heading">
              <div id="aiui-card-title" className="aiui-card__title">
                {status === "invalid" ? t("card.invalidTitle") : t("card.title")}
              </div>
              <div className="aiui-card__author">{author}</div>
            </div>
            {status !== "invalid" && (
              <span className={clsx("aiui-live", !previewVisible && "aiui-live--off")}>
                <span className="aiui-live__dot" aria-hidden="true" />
                {previewVisible ? t("card.livePreview") : t("card.compareActive")}
              </span>
            )}
          </header>

          <p className="aiui-card__summary">{proposal.summary}</p>

          {status === "invalid" ? (
            <p className="aiui-card__error" role="alert">
              {proposal.error}
            </p>
          ) : loading ? (
            <div className="aiui-card__loading">
              <Spinner size={14} /> {t("card.preparing")}
            </div>
          ) : (
            <>
              <div className="aiui-card__label">{t("card.changesLabel")}</div>
              {changes.length > 0 ? (
                <ul className="aiui-changes">
                  {changes.map((c, i) => (
                    <ChangeLine key={i} c={c} />
                  ))}
                </ul>
              ) : (
                <p className="aiui-card__hint">{t("card.noChanges")}</p>
              )}
              {proposal.error && (
                <p className="aiui-card__error" role="alert">
                  {proposal.error}
                </p>
              )}
            </>
          )}

          <div className="aiui-card__safety">
            <ShieldCheck size={12} aria-hidden="true" />
            <span>{t("card.safety")}</span>
          </div>

          <footer className="aiui-card__actions">
            {status === "invalid" ? (
              <Button size="sm" variant="secondary" onClick={discardProposal}>
                {t("card.dismiss")}
              </Button>
            ) : (
              <>
                <button
                  type="button"
                  className="aiui-compare"
                  aria-pressed={!previewVisible}
                  title={t("card.compareHint")}
                  disabled={loading}
                  onClick={() => setPreviewVisible(!previewVisible)}
                >
                  {previewVisible ? <EyeOff size={13} /> : <Eye size={13} />}
                  <span>{t("card.compare")}</span>
                </button>
                <span className="aiui-card__spacer" />
                <Button size="sm" variant="ghost" onClick={discardProposal} disabled={status === "applying"}>
                  {t("card.revert")}
                </Button>
                <Button
                  size="sm"
                  variant="primary"
                  leftIcon={<Check size={13} />}
                  loading={status === "applying"}
                  disabled={loading || changes.length === 0}
                  onClick={() => void keepProposal()}
                >
                  {status === "applying" ? t("card.applying") : t("card.keep")}
                </Button>
              </>
            )}
          </footer>
        </>
      ) : null}
    </MotionCard>
  );
}
