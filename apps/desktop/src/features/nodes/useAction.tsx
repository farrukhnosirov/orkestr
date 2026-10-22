/** Busy / notice state for the Nodes panel actions (RR-G14 aware). */
import { useCallback, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { isUnimplemented } from "./logic";

export type Notice = { tone: "ok" | "error"; text: string } | null;

export function useAction(after?: () => Promise<void> | void) {
  const { t } = useT("nodes");
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const run = useCallback(
    async (key: string, fn: () => Promise<string | null>) => {
      setBusy(key);
      setNotice(null);
      try {
        const text = await fn();
        if (text) setNotice({ tone: "ok", text });
      } catch (e) {
        setNotice({ tone: "error", text: isUnimplemented(e) ? t("notAvailable") : describeError(e) });
      } finally {
        setBusy(null);
        await after?.();
      }
    },
    [after, t],
  );
  return { busy, notice, setNotice, run };
}

export function NoticeLine({ notice }: { notice: Notice }) {
  if (!notice) return null;
  return (
    <p className={notice.tone === "ok" ? "nd-ok" : "nd-error"} role={notice.tone === "ok" ? "status" : "alert"} data-testid="nodes-notice">
      {notice.text}
    </p>
  );
}
