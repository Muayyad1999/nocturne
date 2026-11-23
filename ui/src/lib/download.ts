/**
 * Saving something as a file: the one implementation.
 *
 * A page cannot write a file. What it can do is hand a blob to the browser
 * through a link that carries a `download` name, and the browser decides the
 * rest — where it goes, whether to ask, whether to refuse. Nothing comes back:
 * there is no event for "the file is on the disk". So this module is careful
 * about what it claims:
 *
 * - {@link saveFile} answers `ok: false` with a reason whenever the hand-over
 *   itself could not be made (no document, no blob addresses, a link that
 *   does not know `download`, an exception from any step), and a screen then
 *   says so instead of toasting "Saved" over nothing;
 * - an `ok: true` answer is "handed over", with `confirmed: false` spelled out
 *   in the type, and {@link tellSaved} words it that way. In the desktop
 *   shell it says more, because a WebView without a download handler can drop
 *   the hand-over without a word, and a person who was told "Saved" would go
 *   looking for a file that was never written.
 *
 * What is saved is exactly the text or the blob that was given. The name is
 * cleaned ({@link safeFilename}) because it can come from a device: a report
 * is named after an incident, an incident after an alert, an alert after an
 * interface description.
 */

import { toast } from '@/design/Toast/toasts';

export type SaveOutcome =
  | {
      ok: true;
      /** The name the file was offered under, after cleaning. */
      filename: string;
      /** Always false: a browser never reports that a download reached the disk. */
      confirmed: false;
      /** `shell` inside the desktop application, whose WebView may not save downloads at all. */
      via: 'browser' | 'shell';
    }
  | {
      ok: false;
      filename: string;
      /** A sentence for the person at the keyboard. */
      reason: string;
    };

const TEXT_TYPE = 'text/plain;charset=utf-8';

/**
 * How long the blob's address is kept after the click. The download starts
 * asynchronously (WebKit reads the blob after the click handler has returned),
 * so revoking in the same tick, as the inline copies did, can hand the
 * browser an address that is already dead.
 */
const REVOKE_AFTER_MS = 60_000;

/** Characters no file system takes in a name, the controls, and the direction overrides. */
const UNSAFE = /[\u0000-\u001f\u007f<>:"/\\|?*\u202a-\u202e\u2066-\u2069]+/g;

/**
 * A name a file can have, from text that may have come from anywhere.
 *
 * Path separators, control characters and direction overrides become a
 * hyphen; leading dots go (a download must not be a hidden file, and `..` is
 * not a name); the length is capped where every file system still takes it.
 */
export function safeFilename(name: string, fallback = 'download'): string {
  const cleaned = name
    .normalize('NFC')
    .replace(UNSAFE, '-')
    .replace(/\s+/g, ' ')
    .replace(/^[.\s-]+/, '')
    .replace(/[.\s]+$/, '')
    .slice(0, 180);
  return cleaned === '' ? fallback : cleaned;
}

function inShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Offers `content` to the browser as a file called `filename`.
 *
 * Never throws: every way the hand-over can fail is an `ok: false` outcome
 * with its reason.
 */
export function saveFile(filename: string, content: string | Blob, type: string = TEXT_TYPE): SaveOutcome {
  const name = safeFilename(filename);
  const failed = (reason: string): SaveOutcome => ({ ok: false, filename: name, reason });

  if (typeof document === 'undefined' || typeof URL === 'undefined' || typeof URL.createObjectURL !== 'function') {
    return failed('This window cannot make a file to save.');
  }

  let url: string;
  try {
    const blob = typeof content === 'string' ? new Blob([content], { type }) : content;
    url = URL.createObjectURL(blob);
  } catch (error) {
    return failed(error instanceof Error && error.message ? error.message : 'The file could not be prepared.');
  }

  try {
    const anchor = document.createElement('a');
    if (!('download' in anchor)) {
      URL.revokeObjectURL(url);
      // Without the attribute the click would *open* the blob in this window,
      // which for a drawing or a document made from device data is the one
      // thing that must never happen.
      return failed('This window does not support saving a file from a page.');
    }
    anchor.href = url;
    anchor.download = name;
    anchor.rel = 'noopener';
    anchor.style.display = 'none';
    // Attached while it is clicked: some engines ignore a click on a link
    // that is not in the document.
    document.body.append(anchor);
    try {
      anchor.click();
    } finally {
      anchor.remove();
    }
  } catch (error) {
    URL.revokeObjectURL(url);
    return failed(error instanceof Error && error.message ? error.message : 'The browser refused the download.');
  }

  setTimeout(() => URL.revokeObjectURL(url), REVOKE_AFTER_MS);
  return { ok: true, filename: name, confirmed: false, via: inShell() ? 'shell' : 'browser' };
}

/**
 * Says what happened to a save, in a toast. Returns whether it was handed
 * over, so a caller can stop there.
 */
export function tellSaved(outcome: SaveOutcome): boolean {
  if (!outcome.ok) {
    toast.error(`${outcome.filename} was not saved`, { description: outcome.reason });
    return false;
  }
  if (outcome.via === 'shell') {
    toast.info(`Asked the system to save ${outcome.filename}`, {
      description:
        'The desktop window is not told whether a file was written. If it is not in your downloads folder, copy the content instead.',
    });
    return true;
  }
  toast.success(`Download started: ${outcome.filename}`);
  return true;
}

/** {@link saveFile} and {@link tellSaved} in one call, for a button. */
export function download(filename: string, content: string | Blob, type?: string): boolean {
  return tellSaved(saveFile(filename, content, type));
}
