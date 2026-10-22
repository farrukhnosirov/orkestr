import { Button, EmptyState } from "@orkestr/ui";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { type RemoteClient, errorCode } from "../remote";

/** Remote terminal (xterm over `terminal.*`); requires the device's remote-terminal permission. */
export function TerminalPanel({ client, projectId }: { client: RemoteClient; projectId: string | null }) {
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement | null>(null);
  const [terminalId, setTerminalId] = useState<string | null>(null);
  const [state, setState] = useState<"idle" | "opening" | "disabled" | "error">("idle");

  const open = async () => {
    setState("opening");
    try {
      const r = await client.call("terminal.open", { ...(projectId ? { projectId } : {}), cols: 100, rows: 30 });
      setTerminalId(r.terminal.terminalId);
      setState("idle");
    } catch (e) {
      setState(errorCode(e) === "forbidden" ? "disabled" : "error");
    }
  };

  useEffect(() => {
    if (!terminalId || !host.current) return;
    let disposed = false;
    let cleanup = () => {};
    void import("@xterm/xterm").then(({ Terminal }) => {
      if (disposed || !host.current) return;
      const term = new Terminal({ cols: 100, rows: 30, convertEol: false, fontFamily: "var(--ok-font-mono)" });
      term.open(host.current);
      const input = term.onData((data) => {
        void client.call("terminal.write", { terminalId, data }).catch(() => undefined);
      });
      const unsub = client.subscribe([`terminal:${terminalId}`], (ev) => {
        if (ev.kind === "TerminalOutput") term.write(ev.payload.data);
        if (ev.kind === "TerminalExited") term.write(`\r\n[${t("terminal.exited")}]\r\n`);
      });
      cleanup = () => {
        input.dispose();
        unsub();
        term.dispose();
      };
    });
    return () => {
      disposed = true;
      cleanup();
      void client.call("terminal.close", { terminalId }).catch(() => undefined);
    };
  }, [client, terminalId, t]);

  if (state === "disabled") {
    return <EmptyState title={t("terminal.disabled")} description={t("terminal.disabledHint")} />;
  }
  return (
    <section aria-label={t("tabs.terminal")}>
      {!terminalId && (
        <Button variant="primary" loading={state === "opening"} onClick={() => void open()}>
          {t("terminal.open")}
        </Button>
      )}
      {state === "error" && <p role="alert" className="web-error">{t("errors.internal")}</p>}
      <div ref={host} className="web-terminal" />
    </section>
  );
}
