/**
 * Text helpers for voice intents: normalization, RU→Latin transliteration,
 * a phonetic key and fuzzy similarity, so "Скулифай", "schoolify" and
 * "Schoolify" all find the project "Schoolify". Pure, unit-tested.
 */

/** Lower-case, `ё`→`е`, punctuation → spaces, single spaces. */
export function normalize(text: string): string {
  return text
    .toLowerCase()
    .replace(/ё/g, "е")
    .replace(/[.,!?;:«»"“”„'`’()[\]{}<>…]/g, " ")
    .replace(/[-–—_/\\]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

const RU_LAT: Record<string, string> = {
  а: "a", б: "b", в: "v", г: "g", д: "d", е: "e", ж: "zh", з: "z", и: "i", й: "y", к: "k", л: "l", м: "m",
  н: "n", о: "o", п: "p", р: "r", с: "s", т: "t", у: "u", ф: "f", х: "kh", ц: "ts", ч: "ch", ш: "sh", щ: "sch",
  ъ: "", ы: "y", ь: "", э: "e", ю: "yu", я: "ya",
};

/** Cyrillic → Latin (simple, lossy; for matching only). */
export function transliterate(text: string): string {
  let out = "";
  for (const ch of text.toLowerCase()) out += RU_LAT[ch] ?? ch;
  return out;
}

/**
 * Phonetic-ish key: transliterated, then English spelling folded towards
 * how it sounds (`sch`→`sk`, `oo`→`u`, `ph`→`f`, `j`→`dzh`, `y`→`i`,
 * soft `c`→`s`, doubled letters collapsed, spaces removed).
 */
export function phoneticKey(text: string): string {
  let s = transliterate(normalize(text)).replace(/[^a-z0-9 ]/g, "");
  s = s
    .replace(/sch/g, "sk")
    .replace(/ph/g, "f")
    .replace(/oo/g, "u")
    .replace(/ee/g, "i")
    .replace(/kh/g, "h")
    .replace(/dzh/g, "j")
    .replace(/j/g, "dzh")
    .replace(/c(?=[eiy])/g, "s")
    .replace(/ck/g, "k")
    .replace(/c/g, "k")
    .replace(/q/g, "k")
    .replace(/x/g, "ks")
    .replace(/w/g, "v")
    .replace(/y/g, "i")
    .replace(/ai/g, "i")
    .replace(/ei/g, "i")
    .replace(/([a-z])\1+/g, "$1")
    .replace(/ /g, "");
  return s;
}

/** Levenshtein distance (two-row DP). */
export function levenshtein(a: string, b: string): number {
  if (a === b) return 0;
  if (!a.length) return b.length;
  if (!b.length) return a.length;
  let prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  let cur = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i++) {
    cur[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      cur[j] = Math.min((prev[j] ?? 0) + 1, (cur[j - 1] ?? 0) + 1, (prev[j - 1] ?? 0) + cost);
    }
    [prev, cur] = [cur, prev];
  }
  return prev[b.length] ?? 0;
}

/** Similarity 0..1 of two strings' phonetic keys. */
export function similarity(a: string, b: string): number {
  const ka = phoneticKey(a);
  const kb = phoneticKey(b);
  if (!ka || !kb) return 0;
  if (ka === kb) return 1;
  return 1 - levenshtein(ka, kb) / Math.max(ka.length, kb.length);
}

export interface Named {
  id: string;
  name: string;
}

export interface EntityMatch<T extends Named> {
  item: T;
  score: number;
}

/** Minimum similarity for a fuzzy entity match. */
export const MATCH_THRESHOLD = 0.72;

/**
 * Best match of `query` (free words from an utterance) against `items` by
 * name. Tries the whole query and every word window of the name's length
 * (±1), so "QA по Schoolify пожалуйста" still finds "Schoolify". Agent
 * names such as "Backend Agent #2" also match their first words ("backend").
 */
export function bestMatch<T extends Named>(query: string, items: readonly T[], threshold = MATCH_THRESHOLD): EntityMatch<T> | null {
  const words = normalize(query).split(" ").filter(Boolean);
  if (!words.length) return null;
  let best: EntityMatch<T> | null = null;
  for (const item of items) {
    const nameWords = normalize(item.name.replace(/#\d+/g, "")).split(" ").filter(Boolean);
    const names = [nameWords.join(" ")];
    // "Backend Agent" → also "backend"; "QA Engineer" → "qa".
    const generic = new Set(["agent", "агент", "engineer", "инженер", "bot", "бот"]);
    const core = nameWords.filter((w) => !generic.has(w));
    if (core.length && core.length < nameWords.length) names.push(core.join(" "));
    for (const name of names) {
      const n = name.split(" ").length;
      for (let len = Math.max(1, n - 1); len <= n + 1; len++) {
        for (let i = 0; i + len <= words.length; i++) {
          const window = words.slice(i, i + len).join(" ");
          const score = similarity(window, name);
          if (score >= threshold && (!best || score > best.score)) best = { item, score };
        }
      }
    }
  }
  return best;
}

/** Built-in wake aliases stripped from the start of a command. */
export const WAKE_ALIASES: readonly string[] = ["hey jarvis", "ok jarvis", "jarvis", "эй джарвис", "джарвис", "orkestr", "оркестр", "hey orkestr", "эй оркестр"];

/** Removes a leading wake phrase ("Jarvis, …", "Оркестр, …"). */
export function stripWakePhrase(text: string, phrases: readonly string[] = []): string {
  const norm = normalize(text);
  const all = [...phrases.map(normalize), ...WAKE_ALIASES].filter(Boolean).sort((a, b) => b.length - a.length);
  for (const p of all) {
    if (norm === p) return "";
    if (norm.startsWith(`${p} `)) return norm.slice(p.length + 1).trim();
  }
  return norm;
}
