/**
 * One xterm.js view bound to a core terminal session.
 *
 * Reconnect-safe: it subscribes to `terminal:<id>` first (buffering), then
 * replays `terminal.history` and de-duplicates buffered/live chunks by their
 * byte `offset`. Input goes to `terminal.write` (user sessions only); size
 * changes go to `terminal.resize`.
 */
import "@xterm/xterm/css/xterm.css";

import type { OrkestrClient } from "@orkestr/client";
import type { Event, TerminalSession } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Spinner } from "@orkestr/ui";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { ClipboardPaste, Copy, Eraser, TextSelect } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { readClipboardText, useContextMenu, writeClipboardText } from "../../features/contextMenu";

import { t as i18nT, useT } from "../../i18n";
import { terminalTitle } from "../../lib/coreText";
import { useThemeRevision } from "../../features/theme/useThemeRevision";
import { dedupeChunk, errorMessage, isReadOnly, terminalFontFamily, terminalKeyAction, themeFromTokens } from "./logic";

export interface TerminalViewProps {
  client: OrkestrClient;
  session: TerminalSession;
  /** Whether this tab is visible (inactive views stay mounted to keep scrollback). */
  active: boolean;
  /** Called when the session's process exits. */
  onExited: (terminalId: string, exitCode: number | null) => void;
}

const RESIZE_DEBOUNCE_MS = 60;

