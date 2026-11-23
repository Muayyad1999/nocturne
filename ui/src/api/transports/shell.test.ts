// @vitest-environment happy-dom
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { externalAddress, inDesktopShell, openExternal, readClipboardText } from './shell';

afterEach(() => {
  clearMocks();
  // `mockIPC` leaves the shell's marker on the window; a later test that
  // means "a browser" must not find it.
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});

describe('in the desktop shell', () => {
  it('reads the clipboard through the shell, because a script in the WebView may not', async () => {
    const calls: unknown[] = [];
    mockIPC((command, payload) => {
      calls.push([command, payload]);
      return 'show version';
    });
    // The WebView's own clipboard refuses, as WebKitGTK does by default: it must not even be tried.
    const readText = vi.fn().mockRejectedValue(new DOMException('no', 'NotAllowedError'));
    vi.stubGlobal('navigator', { clipboard: { readText } });

    expect(inDesktopShell()).toBe(true);
    await expect(readClipboardText()).resolves.toBe('show version');
    expect(calls).toEqual([['plugin:clipboard-manager|read_text', {}]]);
    expect(readText).not.toHaveBeenCalled();
  });

  it('says so when the shell has no text to give', async () => {
    mockIPC(() => {
      // What the plugin rejects with: the reason, as a sentence.
      // eslint-disable-next-line @typescript-eslint/only-throw-error
      throw 'The clipboard contents were not available in the requested format or the clipboard is empty.';
    });

    await expect(readClipboardText()).rejects.toMatch(/clipboard is empty/);
  });

  it('opens a link in the system browser, through the opener', async () => {
    const calls: unknown[] = [];
    mockIPC((command, payload) => {
      calls.push([command, payload]);
      return null;
    });
    const open = vi.spyOn(window, 'open').mockReturnValue(null);

    await openExternal('https://example.com/kb/42?x=1');

    expect(calls).toEqual([['plugin:opener|open_url', { url: 'https://example.com/kb/42?x=1' }]]);
    // `window.open` goes nowhere in the WebView; calling it would be the old bug.
    expect(open).not.toHaveBeenCalled();
  });

  it('asks the shell for nothing when the address is not a web address', async () => {
    const calls: unknown[] = [];
    mockIPC((command) => {
      calls.push(command);
      return null;
    });

    await expect(openExternal('file:///etc/passwd')).rejects.toThrow(/Only http and https/);
    await expect(openExternal('javascript:alert(1)')).rejects.toThrow(/Only http and https/);
    expect(calls).toEqual([]);
  });
});

describe('in a browser', () => {
  it('reads the clipboard the browser offers', async () => {
    vi.stubGlobal('navigator', { clipboard: { readText: () => Promise.resolve('show clock') } });

    expect(inDesktopShell()).toBe(false);
    await expect(readClipboardText()).resolves.toBe('show clock');
  });

  it('rejects, rather than throwing where it stands, when the page has no clipboard at all', async () => {
    // Over plain HTTP `navigator.clipboard` does not exist.
    vi.stubGlobal('navigator', {});

    await expect(readClipboardText()).rejects.toThrow(/does not let a page read the clipboard/);
  });

  it('opens a link in a new tab that has no handle on the application', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);

    await openExternal('http://10.0.0.1/status');

    expect(open).toHaveBeenCalledWith('http://10.0.0.1/status', '_blank', 'noopener,noreferrer');
  });
});

describe('externalAddress', () => {
  it('lets http and https through and nothing else', () => {
    expect(externalAddress('https://example.com/a b')).toBe('https://example.com/a%20b');
    expect(externalAddress('HTTP://EXAMPLE.com')).toBe('http://example.com/');
    expect(externalAddress('ssh://admin@10.0.0.1')).toBeNull();
    expect(externalAddress('mailto:noc@example.com')).toBeNull();
    expect(externalAddress('not an address')).toBeNull();
  });
});
