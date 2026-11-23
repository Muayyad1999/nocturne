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

  // The second version of the drawer: variables, variants per device type,
  // and somewhere for an incident to be offered when evidence is kept.
  await engine.rpc('devices.create', {
    name: 'SW2-IOS',
    address: '127.0.0.1',
    port: engine.device.port,
    transport: 'telnet',
    profile: 'cisco_ios',
  });
  await engine.rpc('terminal.snippets.save', {
    name: 'Describe a port',
    text: 'say interface {{port}}\nsay description {{text=uplink to core}}\n',
    tags: ['interfaces', 'config'],
  });
  await engine.rpc('terminal.snippets.save', { name: 'Port bounce', text: 'say bounce {{port}}', tags: ['interfaces'] });
  await engine.rpc('terminal.snippets.save', {
    name: 'Port bounce',
    text: 'say interface {{port}}\nsay shutdown\nsay no shutdown\n',
    profile: 'cisco_ios',
    tags: ['interfaces'],
  });
  await engine.rpc('incidents.create', { title: 'SW2 uplink flapping', severity: 'sev3' });
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

    test('snippets with variables, session logs, capture and clips', async ({ page }) => {
      await page.context().grantPermissions(['clipboard-write']);
      await clearSessions();
      const logs = await engine.rpc<{ name: string }[]>('terminal.logs.list', {});
      if (logs.length > 0) await engine.rpc('terminal.logs.delete', { names: logs.map((log) => log.name) });
      await openApp(page, '/terminal', { backend: engine.backend, theme });

      await page.getByRole('button', { name: /SW2-IOS/ }).first().click();
      await expect(page.getByRole('tab', { name: /SW2-IOS/ })).toBeVisible();
      await page.getByRole('button', { name: 'Log this session to disk' }).click();
      await page.locator('.xterm-screen:visible').first().click();
      for (const command of ['show version', 'show secret', 'say interface Gi0/1 is up', 'lines 1400']) {
        await page.keyboard.type(`${command}\r`, { delay: 15 });
        await page.waitForTimeout(250);
      }

      // The drawer for a cisco_ios session: tags, the variant the engine chose, the others.
      await page.getByRole('button', { name: 'Snippets' }).click();
      const drawer = page.getByRole('dialog', { name: 'Snippets' });
      await drawer.getByRole('button', { name: 'Show 1 other variant' }).click();
      await shoot(page, theme, 'snippets-variants');

      // One snippet about to be sent: a refusal beside its field…
      await drawer.getByRole('button', { name: /Describe a port/ }).first().click();
      const form = drawer.getByTestId('snippet-use');
      await form.getByLabel(/^port/).fill('Gi0/1\tGi0/2');
      await expect(form.getByText(/contains a control character/)).toBeVisible();
      await shoot(page, theme, 'snippet-refused');
      // …and the exact text, once the engine takes the values.
      await form.getByLabel(/^port/).fill('GigabitEthernet0/1');
      await expect(form.getByRole('region', { name: 'What will be typed' })).toContainText('GigabitEthernet0/1');
      await shoot(page, theme, 'snippet-form');
      await form.getByRole('button', { name: /^Type into/ }).click();
      await expect(page.getByRole('dialog', { name: /Send this snippet into/ })).toBeVisible();
      await shoot(page, theme, 'snippet-question');
      await page.keyboard.press('Escape');
      await form.getByRole('button', { name: 'All snippets' }).click();

      // Import: a document with one entry the engine will not take, then without it.
      await drawer.getByRole('button', { name: 'Import and export snippets' }).click();
      await shoot(page, theme, 'snippets-menu');
      await page.getByRole('menuitem', { name: 'Import…' }).click();
      const incoming = {
        format: 'nocturne.snippets/1',
        snippets: [
          { name: 'Interface status', text: 'show interfaces status | include connected', tags: ['interfaces'] },
          { name: 'Spanning tree', text: 'show spanning-tree summary' },
          { name: 'Broken', text: 'show {{' },
        ],
      };
      const importing = drawer.getByTestId('snippet-import');
      await importing.getByLabel('Document').fill(JSON.stringify(incoming, null, 1));
      await importing.getByRole('button', { name: 'Check the document' }).click();
      await expect(importing.getByTestId('import-summary')).toBeVisible();
      await shoot(page, theme, 'import-problem');
      incoming.snippets.pop();
      await importing.getByLabel('Document').fill(JSON.stringify(incoming, null, 1));
      await importing.getByText('Keep both', { exact: true }).click();
      await importing.getByRole('button', { name: 'Check the document' }).click();
      await expect(importing.getByTestId('import-summary')).toBeVisible();
      await shoot(page, theme, 'import-dry-run');
      await page.keyboard.press('Escape');

      // Session logs: the list, a page of one, and the same page as it was written.
      await page.getByRole('button', { name: 'Session logs' }).click();
      const logsDrawer = page.getByRole('dialog', { name: 'Session logs' });
      await expect(logsDrawer.getByRole('list', { name: 'Session logs' })).toBeVisible();
      await shoot(page, theme, 'logs-list');
      await logsDrawer.getByRole('list', { name: 'Session logs' }).getByRole('button').first().click();
      await expect(logsDrawer.getByTestId('log-range')).toBeVisible();
      await shoot(page, theme, 'log-page');
      await logsDrawer.getByRole('button', { name: 'Show unredacted…' }).click();
      await expect(page.getByRole('dialog', { name: /without redaction/ })).toBeVisible();
      await shoot(page, theme, 'log-unredacted-question');
      await page.getByRole('dialog', { name: /without redaction/ }).getByRole('button', { name: 'Show unredacted' }).click();
      await expect(logsDrawer.getByTestId('log-raw')).toBeVisible();
      await shoot(page, theme, 'log-unredacted');
      await logsDrawer.getByRole('button', { name: 'Hide secrets again' }).click();
      await logsDrawer.getByRole('button', { name: 'Keep these lines as evidence…' }).click();
      await expect(page.getByTestId('keep-preview')).toContainText('log started');
      await shoot(page, theme, 'log-keep');
      await page.getByRole('dialog', { name: 'Keep as evidence' }).getByRole('button', { name: 'Cancel' }).click();
      await page.keyboard.press('Escape');

      // Capture: the menu, the last lines as the engine reads them, and a selection.
      await page.locator('.xterm-screen:visible').first().click();
      await page.keyboard.type('show secret\r', { delay: 15 });
      await page.waitForTimeout(300);
      await page.getByRole('button', { name: 'Capture' }).click();
      await shoot(page, theme, 'capture-menu');
      await page.getByRole('menuitem', { name: 'Keep the last screen as evidence…' }).click();
      const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
      await expect(keep.getByTestId('keep-preview')).toContainText('secret');
      await keep.getByLabel('Attach to an incident').selectOption({ label: 'SW2 uplink flapping' });
      await shoot(page, theme, 'capture-lines');
      await keep.getByRole('button', { name: 'Cancel' }).click();

      const line = page.locator('.xterm-rows:visible > div', { hasText: 'enable secret' }).last();
      const box = await line.boundingBox();
      await page.mouse.move(box!.x + 2, box!.y + box!.height / 2);
      await page.mouse.down();
      await page.mouse.move(box!.x + 420, box!.y + box!.height * 1.5, { steps: 8 });
      await page.mouse.up();
      await shoot(page, theme, 'selection');
      await page.getByRole('button', { name: 'Keep', exact: true }).click();
      await expect(keep.getByTestId('keep-preview')).toContainText('Supplied by the operator');
      await shoot(page, theme, 'capture-selection');
      await keep.getByRole('button', { name: 'Keep', exact: true }).click();
      await expect(page.getByTestId('keep-stored')).toBeVisible();
      await shoot(page, theme, 'capture-stored');
      await page.getByRole('dialog', { name: 'Kept as evidence' }).getByRole('button', { name: 'Delete it' }).click();
      // Copied last, so that its toast is not over the dialogs above.
      await page.getByRole('button', { name: 'Copy', exact: true }).click();

      await page.locator('.xterm-screen:visible').first().click({ button: 'right' });
      await expect(page.getByRole('menu')).toBeVisible();
      await shoot(page, theme, 'context-menu');
      await page.keyboard.press('Escape');

      await page.getByRole('button', { name: 'Capture' }).click();
      await page.getByRole('menuitem', { name: 'Save the last lines as a file…' }).click();
      await page.getByRole('dialog', { name: /Save the last lines/ }).getByLabel('Lines from the end').fill('12');
      await expect(page.getByTestId('save-preview')).toContainText('12 lines');
      await shoot(page, theme, 'save-lines');
      await page.keyboard.press('Escape');

      // Clips: what the Copy above left, and what can be done with it.
      await page.getByRole('button', { name: /^Clips, / }).click();
      await expect(page.getByRole('dialog', { name: 'Clips' })).toBeVisible();
      await shoot(page, theme, 'clips');
      await page.keyboard.press('Escape');

      // At 1280 with the dock the toolbar gives its words up, not its buttons.
      await page.setViewportSize({ width: 1280, height: 720 });
      await page.waitForTimeout(300);
      await shoot(page, theme, 'toolbar-1280');

      // A narrow window — a laptop split in two, a tablet: every new piece once more.
      await page.setViewportSize({ width: 760, height: 900 });
      await page.waitForTimeout(300);
      await shoot(page, theme, 'narrow-toolbar');
      await page.getByRole('button', { name: 'Snippets' }).click();
      await drawer.getByRole('button', { name: /Describe a port/ }).first().click();
      await form.getByLabel(/^port/).fill('GigabitEthernet0/1\nshutdown');
      await expect(form.getByText(/contains a line break/)).toBeVisible();
      await shoot(page, theme, 'narrow-snippet-refused');
      await form.getByLabel(/^port/).fill('GigabitEthernet0/1');
      await expect(form.getByRole('region', { name: 'What will be typed' })).toContainText('GigabitEthernet0/1');
      await shoot(page, theme, 'narrow-snippet-form');
      await form.getByRole('button', { name: /^Type into/ }).click();
      await expect(page.getByRole('dialog', { name: /Send this snippet into/ })).toBeVisible();
      await shoot(page, theme, 'narrow-snippet-question');
      await page.keyboard.press('Escape');
      await page.keyboard.press('Escape');
      await expect(drawer).toBeHidden();

      await page.getByRole('button', { name: 'Session logs' }).click();
      await logsDrawer.getByRole('list', { name: 'Session logs' }).getByRole('button').first().click();
      await expect(logsDrawer.getByTestId('log-range')).toBeVisible();
      await shoot(page, theme, 'narrow-log-page');
      await page.keyboard.press('Escape');
      await expect(logsDrawer).toBeHidden();

      await page.getByRole('button', { name: 'Capture' }).click();
      await page.getByRole('menuitem', { name: /Keep the last 200 lines/ }).click();
      await expect(keep.getByTestId('keep-preview')).toContainText('secret');
      await shoot(page, theme, 'narrow-capture-lines');
      await keep.getByRole('button', { name: 'Cancel' }).click();

      await page.getByRole('button', { name: /^Clips, / }).click();
      await expect(page.getByRole('dialog', { name: 'Clips' })).toBeVisible();
      await shoot(page, theme, 'narrow-clips');
      await page.keyboard.press('Escape');
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
