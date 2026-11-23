// @vitest-environment happy-dom
/**
 * What "save this as a file" claims, and what it does when it cannot.
 *
 * The four inline copies this module replaced all ended in an unconditional
 * `toast.success('Saved …')`. These tests hold the replacement to the other
 * half: a hand-over that could not be made is reported as one, and one that
 * was made is not called a file on the disk.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { toast } from '@/design/Toast/toasts';
import { download, safeFilename, saveFile, tellSaved } from './download';

/** What the browser was handed: the blobs, and the links that were clicked. */
const handed: { blobs: Blob[]; clicks: { href: string; download: string; attached: boolean }[]; revoked: string[] } = {
  blobs: [],
  clicks: [],
  revoked: [],
};

beforeEach(() => {
  handed.blobs = [];
  handed.clicks = [];
  handed.revoked = [];
  vi.useFakeTimers();
  vi.spyOn(URL, 'createObjectURL').mockImplementation((blob) => {
    handed.blobs.push(blob as Blob);
    return `blob:test/${handed.blobs.length}`;
  });
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation((url) => {
    handed.revoked.push(url);
  });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    handed.clicks.push({ href: this.href, download: this.download, attached: this.isConnected });
  });
});

afterEach(() => {
  vi.useRealTimers();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe('a file that can be handed to the browser', () => {
  it('is offered under its own name, with exactly the text that was given', async () => {
    const outcome = saveFile('review.md', '# SW1 lost Port-channel1\n');

    expect(outcome).toEqual({ ok: true, filename: 'review.md', confirmed: false, via: 'browser' });
    expect(handed.clicks).toEqual([{ href: 'blob:test/1', download: 'review.md', attached: true }]);
    expect(await handed.blobs[0]?.text()).toBe('# SW1 lost Port-channel1\n');
    expect(handed.blobs[0]?.type).toBe('text/plain;charset=utf-8');
    // The link was only in the document for the click.
    expect(document.querySelector('a[download]')).toBeNull();
  });

  it('keeps the address alive until the browser has had time to read it', () => {
    saveFile('a.txt', 'x');
    // Revoking in the same tick is how a download of nothing happens on WebKit.
    expect(handed.revoked).toEqual([]);
    vi.advanceTimersByTime(60_000);
    expect(handed.revoked).toEqual(['blob:test/1']);
  });

  it('takes a blob as it is, with the type the caller gave it', () => {
    const png = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' });
    expect(saveFile('map.png', png, 'image/png').ok).toBe(true);
    expect(handed.blobs[0]).toBe(png);
  });

  it('is announced as a download that started, never as a file that was saved', () => {
    const success = vi.spyOn(toast, 'success');
    const failure = vi.spyOn(toast, 'error');
    expect(download('review.md', 'text')).toBe(true);
    expect(success).toHaveBeenCalledWith('Download started: review.md');
    expect(failure).not.toHaveBeenCalled();
  });
});

describe('a file that cannot be handed over', () => {
  it('is a failure with the reason when the window cannot make a blob address', () => {
    vi.spyOn(URL, 'createObjectURL').mockImplementation(() => {
      throw new Error('blob addresses are disabled here');
    });
    const outcome = saveFile('review.md', 'text');
    expect(outcome).toEqual({ ok: false, filename: 'review.md', reason: 'blob addresses are disabled here' });
    expect(handed.clicks).toEqual([]);
  });

  it('is a failure when the click is refused, and the address is given back at once', () => {
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {
      throw new Error('downloads are blocked by policy');
    });
    const outcome = saveFile('review.md', 'text');
    expect(outcome).toEqual({ ok: false, filename: 'review.md', reason: 'downloads are blocked by policy' });
    expect(handed.revoked).toEqual(['blob:test/1']);
    expect(document.querySelector('a[download]')).toBeNull();
  });

  it('says so in an error, and does not say anything was saved', () => {
    vi.spyOn(URL, 'createObjectURL').mockImplementation(() => {
      throw new Error('no');
    });
    const success = vi.spyOn(toast, 'success');
    const info = vi.spyOn(toast, 'info');
    const failure = vi.spyOn(toast, 'error');
    expect(download('review.md', 'text')).toBe(false);
    expect(failure).toHaveBeenCalledWith('review.md was not saved', { description: 'no' });
    expect(success).not.toHaveBeenCalled();
    expect(info).not.toHaveBeenCalled();
  });
});

describe('inside the desktop shell', () => {
  it('a hand-over is announced as a request, because the WebView never reports a written file', () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const success = vi.spyOn(toast, 'success');
    const info = vi.spyOn(toast, 'info');
    const outcome = saveFile('review.md', 'text');
    expect(outcome).toMatchObject({ ok: true, via: 'shell', confirmed: false });
    tellSaved(outcome);
    expect(success).not.toHaveBeenCalled();
    expect(info).toHaveBeenCalledTimes(1);
    expect(info.mock.calls[0]?.[0]).toBe('Asked the system to save review.md');
  });
});

describe('a file name that came from a device', () => {
  it('cannot name a path, a hidden file or nothing at all', () => {
    expect(safeFilename('../../etc/passwd')).toBe('etc-passwd');
    expect(safeFilename('C:\\Users\\x\\evil.bat')).toBe('C-Users-x-evil.bat');
    expect(safeFilename('.bashrc')).toBe('bashrc');
    expect(safeFilename('...')).toBe('download');
    expect(safeFilename('   ', 'evidence')).toBe('evidence');
  });

  it('loses its control characters and direction overrides, which can dress one extension as another', () => {
    // "review" + RIGHT-TO-LEFT OVERRIDE + "fdp.exe" displays as "reviewexe.pdf".
    expect(safeFilename('review\u202efdp.exe')).toBe('review-fdp.exe');
    expect(safeFilename('a\u0000b\nc\u001b[31m')).toBe('a-b-c-[31m');
  });

  it('is kept as written when there is nothing wrong with it', () => {
    expect(safeFilename('incident-sw1-uplink-flapping-20261122-2140.md')).toBe(
      'incident-sw1-uplink-flapping-20261122-2140.md',
    );
    expect(safeFilename('Ops SW1 — last 25 lines.txt')).toBe('Ops SW1 — last 25 lines.txt');
  });

  it('is the name the browser is given, not the one that was asked for', () => {
    saveFile('../secret/..\u202egnp.svg', '<svg/>');
    expect(handed.clicks[0]?.download).toBe('secret-..-gnp.svg');
  });
});
