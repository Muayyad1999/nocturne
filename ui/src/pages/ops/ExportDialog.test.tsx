// @vitest-environment happy-dom
/**
 * What the export dialog does with an HTML document, as far as a DOM without
 * a browser around it can show: which frames exist, what each is allowed, and
 * what is said between this page and the frame that prints.
 *
 * That the frames really are inert, and that a print really is asked for,
 * needs an engine — `tests/print.spec.ts` drives Chromium with a document
 * that tries to run script. This file holds the dialog to the arrangement
 * that spec proves safe.
 */

import { StrictMode, act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Format, Rendered } from '@/api/types/ops';
import { useToasts } from '@/design/Toast';
import { ExportDialog } from './ExportDialog';
import { PRINT_SANDBOX, READY_TIMEOUT_MS, printShell } from './PrintFrame';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Text no shell of ours contains: wherever it turns up, the document went there. */
const MARKER = 'CORE-SW1 lost Port-channel1';
const DOCUMENT = `<!doctype html><html><head><title>Review</title></head><body><h1>${MARKER}</h1></body></html>`;

function rendered(format: Format): Rendered {
  return {
    title: 'Review',
    format,
    filename: `review.${format === 'markdown' ? 'md' : format}`,
    content: format === 'html' ? DOCUMENT : `# ${MARKER}`,
    rows: 3,
    generated_at: 1_700_000_000,
  };
}

// The dialog renders nothing itself; the engine's answer is stood in for here.
vi.mock('./data', () => ({
  download: vi.fn(),
  useRendered: (_method: string, params: { format: Format } | null) => ({
    data: params ? rendered(params.format) : undefined,
    isPending: false,
    isError: false,
    error: null,
  }),
}));

let root: Root | null = null;
/** Rendered the way `main.tsx` renders the application in a development build. */
let strict = false;

/** The dialog as its screens hold it: always mounted, shown or not. */
function show(shown: boolean): void {
  const dialog = (
    <ExportDialog
      open={shown}
      onClose={() => undefined}
      title="Post-incident review"
      description="Written by the engine."
      method="incidents.export"
      params={{ id: 'inc_1' }}
    />
  );
  act(() => {
    root?.render(strict ? <StrictMode>{dialog}</StrictMode> : dialog);
  });
}

function open(): void {
  const host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  show(true);
}

function chooseHtml(): void {
  const select = document.querySelector('select');
  if (!select) throw new Error('the dialog has no format selector');
  act(() => {
    select.value = 'html';
    select.dispatchEvent(new Event('change', { bubbles: true }));
  });
}

function button(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find((entry) => entry.textContent?.trim() === name);
  if (!found) throw new Error(`no ${name} button`);
  return found;
}

const preview = () => document.querySelector<HTMLIFrameElement>('iframe[title$="(preview)"]');
const printFrame = () => document.querySelector<HTMLIFrameElement>('iframe[title="Document being printed"]');

/**
 * Gives a frame a window this test can recognise and listen to. What a
 * frame's real window does is the browser's business; who is spoken to, and
 * what is said, is the dialog's.
 */
function windowOf(frame: HTMLIFrameElement | null) {
  if (!frame) throw new Error('the frame is not there');
  const postMessage = vi.fn();
  const touchedPrint = vi.fn();
  const fake = {
    postMessage,
    // A window of another origin: reading `print` is what throws.
    get print(): never {
      touchedPrint();
      throw new DOMException('Blocked a frame from accessing a cross-origin frame.', 'SecurityError');
    },
  };
  Object.defineProperty(frame, 'contentWindow', { configurable: true, get: () => fake });
  return { window: fake as unknown as Window, postMessage, touchedPrint };
}

function says(source: Window, state: string): void {
  act(() => {
    window.dispatchEvent(new MessageEvent('message', { data: { nocturnePrint: state }, source }));
  });
}

const toasts = () => useToasts.getState().toasts.map((toast) => [toast.tone, toast.title]);

