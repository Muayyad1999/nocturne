/**
 * The Settings screens, against a real engine.
 *
 * What is being proved here, more than anything else: a stored secret never
 * reaches the interface, and a section whose backend is not in the build is
 * simply not offered.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';
import { openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async () => {
  test.setTimeout(300_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 180_000,
  });
  backend = await startBackend({ uiDir });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

test('offers only the sections this build has a backend for', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const methods = await rpc<string[]>('app.methods');

  await openApp(page, '/settings', { backend });

  const nav = page.getByRole('navigation', { name: 'Settings sections' });
  for (const section of ['General', 'Security', 'Credentials', 'Device types', 'Connections', 'About']) {
    await expect(nav.getByRole('link', { name: section })).toBeVisible();
  }

  // The feature crates behind these are not in this build, so their sections
  // are absent rather than broken.
  expect(methods).not.toContain('ai.providers');
  await expect(nav.getByRole('link', { name: 'Assistant' })).toHaveCount(0);
  await expect(nav.getByRole('link', { name: 'Logs & traps' })).toHaveCount(0);

  // No section in the address lands on the first one, not on a blank page.
  expect(page.url()).toContain('/settings/general');
  expect(console.messages, backend.log()).toEqual([]);
});

test('saves a general preference to the engine', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/settings/general', { backend });
  await expect(page.getByRole('heading', { name: 'General' })).toBeVisible();

  // The real <input> sits under the drawn dot, so the label is what a person
  // clicks — and what this test clicks.
  const theme = page.getByRole('radiogroup', { name: 'Theme' });
  await theme.getByText('Day', { exact: true }).click();
  await expect(page.getByText('Saved').first()).toBeVisible();
  // The choice is applied here and now, not only stored.
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  expect(await rpc<{ theme: string }>('settings.get', { section: 'general' })).toMatchObject({
    theme: 'day',
  });

  await theme.getByText('Night', { exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  expect(console.messages, backend.log()).toEqual([]);
});

test('sets a master password, and says what the vault state means', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/settings/security', { backend });
  await expect(page.getByText('No master password', { exact: true })).toBeVisible();
  await expect(page.getByText(/anyone who can read this computer/i)).toBeVisible();

  await page.getByRole('button', { name: 'Set a master password' }).click();
  const dialog = page.getByRole('dialog', { name: 'Set a master password' });
  await dialog.getByRole('textbox', { name: 'New master password' }).fill('short');
  await expect(dialog.getByText('At least eight characters.')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'New master password' }).fill('correct horse battery');
  await dialog.getByRole('textbox', { name: 'Repeat it' }).fill('correct horse batter');
  await expect(dialog.getByText('The two do not match.')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Repeat it' }).fill('correct horse battery');
  await dialog.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByText('Unlocked', { exact: true })).toBeVisible();
  expect(await rpc<{ state: string }>('vault.state')).toMatchObject({ state: 'unlocked' });

  // And back, so the rest of the suite runs against an open vault.
  await page.getByRole('button', { name: 'Change master password' }).click();
  const change = page.getByRole('dialog', { name: 'Change the master password' });
  await change
    .getByRole('textbox', { name: 'Current master password' })
    .fill('correct horse battery');
  await change.getByRole('button', { name: 'Remove it instead' }).click();
  await change.getByRole('button', { name: 'Remove the password' }).click();
  await expect(page.getByText('No master password', { exact: true })).toBeVisible();

  expect(console.messages, backend.log()).toEqual([]);
});

test('makes turning host key checking off a deliberate act', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/settings/connections', { backend });
  const policy = page.getByRole('combobox', { name: 'When a host key is not known' });
  await policy.selectOption('strict');
  await expect(page.getByText('Saved').first()).toBeVisible();
  expect(
    await rpc<{ host_key_policy: string }>('settings.get', { section: 'connections' }),
  ).toMatchObject({ host_key_policy: 'strict' });

  await policy.selectOption('accept');
  const confirmation = page.getByRole('dialog', { name: 'Turn host key checking off?' });
  await expect(confirmation).toContainText('will not notice if a device is impersonated');
  await confirmation.getByRole('button', { name: 'Cancel' }).click();

  // Backing out leaves the setting exactly as it was.
  expect(
    await rpc<{ host_key_policy: string }>('settings.get', { section: 'connections' }),
  ).toMatchObject({ host_key_policy: 'strict' });

  await policy.selectOption('tofu');
  expect(console.messages, backend.log()).toEqual([]);
});

test('never shows a stored secret, only whether one is set', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const secret = 'correct-horse-staple-42';

  await openApp(page, '/settings/credentials', { backend });
  // The empty table offers the same action, so take the toolbar's.
  await page.getByRole('button', { name: 'Add credential' }).first().click();

  const drawer = page.getByRole('dialog', { name: 'Add credential' });
  await drawer.getByRole('textbox', { name: 'Name', exact: true }).fill('Core switches');
  await drawer.getByRole('textbox', { name: 'User name', exact: true }).fill('netops');
  await drawer.getByRole('button', { name: 'Set…' }).first().click();
  await drawer.getByRole('textbox', { name: 'Password', exact: true }).fill(secret);
  await drawer.getByRole('button', { name: 'Add credential' }).click();

  await expect(page.getByRole('table', { name: 'Credentials' })).toContainText('Core switches');
  await expect(page.getByRole('table', { name: 'Credentials' })).toContainText('password');

  // Reopen it: the secret is gone for good, and the editor says so.
  await page.getByRole('cell', { name: 'Core switches' }).dblclick();
  const editor = page.getByRole('dialog', { name: 'Edit “Core switches”' });
  await expect(editor.getByText('Set', { exact: true }).first()).toBeVisible();
  await expect(editor.getByRole('button', { name: 'Replace…' }).first()).toBeVisible();

  expect(await page.content(), 'the secret is nowhere in the document').not.toContain(secret);
  // And the backend does not hand it back either.
  expect(JSON.stringify(await rpc('credentials.list'))).not.toContain(secret);

  await editor.getByRole('button', { name: 'Cancel' }).click();
  expect(console.messages, backend.log()).toEqual([]);
});

test('validates a device type as it is typed, without saving it', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/settings/device-types', { backend });
  const table = page.getByRole('table', { name: 'Device types' });
  await expect(table).toContainText('cisco_ios');
  // Maturity is on screen, because it is what decides whether to believe it.
  await expect(table.getByText('Verified').first()).toBeVisible();

  await page.getByRole('button', { name: 'New device type' }).click();
  // The editor takes the whole section: a profile is too wide for a drawer.
  const editor = page.getByRole('textbox', { name: 'Device type YAML' });
  await expect(editor).toBeVisible();

  await expect(page.getByText('Valid', { exact: true })).toBeVisible();
  // The resolved view lists the commands the profile would actually run.
  await expect(page.getByText('facts        show version')).toBeVisible();

  await editor.fill('id: Not A Valid Id\nname: Broken\n');
  await expect(page.getByText('1 problem', { exact: true })).toBeVisible();
  await expect(page.getByRole('listitem').filter({ hasText: /^id: / })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();

  const before = await rpc<{ profiles: unknown[] }>('profiles.list');
  await editor.fill('id: test_switch\nname: Test switch\nvendor: Example\nmaturity: experimental\n');
  await expect(page.getByText('Valid', { exact: true })).toBeVisible();
  // Checking is not saving.
  expect((await rpc<{ profiles: unknown[] }>('profiles.list')).profiles).toHaveLength(
    before.profiles.length,
  );

  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('table', { name: 'Device types' })).toContainText('Test switch');

  expect(console.messages, backend.log()).toEqual([]);
});

test('says where the data lives', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/settings/about', { backend });

  await expect(page.getByRole('heading', { name: 'About' })).toBeVisible();
  await expect(page.getByText('Nocturne', { exact: false }).first()).toBeVisible();
  await expect(page.getByText(backend.dataDir)).toBeVisible();

  expect(console.messages, backend.log()).toEqual([]);
});
