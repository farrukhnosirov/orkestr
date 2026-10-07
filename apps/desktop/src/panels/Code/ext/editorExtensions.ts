/**
 * Applies the declarative contributions of enabled VS Code extensions to
 * Monaco (ADR-0025): languages + language configurations, snippets,
 * TextMate grammars and the editor color theme (chosen separately from the
 * Orkestr UI theme in Settings → Editor). Extension code never runs; every
 * contribution is data fetched from the core.
 */
import type { EditorPrefs, InstalledExtension, ResolvedTheme, Snippet } from "@orkestr/protocol";
import type * as Monaco from "monaco-editor";

import { maybeClient } from "../../../core/client";
import { buildTheme, MONACO_THEME, type ThemeData } from "../theme";
import { toMonacoLanguageConfig } from "./languageConfig";
import { type GrammarSource, TextMateHost } from "./textmate";
import { colorMapRules, rawThemeFromMonaco, toMonacoTheme, toRawTheme } from "./vscodeTheme";

/** Monaco theme name used while a VS Code theme is selected. */
export const EXTENSION_THEME = "orkestr-ext";

interface State {
  monaco: typeof Monaco | null;
  host: TextMateHost | null;
  theme: ResolvedTheme | null;
  themeKey: string | null;
  extensions: InstalledExtension[];
  snippetProviders: Monaco.IDisposable[];
  configured: Set<string>;
  tmLanguages: string[];
  applied: string | null;
}

const state: State = {
  monaco: null,
  host: null,
  theme: null,
  themeKey: null,
  extensions: [],
  snippetProviders: [],
  configured: new Set(),
  tmLanguages: [],
  applied: null,
};

/** Name of the Monaco theme to use for new editors. */
export function editorThemeName(): string {
  return state.theme ? EXTENSION_THEME : MONACO_THEME;
}

/** The active VS Code editor theme, if any. */
export function activeEditorTheme(): ResolvedTheme | null {
  return state.theme;
}

/** Languages currently tokenized by TextMate grammars. */
export function textMateLanguages(): readonly string[] {
  return state.host?.activeLanguages() ?? [];
}

function orkestrTheme(): ThemeData {
  const style = getComputedStyle(document.documentElement);
  return buildTheme((t) => style.getPropertyValue(t));
}

/** (Re)defines and activates the editor theme (call after UI theme or preference changes). */
export function refreshEditorTheme(monaco: typeof Monaco): void {
  state.monaco = monaco;
  if (state.theme) {
    const raw = toRawTheme(state.theme);
    state.host?.setTheme(raw);
    const data = toMonacoTheme(state.theme, state.host?.colorMap() ?? []);
    monaco.editor.defineTheme(EXTENSION_THEME, data);
    monaco.editor.setTheme(EXTENSION_THEME);
  } else {
    const data = orkestrTheme();
    state.host?.setTheme(rawThemeFromMonaco(data));
    data.rules.push(...colorMapRules(state.host?.colorMap() ?? []));
    monaco.editor.defineTheme(MONACO_THEME, data);
    monaco.editor.setTheme(MONACO_THEME);
  }
  state.host?.retokenize();
}

function currentRawTheme(): ReturnType<typeof toRawTheme> {
  return state.theme ? toRawTheme(state.theme) : rawThemeFromMonaco(orkestrTheme());
}

function globToRegExp(glob: string): RegExp | null {
  if (glob.length > 200) return null;
  const re = glob
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*\*\/?/g, "\u0000")
    .replace(/\*/g, "[^/]*")
    .replace(/\?/g, "[^/]")
    .replace(/\u0000/g, ".*");
  try {
    return new RegExp(`(^|/)${re}$`, "i");
  } catch {
    return null;
  }
}

/** Language contributed by an enabled extension for a workspace path, if any. */
export function contributedLanguageFor(path: string, extensions: readonly InstalledExtension[] = state.extensions): string | null {
  const lower = path.toLowerCase();
  const name = lower.slice(lower.lastIndexOf("/") + 1);
  let best: { id: string; score: number } | null = null;
  for (const ext of extensions) {
    if (!ext.enabled) continue;
    for (const lang of ext.contributions.languages) {
      let score = 0;
      if (lang.filenames.some((f) => f.toLowerCase() === name)) score = 3;
      else if (lang.filenamePatterns.some((g) => globToRegExp(g)?.test(lower))) score = 2;
      else if (lang.extensions.some((e) => e && name.endsWith(e.toLowerCase()))) score = 1;
      if (score > (best?.score ?? 0)) best = { id: lang.id, score };
    }
  }
  return best?.id ?? null;
}

