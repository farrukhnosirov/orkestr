/**
 * TextMate tokenization for Monaco with grammars from installed VS Code
 * extensions (ADR-0025): vscode-textmate + the Oniguruma regex engine
 * compiled to WebAssembly (bundled, no CDN; CSP allows `wasm-unsafe-eval`).
 *
 * Grammars are data: the core serves the files (`extensions.readAsset`)
 * and vscode-textmate parses them (JSON or plist). Embedded languages and
 * injection grammars resolve through the same registry. Tokens are named
 * `tm.<colorIndex>.<fontStyle>`; the theme maps color indices back to colors
 * (see `vscodeTheme.ts`).
 */
import type { GrammarContribution } from "@orkestr/protocol";
import type * as Monaco from "monaco-editor";
import * as oniguruma from "vscode-oniguruma";
import onigWasmUrl from "vscode-oniguruma/release/onig.wasm?url";
import * as vsctm from "vscode-textmate";

import { maybeClient } from "../../../core/client";
import { tmToken } from "./vscodeTheme";

/** A grammar provided by an enabled extension. */
export interface GrammarSource {
  extensionId: string;
  grammar: GrammarContribution;
}

const FOREGROUND_MASK = 0xff8000;
const FOREGROUND_OFFSET = 15;
const FONT_STYLE_MASK = 0x7800;
const FONT_STYLE_OFFSET = 11;
/** Lines longer than this are not tokenized (as in VS Code's default). */
const MAX_LINE = 20_000;
/** Per-line time budget (ms). */
const TIME_LIMIT = 500;

let onig: Promise<vsctm.IOnigLib> | null = null;

function onigLib(): Promise<vsctm.IOnigLib> {
  onig ??= fetch(onigWasmUrl)
    .then((r) => r.arrayBuffer())
    .then((data) => oniguruma.loadWASM(data))
    .then(() => ({
      createOnigScanner: (patterns: string[]) => oniguruma.createOnigScanner(patterns),
      createOnigString: (s: string) => oniguruma.createOnigString(s),
    }));
  return onig;
}

class TmState implements Monaco.languages.IState {
  constructor(readonly stack: vsctm.StateStack) {}
  clone(): TmState {
    return new TmState(this.stack.clone());
  }
  equals(other: Monaco.languages.IState): boolean {
    return other instanceof TmState && other.stack.equals(this.stack);
  }
}

/** Decodes vscode-textmate binary tokens into Monaco tokens. */
export function decodeTokens(tokens: Uint32Array): Monaco.languages.IToken[] {
  const out: Monaco.languages.IToken[] = [];
  for (let i = 0; i < tokens.length; i += 2) {
    const start = tokens[i] ?? 0;
    const meta = tokens[i + 1] ?? 0;
    const fg = (meta & FOREGROUND_MASK) >>> FOREGROUND_OFFSET;
    const fs = (meta & FONT_STYLE_MASK) >>> FONT_STYLE_OFFSET;
    out.push({ startIndex: start, scopes: tmToken(fg, fs) });
  }
  return out;
}

/** Owns the vscode-textmate registry and the Monaco token providers it installed. */
export class TextMateHost {
  private registry: vsctm.Registry;
  private readonly sources = new Map<string, GrammarSource>();
  private readonly providers = new Map<string, Monaco.IDisposable>();
  private readonly languageIds = new Map<string, number>();
  private readonly grammars = new Map<string, vsctm.IGrammar>();
  private generation = 0;

  constructor(
    private readonly monaco: typeof Monaco,
    rawTheme: vsctm.IRawTheme,
  ) {
    this.registry = this.createRegistry(rawTheme);
  }

  private createRegistry(rawTheme: vsctm.IRawTheme): vsctm.Registry {
    const reg = new vsctm.Registry({
      onigLib: onigLib(),
      loadGrammar: async (scopeName) => this.loadRaw(scopeName),
      getInjections: (scopeName) =>
        [...this.sources.values()]
          .filter((s) => s.grammar.injectionTo.includes(scopeName))
          .map((s) => s.grammar.scopeName),
    });
    reg.setTheme(rawTheme);
    return reg;
  }

