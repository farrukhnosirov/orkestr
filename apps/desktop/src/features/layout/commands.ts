/**
 * Layout commands (§54): switch preset (`Ctrl+1..5`), save, editor, lock,
 * detach/float, spread across monitors, export/import. Palette, shortcuts
 * and (Phase 3) voice read them from the command registry.
 */
import {
  AppWindow,
  ArrowRightLeft,
  Download,
  Lock,
  MonitorSmartphone,
  PictureInPicture2,
  RotateCcw,
  Save,
  SlidersHorizontal,
  Upload,
} from "lucide-react";

import { t } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import * as layout from "./controller";
import { layoutName } from "./names";
import { BUILTIN_PRESETS } from "./presets";
import { cycleLayout } from "./selection";
import { useLayoutStore } from "./store";
import { canDetach } from "./windowsApi";

const group = () => t("layout:commands.group");

/** Russian search words per preset (both languages stay searchable). */
const RU_WORDS: Record<string, string[]> = {
  coding: ["разработка", "код"],
  "mission-control": ["центр управления"],
  monitoring: ["мониторинг"],
  focus: ["фокус"],
  review: ["ревью", "проверка"],
  custom: ["своя"],
};

const switchCommands: AppCommand[] = BUILTIN_PRESETS.map((p) => {
  const cmd: AppCommand = {
    id: `layout.switch.${p.id}`,
    get title() {
      return t("layout:commands.switch", { name: layoutName(p.id) });
    },
    get group() {
      return group();
    },
    icon: p.icon,
    keywords: ["layout", "preset", "workspace", "раскладка", "пресет", p.id, p.storedName.toLowerCase(), ...(RU_WORDS[p.id] ?? [])],
    run: () => layout.switchLayout(p.id),
  };
  if (p.shortcut) cmd.shortcut = p.shortcut;
  return cmd;
});

export const layoutCommands: AppCommand[] = [
  ...switchCommands,
  {
    id: "layout.next",
    get title() {
      return t("layout:commands.next");
    },
    get group() {
      return group();
    },
    icon: ArrowRightLeft,
    keywords: ["layout", "cycle", "next", "раскладка", "следующая"],
    run: () => {
      const s = useLayoutStore.getState();
      return layout.switchLayout(cycleLayout(s.state, s.activeId, 1));
    },
  },
  {
    id: "layout.saveAs",
    get title() {
      return t("layout:commands.saveAs");
    },
    get group() {
      return group();
    },
    icon: Save,
    keywords: ["layout", "save", "preset", "custom", "раскладка", "сохранить", "своя"],
    enabled: () => useLayoutStore.getState().loaded,
    run: () => useLayoutStore.getState().patch({ dialog: { kind: "saveAs" } }),
  },
  {
    id: "layout.editor",
    get title() {
      return t("layout:commands.editor");
    },
    get group() {
      return group();
    },
    icon: SlidersHorizontal,
    keywords: ["layout", "editor", "docking", "panels", "monitors", "раскладка", "редактор", "панели", "мониторы"],
    run: () => layout.openEditor(!useLayoutStore.getState().editorOpen),
  },
  {
    id: "layout.lock",
    get title() {
      return useLayoutStore.getState().locked ? t("layout:commands.unlock") : t("layout:commands.lock");
    },
    get group() {
      return group();
    },
    icon: Lock,
    keywords: ["layout", "lock", "freeze", "раскладка", "закрепить", "заблокировать", "разблокировать"],
    run: () => layout.setLocked(!useLayoutStore.getState().locked),
  },
  {
    id: "layout.reset",
    get title() {
      return t("layout:commands.reset");
    },
    get group() {
      return group();
    },
    icon: RotateCcw,
    keywords: ["layout", "reset", "default", "раскладка", "сбросить", "по умолчанию"],
    run: () => layout.resetActive(),
  },
  {
    id: "layout.detach",
    get title() {
      return canDetach() ? t("layout:commands.detach") : t("layout:commands.float");
    },
    get group() {
      return group();
    },
    icon: AppWindow,
    keywords: ["detach", "pop out", "window", "undock", "открепить", "отдельное окно", "окно"],
    enabled: () => !useLayoutStore.getState().locked,
    run: () => layout.detachPanel(),
  },
  {
    id: "layout.float",
    get title() {
      return t("layout:commands.float");
    },
    get group() {
      return group();
    },
    icon: PictureInPicture2,
    keywords: ["float", "floating", "плавающая", "поверх"],
    enabled: () => !useLayoutStore.getState().locked,
    run: () => layout.floatPanel(),
  },
  {
    id: "layout.spread",
    get title() {
      return t("layout:commands.spread");
    },
    get group() {
      return group();
    },
    icon: MonitorSmartphone,
    keywords: ["monitor", "multi-monitor", "display", "screens", "монитор", "мониторы", "экраны", "дисплей"],
    enabled: () => canDetach() && !useLayoutStore.getState().locked,
    run: () => layout.spreadAcrossMonitors(),
  },
  {
    id: "layout.export",
    get title() {
      return t("layout:commands.export");
    },
    get group() {
      return group();
    },
    icon: Download,
    keywords: ["layout", "export", "file", "раскладка", "экспорт", "файл"],
    run: () => layout.exportCurrent(),
  },
  {
    id: "layout.import",
    get title() {
      return t("layout:commands.import");
    },
    get group() {
      return group();
    },
    icon: Upload,
    keywords: ["layout", "import", "file", "раскладка", "импорт", "файл"],
    enabled: () => useLayoutStore.getState().loaded,
    run: () => useLayoutStore.getState().patch({ dialog: { kind: "import" } }),
  },
];
