import { useSyncExternalStore } from 'react';

interface Ticker {
  listeners: Set<() => void>;
  timer: ReturnType<typeof setInterval>;
}

/** One timer per interval, shared by every subscriber, however many there are. */
const tickers = new Map<number, Ticker>();

function subscribe(interval: number, listener: () => void): () => void {
  let ticker = tickers.get(interval);
  if (!ticker) {
    const listeners = new Set<() => void>();
    ticker = {
      listeners,
      timer: setInterval(() => {
        for (const notify of [...listeners]) notify();
      }, interval),
    };
    tickers.set(interval, ticker);
  }
  ticker.listeners.add(listener);

  return () => {
    const current = tickers.get(interval);
    if (!current) return;
    current.listeners.delete(listener);
    if (current.listeners.size === 0) {
      clearInterval(current.timer);
      tickers.delete(interval);
    }
  };
}

/**
 * The current time, re-rendering the component every `interval` milliseconds.
 * The value is rounded down to the interval, so it is stable between ticks.
 * A table of five hundred relative times costs one timer, not five hundred.
 */
export function useNow(interval: number): number {
  return useSyncExternalStore(
    (listener) => subscribe(interval, listener),
    () => Math.floor(Date.now() / interval) * interval,
  );
}
