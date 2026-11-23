// @vitest-environment happy-dom
/**
 * Where the focus goes when a layer closes: back to the control that opened
 * it, so the next Tab carries on from there instead of from the top of the
 * page. Two things used to defeat that.
 *
 * - A field that takes the focus with React's `autoFocus`. React focuses it
 *   while it commits the dialog, before the dialog's own effect runs, so the
 *   dialog noted its own field as where the focus came from, and gave it
 *   back to a field that no longer existed.
 * - An action that removes the control it was started from: a draft that has
 *   been issued has no "Issue…" button, a customer marked suspended has no
 *   "Mark suspended". The focus was given back, and then dropped on the page
 *   when the screen redrew.
 *
 * `tests/isp.spec.ts` and `tests/ops.spec.ts` drive the real screens.
 */

import { act, StrictMode, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Dialog } from '../Dialog';
import { Drawer } from '../Drawer';
import { DropdownMenu } from '../DropdownMenu';

let root: Root | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  host = document.createElement('div');
  // The layer stack makes `#root` inert under a modal, as in the app.
  host.id = 'root';
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

function render(element: ReactNode): void {
  act(() => root?.render(element));
}

function named(name: string): HTMLElement | undefined {
  return [...document.querySelectorAll<HTMLElement>('button, input, [role="menuitem"]')].find(
    (element) => element.textContent?.trim() === name || element.getAttribute('aria-label') === name,
  );
}

function press(name: string): void {
  const element = named(name);
  if (!element) throw new Error(`no control named ${name}`);
  act(() => element.click());
}

function pressEscape(): void {
  act(() => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
    );
  });
}

/** Lets the MutationObserver callbacks of the last commit run. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

function Rename({ layer }: { layer: 'dialog' | 'drawer' }) {
  const [open, setOpen] = useState(false);
  const Layer = layer === 'dialog' ? Dialog : Drawer;
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Rename…
      </button>
      <Layer open={open} onClose={() => setOpen(false)} title="Rename">
        <input aria-label="New name" autoFocus />
      </Layer>
    </>
  );
}

describe('a layer whose field takes the focus with autoFocus', () => {
  for (const layer of ['dialog', 'drawer'] as const) {
    it(`gives the focus back to the control that opened the ${layer}`, () => {
      render(<Rename layer={layer} />);
      named('Rename…')?.focus();
      press('Rename…');
      expect(document.activeElement).toBe(named('New name'));

      pressEscape();
      expect(document.querySelector('[role="dialog"]')).toBeNull();
      expect(document.activeElement).toBe(named('Rename…'));
    });
  }

  it('keeps the field focused when StrictMode mounts the dialog open, and still gives the focus back', () => {
    function Mounted() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Rename…
          </button>
          {open && (
            <Dialog open onClose={() => setOpen(false)} title="Rename">
              <input aria-label="New name" autoFocus />
            </Dialog>
          )}
        </>
      );
    }
    render(
      <StrictMode>
        <Mounted />
      </StrictMode>,
    );
    named('Rename…')?.focus();
    press('Rename…');
    // StrictMode unmounts and mounts the dialog again at once; the field
    // React focused is what keeps the focus, not the first control.
    expect(document.activeElement).toBe(named('New name'));

    pressEscape();
    expect(document.activeElement).toBe(named('Rename…'));
  });

  it('gives it back to the menu button when the dialog was opened from a menu that closed with it', () => {
    // The dialog comes before the menu, so React commits it first: the menu
    // gives the focus back to its button only after the dialog has looked.
    function FromMenu() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <Dialog open={open} onClose={() => setOpen(false)} title="Rename">
            <input aria-label="New name" autoFocus />
          </Dialog>
          <DropdownMenu
            aria-label="Row actions"
            trigger={<button type="button">Actions</button>}
            items={[{ label: 'Rename', onSelect: () => setOpen(true) }]}
          />
        </>
      );
    }
    render(<FromMenu />);
    named('Actions')?.focus();
    press('Actions');
    press('Rename');
    expect(document.activeElement).toBe(named('New name'));

    pressEscape();
    expect(document.activeElement).toBe(named('Actions'));
  });
});

it('still gives the focus back from a dialog that marks its field with data-autofocus', () => {
  function Marked() {
    const [open, setOpen] = useState(false);
    return (
      <>
        <button type="button" onClick={() => setOpen(true)}>
          Attach…
        </button>
        <Dialog open={open} onClose={() => setOpen(false)} title="Attach">
          <input aria-label="Incident" data-autofocus />
        </Dialog>
      </>
    );
  }
  render(<Marked />);
  named('Attach…')?.focus();
  press('Attach…');
  expect(document.activeElement).toBe(named('Incident'));
  pressEscape();
  expect(document.activeElement).toBe(named('Attach…'));
});

/** A customer's state buttons: the one for the state it is in is never shown. */
function States({ removeWithTheDialog = false }: { removeWithTheDialog?: boolean }) {
  const [state, setState] = useState('active');
  const [asking, setAsking] = useState<string | null>(null);
  const confirm = () => {
    const next = asking;
    setAsking(null);
    // The screen redraws when the engine has answered, after the dialog has gone…
    if (next && !removeWithTheDialog) setTimeout(() => setState(next), 0);
    // …or, where the answer is assumed, in the same commit as the dialog closes.
    if (next && removeWithTheDialog) setState(next);
  };
  return (
    <>
      <div role="group" aria-label="State">
        {['active', 'suspended', 'cancelled']
          .filter((next) => next !== state)
          .map((next) => (
            <button key={next} type="button" onClick={() => setAsking(next)}>
              Mark {next}
            </button>
          ))}
      </div>
      <Dialog
        open={asking !== null}
        onClose={() => setAsking(null)}
        title={`Mark ${asking}?`}
        footer={
          <button type="button" onClick={confirm}>
            Confirm
          </button>
        }
      />
    </>
  );
}

