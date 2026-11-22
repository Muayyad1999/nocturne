// @vitest-environment happy-dom
/**
 * The two dialogs of the Topology screen that used to lie by omission: the
 * name of a new view (asked with `window.prompt`, which the macOS WebView
 * answers "cancelled" without showing anything) and the export's Copy button
 * (which said "Copied" whatever the clipboard did).
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { toast } from '@/design/Toast';
import { ExportDialog } from './ExportDialog';
import { SaveViewDialog } from './SaveViewDialog';

const clipboard = vi.hoisted(() => ({ accepts: true, written: [] as string[] }));

vi.mock('@/lib/clipboard', () => ({
  copyText: async (text: string) => {
    if (clipboard.accepts) clipboard.written.push(text);
    return clipboard.accepts;
  },
}));

// The dialog's one read, answered as `topology.export` answers it.
vi.mock('@/api/hooks', () => ({
  useRpc: () => ({
    data: { format: 'mermaid', filename: 'topology-physical.mmd', text: 'graph TD\n  A --- B' },
    isPending: false,
    isError: false,
    error: null,
  }),
}));

let root: Root | null = null;
let host: HTMLElement | null = null;

function render(element: React.ReactElement): void {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => root?.render(element));
}

function button(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find(
    (candidate) => candidate.textContent?.trim() === name,
  );
  if (!found) throw new Error(`no button called “${name}”`);
  return found;
}

function type(input: HTMLInputElement, text: string): void {
  // React tracks an input's value itself, so it is set the way the browser
  // sets it and then announced.
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
  act(() => {
    setter?.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  clipboard.accepts = true;
  clipboard.written = [];
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

describe('naming a new view', () => {
  const open = (onSave: (name: string) => void, onClose = () => undefined) =>
    render(<SaveViewDialog open onClose={onClose} onSave={onSave} saving={false} />);

  it('asks in a dialog of its own and never through window.prompt', () => {
    const prompt = vi.fn(() => null);
    vi.stubGlobal('prompt', prompt);
    const saved: string[] = [];
    open((name) => saved.push(name));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain('Save as a new view');
    const input = dialog?.querySelector('input');
    expect(input).toBeTruthy();
    if (!input) return;

    type(input, '  Core and distribution  ');
    act(() => button('Save view').click());

    expect(saved).toEqual(['Core and distribution']);
    expect(prompt).not.toHaveBeenCalled();
  });

  it('saves on Enter, which is how a one-field dialog is answered', () => {
    const saved: string[] = [];
    open((name) => saved.push(name));
    const input = document.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (!input) throw new Error('no name field');

    type(input, 'North campus');
    act(() => {
      input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    expect(saved).toEqual(['North campus']);
  });

  it('leaves the Enter that confirms an input-method composition to the composition', () => {
    // Typing a name in Japanese, Chinese or Korean ends each word with an
    // Enter that belongs to the input method. Taken as "save", it stores the
    // view under a half-typed name and closes the dialog.
    const saved: string[] = [];
    open((name) => saved.push(name));
    const input = document.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (!input) throw new Error('no name field');

    type(input, 'コア');
    act(() => {
      input.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true }),
      );
    });
    expect(saved).toEqual([]);

    // The next Enter is the user's own.
    act(() => {
      input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    expect(saved).toEqual(['コア']);
  });

  it('does not save a view with no name, or one made of spaces', () => {
    const saved: string[] = [];
    open((name) => saved.push(name));
    const input = document.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (!input) throw new Error('no name field');

    expect(button('Save view').disabled).toBe(true);
    type(input, '   ');
    expect(button('Save view').disabled).toBe(true);
    act(() => {
      input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    expect(saved).toEqual([]);
  });

  it('saves nothing when backed out of', () => {
    const saved: string[] = [];
    let closed = 0;
    open(
      (name) => saved.push(name),
      () => {
        closed += 1;
      },
    );
    const input = document.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (!input) throw new Error('no name field');
    type(input, 'Abandoned');
    act(() => button('Cancel').click());
    expect(closed).toBe(1);
    expect(saved).toEqual([]);
  });
});

describe('copying an export', () => {
  const copy = async () => {
    render(<ExportDialog open onClose={() => undefined} params={{}} />);
    await act(async () => {
      button('Copy').click();
      // The click handler awaits the clipboard.
      await Promise.resolve();
      await Promise.resolve();
    });
  };

  it('says “Copied” when the clipboard took the text, and it is the text on screen', async () => {
    const success = vi.spyOn(toast, 'success');
    const failure = vi.spyOn(toast, 'error');
    await copy();
    expect(clipboard.written).toEqual(['graph TD\n  A --- B']);
    expect(success).toHaveBeenCalledWith('Copied');
    expect(failure).not.toHaveBeenCalled();
  });

  it('says the clipboard refused when it did, and never “Copied”', async () => {
    clipboard.accepts = false;
    const success = vi.spyOn(toast, 'success');
    const failure = vi.spyOn(toast, 'error');
    await copy();
    expect(success).not.toHaveBeenCalled();
    expect(failure).toHaveBeenCalledTimes(1);
    expect(failure.mock.calls[0]?.[0]).toBe('Could not copy to the clipboard');
  });
});
