// @vitest-environment happy-dom
/**
 * How the content of an evidence item reaches the page.
 *
 * A stored drawing is not sanitised — it is the record of what the engine
 * returned — so the whole defence is in how it is shown: as an `<img>` on a
 * `data:` address and as nothing else. A DOM without a browser cannot show
 * that an image is inert (`tests/ops.spec.ts` does that in Chromium, with a
 * drawing that carries a script and an onload); what it can show is the
 * arrangement that makes it so, and that is what is held here — together
 * with a reading of the source files, so that the first `innerHTML` anybody
 * adds to them fails a test with its name on it.
 */

import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { EvidenceWithBody } from '@/api/types/ops';
import { TEXT_PAGE_LINES } from './evidenceReading';
import { EvidenceBodyView } from './EvidenceBody';

/** Synthetic and hostile: everything a drawing could carry that must never run. */
const HOSTILE_SVG = [
  '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200" onload="window.__pwned = 1">',
  '<script>window.__pwned = 2</script>',
  '<foreignObject width="400" height="200"><iframe xmlns="http://www.w3.org/1999/xhtml" src="javascript:alert(1)"></iframe></foreignObject>',
  '<image href="http://evil.example/beacon.png"/>',
  '<text>HOSTILE-DRAWING-MARKER</text>',
  '</svg>"><img src=x onerror="window.__pwned = 3">',
].join('');

function item(body: EvidenceWithBody['body'], over: Partial<EvidenceWithBody> = {}): EvidenceWithBody {
  return {
    id: 'a3yp63vh7kuxtrt5',
    kind: body.type === 'svg' ? 'topology' : 'terminal',
    title: 'the lab before the change',
    captured_at: 1795383621545,
    source: { from: body.type === 'svg' ? 'topology.export' : 'terminal.snapshot', lab: true },
    redacted: true,
    bytes: 381,
    sha256: 'b75623cd5cf23cbadc971ac71aa0bf3cfeb903f1495b158217af79fe1b09ad53',
    incident_ids: [],
    body,
    ...over,
  };
}

let root: Root | null = null;
let host: HTMLElement | null = null;

function render(record: EvidenceWithBody): HTMLElement {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => root?.render(<EvidenceBodyView item={record} />));
  return host;
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

describe('a stored drawing', () => {
  it('is one image on a data address, and nothing of it is markup', () => {
    const page = render(item({ type: 'svg', svg: HOSTILE_SVG }));

    const images = page.querySelectorAll('img');
    expect(images).toHaveLength(1);
    const src = images[0]?.getAttribute('src') ?? '';
    expect(src.startsWith('data:image/svg+xml;base64,')).toBe(true);
    // The address holds the drawing exactly, and only in its encoded form.
    expect(atob(src.slice(src.indexOf(',') + 1))).toBe(HOSTILE_SVG);
    expect(images[0]?.getAttribute('draggable')).toBe('false');

    // Nothing the drawing contains became an element or text of this page.
    expect(page.querySelectorAll('svg script, foreignObject, iframe, object, embed, image, script')).toHaveLength(0);
    expect(page.innerHTML).not.toContain('HOSTILE-DRAWING-MARKER');
    expect(page.innerHTML).not.toContain('__pwned');
    expect(page.innerHTML).not.toContain('evil.example');
    // The trailing `"><img …>` did not end the attribute and start an element.
    expect(page.querySelectorAll('[onerror], [onload]')).toHaveLength(0);
  });

  it('says so when what is stored is not a picture, instead of showing a broken one', () => {
    const page = render(item({ type: 'svg', svg: 'not a drawing at all' }));
    const image = page.querySelector('img');
    act(() => {
      image?.dispatchEvent(new Event('error'));
    });
    expect(page.querySelector('img')).toBeNull();
    expect(page.querySelector('[role="alert"]')?.textContent).toContain('cannot be shown as a picture');
    // It can still be taken out, exactly as stored.
    const download = [...page.querySelectorAll('button')].find((button) => button.textContent?.includes('Download SVG'));
    expect(download?.disabled).toBe(false);
  });
});

describe('stored text', () => {
  it('is text in a pre, with markup and control characters as characters', () => {
    const text = 'SW1#\u001b[31mreload\u001b[0m\n<script>window.__pwned = 4</script><b>bold</b>';
    const page = render(item({ type: 'text', text }, { lines: 2 }));

    const pre = page.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre?.textContent).toContain('SW1#\u241b[31mreload\u241b[0m');
    expect(pre?.textContent).toContain('<script>window.__pwned = 4</script><b>bold</b>');
    expect(pre?.textContent).not.toContain('\u001b');
    // It is text: none of it became an element.
    expect(page.querySelectorAll('script, b')).toHaveLength(0);
    // And it is not in a terminal emulator.
    expect(page.querySelector('.xterm')).toBeNull();
    expect(page.textContent).toContain('2 control characters');
  });

  it('says an empty capture is empty, which is what was read', () => {
    const page = render(item({ type: 'text', text: '' }, { lines: 0 }));
    expect(page.querySelector('pre')).toBeNull();
    expect(page.textContent).toContain('The item is empty');
    expect(page.textContent).toContain('not a failure to read');
  });
});

