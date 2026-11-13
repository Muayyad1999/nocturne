/**
 * The shell is alive: it renders, it navigates, the palette drives it, the
 * theme and the dock toggle, and nothing complains in the console.
 *
 * Runs against the dev server with the mock transport. A screen's own spec
 * should instead take a real engine from `support/backend.ts`.
 */

import { expect, test } from '@playwright/test';
import { goTo, openApp, settle, watchConsole } from './support/app';

test('the shell renders', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  const rail = page.getByRole('navigation', { name: 'Primary' });
  await expect(rail).toBeVisible();
  await expect(page.getByText('Nocturne', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Search or run a command' })).toBeVisible();
  await expect(page.getByRole('contentinfo')).toBeVisible();

  // Every destination of the spec is in the rail, in its group.
  for (const label of [
    'Overview',
    'Devices',
    'Terminal',
    'Topology',
    'Monitoring',
    'Config',
    'Changes',
    'Automation',
    'Lab',
    'IPAM',
    'Tools',
    'Operations',
    'Settings',
  ]) {
    await expect(rail.getByRole('link', { name: label, exact: true })).toBeVisible();
  }

  expect(console.messages).toEqual([]);
});

test('the rail navigates, and a page says what it will be', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Devices' }).click();
  await expect(page).toHaveURL(/#\/devices$/);
  await expect(page.getByRole('heading', { level: 1, name: 'Devices' })).toBeVisible();
  // A screen that is built shows the engine's answer, not a placeholder.
  await expect(page.getByText('This screen is not built yet')).toHaveCount(0);

  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Lab' }).click();
  await expect(page).toHaveURL(/#\/lab$/);
  await expect(page.getByRole('heading', { level: 1, name: 'Lab' })).toBeVisible();
  // One that is not built says so, and never shows invented data. As the
  // screens land this moves to whichever is still a stub; when none is, it
  // goes, and nothing in the interface says this again.
  await expect(page.getByText('This screen is not built yet')).toBeVisible();

  // A route that does not exist is a screen, not a blank page.
  await goTo(page, '/nowhere');
  await expect(page.getByRole('heading', { level: 1, name: 'Not found' })).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('the command palette opens and navigates', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  await page.keyboard.press('ControlOrMeta+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await expect(palette).toBeVisible();
  await expect(palette.getByRole('option').first()).toBeVisible();

  // Fuzzy: the keywords find the screen, not only its name.
  await palette.getByRole('combobox').fill('neighbours');
  await expect(palette.getByRole('option', { name: /Go to Topology/ })).toBeVisible();
  await page.keyboard.press('Enter');

  await expect(palette).toBeHidden();
  await expect(page).toHaveURL(/#\/topology$/);
  await expect(page.getByRole('heading', { level: 1, name: 'Topology' })).toBeVisible();

  // Escape closes it and gives focus back.
  await page.keyboard.press('ControlOrMeta+k');
  await expect(palette).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(palette).toBeHidden();

  expect(console.messages).toEqual([]);
});

test('the theme toggles and is remembered', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  const html = page.locator('html');
  await expect(html).toHaveAttribute('data-theme', 'dark');

  await page.getByRole('button', { name: 'Switch to Day theme' }).click();
  await expect(html).toHaveAttribute('data-theme', 'light');

  await page.reload();
  await expect(html).toHaveAttribute('data-theme', 'light');

  await page.getByRole('button', { name: 'Switch to Night theme' }).click();
  await expect(html).toHaveAttribute('data-theme', 'dark');

  expect(console.messages).toEqual([]);
});

test('the assistant dock toggles', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  const dock = page.getByRole('complementary', { name: 'Assistant' });
  await expect(dock).toBeHidden();

  await page.keyboard.press('ControlOrMeta+j');
  await expect(dock).toBeVisible();
  await expect(dock.getByRole('heading', { name: 'Assistant', exact: true })).toBeVisible();

  await dock.getByRole('button', { name: 'Close assistant' }).click();
  await expect(dock).toBeHidden();

  // And from the rail.
  await page.getByRole('button', { name: 'Assistant' }).click();
  await expect(dock).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('the nav rail collapses and is remembered', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page);

  const railWidth = async () => (await page.getByRole('navigation', { name: 'Primary' }).boundingBox())?.width;
  await expect.poll(railWidth).toBe(216);

  await page.keyboard.press('ControlOrMeta+b');
  await expect.poll(railWidth).toBe(56);

  await page.reload();
  await expect.poll(railWidth).toBe(56);

  expect(console.messages).toEqual([]);
});

test('the layout holds at 1280×720 with the dock open', async ({ page }) => {
  const console = watchConsole(page);
  await page.setViewportSize({ width: 1280, height: 720 });
  await openApp(page, '/devices', { dock: true });
  await settle(page);

  await expect(page.getByRole('complementary', { name: 'Assistant' })).toBeVisible();

  // Nothing may push the window sideways, at any size the app claims to fit.
  for (const width of [1280, 1440]) {
    await page.setViewportSize({ width, height: 720 });
    await settle(page);
    const overflow = await page.evaluate(() => ({
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
    }));
    expect(overflow.scrollWidth, `horizontal overflow at ${width}px`).toBeLessThanOrEqual(
      overflow.clientWidth,
    );
  }

  expect(console.messages).toEqual([]);
});

test('the showcase renders every group without complaint', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/dev/components');

  const groups = ['Foundations', 'Actions', 'Forms', 'Status', 'Data', 'Overlays', 'Feedback', 'Code', 'Charts', 'Layout', 'Patterns'];
  for (const group of groups) {
    await page.getByRole('tab', { name: group }).click();
    await expect(page.getByRole('tab', { name: group })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('tabpanel', { name: group })).toBeVisible();
    await settle(page);
  }

  expect(console.messages).toEqual([]);
});