export function TerminalView({ client, session, active, onExited }: TerminalViewProps) {
  const { t } = useT(["terminal", "contextMenu"]);
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Re-derive from tokens on every theme apply (switches and live editor previews).
  const themeId = useThemeRevision();
  const onExitedRef = useRef(onExited);
  onExitedRef.current = onExited;
  const readOnly = isReadOnly(session);
  const running = session.status === "running";
  const runningRef = useRef(running);
  runningRef.current = running;
  const terminalId = session.terminalId;

  // Create the xterm instance, wire the stream (once per session).
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let off: (() => void) | null = null;
    const cleanups: Array<() => void> = [];

    void (async () => {
      const [{ Terminal: XTerm }, { FitAddon: Fit }] = await Promise.all([
        import("@xterm/xterm"),
        import("@xterm/addon-fit"),
      ]);
      if (disposed) return;
      const term = new XTerm({
        allowTransparency: true,
        cursorBlink: !readOnly,
        disableStdin: readOnly,
        fontFamily: terminalFontFamily(),
        fontSize: 13,
        lineHeight: 1.15,
        scrollback: 10_000,
        theme: themeFromTokens(),
        macOptionIsMeta: true,
      });
      const fit = new Fit();
      term.loadAddon(fit);
      term.open(host);
      termRef.current = term;
      fitRef.current = fit;

      // Copy / paste: Ctrl+Shift+C/V always; Ctrl+C copies when there is a selection.
      term.attachCustomKeyEventHandler((e) => {
        if (e.type !== "keydown") return true;
        const action = terminalKeyAction(e, term.hasSelection());
        if (action === "global") return false; // let the global "Open Terminal" shortcut through
        if (action === "copy") {
          const sel = term.getSelection();
          if (sel) void navigator.clipboard?.writeText(sel).catch(() => undefined);
          if (!e.shiftKey) term.clearSelection();
          e.preventDefault();
          return false;
        }
        if (action === "paste") {
          e.preventDefault();
          if (!readOnly && runningRef.current) {
            void navigator.clipboard
              ?.readText()
              .then((text) => {
                if (text) term.paste(text);
              })
              .catch(() => undefined);
          }
          return false;
        }
        return true;
      });

      // Input → core.
      const inputSub = term.onData((data) => {
        if (readOnly || !runningRef.current) return;
        client.call("terminal.write", { terminalId, data }).catch((e: unknown) => {
          term.write(`\r\n\x1b[31m[${i18nT("terminal:view.inputFailed", { error: errorMessage(e) })}]\x1b[0m\r\n`);
        });
      });
      cleanups.push(() => inputSub.dispose());

      // Stream: subscribe first, buffer until history is written.
      let written = -1;
      const pending: Event[] = [];
      const apply = (ev: Event) => {
        if (ev.kind === "TerminalOutput") {
          const r = dedupeChunk(written, ev.payload.offset, ev.payload.data);
          written = r.written;
          if (r.text) term.write(r.text);
        } else if (ev.kind === "TerminalExited") {
          onExitedRef.current(terminalId, ev.payload.exitCode);
        }
      };
      off = client.subscribe([topic.terminal(terminalId)], (ev) => {
        if (written < 0) pending.push(ev);
        else apply(ev);
      });

      try {
        const hist = await client.call("terminal.history", { terminalId });
        if (disposed) return;
        if (hist.truncated) term.write("\x1b[2m[… earlier output trimmed]\x1b[0m\r\n");
        term.write(hist.data);
        written = hist.endOffset;
        for (const ev of pending.splice(0)) apply(ev);
        setReady(true);
      } catch (e) {
        if (!disposed) setError(errorMessage(e));
        written = 0;
      }

      // Size → core.
      let timer: ReturnType<typeof setTimeout> | null = null;
      let last = { cols: session.cols, rows: session.rows };
      const fitNow = () => {
        if (!host.isConnected || host.clientWidth === 0 || host.clientHeight === 0) return;
        try {
          fit.fit();
        } catch {
          return;
        }
        const { cols, rows } = term;
        if (cols === last.cols && rows === last.rows) return;
        last = { cols, rows };
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
          if (!runningRef.current) return;
          client.call("terminal.resize", { terminalId, cols, rows }).catch((err: unknown) => {
            console.warn("terminal.resize failed", err);
          });
        }, RESIZE_DEBOUNCE_MS);
      };
      const ro = new ResizeObserver(() => fitNow());
      ro.observe(host);
      fitNow();
      cleanups.push(() => {
        ro.disconnect();
        if (timer) clearTimeout(timer);
      });
    })().catch((e: unknown) => {
      if (!disposed) setError(errorMessage(e));
    });

    return () => {
      disposed = true;
      off?.();
      for (const c of cleanups) c();
      termRef.current?.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // The view is bound to one session id for its lifetime (readOnly/size are
    // read once at creation and updated by the effects below).
  }, [client, terminalId]);

  // Theme follows the app theme.
  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = themeFromTokens();
  }, [themeId, ready]);

  // Refit + focus when the tab becomes visible.
  useEffect(() => {
    if (!active) return;
    const id = requestAnimationFrame(() => {
      try {
        fitRef.current?.fit();
      } catch {
        // Not measurable yet.
      }
      if (!readOnly) termRef.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [active, ready, readOnly]);

  // Read-only once exited.
  useEffect(() => {
    const term = termRef.current;
    if (term) {
      term.options.disableStdin = readOnly || !running;
      term.options.cursorBlink = !readOnly && running;
    }
  }, [readOnly, running, ready]);

  // Orkestr context menu: Copy, Paste, Clear, Select all.
  const menu = useContextMenu(
    () => {
      const term = termRef.current;
      if (!term) return [];
      const canWrite = !readOnly && runningRef.current;
      return [
        { id: "term.copy", label: t("contextMenu:terminal.copy"), icon: <Copy size={14} />, shortcut: "Ctrl+Shift+C", disabled: !term.hasSelection(), onSelect: () => void writeClipboardText(term.getSelection()) },
        {
          id: "term.paste",
          label: t("contextMenu:terminal.paste"),
          icon: <ClipboardPaste size={14} />,
          shortcut: "Ctrl+Shift+V",
          disabled: !canWrite,
          onSelect: async () => {
            const text = await readClipboardText();
            if (text) term.paste(text);
            term.focus();
          },
        },
        { kind: "separator", id: "term.s1" },
        { id: "term.selectAll", label: t("contextMenu:terminal.selectAll"), icon: <TextSelect size={14} />, onSelect: () => term.selectAll() },
        { id: "term.clear", label: t("contextMenu:terminal.clear"), icon: <Eraser size={14} />, onSelect: () => term.clear() },
      ];
    },
    { replaceDefaults: true },
  );

  return (
    <div className="term-view" hidden={!active} data-terminal-id={terminalId} {...menu}>
      <div
        ref={hostRef}
        className="term-view__host"
        role="application"
        aria-label={readOnly ? t("view.labelReadOnly", { title: terminalTitle(session.title) }) : t("view.label", { title: terminalTitle(session.title) })}
      />
      {!ready && !error && (
        <div className="term-view__overlay">
          <Spinner size={18} /> {t("view.connecting")}
        </div>
      )}
      {error && (
        <div className="term-view__overlay term-view__overlay--error" role="alert">
          {t("view.attachFailed", { error })}
        </div>
      )}
    </div>
  );
}
