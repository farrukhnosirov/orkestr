/**
 * Pinned VS Code extension commands (`VsCodeConfig.pinnedCommands`): icon
 * buttons on the left rail and a quick-actions row (Mission Control widget).
 */
import { Button, IconButton } from "@orkestr/ui";
import { TerminalSquare } from "lucide-react";
import { useEffect } from "react";

import { useLanguage, useT } from "../../i18n";
import { commandLabel, findCommand, useLauncherStore } from "./launcher";
import { useVsCodeStore } from "./store";
import "./launcher.css";

/** Two-letter monogram of a command title for the rail tile. */
export function commandMonogram(title: string): string {
  const words = title.replace(/[^\p{L}\p{N}\s]/gu, " ").split(/\s+/).filter(Boolean);
  const a = words[0]?.[0] ?? "?";
  const b = words[1]?.[0] ?? words[0]?.[1] ?? "";
  return `${a}${b}`.toUpperCase();
}

function usePinned() {
  const language = useLanguage();
  const pins = useVsCodeStore((s) => s.status?.config.pinnedCommands);
  const installed = useVsCodeStore((s) => s.status?.installed != null);
  const extensions = useLauncherStore((s) => s.extensions);
  const hasPins = (pins?.length ?? 0) > 0;
  useEffect(() => {
    if (installed && hasPins) void useLauncherStore.getState().load();
  }, [installed, hasPins, language]);
  return (pins ?? []).map((id) => {
    const found = findCommand(extensions, id);
    return { id, title: found ? commandLabel(found.cmd) : id, shortTitle: found?.cmd.title ?? id };
  });
}

function run(id: string, title: string) {
  void useLauncherStore.getState().launch({ kind: "command", commandId: id, title });
}

/** Rail buttons (one per pinned command). */
export function RailPins() {
  const { t } = useT("vscodeLauncher");
  const pinned = usePinned();
  useEffect(() => {
    if (!useVsCodeStore.getState().status) void useVsCodeStore.getState().load();
  }, []);
  if (pinned.length === 0) return null;
  return (
    <div className="rail__pins" role="group" aria-label={t("pinned.label")}>
      {pinned.map((p) => (
        <IconButton
          key={p.id}
          className="vxl-rail-pin"
          label={t("pinned.run", { title: p.title })}
          icon={<span aria-hidden="true">{commandMonogram(p.shortTitle)}</span>}
          tooltip="right"
          onClick={() => run(p.id, p.shortTitle)}
        />
      ))}
    </div>
  );
}

/** Quick-action buttons (Mission Control). */
export function PinnedQuickActions() {
  const { t } = useT("vscodeLauncher");
  const pinned = usePinned();
  const running = useLauncherStore((s) => s.running);
  useEffect(() => {
    if (!useVsCodeStore.getState().status) void useVsCodeStore.getState().load();
  }, []);
  if (pinned.length === 0) return <p className="vxl-muted">{t("pinned.empty")}</p>;
  return (
    <div className="vxl-quick" role="group" aria-label={t("pinned.label")}>
      {pinned.map((p) => (
        <Button
          key={p.id}
          size="sm"
          variant="secondary"
          leftIcon={<TerminalSquare size={13} />}
          loading={running === p.id}
          disabled={running !== null && running !== p.id}
          title={p.id}
          onClick={() => run(p.id, p.shortTitle)}
        >
          {p.title}
        </Button>
      ))}
    </div>
  );
}