function registerLanguages(monaco: typeof Monaco, extensions: readonly InstalledExtension[]): void {
  const known = new Set(monaco.languages.getLanguages().map((l) => l.id));
  for (const ext of extensions) {
    for (const lang of ext.contributions.languages) {
      const reg: Monaco.languages.ILanguageExtensionPoint = { id: lang.id };
      if (lang.extensions.length) reg.extensions = lang.extensions;
      if (lang.filenames.length) reg.filenames = lang.filenames;
      if (lang.filenamePatterns.length) reg.filenamePatterns = lang.filenamePatterns;
      if (lang.aliases.length) reg.aliases = lang.aliases;
      if (lang.firstLine) reg.firstLine = lang.firstLine;
      monaco.languages.register(reg);
      known.add(lang.id);
    }
  }
}

async function applyLanguageConfigs(monaco: typeof Monaco, extensions: readonly InstalledExtension[]): Promise<void> {
  const client = maybeClient();
  if (!client) return;
  for (const ext of extensions) {
    for (const lang of ext.contributions.languages) {
      if (!lang.configuration) continue;
      const key = `${ext.id}:${ext.version}:${lang.id}`;
      if (state.configured.has(key)) continue;
      state.configured.add(key);
      try {
        const { config } = await client.call("extensions.languageConfig", { extensionId: ext.id, path: lang.configuration });
        monaco.languages.setLanguageConfiguration(lang.id, toMonacoLanguageConfig(config));
      } catch (e) {
        console.warn("language configuration failed", ext.id, lang.id, e);
      }
    }
  }
}

async function applySnippets(monaco: typeof Monaco, extensions: readonly InstalledExtension[]): Promise<void> {
  for (const d of state.snippetProviders) d.dispose();
  state.snippetProviders = [];
  const client = maybeClient();
  if (!client) return;
  const byLanguage = new Map<string, Snippet[]>();
  for (const ext of extensions) {
    for (const s of ext.contributions.snippets) {
      try {
        const { snippets } = await client.call("extensions.snippets", { extensionId: ext.id, path: s.path });
        const list = byLanguage.get(s.language) ?? [];
        list.push(...snippets);
        byLanguage.set(s.language, list);
      } catch (e) {
        console.warn("snippets failed to load", ext.id, s.path, e);
      }
    }
  }
  for (const [language, snippets] of byLanguage) {
    state.snippetProviders.push(
      monaco.languages.registerCompletionItemProvider(language === "*" ? { pattern: "**" } : language, {
        provideCompletionItems: (model, position) => {
          const word = model.getWordUntilPosition(position);
          const range = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
          const suggestions: Monaco.languages.CompletionItem[] = [];
          for (const sn of snippets) {
            for (const prefix of sn.prefixes) {
              suggestions.push({
                label: { label: prefix, description: sn.name },
                kind: monaco.languages.CompletionItemKind.Snippet,
                insertText: sn.body,
                insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
                documentation: sn.description ?? sn.body,
                detail: sn.name,
                range,
                sortText: `~${prefix}`,
              });
            }
          }
          return { suggestions };
        },
      }),
    );
  }
}

function grammarSources(extensions: readonly InstalledExtension[]): GrammarSource[] {
  return extensions.flatMap((ext) => ext.contributions.grammars.map((grammar) => ({ extensionId: ext.id, grammar })));
}

/**
 * Applies installed extensions + preferences to Monaco (idempotent: unchanged
 * inputs do nothing).
 */
export async function applyEditorExtensions(
  monaco: typeof Monaco,
  installed: readonly InstalledExtension[],
  prefs: EditorPrefs,
): Promise<void> {
  state.monaco = monaco;
  const enabled = installed.filter((e) => e.enabled);
  const signature = JSON.stringify([enabled.map((e) => `${e.id}@${e.version}`), prefs]);
  if (state.applied === signature) return;
  state.applied = signature;
  state.extensions = enabled;

  // Editor color theme.
  const themeKey = prefs.editorTheme ? `${prefs.editorTheme.extensionId}/${prefs.editorTheme.themeId}` : null;
  if (themeKey !== state.themeKey) {
    state.themeKey = themeKey;
    state.theme = null;
    const client = maybeClient();
    if (prefs.editorTheme && client) {
      try {
        state.theme = await client.call("extensions.theme", prefs.editorTheme);
      } catch (e) {
        console.warn("editor theme failed to load", e);
      }
    }
  }

  registerLanguages(monaco, enabled);
  state.host ??= new TextMateHost(monaco, currentRawTheme());
  state.tmLanguages = state.host.setGrammars(grammarSources(enabled), currentRawTheme());
  refreshEditorTheme(monaco);
  await applyLanguageConfigs(monaco, enabled);
  await applySnippets(monaco, enabled);
}
