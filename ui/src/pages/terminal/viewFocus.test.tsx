// @vitest-environment happy-dom
/**
 * Where the focus goes when a drawer shows another view: the control that was
 * pressed to get there is gone, and the focus with it. `tests/terminal.spec.ts`
 * walks the real drawers from the keyboard.
 */

import { act, useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { useFocusWhenShown } from './viewFocus';

function List({ onOpen, back }: { onOpen: () => void; back: string | null }) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusWhenShown(ref);
  return (
    <div ref={ref}>
      <input aria-label="Search" />
      <button type="button" data-autofocus={back === 'b' || undefined} onClick={onOpen}>
        b
      </button>
      <button type="button" disabled data-autofocus={back === 'c' || undefined}>
        c
      </button>
    </div>
  );
}

function Item({ onBack }: { onBack: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusWhenShown(ref);
  return (
    <div ref={ref}>
      <button type="button" onClick={onBack}>
        All
      </button>
      <textarea aria-label="Value" data-autofocus />
    </div>
  );
}

function Drawer({ back }: { back: string | null }) {
  const [open, setOpen] = useState(false);
  return open ? <Item onBack={() => setOpen(false)} /> : <List onOpen={() => setOpen(true)} back={back} />;
}

let root: Root | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

const named = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('button, input, textarea')].find(
    (element) => element.textContent === name || element.getAttribute('aria-label') === name,
  );

it('gives the focus to the view that replaced the control that had it', () => {
  act(() => root?.render(<Drawer back="b" />));
  named('b')?.focus();
  act(() => named('b')?.click());
  // The button that was pressed is gone; the new view's marked field has the focus.
  expect(document.activeElement).toBe(named('Value'));

  act(() => named('All')?.click());
  expect(document.activeElement).toBe(named('b'));
});

it('passes over a marked control that cannot take the focus, to the first that can', () => {
  act(() => root?.render(<Drawer back="c" />));
  named('b')?.focus();
  act(() => named('b')?.click());
  act(() => named('All')?.click());
  expect(document.activeElement).toBe(named('Search'));
});

it('leaves the focus where it is when something still has it', () => {
  const outside = document.createElement('button');
  outside.textContent = 'toolbar';
  document.body.append(outside);
  outside.focus();
  // A drawer opening: the button that opened it still has the focus, and the
  // drawer's own handling moves it in and remembers where to give it back.
  act(() => root?.render(<Drawer back="b" />));
  expect(document.activeElement).toBe(outside);
  outside.remove();
});
