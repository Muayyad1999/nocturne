/**
 * Printing a document the engine rendered, in a real browser against a real
 * engine.
 *
 * Two things are proved here, and each needs a browser to prove:
 *
 * 1. **Print prints.** Both Print buttons used to reach into the preview
 *    frame for its `print()`. The preview is sandboxed without
 *    `allow-same-origin`, so the browser answered with a SecurityError and
 *    nothing was printed. Chromium and WebKit agree on that; only a real
 *    engine shows it.
 * 2. **What came from a device stays inert**, on screen and on the way to the
 *    printer. The engine escapes every device string, so no real document can
 *    carry script. The second test therefore swaps one answer of
 *    `incidents.export` for a document that tries every way of running some —
 *    the one deliberate fault injection in this file, marked where it happens
 *    — and a control frame shows that the same document *does* run where
 *    nothing stops it, so the silence of the other two frames means something.
 *
 * `window.print` is replaced by a line in the console in every frame: a
 * headless browser has no print dialog to wait for, and what matters is which
 * frame asked.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import type { Incident, Rendered } from '../src/api/types/ops';
import { openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 120_000 });

let backend: Backend;
let uiDir: string | undefined;
let incidentId = '';

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  let dist = process.env.NOCTURNE_UI_DIST;
  if (!dist) {
    // Built outside `ui/dist`: the server crate embeds that directory, so
    // writing there would make cargo rebuild the binary first.
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
    dist = uiDir;
  }
  backend = await startBackend({ uiDir: dist });
  const rpc = rpcFor(request, backend);
  const incident = await rpc<Incident>('incidents.create', {
    title: 'Uplink flapping on the core',
    summary: 'Port-channel 1 lost a member twice in ten minutes.',
  });
  incidentId = incident.id;
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

const PRINTED = 'nocturne-test: print() asked for by';
const RAN = 'nocturne-test: script ran:';

interface Watch {
  /** Which frames called `print()`: `the page` or `a frame`, in order. */
  readonly printed: string[];
  /** What a script that should not have run managed to say. */
  readonly ran: string[];
  /** Errors, warnings and uncaught exceptions of the page. */
  readonly complaints: string[];
}

/** Records every `print()` and every message a hostile script gets out. */
async function watch(page: Page): Promise<Watch> {
  const printed: string[] = [];
  const ran: string[] = [];
  page.on('console', (message) => {
    const text = message.text();
    if (text.startsWith(PRINTED)) printed.push(text.slice(PRINTED.length + 1));
    if (text.startsWith(RAN)) ran.push(text.slice(RAN.length + 1));
  });
  await page.addInitScript(
    ([printedLine, ranLine]) => {
      window.print = () => console.info(`${printedLine} ${window === window.top ? 'the page' : 'a frame'}`);
      if (window !== window.top) return;
      window.addEventListener('message', (event) => {
        if (typeof event.data === 'string' && event.data.startsWith('RAN:')) {
          console.info(`${ranLine} ${event.data.slice(4)}`);
        }
      });
    },
    [PRINTED, RAN] as const,
  );
  return { printed, ran, complaints: watchConsole(page).messages };
}

/** One value to wait on, so a failure shows what was thrown beside what was not printed. */
function outcome(seen: Watch): Pick<Watch, 'printed' | 'complaints'> {
  return { printed: [...seen.printed], complaints: [...seen.complaints] };
}

async function openIncidentExport(page: Page): Promise<void> {
  await openApp(page, `/ops/incidents?incident=${incidentId}`, { backend });
  await page.getByRole('button', { name: 'Export review' }).click();
  await page.getByRole('dialog').getByLabel('Format').selectOption('html');
}

