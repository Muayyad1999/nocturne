// @vitest-environment happy-dom
/**
 * What is remembered of a copy, and — the half that matters — what is not.
 *
 * The clips are opt-in per call site. These tests hold the two ends of
 * that: a copy that did not ask is kept nowhere, whatever it was; and the
 * toolbox, where the decoded secret, the generated passwords and the private
 * key are copied, has no call site that asks. The second is checked twice:
 * by pressing the toolbox's real copy buttons, and by reading its source.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CodeBlock } from '@/design/CodeBlock';
import { CopyButton } from '@/pages/tools/Sections';
import { CLIP_LIMIT, CLIP_MAX_BYTES, clips, copyText, fitsAsClip } from './clipboard';

/** What reached the system clipboard, in order. */
let written: string[] = [];
let refuse = false;
let reads = 0;

beforeEach(() => {
  written = [];
  refuse = false;
  reads = 0;
  clips.clear();
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: (text: string) => {
        if (refuse) return Promise.reject(new DOMException('not allowed', 'NotAllowedError'));
        written.push(text);
        return Promise.resolve();
      },
      readText: () => {
        reads += 1;
        return Promise.resolve('whatever a password manager put there');
      },
    },
  });
});

afterEach(() => {
  clips.clear();
  window.localStorage.clear();
  window.sessionStorage.clear();
});

describe('a copy that did not ask to be remembered', () => {
  it('reaches the clipboard and is kept nowhere', async () => {
    expect(await copyText('Zq7-hunter2-Xk')).toBe(true);
    expect(written).toEqual(['Zq7-hunter2-Xk']);
    expect(clips.list()).toEqual([]);
  });

  it('is not remembered because an earlier copy asked', async () => {
    await copyText('GigabitEthernet0/1', { clip: { label: 'Interface', source: 'Terminal' } });
    await copyText('enable secret Sup3rS3cret');
    expect(clips.list().map((clip) => clip.text)).toEqual(['GigabitEthernet0/1']);
  });
});

describe('a copy that asked', () => {
  it('is remembered with what it is and where it came from, newest first', async () => {
    await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });
    await copyText('GigabitEthernet0/1', { clip: { label: 'Selection from SW1', source: 'Terminal' } });

    expect(clips.list().map(({ text, label, source }) => ({ text, label, source }))).toEqual([
      { text: 'GigabitEthernet0/1', label: 'Selection from SW1', source: 'Terminal' },
      { text: '10.0.0.1', label: 'Address', source: 'IPAM' },
    ]);
    expect(written).toEqual(['10.0.0.1', 'GigabitEthernet0/1']);
  });

  it('is one clip however often the same text is copied, moved to the front', async () => {
    const origin = { label: 'Address', source: 'IPAM' };
    await copyText('10.0.0.1', { clip: origin });
    await copyText('10.0.0.2', { clip: origin });
    await copyText('10.0.0.1', { clip: { label: 'Selection from SW1', source: 'Terminal' } });

    expect(clips.list().map((clip) => clip.text)).toEqual(['10.0.0.1', '10.0.0.2']);
    // Under the name it was copied by last.
    expect(clips.list()[0]?.label).toBe('Selection from SW1');
  });

  it('is remembered even when the clipboard refused, so it can still be typed or kept', async () => {
    refuse = true;
    expect(await copyText('show interfaces status', { clip: { label: 'Selection', source: 'Terminal' } })).toBe(false);
    expect(clips.list().map((clip) => clip.text)).toEqual(['show interfaces status']);
  });

  it('pushes the oldest out when there are more than the panel holds', async () => {
    for (let index = 0; index < CLIP_LIMIT + 3; index += 1) {
      await copyText(`line ${index}`, { clip: { label: 'Line', source: 'Terminal' } });
    }
    const texts = clips.list().map((clip) => clip.text);
    expect(texts).toHaveLength(CLIP_LIMIT);
    expect(texts[0]).toBe(`line ${CLIP_LIMIT + 2}`);
    expect(texts).not.toContain('line 0');
    expect(texts).not.toContain('line 2');
  });

  it('does not remember a text too long to keep, and never a piece of it', async () => {
    const long = 'x'.repeat(CLIP_MAX_BYTES + 1);
    expect(fitsAsClip(long)).toBe(false);
    expect(await copyText(long, { clip: { label: 'Selection', source: 'Terminal' } })).toBe(true);
    // The clipboard got all of it; the panel got nothing rather than a cut.
    expect(written[0]).toHaveLength(CLIP_MAX_BYTES + 1);
    expect(clips.list()).toEqual([]);
    expect(await copyText('', { clip: { label: 'Selection', source: 'Terminal' } })).toBe(true);
    expect(clips.list()).toEqual([]);
  });

  it('measures a clip as an evidence item is measured: in bytes, not characters', async () => {
    // The engine refused exactly this as evidence: 140,000 characters, 280,000 bytes.
    const accented = 'é'.repeat(140_000);
    expect(accented.length).toBeLessThan(CLIP_MAX_BYTES);
    expect(fitsAsClip(accented)).toBe(false);
    await copyText(accented, { clip: { label: 'Selection', source: 'Terminal' } });
    expect(clips.list()).toEqual([]);

    // To the byte: the longest that fits, and one more.
    expect(fitsAsClip('é'.repeat(CLIP_MAX_BYTES / 2))).toBe(true);
    expect(fitsAsClip(`${'é'.repeat(CLIP_MAX_BYTES / 2)}x`)).toBe(false);
    expect(fitsAsClip('x'.repeat(CLIP_MAX_BYTES))).toBe(true);
  });
});

