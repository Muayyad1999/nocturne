/**
 * Copying text, and the short memory of what was copied.
 *
 * `copyText(text)` writes to the system clipboard and that is all it does.
 * A control that wants the text offered again later — in the Clips panel,
 * to copy once more, type into a terminal or keep as evidence — says so at
 * its own call site: `copyText(text, { clip: { label, source } })`.
 *
 * That it is opt-in per call site is the whole safety of it. There is no
 * hook in here that sees every copy, no `copy` listener on the document and
 * no read of the system clipboard, so a decoded secret, a generated password
 * or a private key copied from the toolbox is not left out by a filter that
 * could miss one: nothing ever asked for it to be kept.
 *
 * Clips live in this module's memory and nowhere else: not in
 * `localStorage`, `sessionStorage` or IndexedDB, not in the engine. A reload
 * empties them, and so does `clips.clear()`.
 */

/** What a call site says about the text it wants remembered. */
export interface ClipOrigin {
  /** What the text is, for the person choosing among clips: "Selection from SW1". */
  label: string;
  /** The screen or control it was copied from: "Terminal". */
  source: string;
}

export interface CopyOptions {
  /**
   * Remember this text as a clip. Leave it out — the default — for anything
   * that is or may be a secret; it is then written to the clipboard and
   * kept nowhere.
   */
  clip?: ClipOrigin;
}

/** Something that was copied through a control that asked for it to be remembered. */
export interface Clip {
  /** Stable for as long as the clip lives; a number counted from 1 in this window. */
  id: number;
  text: string;
  label: string;
  source: string;
  /** When it was copied, in milliseconds. Copying the same text again moves it here. */
  at: number;
}

/** How many clips are held. The oldest goes when one more arrives. */
export const CLIP_LIMIT = 30;

/**
 * The longest text held as a clip, in bytes of UTF-8. It is the size an
 * evidence item may be — which is counted in bytes, so a text in a script
 * other than Latin reaches it at a third of that many characters — so that a
 * clip can still be kept; a longer copy reaches the clipboard as always and
 * is simply not remembered, never cut short.
 */
export const CLIP_MAX_BYTES = 262_144;

let held: readonly Clip[] = [];
let counter = 0;
const listeners = new Set<() => void>();

function publish(next: readonly Clip[]): void {
  held = next;
  for (const listener of listeners) listener();
}

/** Whether `text` would be remembered if a call site asked. */
export function fitsAsClip(text: string): boolean {
  if (text === '') return false;
  // A character is at most three bytes of UTF-8 for each UTF-16 unit, and at
  // least one: only a text between the two has to be measured.
  if (text.length > CLIP_MAX_BYTES) return false;
  if (text.length * 3 <= CLIP_MAX_BYTES) return true;
  return new TextEncoder().encode(text).length <= CLIP_MAX_BYTES;
}

function remember(text: string, origin: ClipOrigin): void {
  if (!fitsAsClip(text)) return;
  // The same text copied again is the same clip, moved to the front under
  // what it was called last: ten copies of one address are not ten clips.
  const others = held.filter((clip) => clip.text !== text);
  counter += 1;
  const clip: Clip = { id: counter, text, label: origin.label, source: origin.source, at: Date.now() };
  publish([clip, ...others].slice(0, CLIP_LIMIT));
}

/**
 * The clips, newest first. A store in the shape `useSyncExternalStore`
 * takes: `list()` returns the same array until something changes.
 */
export const clips = {
  list: (): readonly Clip[] => held,
  subscribe: (listener: () => void): (() => void) => {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
  /** Forgets one clip. */
  remove: (id: number): void => {
    if (held.some((clip) => clip.id === id)) publish(held.filter((clip) => clip.id !== id));
  },
  /** Forgets every clip. */
  clear: (): void => {
    if (held.length > 0) publish([]);
  },
};

/**
 * Copies text to the clipboard. Resolves false if the platform refused.
 *
 * With `clip`, the text is also remembered — whether or not the clipboard
 * took it: over plain HTTP a browser has no clipboard to write to, and the
 * panel is then the only way the text gets from one screen to another.
 */
export async function copyText(text: string, options: CopyOptions = {}): Promise<boolean> {
  if (options.clip) remember(text, options.clip);
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