test.describe('printing an HTML document', () => {
  test('from the export dialog asks a frame to print, and nothing is thrown', async ({ page }) => {
    const seen = await watch(page);
    await openIncidentExport(page);
    const dialog = page.getByRole('dialog');
    await expect(
      dialog.frameLocator('iframe[title$="(preview)"]').getByRole('heading', { level: 1 }),
    ).toContainText('Uplink flapping on the core');

    await dialog.getByRole('button', { name: 'Print' }).click();

    await expect.poll(() => outcome(seen)).toEqual({ printed: ['a frame'], complaints: [] });
    // And the frame that printed is the one holding the engine's document.
    await expect(
      dialog.locator('iframe[title="Document being printed"]').contentFrame().locator('h1'),
    ).toContainText('Uplink flapping on the core');
  });

  test('from the export dialog prints once: opening the dialog again asks for nothing', async ({ page }) => {
    const seen = await watch(page);
    await openIncidentExport(page);
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Print' }).click();
    await expect.poll(() => outcome(seen)).toEqual({ printed: ['a frame'], complaints: [] });

    // The screen keeps the dialog and what it remembers; only what it shows goes.
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await page.getByRole('button', { name: 'Export review' }).click();
    await expect(
      dialog.frameLocator('iframe[title$="(preview)"]').getByRole('heading', { level: 1 }),
    ).toContainText('Uplink flapping on the core');

    // Time for a frame to load its script and print, had one been made.
    await page.waitForTimeout(1_500);
    expect(outcome(seen), 'nobody pressed Print the second time').toEqual({ printed: ['a frame'], complaints: [] });
    await expect(dialog.locator('iframe[title="Document being printed"]')).toHaveCount(0);
  });

  test('from the Reports tab asks a frame to print, and nothing is thrown', async ({ page }) => {
    const seen = await watch(page);
    await openApp(page, '/ops/reports', { backend });
    await page.getByLabel('Format').selectOption('html');
    const preview = page.locator('iframe[title$="(preview)"]');
    await expect(preview.contentFrame().getByRole('heading', { level: 1 })).toBeVisible();
    const title = await preview.contentFrame().getByRole('heading', { level: 1 }).innerText();
    expect(title.trim()).not.toBe('');

    await page.getByRole('button', { name: 'Print' }).click();

    await expect.poll(() => outcome(seen)).toEqual({ printed: ['a frame'], complaints: [] });
    const printing = page.locator('iframe[title="Document being printed"]');
    await expect(printing).toHaveAttribute('sandbox', 'allow-scripts allow-modals');
    // What printed is the report that was on screen.
    await expect(printing.contentFrame().locator('h1')).toHaveText(title);
    // The preview needs no permission for any of it, so it has none.
    await expect(preview).toHaveAttribute('sandbox', '');
  });

  test('a kept report prints as what it was kept as, whatever the format selector says', async ({ page }) => {
    const seen = await watch(page);
    await openApp(page, '/ops/reports', { backend });
    const format = page.getByLabel('Format');
    const print = page.getByRole('button', { name: 'Print' });
    const keep = page.getByRole('button', { name: 'Keep this report' });
    const back = page.getByRole('button', { name: 'Generate a new one instead' });

    // Kept as Markdown: there is no page to print, and choosing HTML for the
    // next report does not turn the kept one into one.
    await expect(page.getByText('# Inventory')).toBeVisible();
    await keep.click();
    await expect(back).toBeVisible();
    await format.selectOption('html');
    await expect(page.getByText('# Inventory')).toBeVisible();
    await expect(print).toHaveCount(0);

    // Kept as HTML: it still prints after the selector has moved on.
    await back.click();
    const preview = page.locator('iframe[title$="(preview)"]');
    await expect(preview.contentFrame().getByRole('heading', { level: 1 })).toHaveText('Inventory');
    await keep.click();
    await expect(back).toBeVisible();
    await format.selectOption('markdown');
    await expect(preview.contentFrame().getByRole('heading', { level: 1 })).toHaveText('Inventory');
    await print.click();

    await expect.poll(() => outcome(seen)).toEqual({ printed: ['a frame'], complaints: [] });
    await expect(page.locator('iframe[title="Document being printed"]').contentFrame().locator('h1')).toHaveText(
      'Inventory',
    );
  });
});

