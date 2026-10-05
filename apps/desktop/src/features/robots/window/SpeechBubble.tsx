/**
 * The robot's speech bubble: a small glass card above the robot with a tail
 * pointing at it. Text is an i18n key from `model/bubble.ts`; it fades in /
 * out with `motion` (instant with reduced motion).
 */
import { clsx } from "clsx";
import { AnimatePresence, motion } from "motion/react";
import { forwardRef } from "react";

import { useT } from "../../../i18n";
import type { BubbleSpec } from "../model/bubble";

export interface SpeechBubbleProps {
  bubble: BubbleSpec | null;
  /** Bump to re-run the enter animation for a new bubble with the same key. */
  nonce: number;
  reducedMotion: boolean;
  onClick?: () => void;
}

export const SpeechBubble = forwardRef<HTMLDivElement, SpeechBubbleProps>(function SpeechBubble(
  { bubble, nonce, reducedMotion, onClick },
  ref,
) {
  const { t } = useT("robots");
  return (
    <div className="robot-bubble-slot" aria-live="polite">
      <AnimatePresence>
        {bubble && (
          <motion.div
            key={`${bubble.key}-${nonce}`}
            ref={ref}
            className={clsx("robot-bubble", `robot-bubble--${bubble.tone}`)}
            initial={reducedMotion ? false : { opacity: 0, y: 10, scale: 0.92 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reducedMotion ? { opacity: 0, transition: { duration: 0 } } : { opacity: 0, y: 6, scale: 0.96 }}
            transition={{ type: "spring", stiffness: 420, damping: 30 }}
            onClick={onClick}
          >
            <span className="robot-bubble__dot" aria-hidden="true" />
            <span className="robot-bubble__text">{t(`bubble.${bubble.key}`, bubble.values)}</span>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
});