describe('text with more lines than a window can lay out at once', () => {
  // Pages of three lines here, so the test is about the paging and not about
  // how long 5,000 lines take to build: seven lines are two full pages and one.
  const SIZE = 3;
  const total = SIZE * 2 + 1;
  const long = Array.from({ length: total }, (_value, index) => `line ${index + 1}`).join('\n');
  const paged = (record: EvidenceWithBody): HTMLElement => {
    host = document.createElement('div');
    document.body.append(host);
    root = createRoot(host);
    act(() => root?.render(<EvidenceBodyView item={record} linesAtOnce={SIZE} />));
    return host;
  };
  const numbers = (page: HTMLElement) =>
    [...page.querySelectorAll('pre [aria-hidden="true"]')].map((number) => number.textContent);
  const shown = (page: HTMLElement) => page.querySelector('[data-testid="evidence-lines-shown"]')?.textContent;
  const press = (page: HTMLElement, name: string) => {
    const button = page.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`);
    act(() => button?.click());
  };

  it('is shown a page at a time, numbered as the whole is, and says which lines are on screen', () => {
    const page = paged(item({ type: 'text', text: long }, { lines: total }));

    // Not every line is in the document: that is what froze the window.
    expect(numbers(page)).toEqual(['1', '2', '3']);
    expect(shown(page)).toBe('Lines 1 to 3 of 7 are on screen. The download is all of them.');
    expect(page.querySelector<HTMLButtonElement>('button[aria-label="Earlier lines"]')?.disabled).toBe(true);

    press(page, 'Later lines');
    expect(shown(page)).toContain('Lines 4 to 6 of 7');
    // The second page is numbered where it stands in the item, and holds its lines only.
    expect(numbers(page)).toEqual(['4', '5', '6']);
    expect(page.querySelector('pre')?.textContent).toContain('line 4\n');
    expect(page.querySelector('pre')?.textContent).not.toContain('line 3\n');

    press(page, 'The last lines');
    expect(numbers(page)).toEqual(['7']);
    expect(page.querySelector('pre')?.textContent).toContain('line 7');
    expect(page.querySelector<HTMLButtonElement>('button[aria-label="Later lines"]')?.disabled).toBe(true);
    // The button that was pressed is disabled now; the focus is on the text, not nowhere.
    expect(document.activeElement).toBe(page.querySelector('pre'));

    press(page, 'The first lines');
    expect(numbers(page)).toEqual(['1', '2', '3']);
  });

  it('puts a page of five thousand lines on screen unless told otherwise', () => {
    expect(TEXT_PAGE_LINES).toBe(5_000);
    const page = render(item({ type: 'text', text: 'one\ntwo\nthree\nfour' }, { lines: 4 }));
    // Four lines fit a page: all of them are there and there is nothing to page through.
    expect(shown(page)).toBeUndefined();
    expect(page.querySelector('button[aria-label="Later lines"]')).toBeNull();
    expect(numbers(page)).toEqual(['1', '2', '3', '4']);
  });
});

describe('content of a kind this interface does not know', () => {
  it('is said, not shown as an empty box', () => {
    const page = render(item({ type: 'pcap', bytes: 'AAAA' } as unknown as EvidenceWithBody['body']));
    expect(page.textContent).toContain('cannot show');
    expect(page.textContent).toContain('pcap');
  });
});

describe('the files that show evidence', () => {
  // By path, not by URL: in this environment `URL` is the DOM's, which the
  // file system does not take.
  const here = dirname(fileURLToPath(import.meta.url));
  const read = (path: string) => readFileSync(resolve(here, path), 'utf8');
  // Every file that takes the content of an item, or the engine's drawing,
  // anywhere near the page.
  const FILES = [
    'EvidenceBody.tsx',
    'EvidenceItem.tsx',
    'Evidence.tsx',
    'AttachEvidence.tsx',
    'Timeline.tsx',
    'evidenceReading.ts',
    '../../lib/capture.ts',
    '../../app/KeepAsEvidence/KeepAsEvidence.tsx',
    '../topology/ExportDialog.tsx',
  ];
  /** Code only: a comment may say what must never be done. */
  const code = (source: string) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

  it.each(FILES)('%s never makes markup, a frame, a window or terminal input of anything', (path) => {
    const source = code(read(path));
    for (const forbidden of [
      'dangerouslySetInnerHTML',
      'innerHTML',
      'outerHTML',
      'insertAdjacentHTML',
      'document.write',
      'DOMParser',
      'createContextualFragment',
      '<iframe',
      '<object',
      '<embed',
      'srcDoc',
      'window.open',
      // Nor the terminal emulator, which would obey what a body contains.
      'xterm',
      'pages/terminal',
    ]) {
      expect(source.includes(forbidden), `${path} contains ${forbidden}`).toBe(false);
    }
  });

  it('an SVG is only ever given the image type in one function', () => {
    const typed = FILES.filter((path) => code(read(path)).includes('image/svg+xml'));
    expect(typed).toEqual(['../../lib/capture.ts']);
    const capture = code(read('../../lib/capture.ts'));
    // And there it is a data address, never a blob of this origin.
    expect(capture.match(/image\/svg\+xml/g)).toHaveLength(1);
    expect(capture).toContain('`data:image/svg+xml;base64,${');
    expect(capture).not.toContain('createObjectURL');
  });
});