describe('where clips live', () => {
  it('is this window’s memory: nothing is written to any storage', async () => {
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    await copyText('GigabitEthernet0/1', { clip: { label: 'Interface', source: 'Terminal' } });
    clips.remove(clips.list()[0]?.id ?? -1);
    await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });

    expect(setItem).not.toHaveBeenCalled();
    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
    setItem.mockRestore();
  });

  it('never reads the system clipboard to build the list', async () => {
    await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });
    clips.list();
    clips.clear();
    expect(reads).toBe(0);
  });

  it('tells whoever is listening when a clip arrives, goes or all are cleared', async () => {
    const heard = vi.fn();
    const stop = clips.subscribe(heard);
    await copyText('10.0.0.1', { clip: { label: 'Address', source: 'IPAM' } });
    expect(heard).toHaveBeenCalledTimes(1);
    const before = clips.list();
    // The same array until something changes: what a store hook needs.
    expect(clips.list()).toBe(before);

    clips.remove(before[0]?.id ?? -1);
    expect(heard).toHaveBeenCalledTimes(2);
    clips.clear();
    // Already empty: nothing to say.
    expect(heard).toHaveBeenCalledTimes(2);

    await copyText('10.0.0.2', { clip: { label: 'Address', source: 'IPAM' } });
    clips.clear();
    expect(heard).toHaveBeenCalledTimes(4);
    expect(clips.list()).toEqual([]);

    stop();
    await copyText('10.0.0.3', { clip: { label: 'Address', source: 'IPAM' } });
    expect(heard).toHaveBeenCalledTimes(4);
  });
});

// ----------------------------------------------------------- the toolbox

describe('the toolbox, where secrets are copied', () => {
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
  });

  async function press(name: RegExp): Promise<void> {
    const button = [...document.querySelectorAll('button')].find((candidate) =>
      name.test(candidate.getAttribute('aria-label') ?? ''),
    );
    if (!button) throw new Error(`no button called ${name}`);
    await act(async () => {
      button.click();
      await Promise.resolve();
    });
  }

  it('copies a decoded secret and a generated password without leaving a clip', async () => {
    // The toolbox's own button, exactly as Secrets.tsx uses it.
    act(() =>
      root?.render(
        <>
          <CopyButton value="Sup3rS3cretEnable" label="the decoded secret" />
          <CopyButton value="kV9!tq2#Lm8@" label="password 1" />
        </>,
      ),
    );
    await press(/the decoded secret/);
    await press(/password 1/);

    expect(written).toEqual(['Sup3rS3cretEnable', 'kV9!tq2#Lm8@']);
    expect(clips.list()).toEqual([]);
  });

  it('copies a private key from its code block without leaving a clip', async () => {
    // Synthetic: the markers of a key around nothing.
    const key = '-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n';
    act(() => root?.render(<CodeBlock code={key} title="Private key (OpenSSH, unencrypted)" />));
    await press(/copy/i);

    expect(written).toEqual([key]);
    expect(clips.list()).toEqual([]);
  });

  it('has no call site that asks for a clip, in its source', () => {
    // Every file of the folders where a secret can be on screen, as text.
    const sources = import.meta.glob<string>(
      ['/src/pages/tools/**/*.{ts,tsx}', '/src/pages/config/**/*.{ts,tsx}', '/src/design/CodeBlock/*.{ts,tsx}'],
      { query: '?raw', import: 'default', eager: true },
    );
    const files = Object.keys(sources).filter((path) => !/\.test\.tsx?$/.test(path));
    // The folders are really there: a glob that matched nothing proves nothing.
    expect(files.some((path) => path.endsWith('/pages/tools/Secrets.tsx'))).toBe(true);
    expect(files.some((path) => path.endsWith('/pages/tools/Sections.tsx'))).toBe(true);
    expect(files.some((path) => path.endsWith('/design/CodeBlock/CodeBlock.tsx'))).toBe(true);

    const calls: { file: string; call: string }[] = [];
    for (const file of files) {
      const source = sources[file] ?? '';
      for (const found of source.matchAll(/copyText\(([^;]*)/g)) calls.push({ file, call: found[1] ?? '' });
      // Neither by its option nor by reaching for the store itself.
      expect(source, file).not.toMatch(/\bclip\s*:/);
      expect(source, file).not.toMatch(/\bclips\b\s*[.,}]/);
    }
    // The buttons pressed above are the ones the source has.
    expect(calls.map((call) => call.file.replace(/^.*\/src\//, '')).sort()).toEqual([
      'design/CodeBlock/CodeBlock.tsx',
      'pages/tools/Sections.tsx',
    ]);
    // One argument each: the text, and no options.
    for (const { file, call } of calls) expect(call, file).toMatch(/^\w+\)\)/);
  });
});