beforeEach(() => {
  useToasts.setState({ toasts: [], history: [], unread: 0 });
  // This DOM does not fetch scripts, and says so loudly for the print
  // frame's. What that script does is the browser's to show (print.spec.ts).
  const happyDOM = (window as { happyDOM?: { settings: { handleDisabledFileLoadingAsSuccess: boolean } } }).happyDOM;
  if (happyDOM) happyDOM.settings.handleDisabledFileLoadingAsSuccess = true;
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  strict = false;
  document.body.replaceChildren();
  vi.useRealTimers();
});

describe('an HTML export on screen', () => {
  it('is shown in a frame that is allowed nothing: no script, and not this page’s origin', () => {
    open();
    chooseHtml();
    expect(preview()?.getAttribute('sandbox')).toBe('');
    expect(preview()?.getAttribute('srcdoc')).toBe(DOCUMENT);
    expect(printFrame(), 'nothing prints until it is asked to').toBeNull();
  });
});

describe('Print', () => {
  it('never reaches into the preview, whose window belongs to another origin', () => {
    open();
    chooseHtml();
    const shown = windowOf(preview());

    act(() => button('Print').click());

    expect(shown.touchedPrint).not.toHaveBeenCalled();
    expect(shown.postMessage).not.toHaveBeenCalled();
    expect(preview()?.getAttribute('sandbox'), 'and the preview is still allowed nothing').toBe('');
  });

  it('starts a frame of its own, without this page’s origin and without the document in it', () => {
    open();
    chooseHtml();
    act(() => button('Print').click());

    const frame = printFrame();
    const allowed = frame?.getAttribute('sandbox')?.split(' ') ?? [];
    expect(allowed.sort()).toEqual(['allow-modals', 'allow-scripts']);
    expect(allowed).not.toContain('allow-same-origin');
    expect(frame?.getAttribute('sandbox')).toBe(PRINT_SANDBOX);
    // The frame starts empty: a report is never parsed where a script may run.
    expect(frame?.getAttribute('srcdoc')).not.toContain(MARKER);
    expect(frame?.getAttribute('srcdoc')).toContain('Content-Security-Policy');
  });

  it('gives the document to that frame when it says it is ready, once, and to no other window', () => {
    open();
    chooseHtml();
    const shown = windowOf(preview());
    act(() => button('Print').click());
    const printing = windowOf(printFrame());

    // Anybody can post "ready" to this page. The preview is the nearest somebody.
    says(shown.window, 'ready');
    expect(shown.postMessage).not.toHaveBeenCalled();
    expect(printing.postMessage).not.toHaveBeenCalled();

    says(printing.window, 'ready');
    expect(printing.postMessage).toHaveBeenCalledTimes(1);
    expect(printing.postMessage).toHaveBeenCalledWith({ nocturnePrint: 'document', html: DOCUMENT }, '*');

    // Asking again gets nothing: by now the frame holds the document.
    says(printing.window, 'ready');
    expect(printing.postMessage).toHaveBeenCalledTimes(1);
    expect(toasts()).toEqual([]);
  });

  it('says so when the frame never answers, instead of doing nothing', () => {
    vi.useFakeTimers();
    open();
    chooseHtml();
    act(() => button('Print').click());
    windowOf(printFrame());

    act(() => vi.advanceTimersByTime(READY_TIMEOUT_MS - 1));
    expect(toasts()).toEqual([]);
    act(() => vi.advanceTimersByTime(1));

    expect(toasts()).toEqual([['error', 'This window could not print the document']]);
    expect(printFrame(), 'the frame that did not answer is gone').toBeNull();
  });

  it('does not time a print that has started: the dialog may be open for minutes', () => {
    vi.useFakeTimers();
    open();
    chooseHtml();
    act(() => button('Print').click());
    const printing = windowOf(printFrame());
    says(printing.window, 'ready');

    act(() => vi.advanceTimersByTime(READY_TIMEOUT_MS * 100));

    expect(toasts()).toEqual([]);
    expect(printFrame()).not.toBeNull();
  });

  it('says so when the frame reports that it could not print', () => {
    open();
    chooseHtml();
    act(() => button('Print').click());
    const printing = windowOf(printFrame());
    says(printing.window, 'ready');

    says(printing.window, 'failed');

    expect(toasts()).toEqual([['error', 'This window could not print the document']]);
    expect(printFrame()).toBeNull();
  });

  it('uses a new frame for each print', () => {
    open();
    chooseHtml();
    act(() => button('Print').click());
    const first = printFrame();
    act(() => button('Print').click());
    expect(printFrame()).not.toBe(first);
    expect(document.querySelectorAll('iframe[title="Document being printed"]')).toHaveLength(1);
  });

  it('prints when Print is pressed and not when the dialog is opened again afterwards', () => {
    open();
    chooseHtml();
    act(() => button('Print').click());
    expect(printFrame()).not.toBeNull();

    // The screens keep the dialog mounted and only stop showing it, so what
    // it remembers outlives what it shows.
    show(false);
    expect(printFrame()).toBeNull();
    show(true);

    expect(preview()?.getAttribute('srcdoc'), 'the dialog is back, on the format it was left on').toBe(DOCUMENT);
    expect(printFrame(), 'nobody pressed Print this time').toBeNull();

    // And Print still prints.
    act(() => button('Print').click());
    const printing = windowOf(printFrame());
    says(printing.window, 'ready');
    expect(printing.postMessage).toHaveBeenCalledTimes(1);
  });

  it('still prints in a development build, where every effect is run, undone and run again', () => {
    strict = true;
    open();
    chooseHtml();
    act(() => button('Print').click());

    const printing = windowOf(printFrame());
    says(printing.window, 'ready');
    expect(printing.postMessage).toHaveBeenCalledTimes(1);
    expect(printing.postMessage).toHaveBeenCalledWith({ nocturnePrint: 'document', html: DOCUMENT }, '*');
    expect(printFrame()).not.toBeNull();
  });
});

