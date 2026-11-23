/**
 * What the desktop shell lends the interface that is not the application.
 *
 * Everything Nocturne *does* is a method on the RPC surface, and that is the
 * only door there is (`tauri.ts`, `http.ts`). Two things are not methods,
 * because they belong to the machine the window is on rather than to the
 * engine — which, in server mode, is a different machine altogether:
 *
 *   * the clipboard of the person at the window, and
 *   * their own web browser.
 *
 * In a browser the page already has both. In the desktop shell the WebView
 * has neither — a script there may not read the clipboard, and `window.open`
 * goes nowhere — so the shell provides them, through the official clipboard
 * and opener plugins and the narrowest permission each has
 * (`apps/desktop/src-tauri/capabilities/default.json`; the reasoning is in
 * `docs/dev/spec-desktop.md`). This file is the one place the interface asks.
 */

/** True when the interface is running inside the desktop shell. */
export function inDesktopShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * The text on the clipboard. Rejects when it cannot be had: the clipboard
 * holds no text, or the browser will not hand it over — it asks the person
 * first and may be told no, and over plain HTTP it does not offer the
 * clipboard to a page at all.
 *
 * Only for a paste that has no event of its own (a toolbar button, a menu
 * entry). A keyboard paste carries its text in the browser's `paste` event,
 * which needs none of this.
 */
export async function readClipboardText(): Promise<string> {
  if (inDesktopShell()) {
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<string>('plugin:clipboard-manager|read_text');
  }
  const clipboard = (navigator as Partial<Navigator>).clipboard;
  if (!clipboard?.readText) {
    throw new Error('This browser does not let a page read the clipboard here.');
  }
  return clipboard.readText();
}

/**
 * The address as it would be opened, or null when it is not one Nocturne
 * opens. Only `http` and `https`: a link in device output is text a device
 * chose, and must not be able to start another program through a scheme of
 * its own.
 */
export function externalAddress(address: string): string | null {
  try {
    const url = new URL(address);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * Opens a web address outside the application: the system browser from the
 * desktop shell, a new tab from a browser. Rejects when it is not an http(s)
 * address or the shell could not open it.
 */
export async function openExternal(address: string): Promise<void> {
  const url = externalAddress(address);
  if (url === null) throw new Error(`Only http and https addresses are opened, and this is neither: ${address}`);
  if (inDesktopShell()) {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('plugin:opener|open_url', { url });
    return;
  }
  // `noopener`: the page that opens gets no handle on this one.
  window.open(url, '_blank', 'noopener,noreferrer');
}
