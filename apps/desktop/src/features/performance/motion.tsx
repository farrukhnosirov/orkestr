/**
 * Motion system (§28): one set of transitions for panels, cards, lists and
 * overlays, scaled by the Performance Mode policy (`durationScale`; 0 =
 * instant) so every animated surface degrades together and honours
 * `prefers-reduced-motion` (already folded into the policy).
 *
 * Use `<MotionCard>` / `<MotionList>` + `<MotionItem>` or `usePerfTransition`
 * with `motion/react` directly. Animations only touch `opacity` and
 * `transform` (compositor-only; no layout thrash on the UI thread).
 */
import { AnimatePresence, motion, type Transition } from "motion/react";
import type { HTMLAttributes, ReactNode } from "react";

import { usePerfPolicy } from "./perfStore";

/** Base durations in seconds (match `--ok-duration-*`). */
export const DURATIONS = { fast: 0.12, base: 0.2, slow: 0.36 } as const;
/** `--ok-ease` as a cubic bezier. */
export const EASE: [number, number, number, number] = [0.2, 0.8, 0.2, 1];

/** A transition for a policy scale (pure; tested). */
export function scaledTransition(scale: number, speed: keyof typeof DURATIONS = "base", spring = false): Transition {
  const duration = DURATIONS[speed] * Math.max(0, scale);
  if (duration === 0) return { duration: 0 };
  if (spring && scale >= 1) return { type: "spring", stiffness: 420, damping: 32, mass: 0.8 };
  return { duration, ease: EASE };
}

/** The transition for the current Performance Mode. */
export function usePerfTransition(speed: keyof typeof DURATIONS = "base", spring = false): Transition {
  const policy = usePerfPolicy();
  return scaledTransition(policy.durationScale, speed, spring);
}

/** Offsets used by the enter/exit variants, shrunk when motion is reduced. */
export function motionOffset(level: "full" | "reduced" | "off"): number {
  return level === "full" ? 10 : level === "reduced" ? 4 : 0;
}

type DivProps = Omit<HTMLAttributes<HTMLDivElement>, "onAnimationStart" | "onDrag" | "onDragStart" | "onDragEnd" | "onDragEnter" | "onDragLeave" | "onDragOver" | "onDrop">;

export interface MotionCardProps extends DivProps {
  /** Whether the card is shown (enter/exit are animated). */
  show: boolean;
  /** Slide in from: below (default), above, the right. */
  from?: "below" | "above" | "right";
  children: ReactNode;
}

/** A card / popover that fades and slides in and out. */
export function MotionCard({ show, from = "below", children, ...rest }: MotionCardProps) {
  const policy = usePerfPolicy();
  const transition = scaledTransition(policy.durationScale, "base", true);
  const d = motionOffset(policy.motion);
  const offset = from === "right" ? { x: d * 2, y: 0 } : { x: 0, y: from === "above" ? -d : d };
  return (
    <AnimatePresence initial={policy.motion !== "off"}>
      {show && (
        <motion.div
          {...rest}
          initial={{ opacity: 0, scale: 0.98, ...offset }}
          animate={{ opacity: 1, scale: 1, x: 0, y: 0 }}
          exit={{ opacity: 0, scale: 0.98, ...offset, transition: scaledTransition(policy.durationScale, "fast") }}
          transition={transition}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Animated list item (enter / exit); wrap a list's items in `<AnimatePresence>`. */
export function MotionItem({ children, index = 0, ...rest }: DivProps & { index?: number; children: ReactNode }) {
  const policy = usePerfPolicy();
  const base = scaledTransition(policy.durationScale, "base");
  const delay = policy.motion === "full" ? Math.min(index, 8) * 0.03 : 0;
  const d = motionOffset(policy.motion);
  return (
    <motion.div
      {...rest}
      layout={policy.motion === "full" ? "position" : false}
      initial={{ opacity: 0, y: d }}
      animate={{ opacity: 1, y: 0, transition: { ...base, delay } }}
      exit={{ opacity: 0, y: -d / 2, transition: scaledTransition(policy.durationScale, "fast") }}
    >
      {children}
    </motion.div>
  );
}

export { AnimatePresence };
