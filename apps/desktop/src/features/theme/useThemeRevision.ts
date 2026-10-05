import { onThemeApplied, themeRevision } from "@orkestr/ui";
import { useEffect, useState } from "react";

/**
 * Re-renders whenever the theme engine applies a theme (switches and live
 * editor previews), so Monaco/xterm can re-derive their colors from tokens.
 */
export function useThemeRevision(): number {
  const [rev, setRev] = useState(themeRevision);
  useEffect(() => onThemeApplied((d) => setRev(d.revision)), []);
  return rev;
}
