/**
 * «Команды» drawer: every slash command the CLI reported, as cards with an
 * icon, a one-line description (curated Russian / English for built-ins, the
 * CLI's own text for custom commands, skills and MCP prompts), an optional
 * argument and a «Выполнить» button. Grouped Контекст / Код и ревью /
 * Настройки / Аккаунт / Диагностика / Свои команды и навыки.
 */
import type { ClaudeSlashCommand } from "@orkestr/protocol";
import { Button, IconButton } from "@orkestr/ui";
import clsx from "clsx";
import {
  Anchor,
  Archive,
  Bot,
  Brain,
  Bug,
  Coins,
  Cpu,
  Download,
  Eraser,
  FileText,
  FolderPlus,
  Gauge,
  History,
  Info,
  Keyboard,
  Layers,
  ListTodo,
  Lock,
  LogIn,
  LogOut,
  Newspaper,
  Palette,
  Pencil,
  Play,
  Plug,
  Puzzle,
  Repeat,
  Search,
  Settings,
  Shield,
  Slash,
  Sparkles,
  Stethoscope,
  Terminal,
  Undo2,
  X,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  catalogEntries,
  catalogSections,
  commandText,
  COMMAND_GROUPS,
  type CatalogEntry,
  type CommandGroup,
  type CommandIcon,
  type NativeAction,
} from "../../../features/claudeCode/catalog";
import { useT } from "../../../i18n";

const ICONS: Record<CommandIcon, LucideIcon> = {
  archive: Archive,
  eraser: Eraser,
  fileText: FileText,
  search: Search,
  shield: Shield,
  coins: Coins,
  gauge: Gauge,
  layers: Layers,
  brain: Brain,
  bot: Bot,
  plug: Plug,
  anchor: Anchor,
  lock: Lock,
  cpu: Cpu,
  settings: Settings,
  info: Info,
  stethoscope: Stethoscope,
  download: Download,
  undo: Undo2,
  folderPlus: FolderPlus,
  palette: Palette,
  listTodo: ListTodo,
  bug: Bug,
  newspaper: Newspaper,
  terminal: Terminal,
  keyboard: Keyboard,
  history: History,
  logIn: LogIn,
  logOut: LogOut,
  zap: Zap,
  sparkles: Sparkles,
  repeat: Repeat,
  pencil: Pencil,
  puzzle: Puzzle,
  slash: Slash,
};

export interface CommandsDrawerProps {
  commands: readonly ClaudeSlashCommand[];
  /** A chat is open and can take a command. */
  canRun: boolean;
  /** Sends `/<name> <arg>` to the chat. */
  onRun: (text: string) => void;
  /** Runs an Orkestr-native equivalent (usage card, MCP list, export, …). */
  onNative: (action: NativeAction) => void;
  onClose: () => void;
}

function CommandCard({ entry, canRun, onRun, onNative }: { entry: CatalogEntry } & Omit<CommandsDrawerProps, "commands" | "onClose">) {
  const { t } = useT("claudeCodePanel");
  const [arg, setArg] = useState("");
  const [confirm, setConfirm] = useState(false);
  const Icon = ICONS[entry.icon];
  const description = entry.curated ? t(`cmd.${entry.name}` as "cmd.compact") : entry.cliDescription;
  const run = () => {
    if (entry.native) {
      onNative(entry.native);
      return;
    }
    if (entry.destructive && !confirm) {
      setConfirm(true);
      return;
    }
    setConfirm(false);
    onRun(commandText(entry.name, arg));
    setArg("");
  };
  const disabled = !canRun && entry.native === null;
  return (
    <li className={clsx("cc2-cmd", entry.destructive && "cc2-cmd--danger")}>
      <div className="cc2-cmd__head">
        <span className="cc2-cmd__icon" aria-hidden="true">
          <Icon size={15} />
        </span>
        <code className="cc2-cmd__name">/{entry.name}</code>
        {entry.native && <span className="cc2-cmd__tag">{t("commands.native")}</span>}
      </div>
      <p className="cc2-cmd__desc" title={entry.curated ? entry.cliDescription : undefined}>
        {description}
      </p>
      {confirm && <p className="cc2-cmd__confirm">{t("commands.confirm", { name: entry.name })}</p>}
      <div className="cc2-cmd__run">
        {entry.arg && !entry.native && (
          <input
            className="cc2-input cc2-cmd__arg"
            value={arg}
            placeholder={entry.argumentHint ?? t("commands.argPlaceholder")}
            aria-label={t("commands.argLabel", { name: entry.name })}
            disabled={disabled}
            onChange={(e) => setArg(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                run();
              }
            }}
          />
        )}
        <Button
          size="sm"
          variant={confirm ? "danger" : "primary"}
          leftIcon={<Play size={12} />}
          disabled={disabled}
          aria-label={confirm ? t("commands.confirmNamed", { name: entry.name }) : t("commands.runNamed", { name: entry.name })}
          onClick={run}
        >
          {confirm ? t("commands.confirmRun") : t("commands.run")}
        </Button>
      </div>
    </li>
  );
}

export function CommandsDrawer({ commands, canRun, onRun, onNative, onClose }: CommandsDrawerProps) {
  const { t } = useT("claudeCodePanel");
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState<CommandGroup | "all">("all");
  const search = useRef<HTMLInputElement>(null);
  const entries = useMemo(() => catalogEntries(commands), [commands]);
  const describe = (e: CatalogEntry) => (e.curated ? t(`cmd.${e.name}` as "cmd.compact") : e.cliDescription);
  const sections = catalogSections(entries, query, describe, group);
  const present = new Set(entries.map((e) => e.group));

  useEffect(() => {
    search.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="cc2-drawer" role="dialog" aria-label={t("commands.title")}>
      <header className="cc2-drawer__head">
        <div>
          <h2 className="cc2-drawer__title">{t("commands.title")}</h2>
          <p className="cc2-drawer__desc">{canRun ? t("commands.description") : t("commands.noSession")}</p>
        </div>
        <IconButton label={t("commands.close")} icon={<X size={16} />} onClick={onClose} />
      </header>
      <label className="cc2-search cc2-search--drawer">
        <Search size={13} aria-hidden />
        <input
          ref={search}
          type="search"
          value={query}
          placeholder={t("commands.searchPlaceholder")}
          aria-label={t("commands.search")}
          onChange={(e) => setQuery(e.target.value)}
        />
      </label>
      <div className="cc2-filters" role="tablist" aria-label={t("commands.title")}>
        {(["all", ...COMMAND_GROUPS.filter((g) => present.has(g))] as const).map((g) => (
          <button
            key={g}
            type="button"
            role="tab"
            aria-selected={group === g}
            className={clsx("cc2-filter", group === g && "cc2-filter--on")}
            onClick={() => setGroup(g)}
          >
            {g === "all" ? t("commands.all") : t(`commands.group.${g}`)}
          </button>
        ))}
      </div>
      <div className="cc2-drawer__body">
        {sections.length === 0 ? (
          <p className="cc2-side__empty">{t("commands.empty")}</p>
        ) : (
          sections.map((sec) => (
            <section key={sec.group} className="cc2-cmdgroup" aria-label={t(`commands.group.${sec.group}`)}>
              <h3 className="cc2-group__title">{t(`commands.group.${sec.group}`)}</h3>
              <ul className="cc2-cmdgrid">
                {sec.entries.map((e) => (
                  <CommandCard key={e.name} entry={e} canRun={canRun} onRun={onRun} onNative={onNative} />
                ))}
              </ul>
            </section>
          ))
        )}
      </div>
    </div>
  );
}
