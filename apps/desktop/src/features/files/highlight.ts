/** Splits text into highlighted / plain segments. */
export interface Segment {
  text: string;
  match: boolean;
}

/** Segments for matched UTF-16 positions (from `files.find`). */
export function segmentsFromPositions(text: string, positions: readonly number[]): Segment[] {
  const set = new Set(positions);
  const out: Segment[] = [];
  for (let i = 0; i < text.length; i++) {
    const match = set.has(i);
    const last = out[out.length - 1];
    if (last && last.match === match) last.text += text[i];
    else out.push({ text: text[i] ?? "", match });
  }
  return out;
}

/** Segments for one `[start, end)` range (from `files.search` previews). */
export function segmentsFromRange(text: string, start: number, end: number): Segment[] {
  const s = Math.max(0, Math.min(start, text.length));
  const e = Math.max(s, Math.min(end, text.length));
  return [
    { text: text.slice(0, s), match: false },
    { text: text.slice(s, e), match: true },
    { text: text.slice(e), match: false },
  ].filter((seg) => seg.text.length > 0);
}

/** Splits positions of a full path into (directory, basename) parts. */
export function splitPathPositions(path: string, positions: readonly number[]): {
  dir: string;
  name: string;
  dirPositions: number[];
  namePositions: number[];
} {
  const cut = path.lastIndexOf("/") + 1;
  return {
    dir: path.slice(0, Math.max(0, cut - 1)),
    name: path.slice(cut),
    dirPositions: positions.filter((p) => p < cut - 1),
    namePositions: positions.filter((p) => p >= cut).map((p) => p - cut),
  };
}