describe('an action that removes the control it was started from', () => {
  it('moves the focus to the control next to it, not to the page', async () => {
    render(<States />);
    named('Mark suspended')?.focus();
    press('Mark suspended');
    press('Confirm');
    // Given back first, to the button that is still there…
    expect(document.activeElement).toBe(named('Mark suspended'));

    // …which the screen then removes.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    await settle();
    expect(named('Mark suspended')).toBeUndefined();
    expect(document.activeElement).toBe(named('Mark active'));
  });

  it('does the same when the control goes in the commit that closes the dialog', async () => {
    render(<States removeWithTheDialog />);
    named('Mark suspended')?.focus();
    press('Mark suspended');
    press('Confirm');
    await settle();
    expect(named('Mark suspended')).toBeUndefined();
    expect(document.activeElement).toBe(named('Mark active'));
  });

  it('leaves the focus alone once the user has moved on', async () => {
    render(<States />);
    named('Mark suspended')?.focus();
    press('Mark suspended');
    press('Confirm');
    // A key pressed before the screen redraws is the user's next move.
    act(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Shift', bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    await settle();
    expect(document.activeElement).toBe(document.body);
  });
});

describe('a drawer whose focused control is removed', () => {
  function Invoice() {
    const [draft, setDraft] = useState(true);
    const [asking, setAsking] = useState(false);
    return (
      <Drawer open onClose={() => undefined} title="Invoice 2026-0042">
        <div role="group" aria-label="Invoice actions">
          {draft && (
            <button type="button" onClick={() => setAsking(true)}>
              Issue…
            </button>
          )}
          <button type="button">Download</button>
        </div>
        <input aria-label="Note" />
        <Dialog
          open={asking}
          onClose={() => setAsking(false)}
          title="Issue this invoice?"
          footer={
            <button
              type="button"
              onClick={() => {
                setAsking(false);
                setTimeout(() => setDraft(false), 0);
              }}
            >
              Issue it
            </button>
          }
        />
      </Drawer>
    );
  }

  it('keeps the focus in the drawer, on the control next to it', async () => {
    render(<Invoice />);
    named('Issue…')?.focus();
    press('Issue…');
    press('Issue it');
    expect(document.activeElement).toBe(named('Issue…'));

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    await settle();
    expect(named('Issue…')).toBeUndefined();
    expect(document.activeElement).toBe(named('Download'));
  });

  it('keeps it in the drawer when nothing but a text field is left near it', async () => {
    function Steps() {
      const [step, setStep] = useState(1);
      return (
        <Drawer open onClose={() => undefined} title="Import">
          <div>
            {step === 1 ? (
              <button type="button" onClick={() => setTimeout(() => setStep(2), 0)}>
                Next
              </button>
            ) : (
              <input aria-label="Name" />
            )}
          </div>
        </Drawer>
      );
    }
    render(<Steps />);
    named('Next')?.focus();
    press('Next');
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    await settle();
    // Not into the text field, where the next key would be typed: the drawer
    // itself, from which Tab reaches its first control.
    const drawer = document.querySelector('[role="dialog"]');
    expect(document.activeElement).toBe(drawer);
  });
});