  private async loadRaw(scopeName: string): Promise<vsctm.IRawGrammar | null> {
    const src = this.sources.get(scopeName);
    const client = maybeClient();
    if (!src || !client) return null;
    try {
      const asset = await client.call("extensions.readAsset", { extensionId: src.extensionId, path: src.grammar.path });
      if (asset.text === null) return null;
      return vsctm.parseRawGrammar(asset.text, src.grammar.path);
    } catch (e) {
      console.warn("grammar failed to load", scopeName, e);
      return null;
    }
  }

  /** Current color map (index → `#rrggbb`). */
  colorMap(): string[] {
    return this.registry.getColorMap();
  }

  /** Re-themes the registry (token colors come from the theme's scope rules). */
  setTheme(rawTheme: vsctm.IRawTheme): void {
    this.registry.setTheme(rawTheme);
  }

  private languageNumber(languageId: string): number {
    let n = this.languageIds.get(languageId);
    if (n === undefined) {
      n = this.languageIds.size + 1;
      this.languageIds.set(languageId, n);
    }
    return n;
  }

  /**
   * Installs the grammars. A language gets a TextMate tokens provider once
   * its grammar has loaded (registering a provider makes Monaco retokenize
   * that language's models); until then Monaco's built-in tokenizer stays.
   * Returns the languages that will be tokenized by TextMate.
   */
  setGrammars(list: readonly GrammarSource[], rawTheme: vsctm.IRawTheme): string[] {
    this.sources.clear();
    for (const s of list) this.sources.set(s.grammar.scopeName, s);
    // A fresh registry drops cached grammars of removed extensions.
    this.registry = this.createRegistry(rawTheme);
    for (const d of this.providers.values()) d.dispose();
    this.providers.clear();
    this.grammars.clear();
    const generation = ++this.generation;
    const languages: string[] = [];
    for (const s of list) {
      const lang = s.grammar.language;
      if (!lang || languages.includes(lang)) continue;
      languages.push(lang);
      void this.load(lang, s, generation);
    }
    return languages;
  }

  private async load(languageId: string, src: GrammarSource, generation: number): Promise<void> {
    const embedded: vsctm.IEmbeddedLanguagesMap = {};
    for (const [scope, lang] of Object.entries(src.grammar.embeddedLanguages)) embedded[scope] = this.languageNumber(lang);
    const tokenTypes: vsctm.ITokenTypeMap = {};
    const types: Record<string, number> = { other: 0, comment: 1, string: 2, regex: 3 };
    for (const [scope, kind] of Object.entries(src.grammar.tokenTypes)) {
      const t = types[kind];
      if (t !== undefined) tokenTypes[scope] = t as vsctm.ITokenTypeMap[string];
    }
    try {
      const grammar = await this.registry.loadGrammarWithConfiguration(src.grammar.scopeName, this.languageNumber(languageId), {
        embeddedLanguages: embedded,
        tokenTypes,
      });
      if (!grammar || generation !== this.generation) return;
      this.grammars.set(languageId, grammar);
      this.register(languageId);
    } catch (e) {
      console.warn("TextMate grammar failed", src.grammar.scopeName, e);
    }
  }

  private register(languageId: string): void {
    const grammar = this.grammars.get(languageId);
    if (!grammar) return;
    this.providers.get(languageId)?.dispose();
    this.providers.set(
      languageId,
      this.monaco.languages.setTokensProvider(languageId, {
        getInitialState: () => new TmState(vsctm.INITIAL),
        tokenize: (line: string, state: Monaco.languages.IState): Monaco.languages.ILineTokens => {
          const st = state as TmState;
          if (line.length > MAX_LINE) return { tokens: [{ startIndex: 0, scopes: "" }], endState: st };
          const r = grammar.tokenizeLine2(line, st.stack, TIME_LIMIT);
          return { tokens: decodeTokens(r.tokens), endState: new TmState(r.ruleStack) };
        },
      }),
    );
  }

  /** Re-registers every provider so Monaco retokenizes (after a theme change: color indices moved). */
  retokenize(): void {
    for (const lang of this.grammars.keys()) this.register(lang);
  }

  /** Languages whose grammar is loaded. */
  activeLanguages(): string[] {
    return [...this.grammars.keys()];
  }

  dispose(): void {
    for (const d of this.providers.values()) d.dispose();
    this.providers.clear();
  }
}
