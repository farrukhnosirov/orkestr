/**
 * Buffer for events that arrive while the Logs stream is paused. Only the
 * newest `max` events are kept: on resume the list is trimmed to the same
 * size anyway, so older ones would be discarded after costing memory.
 */
export class PauseQueue<T> {
  private items: T[] = [];
  /** Events received since the last drain, including dropped ones. */
  private received = 0;

  constructor(private readonly max: number) {}

  push(item: T): void {
    this.received += 1;
    this.items.push(item);
    if (this.items.length > this.max) this.items.splice(0, this.items.length - this.max);
  }

  /** Total received while paused (for the badge). */
  get count(): number {
    return this.received;
  }

  /** Buffered events (at most `max`). */
  get size(): number {
    return this.items.length;
  }

  /** Returns the buffered events and empties the queue. */
  drain(): T[] {
    const out = this.items;
    this.items = [];
    this.received = 0;
    return out;
  }
}
