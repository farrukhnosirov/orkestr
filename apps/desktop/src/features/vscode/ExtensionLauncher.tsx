/**
 * Extension launcher UI: one card per VS Code extension (icon, description,
 * «Open extension panel», «Extension settings») with ITS COMMANDS AS
 * BUTTONS (title + category, hidden/dangerous markers, keybinding, pin),
 * searchable across all extensions. Used by the VS Code panel and the
 * Extensions panel. The confirmation dialog for dangerous commands and the
 * result toast are separate components mounted once (see `LauncherHost`).
 */
import type { VsCodeCommandInfo, VsCodeExtensionCommands } from "@orkestr/protocol";
import { Button, IconButton, Input, Spinner, StatusDot } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  AlertTriangle,
  CheckCircle2,
  EyeOff,
  PanelLeftOpen,
  Pin,
  PinOff,
  Puzzle,
  RefreshCw,
  Search,
  Settings2,
  ShieldAlert,
  TerminalSquare,
  X,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useLanguage, useT } from "../../i18n";
import { ActionDialog } from "../files/Dialog";
import { commandLabel, commandMatches, extensionMatches, useLauncherStore } from "./launcher";
import { useVsCodeStore } from "./store";
import "./launcher.css";

/** Commands shown per card before «Show all». */
export const COLLAPSED_COMMANDS = 9;

function CommandButton({ ext, cmd, pinned }: { ext: VsCodeExtensionCommands; cmd: VsCodeCommandInfo; pinned: boolean }) {
  const { t } = useT("vscodeLauncher");
  const running = useLauncherStore((s) => s.running === cmd.id);
  const busy = useLauncherStore((s) => s.running !== null);
  const run = () => void useLauncherStore.getState().launch({ kind: "command", commandId: cmd.id, title: cmd.title });
  return (
    <li className={clsx("vxl-cmd", cmd.dangerous && "vxl-cmd--danger")}>
      <button
        type="button"
        className="vxl-cmd__run"
        onClick={run}
        disabled={busy && !running}
        aria-label={t("card.run", { title: commandLabel(cmd) })}
        title={cmd.id}
        data-command-id={cmd.id}
      >
        <span className="vxl-cmd__icon" aria-hidden="true">
          {running ? <Spinner size={13} /> : cmd.dangerous ? <ShieldAlert size={14} /> : <TerminalSquare size={14} />}
        </span>
        <span className="vxl-cmd__text">
          <span className="vxl-cmd__title">{cmd.title}</span>
          <span className="vxl-cmd__meta">
            {cmd.category ?? ext.displayName}
            {cmd.keybinding && <kbd className="vxl-kbd">{cmd.keybinding}</kbd>}
          </span>
        </span>
        {cmd.hidden && (
          <span className="vxl-tag" title={t("card.hiddenHint")}>
            <EyeOff size={11} aria-hidden="true" /> {t("card.hidden")}
          </span>
        )}
        {cmd.dangerous && <span className="vxl-sr">{t("card.dangerous")}</span>}
      </button>
      <IconButton
        className={clsx("vxl-cmd__pin", pinned && "vxl-cmd__pin--on")}
        size="sm"
        label={pinned ? t("card.unpin") : t("card.pin")}
        icon={pinned ? <PinOff size={12} /> : <Pin size={12} />}
        onClick={() => void useLauncherStore.getState().togglePin(cmd.id)}
      />
    </li>
  );
}

