/**
 * `pnpm shots` — the Terminal screen, photographed in both themes at the two
 * sizes that matter, with and without the assistant dock.
 *
 * These are taken against the real engine and the fake console of
 * `terminal-support.ts`, because a screenshot of a terminal with nothing in
 * it says nothing about how the screen reads.
 */

import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startSshd, startTerminalEngine, type Sshd, type TerminalEngine } from './terminal-support';

const SHOTS = resolve(fileURLToPath(new URL('./shots/terminal', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let engine: TerminalEngine;
let sshd: Sshd | string = 'not started';

test.beforeAll(async () => {
  test.setTimeout(300_000);
  engine = await startTerminalEngine();

  // An SSH device, so the file panel has something real to show.
  sshd = await startSshd();
  if (typeof sshd !== 'string') {
    const credential = await engine.rpc<{ id: string }>('credentials.create', {
      name: 'Lab key',
      username: sshd.user,
      private_key: sshd.privateKey,
    });
    await engine.rpc('devices.create', {
      name: 'FILESRV',
      address: '127.0.0.1',
      port: sshd.port,
      transport: 'ssh',
      credential_id: credential.id,
    });
  }
  await engine.rpc('terminal.snippets.save', {
    name: 'Interface status',
    text: 'show interfaces status',
    tags: ['interfaces'],
  });
  await engine.rpc('terminal.snippets.save', {
    name: 'Save the configuration',
    text: 'write memory',
    tags: ['config'],
  });
});

test.afterAll(async () => {
  if (typeof sshd !== 'string') await sshd.stop();
  await engine?.stop();
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

async function connect(page: Page): Promise<void> {
  await page.getByRole('button', { name: /SW1-LAB/ }).first().click();
  await expect(page.getByRole('tab', { name: /SW1-LAB/ })).toBeVisible();
  await page.locator('.xterm-screen:visible').first().click();
  for (const command of ['show version', 'say the device is answering', 'lines 12']) {
    await page.keyboard.type(`${command}\r`, { delay: 15 });
    await page.waitForTimeout(200);
  }
}

async function clearSessions(): Promise<void> {
  const sessions = await engine.rpc<{ id: string }[]>('terminal.list');
  for (const session of sessions) await engine.rpc('terminal.close', { session_id: session.id });
}

for (const theme of THEMES) {
  test.describe(`${THEME_FOLDER[theme]} theme`, () => {
    test('the screen', async ({ page }) => {
      // The paste sheet is reached through the clipboard, like a real paste.
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await clearSessions();
      await openApp(page, '/terminal', { backend: engine.backend, theme });

      // Empty: what the screen is for, with the search already focused.
      await shoot(page, theme, 'empty');

      await connect(page);
      await shoot(page, theme, 'session');

      // The transport card, which is where "what did it negotiate?" is answered.
      await page.getByRole('button', { name: /Connection details/ }).first().hover();
      await expect(page.getByRole('tooltip')).toBeVisible();
      await shoot(page, theme, 'transport');
      await page.mouse.move(0, 0);

      // Two panes, broadcasting.
      await page.getByLabel('Transport').selectOption('telnet');
      await page.getByLabel('Host to connect to').fill(`127.0.0.1:${engine.device.port}`);
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await expect(page.getByRole('tab')).toHaveCount(2);
      await page.getByRole('button', { name: 'Move this session to a new pane' }).click();
      await page.locator('.xterm-screen:visible').last().click();
      await page.keyboard.type('say a second console\r', { delay: 15 });
      await page.getByRole('button', { name: 'Broadcast' }).click();
      await shoot(page, theme, 'split-broadcast');
      await page.getByRole('button', { name: 'Stop' }).click();

      // The paste sheet, with the lines classified.
      await page.evaluate(() =>
        navigator.clipboard
          .writeText(
            'configure terminal\ninterface GigabitEthernet0/1\n description uplink to core\n switchport mode trunk\nreload\n',
          )
          .catch(() => undefined),
      );
      await page.locator('.xterm-screen:visible').first().click();
      await page.keyboard.press('Control+Shift+V');
      await expect(page.getByRole('dialog')).toBeVisible();
      await shoot(page, theme, 'paste-sheet');
      await page.keyboard.press('Escape');

      // The snippets drawer.
      await page.getByRole('button', { name: 'Snippets' }).click();
      await expect(page.getByRole('dialog', { name: 'Snippets' })).toBeVisible();
      await shoot(page, theme, 'snippets');
      await page.keyboard.press('Escape');

      // The find bar over a full screen of output.
      await page.locator('.xterm-screen:visible').first().click();
      await page.keyboard.type('lines 400\r', { delay: 15 });
      await page.waitForTimeout(400);
      await page.getByRole('button', { name: 'Find in scrollback' }).first().click();
      await page.getByRole('textbox', { name: 'Find in scrollback' }).fill('line 003');
      await page.waitForTimeout(300);
      await shoot(page, theme, 'find');
    });

    test('the file panel', async ({ page }) => {
      test.skip(typeof sshd === 'string', 'no private sshd here');
      await clearSessions();
      await openApp(page, '/terminal', { backend: engine.backend, theme });
      await page.getByRole('button', { name: /FILESRV/ }).first().click();
      await expect(page.getByRole('tab', { name: /FILESRV/ })).toBeVisible();
      await page.getByRole('button', { name: 'Files on this device' }).click();
      await expect(page.getByRole('complementary', { name: /Files on FILESRV/ })).toBeVisible();
      await page.waitForTimeout(400);
      await shoot(page, theme, 'files');
    });

    test('with the dock, and at 1280', async ({ page }) => {
      await clearSessions();
      await openApp(page, '/terminal', { backend: engine.backend, theme, dock: true });
      await connect(page);
      await shoot(page, theme, 'dock');

      await page.setViewportSize({ width: 1280, height: 720 });
      await page.waitForTimeout(300);
      await shoot(page, theme, '1280-dock');

      await openApp(page, '/terminal', { backend: engine.backend, theme });
      await page.waitForTimeout(500);
      await shoot(page, theme, '1280');
    });
  });
}
