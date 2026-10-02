/**
 * Fuzzy matching with match positions for highlighting (command palette,
 * global search). Substring matches rank above scattered subsequences;
 * matches at word starts and consecutive runs score higher.
 */

export interface FuzzyMatch {
  score: number;
  /** Indices into the original text of matched characters (ascending). */
  indices: number[];
}

function isWordStart(text: string, i: number): boolean {
  if (i === 0) return true;
  const prev = text[i - 1] ?? "";
  const cur = text[i] ?? "";
  return /[\s\-_./:\\]/.test(prev) || (prev === prev.toLowerCase() && cur !== cur.toLowerCase());
}

/** Matches `query` against `text`; `null` when not all characters are found in order. */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = query.toLowerCase().trim();
  if (!q) return { score: 0, indices: [] };
  const t = text.toLowerCase();
  const idx = t.indexOf(q);
  if (idx >= 0) {
    const indices = Array.from({ length: q.length }, (_, k) => idx + k);
    const startBonus = isWordStart(text, idx) ? 50 : 0;
    return { score: 1000 + startBonus - idx * 2 - (t.length - q.length) * 0.1, indices };
  }
  const indices: number[] = [];
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    if (ch === " ") continue;
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    streak = found === ti && indices.length > 0 ? streak + 1 : 0;
    score += 10 + streak * 5 - Math.min(found - ti, 10) + (isWordStart(text, found) ? 8 : 0);
    indices.push(found);
    ti = found + 1;
  }
  return { score, indices };
}

/** Legacy score API (higher is better; -1 = no match, 0 = empty query). */
export function fuzzyScore(query: string, text: string): number {
  return fuzzyMatch(query, text)?.score ?? -1;
}

export interface Segment {
  text: string;
  match: boolean;
}

/** Splits `text` into matched / unmatched runs for rendering `<mark>`s. */
export function highlightSegments(text: string, indices: readonly number[]): Segment[] {
  if (indices.length === 0) return [{ text, match: false }];
  const set = new Set(indices);
  const out: Segment[] = [];
  for (let i = 0; i < text.length; i++) {
    const match = set.has(i);
    const last = out[out.length - 1];
    if (last && last.match === match) last.text += text[i];
    else out.push({ text: text[i] ?? "", match });
  }
  return out;
}