function ExtensionCard({
  ext,
  commands,
  pins,
  expandedByDefault,
}: {
  ext: VsCodeExtensionCommands;
  commands: VsCodeCommandInfo[];
  pins: ReadonlySet<string>;
  expandedByDefault: boolean;
}) {
  const { t } = useT("vscodeLauncher");
  const [expanded, setExpanded] = useState(expandedByDefault);
  const running = useLauncherStore((s) => s.running);
  const shown = expanded ? commands : commands.slice(0, COLLAPSED_COMMANDS);
  const view = ext.viewContainers[0];
  return (
    <article className="vxl-card" aria-labelledby={`vxl-${ext.id}`} data-extension-id={ext.id}>
      <header className="vxl-card__head">
        {ext.icon ? (
          <img className="vxl-card__icon" src={ext.icon} alt="" aria-hidden="true" draggable={false} />
        ) : (
          <span className="vxl-card__icon vxl-card__icon--placeholder" aria-hidden="true">
            <Puzzle size={20} />
          </span>
        )}
        <div className="vxl-card__title">
          <h5 id={`vxl-${ext.id}`}>{ext.displayName}</h5>
          <span className="vxl-muted">
            {ext.publisher}
            {ext.version && ` · ${ext.version}`} · {t("card.commands", { count: ext.commands.length })}
          </span>
        </div>
      </header>
      {ext.description && <p className="vxl-card__desc">{ext.description}</p>}
      <div className="vxl-card__actions">
        <Button
          size="sm"
          variant="primary"
          leftIcon={<PanelLeftOpen size={13} />}
          loading={running === `view:${ext.id}`}
          disabled={running !== null}
          onClick={() =>
            void useLauncherStore
              .getState()
              .launch({ kind: "view", extensionId: ext.id, title: view?.title ?? ext.displayName })
          }
        >
          {view ? t("card.openView") : t("card.openPage")}
        </Button>
        {ext.viewContainers.slice(1, 3).map((v) => (
          <Button
            key={v.id}
            size="sm"
            variant="secondary"
            disabled={running !== null}
            onClick={() =>
              void useLauncherStore
                .getState()
                .launch({ kind: "view", extensionId: ext.id, containerId: v.id, title: v.title })
            }
          >
            {t("card.openViewNamed", { name: v.title })}
          </Button>
        ))}
        {ext.hasSettings && (
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<Settings2 size={13} />}
            loading={running === `settings:${ext.id}`}
            disabled={running !== null}
            onClick={() =>
              void useLauncherStore
                .getState()
                .launch({ kind: "settings", extensionId: ext.id, title: t("card.settings") })
            }
          >
            {t("card.settings")}
          </Button>
        )}
      </div>
      {commands.length === 0 ? (
        <p className="vxl-muted">{t("card.noCommands")}</p>
      ) : (
        <ul className="vxl-cmds">
          {shown.map((cmd) => (
            <CommandButton key={cmd.id} ext={ext} cmd={cmd} pinned={pins.has(cmd.id)} />
          ))}
        </ul>
      )}
      {commands.length > COLLAPSED_COMMANDS && (
        <Button size="sm" variant="ghost" onClick={() => setExpanded((v) => !v)}>
          {expanded ? t("card.showLess") : t("card.showAll", { count: commands.length })}
        </Button>
      )}
    </article>
  );
}

/** Bridge state line (connected / closed / opening). */
export function BridgeState() {
  const { t } = useT("vscodeLauncher");
  const connected = useVsCodeStore((s) => s.status?.bridgeConnected ?? false);
  const opening = useLauncherStore((s) => s.opening);
  return (
    <span className="vxl-bridge" role="status">
      {opening ? <Spinner size={12} /> : <StatusDot status={connected ? "online" : "idle"} />}
      {opening ? t("bridge.opening") : connected ? t("bridge.connected") : t("bridge.disconnected")}
    </span>
  );
}

/**
 * The launcher: search + extension cards. `only` limits it to one extension
 * (Extensions panel details).
 */
