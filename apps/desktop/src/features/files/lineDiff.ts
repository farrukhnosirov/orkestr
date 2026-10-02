/**
 * Line diff used for agent change markers: which lines of `next` are new or
 * modified compared to `prev`. Common prefix/suffix are trimmed first; the
 * middle uses an LCS table when small enough, otherwise every middle line
 * counts as changed (still correct, just coarser).
 */

const MAX_CELLS = 4_000_000;

/** Splits text into lines (handles `\r\n`). */
export function splitLines(text: string): string[] {
  return text.split(/\r?\n/);
}

/**
 * 1-based line numbers in `next` that are added or changed relative to `prev`.
 * Pure deletions are reported as the line that now sits at the deletion point
 * (so the gutter still shows *something* happened there).
 */
export function changedLines(prev: string, next: string): number[] {
  if (prev === next) return [];
  const a = splitLines(prev);
  const b = splitLines(next);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length - 1;
  let endB = b.length - 1;
  while (endA >= start && endB >= start && a[endA] === b[endB]) {
    endA--;
    endB--;
  }
  const midA = a.slice(start, endA + 1);
  const midB = b.slice(start, endB + 1);
  if (midB.length === 0) {
    // Only deletions: mark the line now at the deletion point.
    return [Math.min(start + 1, b.length)];
  }
  if (midA.length === 0 || midA.length * midB.length > MAX_CELLS) {
    return range(start + 1, start + midB.length);
  }
  // LCS over the middle.
  const n = midA.length;
  const m = midB.length;
  const table: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    const row = table[i]!;
    const below = table[i + 1]!;
    for (let j = m - 1; j >= 0; j--) {
      row[j] = midA[i] === midB[j] ? below[j + 1]! + 1 : Math.max(below[j]!, row[j + 1]!);
    }
  }
  const out: number[] = [];
  let i = 0;
  let j = 0;
  let pendingDelete = false;
  while (j < m) {
    if (i < n && midA[i] === midB[j]) {
      if (pendingDelete) out.push(start + j + 1);
      pendingDelete = false;
      i++;
      j++;
    } else if (i < n && table[i + 1]![j]! >= table[i]![j + 1]!) {
      pendingDelete = true;
      i++;
    } else {
      out.push(start + j + 1);
      pendingDelete = false;
      j++;
    }
  }
  return [...new Set(out)].sort((x, y) => x - y);
}

function range(from: number, to: number): number[] {
  const out: number[] = [];
  for (let k = from; k <= to; k++) out.push(k);
  return out;
}

/** Collapses sorted line numbers into inclusive ranges: [1,2,3,7] → [[1,3],[7,7]]. */
export function toRanges(lines: readonly number[]): [number, number][] {
  const out: [number, number][] = [];
  for (const l of lines) {
    const last = out[out.length - 1];
    if (last && l === last[1] + 1) last[1] = l;
    else out.push([l, l]);
  }
  return out;
}