/**
 * A document that tries to get a script run every way markup offers. Each
 * attempt that succeeds says which one it was, in a message to the page.
 */
function hostileDocument(origin: string): string {
  const say = (how: string) => `top.postMessage('RAN:${how}','*')`;
  return [
    '<!doctype html>',
    '<html lang="en"><head><meta charset="utf-8">',
    '<title>Hostile review</title>',
    `<meta http-equiv="refresh" content="0;url=${origin}/print-spec/hostile.html">`,
    '<style>h1 { color: rgb(1, 2, 3); }</style>',
    `<script>${say('an inline script')}</script>`,
    `<script src="${origin}/print-spec/hostile.js"></script>`,
    '</head>',
    `<body onload="${say('a body onload handler')}">`,
    '<h1>Hostile review</h1>',
    `<img src="x" onerror="${say('an image error handler')}">`,
    `<svg onload="${say('an svg onload handler')}"><script>${say('a script inside svg')}</script></svg>`,
    `<iframe srcdoc="&lt;script&gt;${say('a script in a nested frame')}&lt;/script&gt;"></iframe>`,
    `<iframe src="javascript:${say('a javascript: address')}"></iframe>`,
    `<iframe src="${origin}/print-spec/hostile.html"></iframe>`,
    `<object data="${origin}/print-spec/hostile.html"></object>`,
    `<embed src="${origin}/print-spec/hostile.html">`,
    // A frame inside the document is parsed afresh wherever the document
    // lands, so nothing about how the document itself was parsed protects it.
    `<iframe srcdoc="&lt;script src=&quot;${origin}/print-spec/hostile.js&quot;&gt;&lt;/script&gt;"></iframe>`,
    `<iframe srcdoc="&lt;meta http-equiv=&quot;refresh&quot; content=&quot;0;url=${origin}/print-spec/hostile.html&quot;&gt;"></iframe>`,
    `<details open ontoggle="${say('a toggle handler')}"><summary>more</summary>text</details>`,
    `<input autofocus onfocus="${say('an autofocus handler')}">`,
    // Not script, but a request: where the document is printed it may make none.
    `<link rel="stylesheet" href="${origin}/print-spec/hostile.css">`,
    `<img src="${origin}/print-spec/hostile.png" alt="">`,
    // What a document carries in itself is the document: this is how the
    // engine embeds a drawing, and it has to reach the paper.
    `<img id="carried" alt="" src="data:image/svg+xml,${encodeURIComponent(CARRIED_DRAWING)}">`,
    '<p>The rest of the review.</p>',
    // A second pragma, out of place and shouting.
    `<META HTTP-EQUIV="Refresh" CONTENT="1;URL=${origin}/print-spec/hostile.html">`,
    '</body></html>',
  ].join('\n');
}

/** What the application's own address would serve if a hostile file ever got onto it. */
const HOSTILE_SCRIPT = "top.postMessage('RAN:a script served by the application','*')";
const HOSTILE_PAGE = '<!doctype html><script src="/print-spec/hostile.js"></script><p>elsewhere</p>';
const HOSTILE_STYLE = 'h1 { color: rgb(9, 9, 9) !important; }';
const CARRIED_DRAWING = '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="7"><rect width="12" height="7"/></svg>';