export function ExtensionLauncher({ only, showHeader = true }: { only?: string; showHeader?: boolean }) {
  const { t } = useT("vscodeLauncher");
  const language = useLanguage();
  const extensions = useLauncherStore((s) => s.extensions);
  const loading = useLauncherStore((s) => s.loading);
  const error = useLauncherStore((s) => s.error);
  const trusted = useLauncherStore((s) => s.workspaceTrusted);
  const installed = useVsCodeStore((s) => s.status?.installed != null);
  const statusLoaded = useVsCodeStore((s) => s.status !== null);
  const bridge = useVsCodeStore((s) => s.status?.bridgeConnected ?? false);
  const pinsList = useVsCodeStore((s) => s.status?.config.pinnedCommands);
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!useVsCodeStore.getState().status) void useVsCodeStore.getState().load();
  }, []);
  // Reload on language change and when a window connects (runtime-only commands appear).
  useEffect(() => {
    if (installed) void useLauncherStore.getState().load(true);
  }, [installed, language, bridge]);

  const pins = useMemo(() => new Set(pinsList ?? []), [pinsList]);
  const cards = useMemo(() => {
    const list = (extensions ?? []).filter((e) => !only || e.id === only.toLowerCase());
    return list
      .map((ext) => ({
        ext,
        // A query naming the extension shows all of its commands.
        commands: extensionMatches(ext, query) ? ext.commands : ext.commands.filter((c) => commandMatches(ext, c, query)),
      }))
      .filter((c) => !query.trim() || c.commands.length > 0);
  }, [extensions, only, query]);

  if (statusLoaded && !installed) {
    return only ? null : <p className="vxl-muted">{t("notInstalled")}</p>;
  }

  return (
    <section className={clsx("vxl", only && "vxl--single")} aria-label={t("title")}>
      {showHeader && (
        <div className="vxl-head">
          <div>
            <h4>{t("title")}</h4>
            {!only && <p className="vxl-muted">{t("subtitle")}</p>}
          </div>
          <span className="vxl-spacer" />
          <BridgeState />
          <IconButton
            label={t("refresh")}
            icon={<RefreshCw size={14} />}
            tooltip="bottom"
            onClick={() => void useLauncherStore.getState().load(true)}
          />
        </div>
      )}
      {!only && (
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("searchPlaceholder")}
          aria-label={t("searchLabel")}
          leftIcon={<Search size={13} />}
          spellCheck={false}
        />
      )}
      {trusted === false && bridge && (
        <div className="vxl-warn" role="status">
          <ShieldAlert size={14} aria-hidden="true" />
          <span>
            <strong>{t("restricted.title")}</strong> {t("restricted.body")}
          </span>
          <Button
            size="sm"
            variant="secondary"
            onClick={() =>
              void useLauncherStore
                .getState()
                .launch({ kind: "command", commandId: "workbench.trust.manage", title: t("restricted.manage") })
            }
          >
            {t("restricted.manage")}
          </Button>
        </div>
      )}
      {error && (
        <div className="vxl-alert" role="alert">
          <AlertTriangle size={14} aria-hidden="true" /> <span>{error}</span>
        </div>
      )}
      {extensions === null ? (
        loading ? (
          <p className="vxl-row vxl-muted">
            <Spinner size={14} /> {t("loading")}
          </p>
        ) : null
      ) : cards.length === 0 ? (
        only ? null : (
          <div className="vxl-empty">
            <p>{query.trim() ? t("noMatches", { query: query.trim() }) : t("empty")}</p>
            {!query.trim() && <p className="vxl-muted">{t("emptyHint")}</p>}
          </div>
        )
      ) : (
        <div className="vxl-grid">
          {cards.map(({ ext, commands }) => (
            <ExtensionCard
              key={ext.id}
              ext={ext}
              commands={commands}
              pins={pins}
              expandedByDefault={Boolean(only) || query.trim().length > 0}
            />
          ))}
        </div>
      )}
    </section>
  );
}

/** Confirmation for dangerous commands (mounted once, app-wide). */
export function LauncherConfirmDialog() {
  const { t } = useT("vscodeLauncher");
  const req = useLauncherStore((s) => s.confirm);
  if (!req) return null;
  const cancel = () => useLauncherStore.getState().resolveConfirm(false);
  return (
    <ActionDialog
      title={t("confirm.title", { title: req.title })}
      subtitle={t("confirm.subtitle")}
      icon={<ShieldAlert size={18} />}
      tone="warning"
      onCancel={cancel}
      actions={[
        { id: "cancel", label: t("confirm.cancel"), variant: "ghost", autoFocus: true, onSelect: cancel },
        { id: "run", label: t("confirm.run"), variant: "danger", onSelect: () => useLauncherStore.getState().resolveConfirm(true) },
      ]}
    >
      <p>{t("confirm.body")}</p>
      {req.kind === "command" && <p className="vxl-mono">{t("confirm.id", { id: req.commandId })}</p>}
    </ActionDialog>
  );
}

/** Result of the last launch (auto-hides after a few seconds). */
export function LauncherToast() {
  const { t } = useT("vscodeLauncher");
  const last = useLauncherStore((s) => s.last);
  useEffect(() => {
    if (!last) return;
    const id = setTimeout(() => useLauncherStore.getState().clearLast(), last.ok ? 3500 : 9000);
    return () => clearTimeout(id);
  }, [last]);
  if (!last) return null;
  return (
    <div className={clsx("vxl-toast", !last.ok && "vxl-toast--error")} role={last.ok ? "status" : "alert"}>
      {last.ok ? <CheckCircle2 size={14} aria-hidden="true" /> : <AlertTriangle size={14} aria-hidden="true" />}
      <span>
        {last.ok ? t("result.ok", { title: last.title }) : t("result.failed", { title: last.title, message: last.message ?? "" })}
      </span>
      <IconButton size="sm" label={t("result.dismiss")} icon={<X size={12} />} onClick={() => useLauncherStore.getState().clearLast()} />
    </div>
  );
}

/** Dialog + toast, mounted once by the shell. */
export function LauncherHost() {
  return (
    <>
      <LauncherConfirmDialog />
      <LauncherToast />
    </>
  );
}
