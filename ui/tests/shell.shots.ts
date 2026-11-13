/**
 * `pnpm shots` — writes PNGs of the shell and the component showcase, in both
 * themes, to `tests/shots/` for visual review. They are not compared against
 * anything: they exist so a human (or whoever is reviewing the diff) can look
 * at the thing instead of imagining it.
 *
 * 1440×900 at deviceScaleFactor 2, as the spec asks, plus the sizes that tend
 * to break: 1280×720, and the shell with the assistant dock open.
 */

import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';

const SHOTS = resolve(fileURLToPath(new URL('./shots', import.meta.url)));

const THEMES: readonly Theme[] = ['dark', 'light'];
/** The folder name reads as the theme's name in the product, not its token. */
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };

const SHOWCASE_GROUPS = [
  'foundations',
  'actions',
  'forms',
  'status',
  'data',
  'overlays',
  'feedback',
  'code',
  'charts',
  'layout',
  'patterns',
] as const;

/**
 * The shell is a full-height grid whose page scrolls inside `main`, so a
 * `fullPage` screenshot of it would only ever show one window's worth with
 * blank paper below. For a shot that must show a whole page — the showcase —
 * the scrolling is lifted onto the document first, and put back after.
 */
const UNCLIP_ID = 'nocturne-shot-unclip';
const UNCLIP_CSS = `
  html, body, #root { height: auto !important; }
  #root > div { height: auto !important; }
  main, main > div { overflow: visible !important; }
`;

async function unclip(page: Page, on: boolean): Promise<void> {
  await page.evaluate(
    ([id, css, enable]) => {
      document.getElementById(id as string)?.remove();
      if (!enable) return;
      const style = document.createElement('style');
      style.id = id as string;
      style.textContent = css as string;
      document.head.append(style);
    },
    [UNCLIP_ID, UNCLIP_CSS, on] as const,
  );
}

async function shoot(page: Page, theme: Theme, name: string, fullPage = false): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  if (fullPage) await unclip(page, true);
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), fullPage, animations: 'disabled' });
  if (fullPage) await unclip(page, false);
}

for (const theme of THEMES) {
  test.describe(`${THEME_FOLDER[theme]} theme`, () => {
    test('the shell', async ({ page }) => {
      // Zero latency: the shot shows the settled screen, not a flash of loading.
      await openApp(page, '/', { theme, latency: 0 });
      await shoot(page, theme, 'shell-overview');

      await openApp(page, '/devices', { theme, latency: 0 });
      await shoot(page, theme, 'shell-devices');

      // With the assistant dock: the content column drops to about 900px.
      await openApp(page, '/devices', { theme, latency: 0, dock: true });
      await shoot(page, theme, 'shell-dock');

      // Collapsed rail.
      await openApp(page, '/monitoring', { theme, latency: 0, navCollapsed: true });
      await shoot(page, theme, 'shell-nav-collapsed');

      // The command palette, open on its default list.
      await openApp(page, '/', { theme, latency: 0 });
      await page.keyboard.press('ControlOrMeta+k');
      await expect(page.getByRole('dialog', { name: 'Command palette' })).toBeVisible();
      await shoot(page, theme, 'shell-palette');

      // The smaller window the spec must still hold at.
      await page.setViewportSize({ width: 1280, height: 720 });
      await page.keyboard.press('Escape');
      await openApp(page, '/changes', { theme, latency: 0, dock: true });
      await shoot(page, theme, 'shell-1280-dock');
    });

    test('the showcase', async ({ page }) => {
      for (const group of SHOWCASE_GROUPS) {
        await openApp(page, `/dev/components/${group}`, { theme, latency: 0 });
        await expect(page.getByRole('tabpanel').first()).toBeVisible();
        await shoot(page, theme, `showcase-${group}`, true);
      }
    });

    // The floating layers only exist once something opens them, so the
    // showcase pages above cannot show them. These shots can.
    test('the floating layers', async ({ page }) => {
      await openApp(page, '/dev/components/overlays', { theme, latency: 0 });

      await page.getByRole('button', { name: 'Open dialog' }).click();
      await expect(page.getByRole('dialog').first()).toBeVisible();
      await shoot(page, theme, 'overlay-dialog');
      await page.keyboard.press('Escape');

      await page.getByRole('button', { name: 'Delete backups' }).click();
      await expect(page.getByRole('dialog').first()).toBeVisible();
      await shoot(page, theme, 'overlay-confirm');
      await page.keyboard.press('Escape');

      await page.getByRole('button', { name: 'Open drawer' }).click();
      await expect(page.getByRole('dialog').first()).toBeVisible();
      await shoot(page, theme, 'overlay-drawer');
      await page.keyboard.press('Escape');

      await page.getByRole('button', { name: 'Filters' }).click();
      await shoot(page, theme, 'overlay-popover');
      await page.keyboard.press('Escape');

      for (const name of ['Success', 'Warning', 'Error with action']) {
        await page.getByRole('button', { name, exact: true }).click();
      }
      await shoot(page, theme, 'overlay-toasts');

      await openApp(page, '/dev/components/actions', { theme, latency: 0 });
      await page.getByRole('button', { name: 'Actions' }).click();
      await expect(page.getByRole('menu').first()).toBeVisible();
      await shoot(page, theme, 'overlay-menu');
    });
  });
}
