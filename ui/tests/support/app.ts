/**
 * Opening the app in a test, and watching the console while it runs.
 *
 * By default a spec runs against the Vite dev server with the mock transport
 * (`?mock`), so no engine is needed. Pass a `backend` (see `backend.ts`) to
 * run the same spec against a real `nocturne-server`.
 */

import type { ConsoleMessage, Page } from '@playwright/test';
import type { Backend } from './backend';

export type Theme = 'dark' | 'light';

export interface OpenOptions {
  /** A running engine to talk to. Without one the mock transport is used. */
  backend?: Backend;
  /** The theme to start in. Default: the app's own default (Night). */
  theme?: Theme;
  /** Open with the assistant dock showing. Default: closed. */
  dock?: boolean;
  /** Collapse the nav rail. Default: expanded. */
  navCollapsed?: boolean;
  /** Milliseconds the mock waits before answering. `0` makes screenshots deterministic. */
  latency?: number;
}

/** The key the stores persist under (see `lib/storage.ts`). */
const STORE_PREFIX = 'nocturne.';

/** Counts `openApp` calls, so each one's seed is applied exactly once. */
let seeded = 0;

/**
 * Writes the shell's persisted state before the app boots, so a test starts
 * in a known layout instead of clicking its way there — and so what one
 * `openApp` left behind cannot leak into the next one in the same test.
 * Everything not asked for is set to the app's own default.
 */
async function seedState(page: Page, options: OpenOptions): Promise<void> {
  const seed = {
    theme: options.theme ?? 'dark',
    dock: options.dock ?? false,
    navCollapsed: options.navCollapsed ?? false,
  };

  // Init scripts accumulate on a page and run in the order they were added,
  // so a later `openApp` overwrites what an earlier one seeded. Each call
  // seeds once: a `page.reload()` inside a test must keep what the test did,
  // which is how "…and is remembered" is tested at all.
  seeded += 1;
  await page.addInitScript(
    ([prefix, layout, id]) => {
      try {
        if (window.sessionStorage.getItem(`${prefix}seed`) === String(id)) return;
        window.sessionStorage.setItem(`${prefix}seed`, String(id));
        const write = (key: string, state: unknown) => {
          window.localStorage.setItem(`${prefix}${key}`, JSON.stringify({ state, version: 1 }));
        };
        write('theme', { theme: layout.theme });
        write('layout', { navCollapsed: layout.navCollapsed, dockOpen: layout.dock, dockWidth: 380 });
      } catch {
        // No storage on this document (about:blank between two loads).
      }
    },
    [STORE_PREFIX, seed, seeded] as const,
  );
}

/**
 * Opens the app at `route` (a hash route such as `/devices`) and waits for the
 * shell to be on screen.
 */
export async function openApp(page: Page, route = '/', options: OpenOptions = {}): Promise<void> {
  await seedState(page, options);

  // Going from `…#/a` to `…#/b` is a fragment change, not a load: the app
  // would keep running with whatever state it already had. A blank page in
  // between makes every `openApp` a real boot.
  if (!page.url().startsWith('about:')) await page.goto('about:blank');

  const { backend, latency } = options;
  if (backend) {
    // The token arrives in the fragment; the app stores it and cleans the
    // address, which is why the route is set in a second step.
    await page.goto(`${backend.url}/#token=${backend.token}`);
  } else {
    const query = latency === undefined ? 'mock' : `mock&latency=${latency}`;
    await page.goto(`/?${query}#/`);
  }

  await page.getByRole('navigation', { name: 'Primary' }).waitFor();
  if (route !== '/') await goTo(page, route);
  await pageLoaded(page);
}

/** Follows a hash route in the page that is already open. */
export async function goTo(page: Page, route: string): Promise<void> {
  await page.evaluate((hash) => {
    window.location.hash = hash;
  }, route);
  await pageLoaded(page);
}

/**
 * Waits for the screen itself, not the skeleton the router shows while the
 * screen's code is still on its way.
 */
export async function pageLoaded(page: Page): Promise<void> {
  await page.locator('[aria-label="Loading screen"]').waitFor({ state: 'detached' });
}

export interface ConsoleWatch {
  /** Console errors and warnings, and uncaught exceptions, in the order they happened. */
  readonly messages: string[];
}

/**
 * Collects everything the page complained about. A clean screen produces an
 * empty list: a warning in the console is a defect like any other.
 */
export function watchConsole(page: Page): ConsoleWatch {
  const messages: string[] = [];
  page.on('console', (message: ConsoleMessage) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      messages.push(`${message.type()}: ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => {
    messages.push(`pageerror: ${error.message}`);
  });
  return { messages };
}

/** Waits until nothing is animating, so a screenshot is the same every time. */
export async function settle(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(150);
}
