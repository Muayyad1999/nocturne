// @vitest-environment happy-dom
/**
 * The Clips panel: a view of what opted-in copy controls remembered, with
 * only the actions the screen showing it can really perform.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useToasts } from '@/design/Toast';
import { clips, copyText, type Clip } from '@/lib/clipboard';
import { clipPreview, ClipsPanel, type ClipsPanelProps } from './ClipsPanel';

let root: Root | null = null;
let host: HTMLElement | null = null;
let written: string[] = [];

function show(props: ClipsPanelProps = {}): void {
  act(() => root?.render(<ClipsPanel {...props} />));
}

function labelled(name: RegExp): HTMLButtonElement | undefined {
  return [...document.querySelectorAll('button')].find((button) => name.test(button.getAttribute('aria-label') ?? ''));
}

function named(text: string): HTMLButtonElement | undefined {
  return [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === text);
}

async function press(button: HTMLButtonElement | undefined): Promise<void> {
  if (!button) throw new Error('that button is not on screen');
  await act(async () => {
    button.click();
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  written = [];
  clips.clear();
  useToasts.setState({ toasts: [], history: [], unread: 0 });
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: (text: string) => {
        written.push(text);
        return Promise.resolve();
      },
    },
  });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  clips.clear();
});

describe('the panel', () => {
  it('says there is nothing, and where clips come from, when there are none', () => {
    show();
    expect(host?.textContent).toContain('Nothing has been copied through a button that keeps a clip');
    expect(named('Clear')?.disabled).toBe(true);
  });

  it('shows each clip as one line of text with what it is and where it came from', async () => {
    await copyText('SW1> show clock\n*03:05:11.000 UTC Mon Nov 23 2026\n', { clip: { label: 'Selection from SW1', source: 'Terminal' } });
    show();

    expect(host?.textContent).toContain('Selection from SW1');
    expect(host?.textContent).toContain('Terminal');
    expect(host?.querySelector('code')?.textContent).toBe('SW1> show clock');
    expect(host?.textContent).toContain('2 lines');
  });

  it('renders a clip that looks like markup as the text it is', async () => {
    await copyText('<img src=x onerror=alert(1)>', { clip: { label: 'Interface description', source: 'Terminal' } });
    show();
    expect(host?.querySelector('img')).toBeNull();
    expect(host?.querySelector('code')?.textContent).toBe('<img src=x onerror=alert(1)>');
  });

  it('follows the store: a clip copied while it is open appears, and Clear empties it', async () => {
    show();
    await act(async () => {
      await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });
    });
    expect(host?.textContent).toContain('10.0.0.1');
    expect(host?.textContent).toContain('1 clip');

    await press(named('Clear'));
    expect(clips.list()).toEqual([]);
    expect(host?.textContent).not.toContain('10.0.0.1');
  });

  it('copies a clip again and forgets one on request', async () => {
    await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });
    await copyText('10.0.0.2', { clip: { label: 'Gateway', source: 'IPAM' } });
    show();
    written = [];

    await press(labelled(/^Copy “Address” again/));
    expect(written).toEqual(['10.0.0.1']);
    // Copied again, so it is the newest — and still one clip.
    expect(clips.list().map((clip) => clip.text)).toEqual(['10.0.0.1', '10.0.0.2']);

    await press(labelled(/^Forget “Gateway”/));
    expect(clips.list().map((clip) => clip.text)).toEqual(['10.0.0.1']);
  });
});

describe('what can be done with a clip', () => {
  const seed = () => copyText('show interfaces status', { clip: { label: 'Selection from SW1', source: 'Terminal' } });

  it('draws no button for an action the screen did not give', async () => {
    await seed();
    show();
    expect(labelled(/^Copy “Selection from SW1” again/)).toBeDefined();
    expect(labelled(/^Type “/)).toBeUndefined();
    expect(labelled(/^Keep “/)).toBeUndefined();
    expect(labelled(/^Save “/)).toBeUndefined();
  });

  it('hands the clip to the screen for typing, keeping and saving, and does none of them itself', async () => {
    await seed();
    const onType = vi.fn<(clip: Clip) => void>();
    const onKeep = vi.fn<(clip: Clip) => void>();
    const onSnippet = vi.fn<(clip: Clip) => void>();
    show({ onType, typeInto: 'SW1', onKeep, onSnippet });

    await press(labelled(/^Type “Selection from SW1” into SW1/));
    await press(labelled(/^Keep “Selection from SW1” as evidence/));
    await press(labelled(/^Save “Selection from SW1” as a snippet/));

    for (const handler of [onType, onKeep, onSnippet]) {
      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler.mock.calls[0]?.[0]).toMatchObject({ text: 'show interfaces status', label: 'Selection from SW1' });
    }
  });

  it('cannot type a clip when no session is in front', async () => {
    await seed();
    const onType = vi.fn();
    show({ onType, typeInto: null });
    const type = labelled(/^Type “Selection from SW1” into a session/);
    expect(type?.disabled).toBe(true);
    await press(type);
    expect(onType).not.toHaveBeenCalled();
  });
});

describe('clipPreview', () => {
  it('is the first line and the number of lines', () => {
    expect(clipPreview('a\r\nb\n')).toEqual({ first: 'a', lines: 2 });
    expect(clipPreview('one')).toEqual({ first: 'one', lines: 1 });
  });
});