test.describe('a document that tries to run script', () => {
  test('runs none on screen and none on its way to the printer', async ({ page }) => {
    const seen = await watch(page);
    const hostile = hostileDocument(backend.url);

    // The application's policy lets a page run scripts from its own address.
    // These stand for the worst case: hostile files that are served there.
    // `served` is every one of them a frame was actually given: a request the
    // browser refused to make never gets this far.
    const served: string[] = [];
    await page.route(`${backend.url}/print-spec/**`, (route) => {
      const { pathname } = new URL(route.request().url());
      served.push(pathname);
      if (pathname.endsWith('.js')) return route.fulfill({ contentType: 'text/javascript', body: HOSTILE_SCRIPT });
      if (pathname.endsWith('.html')) return route.fulfill({ contentType: 'text/html', body: HOSTILE_PAGE });
      if (pathname.endsWith('.css')) return route.fulfill({ contentType: 'text/css', body: HOSTILE_STYLE });
      return route.fulfill({ status: 404, body: '' });
    });

    // FAULT INJECTION. The engine escapes every string it puts in a document,
    // so a hostile one cannot be had from it. Its answer to this one call is
    // kept, and only the document inside is replaced.
    await page.route('**/api/rpc', async (route) => {
      const call = route.request().postDataJSON() as { method?: string; params?: { format?: string } } | null;
      if (call?.method !== 'incidents.export' || call.params?.format !== 'html') return route.continue();
      const response = await route.fetch();
      const reply = (await response.json()) as { ok: true; result: Rendered };
      await route.fulfill({ response, json: { ...reply, result: { ...reply.result, content: hostile } } });
    });

    await openIncidentExport(page);
    const dialog = page.getByRole('dialog');
    const preview = dialog.locator('iframe[title$="(preview)"]');
    await expect(preview).toHaveAttribute('srcdoc', hostile);
    await expect(preview).toHaveAttribute('sandbox', '');
    await expect(preview.contentFrame().getByRole('heading', { level: 1 })).toHaveText('Hostile review');
    // Time for whatever was going to run: the refresh is due at once.
    await page.waitForTimeout(1_500);
    expect(seen.ran, 'the preview').toEqual([]);
    // The preview may load what a document names — it is the document as
    // written — and that is how this test knows such a request is seen at all.
    expect(served, 'the preview').toContain('/print-spec/hostile.css');
    await expect(preview.contentFrame().locator('h1')).toHaveCSS('color', 'rgb(9, 9, 9)');
    const servedOnScreen = served.length;

    await dialog.getByRole('button', { name: 'Print' }).click();
    await expect.poll(() => seen.printed).toEqual(['a frame']);

    const printing = dialog.locator('iframe[title="Document being printed"]');
    const allowed = ((await printing.getAttribute('sandbox')) ?? '').split(' ');
    expect(allowed).not.toContain('allow-same-origin');
    expect(
      await printing.evaluate((frame: HTMLIFrameElement) => frame.contentDocument === null),
      'this page cannot reach the frame that prints: it is of another origin',
    ).toBe(true);
    // The whole document is what prints, with its own styles and without its
    // pragmas — and without the stylesheet it names, which is somebody else's.
    const printed = printing.contentFrame();
    await expect(printed.getByRole('heading', { level: 1, includeHidden: true })).toHaveText('Hostile review');
    await expect(printed.locator('h1')).toHaveCSS('color', 'rgb(1, 2, 3)');
    await expect(printed.locator('p')).toHaveText('The rest of the review.');
    await expect(printed.locator('meta[http-equiv]')).toHaveCount(0);
    await expect
      .poll(() => printed.locator('#carried').evaluate((image: HTMLImageElement) => image.naturalWidth))
      .toBe(12);
    // Time for the pragma that waits a second, had it been left in.
    await page.waitForTimeout(2_000);
    expect(seen.ran, 'the frame that prints').toEqual([]);
    expect(seen.printed, 'and it printed once, not the page it was sent to').toEqual(['a frame']);
    expect(served.slice(servedOnScreen), 'the frame that prints was given nothing but its own script').toEqual([]);

    // The control: the same document where scripts may run and nothing else
    // is in the way. If this stays silent, the two silences above prove nothing.
    await page.evaluate((html) => {
      const frame = document.createElement('iframe');
      frame.setAttribute('sandbox', 'allow-scripts');
      frame.srcdoc = html;
      document.body.append(frame);
    }, hostile);
    await expect.poll(() => seen.ran).toContain('a script served by the application');
  });
});
