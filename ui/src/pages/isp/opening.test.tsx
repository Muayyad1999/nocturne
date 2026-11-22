// @vitest-environment happy-dom
/**
 * When a dialog's form is prepared: as it opens, once. The case this exists
 * for is the second test — what the dialog was given changing under it while
 * it is open, which is what a save does to the invoice it was made from.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { useOnOpen } from './opening';

interface ProbeProps {
  open: boolean;
  /** Stands for the invoice or the customer a dialog is opened on. */
  given: { id: string; outstanding: number };
  prepared: string[];
}

function Probe({ open, given, prepared }: ProbeProps) {
  useOnOpen(open, () => prepared.push(`${given.id}:${given.outstanding}`));
  return null;
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

function show(props: ProbeProps): void {
  act(() => root?.render(<Probe {...props} />));
}

describe('preparing a dialog', () => {
  it('happens when it opens, with what it was given at that moment', () => {
    const prepared: string[] = [];
    show({ open: false, given: { id: 'i1', outstanding: 225 }, prepared });
    expect(prepared).toEqual([]);
    // What it is given may change while it is closed; nothing is prepared.
    show({ open: false, given: { id: 'i2', outstanding: 80 }, prepared });
    expect(prepared).toEqual([]);
    show({ open: true, given: { id: 'i2', outstanding: 80 }, prepared });
    expect(prepared).toEqual(['i2:80']);
  });

  it('does not happen again when what it was given changes while it is open', () => {
    const prepared: string[] = [];
    show({ open: true, given: { id: 'i1', outstanding: 225 }, prepared });
    expect(prepared).toEqual(['i1:225']);
    // The save went through and the invoice was read again: the same
    // invoice, a new object, a new figure. The form — and the answer the
    // save is about to give — must be left alone.
    show({ open: true, given: { id: 'i1', outstanding: 125 }, prepared });
    show({ open: true, given: { id: 'i1', outstanding: 125 }, prepared });
    expect(prepared).toEqual(['i1:225']);
  });

  it('happens again the next time it opens, with what it is given then', () => {
    const prepared: string[] = [];
    show({ open: true, given: { id: 'i1', outstanding: 225 }, prepared });
    show({ open: false, given: { id: 'i1', outstanding: 125 }, prepared });
    show({ open: true, given: { id: 'i1', outstanding: 125 }, prepared });
    expect(prepared).toEqual(['i1:225', 'i1:125']);
  });
});
