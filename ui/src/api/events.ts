/**
 * Event topics and the subscription registry shared by every transport.
 */

/** Something the backend announced. */
export interface AppEvent<P = unknown> {
  /** Dotted name, e.g. `devices.changed`. */
  topic: string;
  payload: P;
  /** Milliseconds since the epoch. */
  at: number;
}

export type EventHandler<P = unknown> = (event: AppEvent<P>) => void;

/**
 * Whether `topic` is covered by `pattern`. A pattern is an exact topic, or a
 * prefix ending in `*`: `devices.*` covers `devices.changed`, and `*` covers
 * everything.
 */
export function topicMatches(pattern: string, topic: string): boolean {
  return pattern.endsWith('*') ? topic.startsWith(pattern.slice(0, -1)) : pattern === topic;
}

/** Parses one event frame; null if it is not an event. */
export function parseEvent(value: unknown): AppEvent | null {
  if (typeof value !== 'object' || value === null) return null;
  const { topic, payload, at } = value as Record<string, unknown>;
  if (typeof topic !== 'string' || typeof at !== 'number') return null;
  return { topic, payload, at };
}

function report(error: unknown): void {
  // Surface the failure like any uncaught error without stopping delivery.
  if (typeof globalThis.reportError === 'function') globalThis.reportError(error);
  else console.error(error);
}

/** Routes events to the handlers whose pattern covers their topic. */
export class EventHub {
  private readonly handlers = new Map<string, Set<EventHandler>>();

  subscribe<P = unknown>(pattern: string, handler: EventHandler<P>): () => void {
    let set = this.handlers.get(pattern);
    if (!set) {
      set = new Set();
      this.handlers.set(pattern, set);
    }
    // Each subscription is its own entry, so the same function can subscribe twice.
    const entry: EventHandler = (event) => handler(event as AppEvent<P>);
    set.add(entry);

    return () => {
      const current = this.handlers.get(pattern);
      if (!current?.delete(entry)) return;
      if (current.size === 0) this.handlers.delete(pattern);
    };
  }

  dispatch(event: AppEvent): void {
    for (const [pattern, set] of [...this.handlers]) {
      if (!topicMatches(pattern, event.topic)) continue;
      for (const handler of [...set]) {
        // A handler removed by an earlier one during this dispatch is skipped.
        if (!set.has(handler)) continue;
        try {
          handler(event);
        } catch (error) {
          report(error);
        }
      }
    }
  }
}
