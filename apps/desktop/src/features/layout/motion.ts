/**
 * Motion policy for the layout UI (§31, §70): animations are off under
 * `prefers-reduced-motion`, in Performance / Battery Saver mode and when
 * the theme disables animations.
 */
import { useReducedMotion } from "motion/react";
import { useEffect, useState } from "react";

function calmFromDom(): boolean {
  if (typeof document === "undefined") return true;
  const ds = document.documentElement.dataset;
  return ds["okPerformance"] === "Performance" || ds["okPerformance"] === "BatterySaver" || ds["okAnimations"] === "off";
}

/** True when the layout UI should not animate. */
export function useCalmMotion(): boolean {
  const reduced = useReducedMotion() ?? false;
  const [calm, setCalm] = useState(calmFromDom);
  useEffect(() => {
    if (typeof MutationObserver === "undefined") return;
    const obs = new MutationObserver(() => setCalm(calmFromDom()));
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ["data-ok-performance", "data-ok-animations"] });
    return () => obs.disconnect();
  }, []);
  return reduced || calm;
}