describe('the empty document the print frame starts with', () => {
  const SCRIPT = 'http://127.0.0.1:8787/assets/printFrame-abc123.js';

  function directives(shell: string): Map<string, string[]> {
    const parsed = new DOMParser().parseFromString(shell, 'text/html');
    const policy = parsed.querySelector('meta[http-equiv="Content-Security-Policy"]')?.getAttribute('content') ?? '';
    return new Map(
      policy.split(';').map((directive) => {
        const [name = '', ...sources] = directive.trim().split(/\s+/);
        return [name, sources] as const;
      }),
    );
  }

  it('may run the one script that prints, and no other: not inline, not from this address', () => {
    const policy = directives(printShell(SCRIPT));
    expect(policy.get('script-src')).toEqual([SCRIPT]);
    expect(policy.get('default-src')).toEqual(["'none'"]);
    expect(policy.get('base-uri')).toEqual(["'none'"]);
  });

  it('may ask the network for nothing: a report carries its own styles, images and fonts or has none', () => {
    const policy = directives(printShell(SCRIPT));
    const sources = [...policy].filter(([name]) => name !== 'script-src').flatMap(([, list]) => list);
    expect(new Set(sources)).toEqual(new Set(["'none'", "'unsafe-inline'", 'data:']));
    expect(policy.get('style-src')).toEqual(["'unsafe-inline'"]);
  });

  it('holds the policy before the script, and nothing else', () => {
    const parsed = new DOMParser().parseFromString(printShell(SCRIPT), 'text/html');
    const head = [...parsed.head.children].map((element) => element.tagName.toLowerCase());
    expect(head).toEqual(['meta', 'meta', 'script']);
    expect(parsed.head.children[1]?.getAttribute('http-equiv')).toBe('Content-Security-Policy');
    expect(parsed.querySelector('script')?.getAttribute('src')).toBe(SCRIPT);
    expect(parsed.querySelector('script')?.textContent).toBe('');
    expect(parsed.body.childNodes).toHaveLength(0);
  });
});
