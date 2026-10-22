import { beforeAll, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { PANELS } from "../../panels/registry";
import {
  categoryCounts,
  cycleFilter,
  entryShortcut,
  filterEntries,
  libraryEntries,
  moveSelection,
  searchEntries,
  swapLayout,
} from "./catalog";
import { previewUrl } from "./previews";
import { pushFront, toggleIn } from "./store";

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

const ids = (q: string, lang: "en" | "ru" = "en") => searchEntries(libraryEntries(), q, lang).map((h) => h.entry.id);

describe("library entries", () => {
  it("lists every registered panel once plus the dialog windows", () => {
    const entries = libraryEntries();
    for (const p of PANELS) expect(entries.filter((e) => e.id === p.id)).toHaveLength(1);
    expect(entries.filter((e) => e.kind === "window").map((e) => e.id)).toEqual(["settings", "themeEditor", "layoutEditor"]);
    expect(new Set(entries.map((e) => e.id)).size).toBe(entries.length);
  });

  it("groups entries into ordered, non-empty categories", () => {
    const cats = categoryCounts(libraryEntries());
    expect(cats.map((c) => c.id)).toEqual(["work", "ai", "code", "quality", "ops", "monitoring", "ecosystem", "system"]);
    expect(cats.reduce((n, c) => n + c.count, 0)).toBe(libraryEntries().length);
    expect(cats.find((c) => c.id === "system")?.count).toBe(3);
  });

  it("shows the shortcut of the command that opens an entry", () => {
    const byId = new Map(libraryEntries().map((e) => [e.id, e]));
    expect(entryShortcut(byId.get("terminal")!)).toBe("Ctrl+`");
    expect(entryShortcut(byId.get("settings")!)).toBe("Ctrl+,");
    expect(entryShortcut(byId.get("files")!)).toBeUndefined();
  });

  it("filters by category, favorites and recent (in list order)", () => {
    const entries = libraryEntries();
    const lists = { favorites: ["git", "nope", "costs"], recent: ["logs", "manager"] };
    expect(filterEntries(entries, "all", lists)).toHaveLength(entries.length);
    expect(filterEntries(entries, "favorites", lists).map((e) => e.id)).toEqual(["git", "costs"]);
    expect(filterEntries(entries, "recent", lists).map((e) => e.id)).toEqual(["logs", "manager"]);
    expect(filterEntries(entries, "ecosystem", lists).map((e) => e.id).sort()).toEqual(["extensions", "marketplace"]);
  });
});

describe("search", () => {
  it("empty query keeps registry order", () => {
    expect(ids("")).toEqual(libraryEntries().map((e) => e.id));
  });

  it("finds by English and Russian title regardless of the UI language", () => {
    expect(ids("terminal")[0]).toBe("terminal");
    expect(ids("терминал")[0]).toBe("terminal");
    expect(ids("расходы", "en")[0]).toBe("costs");
    expect(ids("costs", "ru")[0]).toBe("costs");
  });

  it("finds by fuzzy title, tags and description words", () => {
    expect(ids("mssn ctrl")[0]).toBe("missionControl");
    expect(ids("kanban")[0]).toBe("tasks");
    expect(ids("канбан")[0]).toBe("tasks");
    expect(ids("budget")).toContain("costs");
    expect(ids("worktree")).toContain("git");
    expect(ids("цвета")[0]).toBe("themeEditor");
  });

  it("every word must match (multi-word narrows)", () => {
    const vs = ids("vs code");
    expect(vs[0]).toBe("vscode");
    expect(ids("deploy release")).toContain("deployments");
    expect(ids("zzzz qqqq")).toEqual([]);
  });

  it("tolerates the wrong keyboard layout", () => {
    expect(swapLayout("rjl")).toBe("код");
    expect(ids("nthvbyfk")[0]).toBe("terminal"); // "терминал" typed on EN layout
    expect(ids("пше")[0]).toBe("git"); // "git" typed on RU layout
  });

  it("returns title highlight indices in the current language", () => {
    const [hit] = searchEntries(libraryEntries(), "log", "en");
    expect(hit?.entry.id).toBe("logs");
    expect(hit?.indices).toEqual([0, 1, 2]);
  });

  it("ranks planned (not yet available) panels after available ones", () => {
    const hits = searchEntries(libraryEntries(), "agent", "en").map((h) => h.entry.id);
    expect(hits.indexOf("agents")).toBeLessThan(hits.indexOf("browser"));
  });
});

describe("keyboard navigation", () => {
  it("moves in a grid", () => {
    // 10 cards in 4 columns: 0..3 / 4..7 / 8..9
    expect(moveSelection(0, "ArrowRight", 4, 10)).toBe(1);
    expect(moveSelection(0, "ArrowLeft", 4, 10)).toBe(0);
    expect(moveSelection(1, "ArrowDown", 4, 10)).toBe(5);
    expect(moveSelection(5, "ArrowDown", 4, 10)).toBe(9);
    expect(moveSelection(6, "ArrowDown", 4, 10)).toBe(6); // no card below
    expect(moveSelection(5, "ArrowUp", 4, 10)).toBe(1);
    expect(moveSelection(1, "ArrowUp", 4, 10)).toBe(1);
    expect(moveSelection(9, "ArrowRight", 4, 10)).toBe(9);
    expect(moveSelection(4, "Home", 4, 10)).toBe(0);
    expect(moveSelection(4, "End", 4, 10)).toBe(9);
    expect(moveSelection(4, "Enter", 4, 10)).toBeNull();
    expect(moveSelection(0, "ArrowDown", 4, 0)).toBeNull();
  });

  it("cycles category filters", () => {
    const f = ["all", "favorites", "recent", "work"];
    expect(cycleFilter(f, "all", 1)).toBe("favorites");
    expect(cycleFilter(f, "work", 1)).toBe("all");
    expect(cycleFilter(f, "all", -1)).toBe("work");
    expect(cycleFilter(f, "gone", 1)).toBe("all");
  });
});

describe("lists and previews", () => {
  it("recent / favorites helpers", () => {
    expect(pushFront(["a", "b", "c"], "c", 3)).toEqual(["c", "a", "b"]);
    expect(pushFront(["a", "b", "c"], "d", 3)).toEqual(["d", "a", "b"]);
    expect(toggleIn(["a"], "b")).toEqual(["a", "b"]);
    expect(toggleIn(["a", "b"], "a")).toEqual(["b"]);
    expect(toggleIn(["a", "b"], "c", 2)).toEqual(["b", "c"]);
  });

  it("picks the preview for language and theme mode with fallbacks", () => {
    const m = new Map([
      ["code.ru.dark", "ru-dark"],
      ["code.en.light", "en-light"],
      ["git.en.dark", "git-en-dark"],
    ]);
    expect(previewUrl("code", "ru", "dark", m)).toBe("ru-dark");
    expect(previewUrl("code", "en", "light", m)).toBe("en-light");
    expect(previewUrl("code", "ru", "light", m)).toBe("en-light");
    expect(previewUrl("git", "ru", "light", m)).toBe("git-en-dark");
    expect(previewUrl("nope", "en", "dark", m)).toBeUndefined();
  });
});
