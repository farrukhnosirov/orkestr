/**
 * Mounts the startup animation (ADR-0053) in the main window: the version is
 * decided once, before the first paint; a settings preview replays it over
 * the running app.
 */
import { useEffect, useState } from "react";

import { BootOverlay } from "./BootOverlay";
import { APP_VERSION, planBoot, type BootPlan } from "./plan";
import { writeLastVersion } from "./prefs";
import { useBootStore } from "./store";

export function BootHost({ ready, compact }: { ready: boolean; compact: boolean }) {
  const [plan] = useState<BootPlan>(planBoot);
  const [playing, setPlaying] = useState(plan.variant !== "none");
  const preview = useBootStore((s) => s.preview);

  useEffect(() => {
    if (plan.variant === "none") useBootStore.getState().finish();
  }, [plan.variant]);
  // The version counts as seen once it started (first-launch detection).
  useEffect(() => {
    if (ready) writeLastVersion(APP_VERSION);
  }, [ready]);

  // Debug hook (`localStorage["orkestr.debugHooks"] = "1"`): replay a version for screenshots.
  useEffect(() => {
    let on = false;
    try {
      on = window.localStorage.getItem("orkestr.debugHooks") === "1";
    } catch {
      on = false;
    }
    if (!on) return undefined;
    const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
    w.__orkestrDebug = { ...(w.__orkestrDebug ?? {}), bootPlay: (v: "full" | "short" | "static" | null) => useBootStore.getState().setPreview(v) };
    return () => {
      if (w.__orkestrDebug) delete w.__orkestrDebug.bootPlay;
    };
  }, []);

  if (playing && plan.variant !== "none") {
    return <BootOverlay plan={{ ...plan, variant: plan.variant }} ready={ready} compact={compact} onDone={() => setPlaying(false)} />;
  }
  if (preview && ready) {
    return (
      <BootOverlay
        key={`preview-${preview}`}
        plan={{ ...plan, variant: preview, firstLaunch: true }}
        ready
        compact={compact}
        preview
        onDone={() => useBootStore.getState().setPreview(null)}
      />
    );
  }
  return null;
}
