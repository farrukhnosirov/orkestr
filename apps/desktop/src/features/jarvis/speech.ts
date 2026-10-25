/**
 * Streaming speech (ADR-0049): a reply arrives in deltas; complete sentences
 * are spoken one after another while the rest is still being written. The
 * command hand-off line (`<<run: …>>`) is never shown or spoken.
 */

/** Text before the hand-off marker (`<<` … — possibly still incomplete). */
export function visibleText(text: string, trim = true): string {
  const i = text.indexOf("<<");
  const v = (i >= 0 ? text.slice(0, i) : text).replace(/[*_`#]+/g, "");
  return trim ? v.trimEnd() : v;
}

/**
 * Splits off complete sentences: `[sentences, rest]`. A sentence ends with
 * `.`, `!`, `?`, `…` or a newline followed by whitespace; very short pieces
 * («Т.е.», «1.») are merged with the next one.
 */
export function splitSentences(text: string, minChars = 12): [string[], string] {
  const out: string[] = [];
  let start = 0;
  const re = /([.!?…]+["»”)]?)(\s+)|\n+/g;
  let m: RegExpExecArray | null;
  let pending = "";
  while ((m = re.exec(text)) !== null) {
    const end = m.index + (m[1]?.length ?? 0);
    const piece = (pending + text.slice(start, end)).trim();
    start = m.index + m[0].length;
    if (piece.length < minChars) {
      pending = piece ? `${piece} ` : "";
      continue;
    }
    out.push(piece);
    pending = "";
  }
  return [out, pending + text.slice(start)];
}

/**
 * Speaks streamed text sentence by sentence through `speak` (awaited in
 * order, so utterances never interrupt each other). `cancel()` drops the
 * queue; `finish()` speaks the remainder and resolves when all is said.
 */
export class SentenceSpeaker {
  private buffer = "";
  private queue: string[] = [];
  private running: Promise<void> | null = null;
  private cancelled = false;
  /** Everything handed to `speak` so far. */
  readonly spoken: string[] = [];

  constructor(private readonly speak: (sentence: string) => Promise<void>) {}

  /** Adds a delta (`reset`: the provider switched — start over). */
  push(delta: string, reset = false): void {
    if (this.cancelled) return;
    if (reset) {
      this.buffer = "";
      this.queue = [];
      return;
    }
    this.buffer += delta;
    const visible = visibleText(this.buffer, false);
    const hidden = this.buffer.includes("<<");
    const [sentences, rest] = splitSentences(visible);
    if (sentences.length === 0) return;
    this.queue.push(...sentences);
    // keep the unspoken rest (and the hand-off marker, if any)
    this.buffer = hidden ? `${rest}${this.buffer.slice(this.buffer.indexOf("<<"))}` : rest;
    this.pump();
  }

  private pump(): void {
    if (this.running) return;
    this.running = (async () => {
      while (!this.cancelled && this.queue.length > 0) {
        const s = this.queue.shift()!;
        this.spoken.push(s);
        try {
          await this.speak(s);
        } catch {
          /* a failed utterance does not stop the rest */
        }
      }
      this.running = null;
    })();
  }

  /** Speaks what is left; resolves when everything was said. */
  async finish(): Promise<void> {
    const rest = visibleText(this.buffer).trim();
    this.buffer = "";
    if (rest && !this.cancelled) this.queue.push(rest);
    this.pump();
    while (this.running) await this.running;
  }

  cancel(): void {
    this.cancelled = true;
    this.queue = [];
  }
}
