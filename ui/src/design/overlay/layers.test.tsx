// @vitest-environment happy-dom
/**
 * A floating layer is usable from the moment it is on screen. React runs
 * passive effects after the browser has painted, so a layer that started
 * listening there was visible and deaf for an instant: an Escape pressed in
 * that instant was lost, and the dialog stayed open.
 */

import { act, useLayoutEffect } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Dialog } from '../Dialog';

let root: Root | null = null;
let host: HTMLElement | null = null;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  host = document.createElement('div');
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

/**
 * Presses Escape from a layout effect: after the layers rendered before it
 * are in the page, and before the browser has painted anything.
 */
function EscapeBeforePaint() {
  useLayoutEffect(() => {
    (document.activeElement ?? document).dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
    );
  }, []);
  return null;
}

it('closes a dialog on an Escape pressed before the page has been painted', () => {
  const onClose = vi.fn();
  act(() =>
    root?.render(
      <>
        <Dialog open onClose={onClose} title="Paste into SW1-LAB" />
        <EscapeBeforePaint />
      </>,
    ),
  );
  expect(onClose).toHaveBeenCalledTimes(1);
});
