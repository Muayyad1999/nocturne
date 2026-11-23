/**
 * The Terminal screen, against a real engine and a real device.
 *
 * Nothing here is mocked: `nocturne-server` on a fresh data directory, the
 * production UI bundle it serves, and a console the test itself answers for.
 * What is being proved is the part a terminal lives or dies by — that a
 * session survives leaving the screen and reloading the window, and comes
 * back with every byte, once.
 */

import { appendFile, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { goTo, openApp, pageLoaded, watchConsole } from './support/app';
import {
  MARKUP,
  PRIVATE_KEY,
  SECRETS,
  announceLag,
  everythingStored,
  loseOutputOnRequest,
  losingOutput,
  openInDesktopShell,
  outputLossSettled,
  setShellClipboard,
  shellRecord,
  startSshd,
  startTerminalEngine,
  watchClipboardReads,
  watchSent,
  withholdMethods,
  type Sshd,
  type TerminalEngine,
} from './terminal-support';

test.describe('Terminal', () => {
  test.describe.configure({ mode: 'serial', timeout: 240_000 });

  let engine: TerminalEngine;
  /** A private OpenSSH daemon for the SFTP test, or why there is none. */
  let sshd: Sshd | string = 'not started';

  test.beforeAll(async () => {
    test.setTimeout(300_000);
    engine = await startTerminalEngine();

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
  });

  test.afterAll(async () => {
    if (typeof sshd !== 'string') await sshd.stop();
    await engine?.stop();
  });

  /** Leaves the engine with no sessions, so each test starts from the same place. */
  test.beforeEach(async () => {
    const sessions = await engine.rpc<{ id: string }[]>('terminal.list');
    for (const session of sessions) await engine.rpc('terminal.close', { session_id: session.id });
    engine.device.forget();
  });

  // ------------------------------------------------------------- helpers

  /** The text on the visible terminal, row by row. */
  async function screenText(page: Page): Promise<string> {
    return page.evaluate(() => {
      const rows = [...document.querySelectorAll('.xterm-rows')].find(
        (element) => (element as HTMLElement).offsetParent !== null,
      );
      if (!rows) return '';
      return [...rows.children]
        .map((row) => (row.textContent ?? '').replace(/ /g, ' ').trimEnd())
        .join('\n');
    });
  }

  /** Every visible terminal, for the split-pane tests. */
  async function allScreens(page: Page): Promise<string[]> {
    return page.evaluate(() =>
      [...document.querySelectorAll('.xterm-rows')]
        .filter((element) => (element as HTMLElement).offsetParent !== null)
        .map((rows) =>
          [...rows.children].map((row) => (row.textContent ?? '').replace(/ /g, ' ').trimEnd()).join('\n'),
        ),
    );
  }

  async function expectOnScreen(page: Page, text: string): Promise<void> {
    await expect
      .poll(async () => (await screenText(page)).includes(text), {
        message: `waiting for “${text}” on the terminal`,
        timeout: 15_000,
      })
      .toBe(true);
  }

  function occurrences(haystack: string, needle: string): number {
    return haystack.split(needle).length - 1;
  }

  /** Clicks into the focused terminal and types, as a person would. */
  async function typeInTerminal(page: Page, text: string): Promise<void> {
    await page.locator('.xterm-screen:visible').first().click();
    // With a gap between keystrokes, because without one they can arrive out
    // of order. On a machine with every core busy `say TO-BOTH` reached the
    // device as `sya TO-BOTH` — identically in both panes, so the broadcast
    // was right and the typing was not. Nothing types at zero delay but a
    // test, and asserting that the terminal survives it is not the point of
    // any test here.
    await page.keyboard.type(text, { delay: 15 });
  }

  async function openDeviceSession(page: Page): Promise<void> {
    await page.getByRole('button', { name: /SW1-LAB/ }).first().click();
    await expect(page.getByRole('tab', { name: /SW1-LAB/ })).toBeVisible();
    await expectOnScreen(page, 'FakeOS 1.0 console');
  }

  // --------------------------------------------------------------- tests

  test('opens a session to a device and the device answers', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Terminal' })).toBeVisible();
    // The empty state says what the screen is for and puts the cursor in the search.
    await expect(page.getByText('Connect to a device')).toBeVisible();
    await expect(page.getByLabel('Search devices')).toBeFocused();

    await openDeviceSession(page);
    await typeInTerminal(page, 'show version\r');
    await expectOnScreen(page, 'FakeOS Software, Version 1.0');

    // The engine, not the browser, is where the session lives.
    const sessions = await engine.rpc<{ title: string; state: string; device_id: string }[]>('terminal.list');
    expect(sessions).toHaveLength(1);
    expect(sessions[0]).toMatchObject({ title: 'SW1-LAB', state: 'open', device_id: engine.deviceId });
    await expect(page.getByRole('contentinfo').getByText('1 session')).toBeVisible();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('survives leaving the screen and reloading the window, with no gap and nothing twice', async ({
    page,
  }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await typeInTerminal(page, 'say FIRST-LINE\r');
    await expectOnScreen(page, '[FIRST-LINE]');

    // 1. Another screen and back: the terminal is still there, untouched.
    await goTo(page, '/devices');
    await expect(page.getByRole('heading', { level: 1, name: 'Devices' })).toBeVisible();
    await goTo(page, '/terminal');
    await expectOnScreen(page, '[FIRST-LINE]');
    expect(occurrences(await screenText(page), '[FIRST-LINE]')).toBe(1);

    // 2. The window goes away entirely, and the device keeps printing.
    const [session] = await engine.rpc<{ id: string }[]>('terminal.list');
    await page.goto('about:blank');
    await engine.rpc('terminal.input', { session_id: session!.id, data: 'say WHILE-AWAY\r' });
    await expect
      .poll(async () => {
        const snapshot = await engine.rpc<{ text: string }>('terminal.snapshot', {
          session_id: session!.id,
        });
        return snapshot.text.includes('[WHILE-AWAY]');
      })
      .toBe(true);

    // 3. Back in the window: the scrollback, and then everything since.
    await page.goBack();
    await pageLoaded(page);
    await expectOnScreen(page, '[WHILE-AWAY]');

    const after = await screenText(page);
    expect(occurrences(after, '[FIRST-LINE]'), after).toBe(1);
    expect(occurrences(after, '[WHILE-AWAY]'), after).toBe(1);
    expect(after.indexOf('[FIRST-LINE]')).toBeLessThan(after.indexOf('[WHILE-AWAY]'));

    // 4. And it is a live session, not a picture of one.
    await typeInTerminal(page, 'say STILL-LIVE\r');
    await expectOnScreen(page, '[STILL-LIVE]');
    const live = await screenText(page);
    expect(occurrences(live, '[FIRST-LINE]'), live).toBe(1);
    expect(occurrences(live, '[WHILE-AWAY]'), live).toBe(1);

    // 5. The same again through a plain reload of the window.
    await page.reload();
    await pageLoaded(page);
    await expectOnScreen(page, '[STILL-LIVE]');
    const reloaded = await screenText(page);
    for (const marker of ['[FIRST-LINE]', '[WHILE-AWAY]', '[STILL-LIVE]']) {
      expect(occurrences(reloaded, marker), reloaded).toBe(1);
    }
    await typeInTerminal(page, 'say AFTER-RELOAD\r');
    await expectOnScreen(page, '[AFTER-RELOAD]');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('takes a few megabytes from a device without losing a line or the window', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // ~2.6 MB, in one go, as `show tech-support` would.
    const started = Date.now();
    await typeInTerminal(page, 'lines 40000\r');
    await expectOnScreen(page, 'line 39999');
    const elapsed = Date.now() - started;

    // The window is still answering: a frame no worse than a few of them.
    const worstFrameGap = await page.evaluate(
      () =>
        new Promise<number>((resolve) => {
          const start = performance.now();
          let last = start;
          let worst = 0;
          const tick = (now: number) => {
            worst = Math.max(worst, now - last);
            last = now;
            if (now - start < 1000) requestAnimationFrame(tick);
            else resolve(worst);
          };
          requestAnimationFrame(tick);
        }),
    );
    expect(worstFrameGap, `after ${elapsed} ms of output`).toBeLessThan(200);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('puts a seat belt in front of a paste, one keystroke wide', async ({ page, context }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await page.evaluate(() =>
      navigator.clipboard.writeText('say PASTED-ONE\nsay PASTED-TWO\nreload\n'),
    );

    // Backing out sends nothing at all.
    await page.locator('.xterm-screen:visible').first().click();
    await page.keyboard.press('Control+Shift+V');
    const sheet = page.getByRole('dialog', { name: /Paste into SW1-LAB/ });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByText('reload', { exact: true })).toBeVisible();
    // The policy's own words about the worst line in it.
    await expect(sheet.getByText('disruptive', { exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    expect(await screenText(page)).not.toContain('[PASTED-ONE]');

    // And Enter alone lets it through: a seat belt, not a lock.
    await page.locator('.xterm-screen:visible').first().click();
    await page.keyboard.press('Control+Shift+V');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    await expectOnScreen(page, '[PASTED-TWO]');

    // One read-only line is not worth a sheet.
    await page.evaluate(() => navigator.clipboard.writeText('show version'));
    await page.locator('.xterm-screen:visible').first().click();
    await page.keyboard.press('Control+Shift+V');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expectOnScreen(page, 'SW1> show version');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('holds a paste the browser raises itself until it is answered, and sends it once', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    // Writing is all this test needs. Reading is deliberately not granted: a
    // paste the browser raises carries its own text, so nothing here may
    // depend on the page being allowed to read the clipboard — which it is
    // not over plain HTTP, and not until the person has said yes anywhere.
    await context.grantPermissions(['clipboard-write']);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.evaluate(() => navigator.clipboard.writeText('say NATIVE-ONE\nsay NATIVE-TWO\n'));
    const sheet = page.getByRole('dialog', { name: /Paste into SW1-LAB/ });

    // Shift+Insert is the browser's own paste: no key handler of ours sees it.
    await page.locator('.xterm-screen:visible').first().click();
    await page.keyboard.press('Shift+Insert');
    await expect(sheet).toBeVisible();
    // The question is on screen and unanswered, so the device has run nothing.
    // The screen cannot show this — the lines would be behind the sheet — but
    // the console knows what it executed. The wait is for a paste that was
    // sent anyway to have arrived: it used to, within a few milliseconds.
    await page.waitForTimeout(500);
    expect(engine.device.said).toEqual([]);

    // Backing out sends nothing, then or afterwards.
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    await typeInTerminal(page, 'say AFTER-CANCEL\r');
    await expectOnScreen(page, '[AFTER-CANCEL]');
    expect(engine.device.said).toEqual(['say AFTER-CANCEL']);

    // Saying yes sends each line once.
    await page.keyboard.press('Shift+Insert');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    await expectOnScreen(page, '[NATIVE-TWO]');
    await typeInTerminal(page, 'say AFTER-PASTE\r');
    await expectOnScreen(page, '[AFTER-PASTE]');
    expect(engine.device.said).toEqual([
      'say AFTER-CANCEL',
      'say NATIVE-ONE',
      'say NATIVE-TWO',
      'say AFTER-PASTE',
    ]);

    // Ctrl+Shift+V is the browser's paste too, and needs no permission either.
    await page.keyboard.press('Control+Shift+V');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();

    // Text dropped on the terminal is a paste by another road, and is asked
    // about in the same way.
    await page.evaluate(() => {
      const screen = [...document.querySelectorAll('.xterm-screen')].find(
        (element) => (element as HTMLElement).offsetParent !== null,
      );
      const transfer = new DataTransfer();
      transfer.setData('text/plain', 'say DROPPED-ONE\nsay DROPPED-TWO\n');
      for (const type of ['dragover', 'drop']) {
        screen?.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: transfer }));
      }
    });
    const dropSheet = page.getByRole('dialog', { name: /Drop this text into SW1-LAB/ });
    await expect(dropSheet).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dropSheet).toBeHidden();

    await typeInTerminal(page, 'say THE-END\r');
    await expectOnScreen(page, '[THE-END]');
    expect(engine.device.said.slice(4)).toEqual(['say THE-END']);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('presses Enter once for each pasted line, whatever line endings the clipboard had', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // What a Windows clipboard holds: CR and LF at the end of every line.
    await page.evaluate(() => navigator.clipboard.writeText('say CRLF-ONE\r\nsay CRLF-TWO\r\n'));
    await page.locator('.xterm-screen:visible').first().click();
    await page.keyboard.press('Control+Shift+V');
    const sheet = page.getByRole('dialog', { name: /Paste into SW1-LAB/ });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole('listitem')).toHaveCount(2);
    await page.keyboard.press('Enter');
    await expectOnScreen(page, '[CRLF-TWO]');

    // The console answers CR and LF alike, so the screen looks the same
    // either way; the bytes are what differ. A line feed after the carriage
    // return is a second Enter, and on a real device it answers whatever
    // prompt the first one raised.
    await expect
      .poll(() => engine.device.receivedWith('CRLF-ONE'))
      .toBe('say CRLF-ONE\rsay CRLF-TWO\r');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('brackets a paste for a program that asked for it, so a shell holds the lines', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-write']);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // The far end turns bracketed paste on, as bash and zsh do.
    await typeInTerminal(page, 'brackets on\r');
    await expect.poll(() => engine.device.executed).toContain('brackets on');

    await page.evaluate(() => navigator.clipboard.writeText('say HELD-ONE\nsay HELD-TWO\n'));
    await page.keyboard.press('Shift+Insert');
    const sheet = page.getByRole('dialog', { name: /Paste into SW1-LAB/ });
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();

    // It arrived as one paste, between the brackets, and nothing in it ran.
    await expect.poll(() => engine.device.bracketed).toEqual(['say HELD-ONE\rsay HELD-TWO\r']);
    expect(engine.device.said).toEqual([]);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('splits into panes and can type into both at once', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // A second session, by hand, to the same console.
    await page.getByLabel('Transport').selectOption('telnet');
    await page.getByLabel('Host to connect to').fill(`127.0.0.1:${engine.device.port}`);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.getByRole('tab')).toHaveCount(2);

    await page.getByRole('button', { name: 'Move this session to a new pane' }).click();
    await expect(page.locator('.xterm-screen:visible')).toHaveCount(2);

    // Both panes have to be attached and showing their prompt before the
    // broadcast means anything.
    await expect
      .poll(async () => (await allScreens(page)).filter((screen) => screen.includes('SW1>')).length)
      .toBe(2);

    await page.getByRole('button', { name: 'Broadcast' }).click();
    await expect(page.getByText(/Typing goes to 2 sessions/)).toBeVisible();

    await typeInTerminal(page, 'say TO-BOTH\r');
    await expect
      .poll(
        async () => {
          const screens = await allScreens(page);
          return {
            panes: screens.length,
            answered: screens.filter((screen) => screen.includes('[TO-BOTH]')).length,
          };
        },
        { timeout: 15_000, message: 'both panes should show what was typed into one of them' },
      )
      .toEqual({ panes: 2, answered: 2 });

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('finds text in the scrollback', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await typeInTerminal(page, 'lines 400\r');
    await expectOnScreen(page, 'line 00399');

    await page.getByRole('button', { name: 'Find in scrollback' }).first().click();
    await page.getByRole('textbox', { name: 'Find in scrollback' }).fill('line 00042');
    await expect(page.getByText('1 of 1')).toBeVisible();
    await expectOnScreen(page, 'line 00042');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('types a snippet in without ever pressing Enter for you', async ({ page }) => {
    const console = watchConsole(page);
    await engine.rpc('terminal.snippets.save', { name: 'Version', text: 'show version' });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await expect(drawer).toBeVisible();
    await drawer.getByRole('button', { name: /Version/ }).first().click();

    await expectOnScreen(page, 'SW1> show version');
    // Typed, not run: the device has not answered it.
    expect(await screenText(page)).not.toContain('FakeOS Software, Version 1.0');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('keeps the tags and the device type of a snippet that is edited', async ({ page }) => {
    const console = watchConsole(page);
    // A device with a type of its own, on the same console: the drawer must
    // not rewrite a snippet as "for this type" just because it was edited
    // from such a session.
    await engine.rpc('devices.create', {
      name: 'SW2-IOS',
      address: '127.0.0.1',
      port: engine.device.port,
      transport: 'telnet',
      profile: 'cisco_ios',
    });
    const saved = await engine.rpc<{ id: string }>('terminal.snippets.save', {
      name: 'Clock',
      text: 'show clock',
      tags: ['daily', 'ops'],
    });

    await openApp(page, '/terminal', { backend: engine.backend });
    await page.getByRole('button', { name: /SW2-IOS/ }).first().click();
    await expect(page.getByRole('tab', { name: /SW2-IOS/ })).toBeVisible();
    await expectOnScreen(page, 'FakeOS 1.0 console');

    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await drawer.getByRole('button', { name: 'Edit Clock' }).click();
    await drawer.getByLabel('Name').fill('Clock, twice a day');
    await drawer.getByRole('button', { name: 'Save snippet' }).click();
    await expect(drawer.getByRole('button', { name: 'Edit Clock, twice a day' })).toBeVisible();

    const snippets = await engine.rpc<{ id: string; name: string; profile: string | null; tags: string[] }[]>(
      'terminal.snippets.list',
    );
    const edited = snippets.find((snippet) => snippet.id === saved.id);
    expect(edited).toMatchObject({ name: 'Clock, twice a day', profile: null, tags: ['daily', 'ops'] });

    // And both can be changed on purpose.
    await drawer.getByRole('button', { name: 'Edit Clock, twice a day' }).click();
    await drawer.getByLabel('Device type').selectOption('cisco_ios');
    await drawer.getByLabel('Tags').fill('ntp');
    await drawer.getByLabel('Tags').press('Enter');
    await drawer.getByRole('button', { name: 'Save snippet' }).click();
    await expect
      .poll(async () => {
        const list = await engine.rpc<{ id: string; profile: string | null; tags: string[] }[]>(
          'terminal.snippets.list',
        );
        const found = list.find((snippet) => snippet.id === saved.id);
        return found ? { profile: found.profile, tags: [...found.tags].sort() } : null;
      })
      .toEqual({ profile: 'cisco_ios', tags: ['daily', 'ntp', 'ops'] });

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('opens a link from the output in a new tab and stays where it is', async ({ page, context }) => {
    const console = watchConsole(page);
    // The address is never really fetched: the test answers for it.
    await context.route('https://nocturne.example/**', (route) =>
      route.fulfill({ contentType: 'text/html', body: '<title>opened</title>' }),
    );
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'link https://nocturne.example/kb/42\r');
    await expectOnScreen(page, 'https://nocturne.example/kb/42');

    // The device's own line, not the echo of the command: it starts the row.
    const row = page.locator('.xterm-rows:visible > div').filter({ hasText: /^https:\/\/nocturne\.example/ }).first();
    const box = await row.boundingBox();
    expect(box).not.toBeNull();
    const opened = context.waitForEvent('page');
    await page.mouse.move(box!.x + 40, box!.y + box!.height / 2);
    await page.mouse.click(box!.x + 40, box!.y + box!.height / 2);
    const tab = await opened;
    await tab.waitForURL('https://nocturne.example/kb/42');
    // A page opened from device output gets no handle on the application.
    expect(await tab.evaluate(() => window.opener === null)).toBe(true);
    await tab.close();

    await expect(page.getByRole('tab', { name: /SW1-LAB/ })).toBeVisible();
    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('shows every byte, once, when output is lost on a connection that stays open', async ({ page }) => {
    const console = watchConsole(page);
    await loseOutputOnRequest(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'say BEFORE\r');
    await expectOnScreen(page, '[BEFORE]');

    // What the device prints next never reaches the interface, and nothing
    // reconnects: the socket is as open as it was.
    await losingOutput(page, true);
    await typeInTerminal(page, 'say LOST\r');
    await expect.poll(() => engine.device.said).toContain('say LOST');
    const lost = await outputLossSettled(page);
    await losingOutput(page, false);
    expect(await screenText(page)).not.toContain('[LOST]');

    // The next output is not the next number. Printed as it comes it would
    // follow `[BEFORE]` as if the device had said nothing in between.
    await typeInTerminal(page, 'say AFTER\r');
    await expectOnScreen(page, '[AFTER]');
    await expectOnScreen(page, '[LOST]');
    const recovered = await screenText(page);
    for (const answer of ['[BEFORE]', '[LOST]', '[AFTER]']) expect(occurrences(recovered, answer), answer).toBe(1);
    expect(recovered.indexOf('[BEFORE]')).toBeLessThan(recovered.indexOf('[LOST]'));
    expect(recovered.indexOf('[LOST]')).toBeLessThan(recovered.indexOf('[AFTER]'));

    // A session that goes quiet straight after the loss never sees a wrong
    // number. The engine's notice that it dropped events is all it gets.
    await losingOutput(page, true);
    await typeInTerminal(page, 'say QUIET\r');
    await expect.poll(() => engine.device.said).toContain('say QUIET');
    await outputLossSettled(page, lost);
    await losingOutput(page, false);
    // Nothing arrives to give the loss away, however long one waits.
    await page.waitForTimeout(500);
    expect(await screenText(page)).not.toContain('[QUIET]');
    await announceLag(page, 3);
    await expectOnScreen(page, '[QUIET]');
    expect(occurrences(await screenText(page), '[QUIET]')).toBe(1);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('pastes and opens links through the desktop shell, where the WebView offers neither', async ({ page }) => {
    const console = watchConsole(page);
    await openInDesktopShell(page, engine.backend, '/terminal');
    await expect(page.getByRole('heading', { level: 1, name: 'Terminal' })).toBeVisible();
    await openDeviceSession(page);
    const sheet = page.getByRole('dialog', { name: /Paste into SW1-LAB/ });

    // The toolbar button has no `paste` event to carry the text, and here a
    // script may not read the clipboard: the text has to come from the shell.
    await setShellClipboard(page, 'say SHELL-ONE\nsay SHELL-TWO\n');
    await page.getByRole('button', { name: 'Paste into this session' }).click();
    await expect(sheet).toBeVisible();
    await page.waitForTimeout(300);
    expect(engine.device.said).toEqual([]);
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    await expectOnScreen(page, '[SHELL-TWO]');
    expect(engine.device.said).toEqual(['say SHELL-ONE', 'say SHELL-TWO']);

    // The context menu is the other door without an event of its own.
    await page.locator('.xterm-screen:visible').first().click({ button: 'right' });
    await page.getByRole('menuitem', { name: /Paste/ }).click();
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();

    // With nothing to paste the person is told why, in the shell's words,
    // and which keys work without reading the clipboard at all.
    await setShellClipboard(page, null);
    await page.getByRole('button', { name: 'Paste into this session' }).click();
    const told = page.getByRole('status').filter({ hasText: 'Nothing was pasted' });
    await expect(told).toContainText('the clipboard is empty');
    await expect(told).toContainText('Ctrl+Shift+V or Shift+Insert');

    // A link in the output is not an element the opener plugin could see,
    // and `window.open` goes nowhere: it has to be handed to the opener.
    await typeInTerminal(page, 'link https://nocturne.example/kb/42\r');
    await expectOnScreen(page, 'https://nocturne.example/kb/42');
    const row = page.locator('.xterm-rows:visible > div').filter({ hasText: /^https:\/\/nocturne\.example/ }).first();
    const box = await row.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move(box!.x + 40, box!.y + box!.height / 2);
    await page.mouse.click(box!.x + 40, box!.y + box!.height / 2);
    await expect.poll(async () => (await shellRecord(page)).opened).toEqual(['https://nocturne.example/kb/42']);

    // Neither of the things the WebView refuses was even tried.
    expect(await shellRecord(page)).toMatchObject({ pageClipboardReads: 0, windowOpens: 0 });
    expect(engine.device.said).toEqual(['say SHELL-ONE', 'say SHELL-TWO']);
    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('says plainly that this machine is out of reach from a server', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });

    // `nocturne-server` holds back what only makes sense at the machine
    // itself, and the screen repeats its sentence rather than hiding the door.
    await expect(page.getByText(/only available in the desktop app/)).toBeVisible();
    const shell = page.getByRole('button', { name: /Local shell/ });
    await expect(shell).toHaveAttribute('aria-disabled', 'true');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('writes a session log when asked, and says that it is', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await page.getByRole('button', { name: 'Log this session to disk' }).click();
    // The pane wears a mark while a log is being written.
    await expect(page.getByText('LOG', { exact: true })).toBeVisible();
    await typeInTerminal(page, 'say INTO-THE-LOG\r');

    await expect
      .poll(async () => {
        const sessions = await engine.rpc<{ logging: string | null }[]>('terminal.list');
        return sessions[0]?.logging ?? null;
      })
      .not.toBeNull();

    const [session] = await engine.rpc<{ logging: string }[]>('terminal.list');
    await expect
      .poll(async () => {
        const log = await engine.rpc<{ text: string }>('terminal.logs.read', { name: session!.logging });
        return log.text.includes('[INTO-THE-LOG]');
      })
      .toBe(true);

    await page.getByRole('button', { name: 'Stop logging this session' }).click();
    await expect(page.getByText('LOG', { exact: true })).toBeHidden();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('offers the selection to the assistant, and to the clipboard', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'say SELECT-ME\r');
    await expectOnScreen(page, '[SELECT-ME]');

    // Drag across the line the device printed.
    const line = page.locator('.xterm-rows:visible > div', { hasText: '[SELECT-ME]' }).first();
    const box = await line.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move(box!.x + 4, box!.y + box!.height / 2);
    await page.mouse.down();
    await page.mouse.move(box!.x + 160, box!.y + box!.height / 2, { steps: 8 });
    await page.mouse.up();

    await expect(page.getByRole('button', { name: 'Explain' })).toBeVisible();
    await page.getByRole('button', { name: 'Explain' }).click();
    // The dock opens with the selection attached; the Assistant slice fills it.
    await expect(page.getByRole('complementary', { name: 'Assistant' })).toBeVisible();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('browses the files of an SSH device beside its session', async ({ page }) => {
    test.skip(typeof sshd === 'string', `no private sshd here: ${sshd as string}`);
    const daemon = sshd as Sshd;
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });

    await page.getByRole('button', { name: /FILESRV/ }).first().click();
    await expect(page.getByRole('tab', { name: /FILESRV/ })).toBeVisible();

    await page.getByRole('button', { name: 'Files on this device' }).click();
    const files = page.getByRole('complementary', { name: /Files on FILESRV/ });
    await expect(files).toBeVisible();
    await expect(files.getByRole('button', { name: new RegExp(daemon.file) })).toBeVisible();

    // Asking for a copy says where it would land, on which machine.
    await files.getByRole('button', { name: new RegExp(daemon.file) }).click();
    const ask = page.getByRole('dialog', { name: new RegExp(`Download ${daemon.file}`) });
    await expect(ask).toBeVisible();
    await expect(ask.getByText(`Remote: ${daemon.home}/${daemon.file}`)).toBeVisible();
    await ask.getByRole('button', { name: 'Cancel' }).click();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('says when the far end hangs up, and dials again on request', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // The device drops the connection, as a reboot would.
    engine.device.hangUp();
    await expectOnScreen(page, 'session closed');
    await expect(page.getByRole('tab', { name: /Closed/ })).toBeVisible();

    await page.getByRole('button', { name: 'Reconnect this session' }).click();
    await expectOnScreen(page, 'FakeOS 1.0 console');
    await expect
      .poll(async () => {
        const sessions = await engine.rpc<{ state: string }[]>('terminal.list');
        return sessions.map((session) => session.state);
      })
      .toEqual(['open']);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('asks before closing a live session, and closes it in the engine', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    await page.getByRole('tab', { name: /SW1-LAB/ }).hover();
    await page.getByRole('button', { name: 'Close SW1-LAB' }).first().click();
    const ask = page.getByRole('dialog', { name: /Close SW1-LAB/ });
    await expect(ask).toBeVisible();
    await ask.getByRole('button', { name: 'Close session' }).click();

    await expect(page.getByText('Connect to a device')).toBeVisible();
    await expect.poll(async () => (await engine.rpc<unknown[]>('terminal.list')).length).toBe(0);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------------- snippets, second version

  /** Drags across the row that holds `text`, as a person selecting a line would. */
  async function selectLine(page: Page, text: string): Promise<void> {
    const line = page.locator('.xterm-rows:visible > div', { hasText: text }).first();
    const box = await line.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move(box!.x + 2, box!.y + box!.height / 2);
    await page.mouse.down();
    await page.mouse.move(box!.x + box!.width - 2, box!.y + box!.height / 2, { steps: 8 });
    await page.mouse.up();
  }

  interface AuditEntry {
    action: string;
    message: string;
    detail: unknown;
  }

  async function audit(action: string): Promise<AuditEntry[]> {
    const found = await engine.rpc<{ entries: AuditEntry[] }>('audit.search', { action, limit: 100 });
    return found.entries;
  }

  test('fills in a snippet’s variables and types only the text that was shown and confirmed', async ({ page }) => {
    const console = watchConsole(page);
    const saved = await engine.rpc<{ id: string }>('terminal.snippets.save', {
      name: 'Say twice',
      text: 'say {{what}} on {{port=Gi0/1}}\nsay done\n',
      tags: ['ports'],
    });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    const sentBefore = (await audit('terminal.snippet.send')).length;

    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });

    // The tag narrows the list to the snippets that carry it.
    await drawer.getByRole('group', { name: 'Filter by tag' }).getByRole('button', { name: /^ports/ }).click();
    await expect(drawer.getByRole('list', { name: 'Snippets' }).getByRole('listitem')).toHaveCount(1);

    // A snippet with variables opens its form instead of being typed.
    await drawer.getByRole('button', { name: /Say twice/ }).first().click();
    const form = drawer.getByTestId('snippet-use');
    await expect(form).toBeVisible();
    // The default is on screen, the variable without one is marked and empty.
    await expect(form.getByLabel('port')).toHaveValue('Gi0/1');
    await expect(form.getByLabel(/^what/)).toHaveValue('');
    await expect(form.getByLabel(/^what/)).toHaveAttribute('required', '');
    await expect(form.getByTestId('snippet-nothing-yet')).toContainText('what needs a value');
    const send = form.getByRole('button', { name: /^Type i/ });
    await expect(send).toBeDisabled();

    // A value the engine will not type is refused in the engine's words,
    // beside the field it is about. The form itself filters nothing.
    await form.getByLabel(/^what/).fill('up\tlink');
    await expect(form.getByText('The value of `what` contains a control character (U+0009).')).toBeVisible();
    await expect(send).toBeDisabled();

    // With a value it takes, the exact text is on screen before anything else.
    await form.getByLabel(/^what/).fill('HELLO');
    const exact = form.getByRole('region', { name: 'What will be typed' });
    await expect(exact).toContainText('say HELLO on Gi0/1');
    await expect(exact).toContainText('say done');
    await expect(exact).toContainText('2 lines, exactly as shown');
    await expect(send).toHaveText(/Type into SW1-LAB/);
    expect(engine.device.said).toEqual([]);

    // Two lines are worth a question, on the sheet a paste gets.
    await send.click();
    const sheet = page.getByRole('dialog', { name: 'Send this snippet into SW1-LAB?' });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole('listitem')).toHaveCount(2);
    await page.waitForTimeout(500);
    expect(engine.device.said).toEqual([]);

    // No is no: nothing is typed, and the form is still there.
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    await expect(form).toBeVisible();
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('HELLO')).toBeNull();

    // Yes types it once: one Enter per line break, and none after the last line.
    await send.click();
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expectOnScreen(page, '[HELLO on Gi0/1]');
    await expectOnScreen(page, 'SW1> say done');
    await expect.poll(() => engine.device.receivedWith('HELLO')).toBe('say HELLO on Gi0/1\rsay done');
    expect(engine.device.said).toEqual(['say HELLO on Gi0/1']);

    // The engine typed it, so there is a record of what went where — and
    // the record holds neither the text nor the value.
    const entries = await audit('terminal.snippet.send');
    expect(entries).toHaveLength(sentBefore + 1);
    const record = JSON.stringify(entries[0]);
    expect(record).toContain(saved.id);
    expect(record).not.toContain('HELLO');

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('shows the variant the engine chose for a session, and sends to the sessions that are ticked', async ({
    page,
  }) => {
    const console = watchConsole(page);
    await engine.rpc('devices.create', {
      name: 'SW3-IOS',
      address: '127.0.0.1',
      port: engine.device.port,
      transport: 'telnet',
      profile: 'cisco_ios',
    });
    await engine.rpc('terminal.snippets.save', { name: 'Bounce', text: 'say generic bounce {{port}}' });
    await engine.rpc('terminal.snippets.save', { name: 'Bounce', text: 'say ios bounce {{port}}', profile: 'cisco_ios' });
    await engine.rpc('terminal.snippets.save', { name: 'Both', text: 'say {{word}}' });

    await openApp(page, '/terminal', { backend: engine.backend });
    await page.getByRole('button', { name: /SW3-IOS/ }).first().click();
    await expect(page.getByRole('tab', { name: /SW3-IOS/ })).toBeVisible();
    await expectOnScreen(page, 'FakeOS 1.0 console');

    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await drawer.getByLabel('Search snippets').fill('Bounce');

    // For a cisco_ios session the engine picks the cisco_ios variant; the
    // one for any device is not gone, it is one click away with the reason.
    const offered = drawer.getByRole('list', { name: 'Snippets' });
    await expect(offered.getByRole('listitem')).toHaveCount(1);
    await expect(offered).toContainText('say ios bounce');
    await expect(drawer.getByRole('list', { name: 'Other variants' })).toHaveCount(0);
    await drawer.getByRole('button', { name: 'Show 1 other variant' }).click();
    const others = drawer.getByRole('list', { name: 'Other variants' });
    await expect(others).toContainText('say generic bounce');
    await expect(others).toContainText('Not used in this session: the cisco_ios variant is.');
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();

    // A second session, to a device whose type nothing knows. Now that it is
    // in front, the same question has another answer — the engine's, again.
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Snippets' }).click();
    await drawer.getByLabel('Search snippets').fill('Bounce');
    await expect(offered).toContainText('say generic bounce');
    await drawer.getByRole('button', { name: 'Show 1 other variant' }).click();
    await expect(others).toContainText('This session’s device type is not known');

    // One snippet into both sessions: ticked, asked about once, typed into each.
    await drawer.getByLabel('Search snippets').fill('Both');
    await drawer.getByRole('button', { name: /Both/ }).first().click();
    const form = drawer.getByTestId('snippet-use');
    await form.getByLabel(/^word/).fill('TO-TWO');
    await expect(form.getByRole('checkbox', { name: 'SW1-LAB' })).toBeChecked();
    await expect(form.getByRole('checkbox', { name: 'SW3-IOS' })).not.toBeChecked();
    await form.getByRole('checkbox', { name: 'SW3-IOS' }).check();
    await form.getByRole('button', { name: 'Type into 2 sessions' }).click();

    const sheet = page.getByRole('dialog', { name: 'Send this snippet into 2 sessions?' });
    await expect(sheet).toBeVisible();
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('TO-TWO')).toBeNull();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();

    // Typed, not run, in both — and nowhere twice.
    await expect
      .poll(async () => {
        const sessions = await engine.rpc<{ id: string }[]>('terminal.list');
        const texts = await Promise.all(
          sessions.map((session) => engine.rpc<{ text: string }>('terminal.snapshot', { session_id: session.id })),
        );
        return texts.map((snapshot) => occurrences(snapshot.text, 'say TO-TWO'));
      })
      .toEqual([1, 1]);
    expect(engine.device.said).toEqual([]);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('imports snippets only after showing what would change, and exports them as a file', async ({ page }) => {
    const console = watchConsole(page);
    await engine.rpc('terminal.snippets.save', { name: 'Uptime', text: 'show uptime' });
    await openApp(page, '/terminal', { backend: engine.backend });
    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });

    // Out: a file with the library as people wrote it.
    await drawer.getByRole('button', { name: 'Import and export snippets' }).click();
    const downloading = page.waitForEvent('download');
    await page.getByRole('menuitem', { name: 'Export all' }).click();
    const download = await downloading;
    expect(download.suggestedFilename()).toMatch(/^nocturne-snippets-\d{8}\.json$/);
    const exported = JSON.parse(await readFile(await download.path(), 'utf8')) as {
      format: string;
      snippets: { name: string; text: string }[];
    };
    expect(exported.format).toBe('nocturne.snippets/1');
    expect(exported.snippets).toContainEqual(expect.objectContaining({ name: 'Uptime', text: 'show uptime' }));

    // In: a document with one entry the engine will not take.
    const incoming = {
      format: 'nocturne.snippets/1',
      snippets: [
        { name: 'Uptime', text: 'show uptime detail' },
        { name: 'Imported clock', text: 'show clock' },
        { name: 'Broken', text: 'show {{' },
      ],
    };
    await drawer.getByRole('button', { name: 'Import and export snippets' }).click();
    await page.getByRole('menuitem', { name: 'Import…' }).click();
    const form = drawer.getByTestId('snippet-import');
    const apply = form.getByRole('button', { name: 'Import', exact: true });
    await expect(apply).toBeDisabled();
    await form.getByLabel('Document').fill(JSON.stringify(incoming));
    // The option is chosen by its words: the design system draws its own dot over the input.
    await form.getByText('Keep both', { exact: true }).click();
    await expect(form.getByRole('radio', { name: /Keep both/ })).toBeChecked();
    await form.getByRole('button', { name: 'Check the document' }).click();

    // The bad entry is named with the engine's reason, and nothing can be imported.
    await expect(form.getByTestId('import-summary')).toContainText('1 entry cannot be imported');
    const problems = form.getByRole('list', { name: 'Entries that cannot be imported' });
    await expect(problems).toContainText('Entry 3 · Broken');
    await expect(problems).toContainText('opens a variable that is not closed');
    await expect(apply).toBeDisabled();

    // Without it the dry run says what would happen — and has written nothing.
    incoming.snippets.pop();
    await form.getByLabel('Document').fill(JSON.stringify(incoming));
    // A check is about one document: changing it takes the old answer away.
    await expect(form.getByTestId('import-summary')).toHaveCount(0);
    await form.getByRole('button', { name: 'Check the document' }).click();
    await expect(form.getByTestId('import-summary')).toHaveText('2 snippets would be added (1 under a new name).');
    const names = async () =>
      (await engine.rpc<{ name: string }[]>('terminal.snippets.list')).map((snippet) => snippet.name);
    expect(await names()).not.toContain('Imported clock');
    expect(await names()).not.toContain('Uptime (2)');

    await apply.click();
    await expect(form.getByTestId('import-summary')).toHaveText('2 snippets were added (1 under a new name).');
    expect(await names()).toEqual(expect.arrayContaining(['Uptime', 'Uptime (2)', 'Imported clock']));

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------------------------ session logs

  test('reads a session log page by page with its secrets hidden, until asked and recorded', async ({ page }) => {
    const console = watchConsole(page);
    // Logs that earlier tests left on disk are not what this one is about.
    // No session is open (see `beforeEach`), so none of them is being written.
    const left = await engine.rpc<{ name: string }[]>('terminal.logs.list', {});
    if (left.length > 0) await engine.rpc('terminal.logs.delete', { names: left.map((log) => log.name) });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Log this session to disk' }).click();
    await expect(page.getByText('LOG', { exact: true })).toBeVisible();
    await typeInTerminal(page, 'show secret\r');
    await expectOnScreen(page, SECRETS.lines[1]);
    // Enough for more than two pages of the log.
    await typeInTerminal(page, 'lines 2500\r');
    await expectOnScreen(page, 'line 02499');

    const [session] = await engine.rpc<{ id: string; logging: string }[]>('terminal.list');
    const name = session!.logging;
    await expect
      .poll(async () => {
        const size = (await engine.rpc<{ size: number }>('terminal.logs.read', { name, limit: 1 })).size;
        const tail = await engine.rpc<{ text: string }>('terminal.logs.read', { name, offset: Math.max(0, size - 4096) });
        return tail.text.includes('line 02499');
      })
      .toBe(true);
    const viewedBefore = (await audit('terminal.view_unredacted')).length;

    await page.getByRole('button', { name: 'Session logs' }).click();
    const drawer = page.getByRole('dialog', { name: 'Session logs' });
    const row = drawer.getByRole('list', { name: 'Session logs' }).getByRole('button', { name: /SW1-LAB/ });
    await expect(row).toContainText('being written');
    await expect(drawer.getByTestId('logs-coverage')).toContainText('log');
    await row.click();

    // The first page: where it is in the file, and nothing a device kept secret.
    const reader = drawer.getByTestId('log-reader');
    const range = reader.getByTestId('log-range');
    await expect(range).toContainText('Bytes 0 to');
    await expect(range).toContainText('the start of the log');
    await expect(reader).toContainText('log started: SW1-LAB');
    await expect(reader).toContainText('enable secret <secret:');
    for (const secret of SECRETS.values) await expect(reader).not.toContainText(secret);
    await expect(reader).toContainText('Secrets on this page are replaced.');

    // Forward: the next page starts exactly where this one ended.
    const first = await engine.rpc<{ next_offset: number; size: number }>('terminal.logs.read', {
      name,
      offset: 0,
      limit: 65_536,
    });
    await reader.getByRole('button', { name: 'The page after' }).click();
    await expect(range).toHaveAttribute('data-offset', String(first.next_offset));
    await expect(reader).not.toContainText('log started');

    // The end, and the page before it: adjacent, with no line shown twice.
    await reader.getByRole('button', { name: 'The end of the log' }).click();
    await expect(range).toContainText('the end of the log');
    await expect(reader).toContainText('line 02499');
    const lastStart = await range.getAttribute('data-offset');
    const shownLast = (await reader.locator('pre').textContent()) ?? '';
    await reader.getByRole('button', { name: 'The page before' }).click();
    await expect(range).toHaveAttribute('data-next', lastStart!);
    const shownBefore = (await reader.locator('pre').textContent()) ?? '';
    // Every line of a log starts with its own timestamp and number, so a
    // line that was on both pages would be found.
    const lastLineBefore = shownBefore.trimEnd().split('\n').at(-1)!;
    expect(lastLineBefore).toMatch(/line \d{5}/);
    expect(shownLast).not.toContain(lastLineBefore);

    // Unredacted is asked for, and the asking says it will be recorded —
    // before any such read is made.
    await reader.getByRole('button', { name: 'The start of the log' }).click();
    await expect(range).toContainText('the start of the log');
    await reader.getByRole('button', { name: 'Show unredacted…' }).click();
    const ask = page.getByRole('dialog', { name: /Read SW1-LAB without redaction\?/ });
    await expect(ask).toContainText('records every page read this way in the audit trail');
    expect(await audit('terminal.view_unredacted')).toHaveLength(viewedBefore);
    await page.keyboard.press('Escape');
    await expect(ask).toBeHidden();
    for (const secret of SECRETS.values) await expect(reader).not.toContainText(secret);
    expect(await audit('terminal.view_unredacted')).toHaveLength(viewedBefore);

    await reader.getByRole('button', { name: 'Show unredacted…' }).click();
    await ask.getByRole('button', { name: 'Show unredacted' }).click();
    await expect(reader.getByTestId('log-raw')).toBeVisible();
    await expect(reader).toContainText(SECRETS.lines[0]);
    const viewed = await audit('terminal.view_unredacted');
    expect(viewed).toHaveLength(viewedBefore + 1);
    expect(viewed[0]!.detail).toMatchObject({ source: 'log', name, offset: 0 });
    expect(JSON.stringify(viewed[0])).not.toContain(SECRETS.values[0]);

    await reader.getByRole('button', { name: 'Hide secrets again' }).click();
    await expect(reader.getByTestId('log-raw')).toBeHidden();
    await expect(reader).toContainText('enable secret <secret:');
    for (const secret of SECRETS.values) await expect(reader).not.toContainText(secret);

    // "Keep these lines": the engine reads the page again itself, redacted.
    await reader.getByRole('button', { name: 'Keep these lines as evidence…' }).click();
    const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
    await expect(keep.getByTestId('keep-preview')).toContainText('enable secret <secret:');
    await expect(keep.getByTestId('keep-preview')).not.toContainText(SECRETS.values[0]);
    await keep.getByRole('button', { name: 'Keep', exact: true }).click();
    await expect(keep).toBeHidden();
    await expect(page.getByRole('status').filter({ hasText: 'Kept as evidence' })).toBeVisible();
    const kept = await engine.rpc<{ items: { id: string; kind: string; source: { from: string; params: unknown } }[] }>(
      'evidence.list',
      { kind: 'log' },
    );
    expect(kept.items[0]).toMatchObject({
      kind: 'log',
      source: { from: 'terminal.logs.read', params: { name, offset: 0, limit: first.next_offset } },
    });
    const stored = await engine.rpc<{ body: { text: string } }>('evidence.get', { id: kept.items[0]!.id });
    expect(stored.body.text).toContain('log started: SW1-LAB');
    for (const secret of SECRETS.values) expect(stored.body.text).not.toContain(secret);

    // A log that a session is still writing is not deleted: the engine says
    // why, and the log stays.
    await reader.getByRole('button', { name: 'Delete the log of SW1-LAB' }).click();
    const confirmDelete = page.getByRole('dialog', { name: 'Delete the log of SW1-LAB?' });
    await confirmDelete.getByRole('button', { name: 'Delete log' }).click();
    await expect(reader.getByTestId('log-refused')).toContainText('is being written by an open session');
    expect((await engine.rpc<unknown[]>('terminal.logs.list', {})).length).toBe(1);

    await engine.rpc('terminal.logging', { session_id: session!.id, enabled: false });
    await reader.getByRole('button', { name: 'Delete the log of SW1-LAB' }).click();
    await confirmDelete.getByRole('button', { name: 'Delete log' }).click();
    await expect(drawer.getByText('No session has been logged')).toBeVisible();
    expect(await engine.rpc<unknown[]>('terminal.logs.list', {})).toEqual([]);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ---------------------------------------------------------------- capture

  test('keeps the last lines as evidence the engine read itself, and a selection as the operator’s', async ({
    page,
  }) => {
    const console = watchConsole(page);
    const incident = await engine.rpc<{ id: string }>('incidents.create', {
      title: 'SW1 uplink flapping',
      severity: 'sev3',
    });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'show secret\r');
    await expectOnScreen(page, SECRETS.lines[1]);
    await typeInTerminal(page, 'say KEEP-THIS\r');
    await expectOnScreen(page, '[KEEP-THIS]');
    const [session] = await engine.rpc<{ id: string }[]>('terminal.list');

    // The last lines: what the dialog shows is the engine's redacted read.
    await page.getByRole('button', { name: 'Capture' }).click();
    await page.getByRole('menuitem', { name: 'Keep the last 200 lines as evidence…' }).click();
    const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
    const preview = keep.getByTestId('keep-preview');
    await expect(preview).toContainText('[KEEP-THIS]');
    await expect(preview).toContainText('FakeOS 1.0 console');
    await expect(preview).toContainText('enable secret <secret:');
    for (const secret of SECRETS.values) await expect(preview).not.toContainText(secret);

    // Fewer lines: the engine is asked again, and the item will hold those.
    await keep.getByLabel('Lines from the end').fill('3');
    await expect(preview).not.toContainText('FakeOS 1.0 console');
    await expect(preview).toContainText('[KEEP-THIS]');

    // The open incidents are on offer.
    await keep.getByLabel('Attach to an incident').selectOption({ label: 'SW1 uplink flapping' });
    await keep.getByLabel('Title').fill('SW1 after the flap');
    await keep.getByRole('button', { name: 'Keep and attach' }).click();
    await expect(keep).toBeHidden();
    await expect(page.getByRole('status').filter({ hasText: 'Kept as evidence and attached' })).toBeVisible();

    const terminalItems = await engine.rpc<{
      items: { id: string; title: string; lines: number; incident_ids: string[]; source: Record<string, unknown> }[];
    }>('evidence.list', { kind: 'terminal' });
    const fromSession = terminalItems.items.find((item) => item.title === 'SW1 after the flap');
    expect(fromSession).toMatchObject({
      lines: 3,
      incident_ids: [incident.id],
      source: { from: 'terminal.snapshot', session_id: session!.id, params: { lines: 3 } },
    });

    // The selection: text this window holds, said to be the operator's
    // before it is sent and wherever it is shown afterwards.
    await selectLine(page, SECRETS.lines[0]);
    await page.getByRole('button', { name: 'Keep', exact: true }).click();
    await expect(keep).toBeVisible();
    await expect(keep.getByTestId('keep-preview')).toContainText('Supplied by the operator');
    await expect(keep.getByTestId('keep-preview')).toContainText(SECRETS.lines[0]);
    await expect(keep.getByLabel('Title')).toHaveValue('Selection from SW1-LAB');
    await keep.getByRole('button', { name: 'Keep', exact: true }).click();

    // What was stored is shown, as the engine redacted it on arrival.
    const stored = page.getByRole('dialog', { name: 'Kept as evidence' });
    await expect(stored.getByTestId('keep-stored')).toContainText('enable secret <secret:');
    await expect(stored.getByTestId('keep-stored')).not.toContainText(SECRETS.values[0]);
    const textItems = await engine.rpc<{ items: { id: string; source: Record<string, unknown> }[] }>('evidence.list', {
      kind: 'text',
    });
    expect(textItems.items[0]).toMatchObject({ source: { from: 'operator', session_id: session!.id } });

    // And it can be thrown away from the same place.
    await stored.getByRole('button', { name: 'Delete it' }).click();
    await expect(stored).toBeHidden();
    expect((await engine.rpc<{ items: unknown[] }>('evidence.list', { kind: 'text' })).items).toEqual([]);

    // The same two from the context menu.
    await page.locator('.xterm-screen:visible').first().click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Keep the selection as evidence…' })).toBeVisible();
    await page.getByRole('menuitem', { name: 'Keep the last lines as evidence…' }).click();
    await expect(keep.getByLabel('Lines from the end')).toHaveValue('200');
    await keep.getByRole('button', { name: 'Cancel' }).click();

    // "Save the last lines": the file is the text the dialog showed.
    await page.getByRole('button', { name: 'Capture' }).click();
    await page.getByRole('menuitem', { name: 'Save the last lines as a file…' }).click();
    const save = page.getByRole('dialog', { name: 'Save the last lines of SW1-LAB' });
    await save.getByLabel('Lines from the end').fill('4');
    const written = save.getByTestId('save-preview');
    await expect(written).toContainText('[KEEP-THIS]');
    await expect(written).not.toContainText('FakeOS 1.0 console');
    await expect(written).toContainText('4 lines');
    const downloading = page.waitForEvent('download');
    await save.getByRole('button', { name: 'Save as a file' }).click();
    const file = await downloading;
    expect(file.suggestedFilename()).toMatch(/^SW1-LAB-last-4-lines-\d{8}-\d{6}\.txt$/);
    const saved = await readFile(await file.path(), 'utf8');
    expect(saved).toContain('[KEEP-THIS]');
    expect(saved.trimEnd().split('\n')).toHaveLength(4);
    for (const secret of SECRETS.values) expect(saved).not.toContain(secret);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------------------------------ clips

  test('remembers what the terminal’s Copy copied, in memory only, and types a clip in through the question', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-write']);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'say CLIP-ONE\r');
    await expectOnScreen(page, '[CLIP-ONE]');
    await expect(page.getByRole('button', { name: 'Clips', exact: true })).toBeVisible();

    // Two rows, so that typing it back is worth a question.
    const first = await page.locator('.xterm-rows:visible > div', { hasText: 'SW1> say CLIP-ONE' }).first().boundingBox();
    const second = await page.locator('.xterm-rows:visible > div', { hasText: '[CLIP-ONE]' }).first().boundingBox();
    await page.mouse.move(first!.x + 2, first!.y + first!.height / 2);
    await page.mouse.down();
    await page.mouse.move(second!.x + 120, second!.y + second!.height / 2, { steps: 8 });
    await page.mouse.up();
    await page.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Copied 2 lines' })).toBeVisible();

    // It is under Clips, named for where it came from…
    await page.getByRole('button', { name: 'Clips, 1' }).click();
    const clips = page.getByRole('dialog', { name: 'Clips' });
    await expect(clips).toContainText('Selection from SW1-LAB');
    await expect(clips).toContainText('SW1> say CLIP-ONE');
    await expect(clips).toContainText('2 lines');

    // …and nowhere a reload would find it.
    const persisted = await page.evaluate(async () => {
      const databases = 'databases' in indexedDB ? await indexedDB.databases() : [];
      return JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, databases });
    });
    expect(persisted).not.toContain('CLIP-ONE');

    // Typing it into the session is a paste by another door: asked first.
    engine.device.forget();
    await clips.getByRole('button', { name: /^Type “Selection from SW1-LAB” into SW1-LAB/ }).click();
    const sheet = page.getByRole('dialog', { name: 'Type this clip into SW1-LAB?' });
    await expect(sheet).toBeVisible();
    await page.waitForTimeout(400);
    expect(engine.device.executed).toEqual([]);
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    await page.waitForTimeout(300);
    expect(engine.device.executed).toEqual([]);

    await page.getByRole('button', { name: 'Clips, 1' }).click();
    await clips.getByRole('button', { name: /^Type “Selection from SW1-LAB” into SW1-LAB/ }).click();
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    // The first line ran once (the console does not know it); the second is at the prompt.
    await expect.poll(() => engine.device.executed).toEqual(['SW1> say CLIP-ONE']);

    // A clip can become a snippet, or evidence that says who supplied it.
    await page.getByRole('button', { name: 'Clips, 1' }).click();
    await clips.getByRole('button', { name: /^Save “Selection from SW1-LAB” as a snippet/ }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await expect(drawer.getByLabel('Commands')).toHaveValue(/SW1> say CLIP-ONE\n\[CLIP-ONE\]/);
    await drawer.getByRole('button', { name: 'Cancel' }).click();
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();

    await page.getByRole('button', { name: 'Clips, 1' }).click();
    await clips.getByRole('button', { name: /^Keep “Selection from SW1-LAB” as evidence/ }).click();
    const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
    await expect(keep.getByTestId('keep-preview')).toContainText('Supplied by the operator');
    await expect(keep.getByTestId('keep-preview')).toContainText('[CLIP-ONE]');
    await keep.getByRole('button', { name: 'Cancel' }).click();

    // Clear empties it; a reload would have too.
    await page.getByRole('button', { name: 'Clips, 1' }).click();
    await clips.getByRole('button', { name: 'Clear' }).click();
    await expect(clips).toContainText('Nothing has been copied');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Clips', exact: true })).toBeVisible();

    await selectLine(page, '[CLIP-ONE]');
    await page.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Clips, 1' })).toBeVisible();
    await page.reload();
    await pageLoaded(page);
    await expectOnScreen(page, '[CLIP-ONE]');
    await expect(page.getByRole('button', { name: 'Clips', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Clips, 1' })).toHaveCount(0);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ----------------------------------------------------------- an older build

  test('draws no control for what the engine does not have, and types a snippet the old way', async ({ page }) => {
    const console = watchConsole(page);
    await engine.rpc('terminal.snippets.save', { name: 'Old road', text: 'show version' });
    // The engine's own list of methods with the newer ones taken out: the
    // only thing changed, and the one thing a build without them differs in.
    const withheld = (method: string) =>
      method.startsWith('evidence.') ||
      method.startsWith('terminal.logs.') ||
      method === 'terminal.snapshot' ||
      ['render', 'send', 'export', 'import'].some((name) => method === `terminal.snippets.${name}`);
    const calls = await withholdMethods(page, withheld);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await typeInTerminal(page, 'say OLD-BUILD\r');
    await expectOnScreen(page, '[OLD-BUILD]');

    // Nothing to capture with, no logs to read: no buttons for either.
    await expect(page.getByRole('button', { name: 'Snippets' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Capture' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Session logs' })).toHaveCount(0);
    await selectLine(page, '[OLD-BUILD]');
    await expect(page.getByRole('button', { name: 'Copy', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Keep', exact: true })).toHaveCount(0);
    await page.locator('.xterm-screen:visible').first().click({ button: 'right' });
    await expect(page.getByRole('menuitem', { name: 'Copy' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: /evidence|Save the last/ })).toHaveCount(0);
    await page.keyboard.press('Escape');

    // The drawer is the one it was: no import, no export, no form — and a
    // click types the snippet down the paste funnel.
    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await expect(drawer.getByRole('button', { name: /Old road/ }).first()).toBeVisible();
    await expect(drawer.getByRole('button', { name: 'Import and export snippets' })).toHaveCount(0);
    await expect(drawer.getByRole('button', { name: /Choose where to send/ })).toHaveCount(0);
    await drawer.getByRole('button', { name: /Old road/ }).first().click();
    await expectOnScreen(page, 'SW1> show version');
    expect(await screenText(page)).not.toContain('FakeOS Software, Version 1.0');

    // A clip can still be typed and saved as a snippet; keeping it needs evidence.
    await page.getByRole('button', { name: 'Clips', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Clips' })).toBeVisible();

    // And none of the withheld methods was so much as tried.
    expect(calls.called.filter(withheld)).toEqual([]);
    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------- nothing reaches a device unconfirmed

  test('types a snippet into every session that was ticked or into none, when one of them ends under the question', async ({
    page,
  }) => {
    const console = watchConsole(page);
    const trio = ['TRIO-A', 'TRIO-B', 'TRIO-C'];
    for (const name of trio) {
      await engine.rpc('devices.create', { name, address: '127.0.0.1', port: engine.device.port, transport: 'telnet' });
    }
    await engine.rpc('terminal.snippets.save', { name: 'Trio', text: 'say {{word}}' });
    await openApp(page, '/terminal', { backend: engine.backend });
    for (const name of trio) {
      await page.getByRole('button', { name: new RegExp(name) }).first().click();
      await expect(page.getByRole('tab', { name: new RegExp(name) })).toBeVisible();
      await expectOnScreen(page, 'FakeOS 1.0 console');
    }
    const sentBefore = (await audit('terminal.snippet.send')).length;

    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await drawer.getByLabel('Search snippets').fill('Trio');
    await drawer.getByRole('button', { name: /Trio/ }).first().click();
    const form = drawer.getByTestId('snippet-use');
    await form.getByLabel(/^word/).fill('TRIO-1');
    // The session in front is ticked; the other two are ticked by hand.
    await expect(form.getByRole('checkbox', { name: 'TRIO-C' })).toBeChecked();
    await form.getByRole('checkbox', { name: 'TRIO-A' }).check();
    await form.getByRole('checkbox', { name: 'TRIO-B' }).check();
    await form.getByRole('button', { name: 'Type into 3 sessions' }).click();

    // More than one session is always worth the question, however harmless the line.
    const three = page.getByRole('dialog', { name: 'Send this snippet into 3 sessions?' });
    await expect(three).toBeVisible();
    await expect(three.getByRole('listitem')).toHaveCount(1);
    await expect(three.getByRole('listitem')).toContainText('say TRIO-1');
    await page.waitForTimeout(400);
    expect(engine.device.everyReceivedWith('TRIO-1')).toEqual([]);

    // One of the three ends while the question is still open…
    const open = await engine.rpc<{ id: string; title: string }[]>('terminal.list');
    await engine.rpc('terminal.close', { session_id: open.find((session) => session.title === 'TRIO-B')!.id });

    // …so the yes that follows reaches none of them, and the screen says why.
    await page.keyboard.press('Enter');
    await expect(three).toBeHidden();
    const refused = form.getByTestId('snippet-refused');
    await expect(refused).toContainText('Nothing was sent.');
    await expect(refused).toContainText('nothing was sent to any session');
    await page.waitForTimeout(400);
    expect(engine.device.everyReceivedWith('TRIO-1')).toEqual([]);
    expect(await audit('terminal.snippet.send')).toHaveLength(sentBefore);

    // The session that ended is no longer on offer. The two that are left
    // are asked about again, and "no" is still no.
    await expect(form.getByRole('checkbox')).toHaveCount(2);
    await form.getByRole('button', { name: 'Type into 2 sessions' }).click();
    const two = page.getByRole('dialog', { name: 'Send this snippet into 2 sessions?' });
    await expect(two).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(two).toBeHidden();
    await page.waitForTimeout(300);
    expect(engine.device.everyReceivedWith('TRIO-1')).toEqual([]);

    // Yes: the line that was shown, once in each of the two, and no Enter after it.
    await form.getByRole('button', { name: 'Type into 2 sessions' }).click();
    await expect(two).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expect.poll(() => engine.device.everyReceivedWith('TRIO-1')).toEqual(['say TRIO-1', 'say TRIO-1']);
    await page.waitForTimeout(400);
    expect(engine.device.everyReceivedWith('TRIO-1')).toEqual(['say TRIO-1', 'say TRIO-1']);
    expect(engine.device.said).toEqual([]);
    expect(await audit('terminal.snippet.send')).toHaveLength(sentBefore + 1);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('types nothing when a snippet was changed after it was shown, and says so', async ({ page }) => {
    const console = watchConsole(page);
    const shifty = await engine.rpc<{ id: string }>('terminal.snippets.save', {
      name: 'Shifty',
      text: 'say {{word}} as shown\nsay second',
    });
    const steady = await engine.rpc<{ id: string }>('terminal.snippets.save', {
      name: 'Steady',
      text: 'say steady as shown\nsay second',
    });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    const sheet = page.getByRole('dialog', { name: 'Send this snippet into SW1-LAB?' });

    // A snippet with variables: shown, asked about — and edited by someone
    // else before the answer.
    await drawer.getByLabel('Search snippets').fill('Shifty');
    await drawer.getByRole('button', { name: /Shifty/ }).first().click();
    const form = drawer.getByTestId('snippet-use');
    const exact = form.getByRole('region', { name: 'What will be typed' });
    await form.getByLabel(/^word/).fill('CHG');
    await expect(exact).toContainText('say CHG as shown');
    await form.getByRole('button', { name: /^Type into SW1-LAB/ }).click();
    await expect(sheet).toContainText('say CHG as shown');
    await engine.rpc('terminal.snippets.save', {
      id: shifty.id,
      name: 'Shifty',
      text: 'say {{word}} as changed\nsay second',
    });

    // The yes is to a text the snippet no longer gives. The engine holds the
    // send to the hash of what was shown, types nothing, and the screen says so.
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    const said = page.getByRole('alert').filter({ hasText: '“Shifty” was not sent' });
    await expect(said).toContainText('no longer renders to the text that was confirmed');
    // What is on screen now is what it gives now, to be confirmed afresh.
    await expect(drawer).toBeVisible();
    await expect(form.getByLabel(/^word/)).toHaveValue('');
    await form.getByLabel(/^word/).fill('CHG');
    await expect(exact).toContainText('say CHG as changed');
    await page.waitForTimeout(400);
    expect(engine.device.receivedWith('CHG')).toBeNull();

    // The same for a snippet without variables, which is sent from its row.
    await form.getByRole('button', { name: 'All snippets' }).click();
    await drawer.getByLabel('Search snippets').fill('Steady');
    await drawer.getByRole('button', { name: /Steady/ }).first().click();
    await expect(sheet).toContainText('say steady as shown');
    await engine.rpc('terminal.snippets.save', { id: steady.id, name: 'Steady', text: 'say steady as changed\nsay second' });
    await page.keyboard.press('Enter');
    await expect(sheet).toBeHidden();
    await expect(page.getByRole('alert').filter({ hasText: '“Steady” was not sent' })).toContainText(
      'no longer renders to the text that was confirmed',
    );
    await page.waitForTimeout(400);
    expect(engine.device.receivedWith('steady')).toBeNull();
    expect(engine.device.said).toEqual([]);
    expect(await audit('terminal.snippet.send')).toEqual(
      expect.not.arrayContaining([expect.objectContaining({ message: expect.stringMatching(/Shifty|Steady/) })]),
    );

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  test('shows the engine’s refusal of a value that carries a second command, and tidies nothing itself', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await engine.rpc('terminal.snippets.save', { name: 'Inject', text: 'say {{what}}' });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Snippets' }).click();
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    await drawer.getByLabel('Search snippets').fill('Inject');
    await drawer.getByRole('button', { name: /Inject/ }).first().click();
    const form = drawer.getByTestId('snippet-use');
    const what = form.getByLabel(/^what/);
    const send = form.getByRole('button', { name: /^Type i/ });
    const exact = form.getByRole('region', { name: 'What will be typed' });

    // Each of these would be a second command, a keystroke or a way out of a
    // bracketed paste if it were typed. The engine refuses every one; what
    // the screen has to do is hand the value over as it arrived and show the
    // refusal — not make the value acceptable by taking something out of it.
    const hostile: { name: string; value: string; held: string; says: string }[] = [
      {
        name: 'a line feed',
        value: 'INJ\nsay PWNED',
        held: 'INJ\nsay PWNED',
        says: 'The value of `what` contains a line break.',
      },
      {
        // A text field gives a carriage return back as a line feed; it is
        // still a line break, and still there.
        name: 'a carriage return',
        value: 'INJ\rsay PWNED',
        held: 'INJ\nsay PWNED',
        says: 'The value of `what` contains a line break.',
      },
      {
        name: 'a line separator',
        value: 'INJ\u2028say PWNED',
        held: 'INJ\u2028say PWNED',
        says: 'The value of `what` contains a line separator (U+2028).',
      },
      {
        name: 'an escape sequence',
        value: 'INJ\u001b[2Jsay PWNED',
        held: 'INJ\u001b[2Jsay PWNED',
        says: 'The value of `what` contains a control character (U+001B).',
      },
      {
        name: 'the marker that ends a bracketed paste',
        value: 'INJ\u001b[201~say PWNED',
        held: 'INJ\u001b[201~say PWNED',
        says: 'The value of `what` contains a control character (U+001B).',
      },
    ];
    for (const { name, value, held, says } of hostile) {
      await what.fill(value);
      await expect(what, name).toHaveValue(held);
      await expect(form.getByText(says, { exact: true }), name).toBeVisible();
      await expect(what, name).toHaveAttribute('aria-invalid', 'true');
      await expect(exact, name).toContainText('the engine will not type this');
      await expect(send, name).toBeDisabled();
      // Enter in the field sends the form — when there is something to send.
      await what.press('Enter');
      await page.waitForTimeout(200);
      await expect(page.getByRole('dialog', { name: /Send this snippet/ }), name).toHaveCount(0);
    }

    // The same through the door a person would use: the clipboard.
    await what.fill('');
    await page.evaluate(() => navigator.clipboard.writeText('INJ\nsay PWNED'));
    await what.focus();
    await page.keyboard.press('Control+V');
    await expect(what).toHaveValue('INJ\nsay PWNED');
    await expect(form.getByText('The value of `what` contains a line break.', { exact: true })).toBeVisible();
    await expect(send).toBeDisabled();

    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('INJ')).toBeNull();

    // A value that looks like a placeholder is not one: it goes in once, as
    // it is, and the text on screen says exactly that.
    await what.fill('{{what}} {{x}}');
    await expect(exact).toContainText('say {{what}} {{x}}');
    await expect(send).toBeEnabled();
    await what.press('Enter');
    // `say` is not a command the policy knows to be read-only, so it is asked about.
    const sheet = page.getByRole('dialog', { name: 'Send this snippet into SW1-LAB?' });
    await expect(sheet.getByRole('listitem')).toHaveCount(1);
    await expect(sheet).toContainText('say {{what}} {{x}}');
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('{{x}}')).toBeNull();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expect.poll(() => engine.device.receivedWith('{{x}}')).toBe('say {{what}} {{x}}');
    expect(engine.device.said).toEqual([]);
    expect(engine.device.receivedWith('PWNED')).toBeNull();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------------- where a copy is kept

  test('keeps a clip only of what a control asked to keep: in memory, told to no one, never read off the clipboard', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    // Writing only. Nothing in this test may read the clipboard, and the
    // page is not given leave to.
    await context.grantPermissions(['clipboard-write']);
    const reads = await watchClipboardReads(page);
    const told = watchSent(page);
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);

    // A control that asks: the terminal's Copy. What is copied here is text
    // the device printed and the page never typed, so that it can be looked
    // for afterwards in everything the page sent.
    await typeInTerminal(page, 'lines 3\r');
    await expectOnScreen(page, 'line 00002');
    await selectLine(page, 'line 00001');
    await page.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Clips, 1' })).toBeVisible();
    const printed = 'the quick brown fox';

    // Controls that do not ask: the toolbox, where what is copied is a secret.
    const tool = page.getByRole('article');
    await goTo(page, '/tools/tools.secret.decode');
    // Cisco type 7 of a word made up for this test.
    await tool
      .getByLabel('The value, or the line it sits in', { exact: true })
      .fill('username admin password 7 081B5D193D160A1B01380907382E3071');
    const decoded = 'Zq7ToolsSecret9';
    await expect(tool.getByText(decoded, { exact: true })).toBeVisible();
    await tool.getByRole('button', { name: 'Copy the decoded secret' }).click();
    await expect(tool.getByRole('button', { name: 'the decoded secret copied' })).toBeVisible();

    await goTo(page, '/tools/tools.password');
    await tool.getByRole('button', { name: 'Generate', exact: true }).click();
    const copyPassword = tool.getByRole('button', { name: 'Copy password 1' });
    await expect(copyPassword).toBeVisible();
    const password =
      (await tool
        .locator('li', { has: page.getByRole('button', { name: 'Copy password 1' }) })
        .locator('code')
        .textContent()) ?? '';
    expect(password.length).toBeGreaterThanOrEqual(12);
    await copyPassword.click();
    await expect(tool.getByRole('button', { name: 'password 1 copied' })).toBeVisible();

    await goTo(page, '/tools/tools.keys.generate');
    await tool.getByRole('button', { name: 'Generate a key pair' }).click();
    const keyText = tool.locator('pre', { hasText: 'BEGIN OPENSSH PRIVATE KEY' });
    await expect(keyText).toBeVisible();
    const keyLine = ((await keyText.textContent()) ?? '').split('\n')[1] ?? '';
    expect(keyLine.length).toBeGreaterThan(40);
    await tool
      .locator('div', { has: page.locator('pre', { hasText: 'BEGIN OPENSSH PRIVATE KEY' }) })
      .last()
      .getByRole('button', { name: 'Copy', exact: true })
      .click();
    await expect(tool.getByText('Copied to the clipboard')).toBeAttached();

    // Back on the Terminal the panel holds the one copy that asked, and
    // nothing of the three that did not.
    const held = [printed, decoded, password, keyLine];
    await goTo(page, '/terminal');
    await page.getByRole('button', { name: 'Clips, 1' }).click();
    const clips = page.getByRole('dialog', { name: 'Clips' });
    await expect(clips.getByRole('listitem')).toHaveCount(1);
    await expect(clips).toContainText(printed);
    for (const secret of [decoded, password, keyLine]) await expect(clips).not.toContainText(secret);
    await page.keyboard.press('Escape');

    // None of the four is anywhere a reload would find it, nor was any of
    // them sent anywhere: the engine printed, decoded and generated them,
    // and the page only ever received them.
    const before = await everythingStored(page);
    for (const text of held) expect(before, text).not.toContain(text);
    for (const text of held) expect(told.sent.join('\n'), text).not.toContain(text);
    expect(told.sent.some((entry) => entry.includes('/api/rpc'))).toBe(true);

    await page.reload();
    await pageLoaded(page);
    await expectOnScreen(page, 'line 00002');
    await page.getByRole('button', { name: 'Clips', exact: true }).click();
    await expect(clips).toContainText('Nothing has been copied');
    await expect(clips.getByRole('listitem')).toHaveCount(0);
    const after = await everythingStored(page);
    for (const text of held) expect(after, text).not.toContain(text);
    for (const text of held) expect(told.sent.join('\n'), text).not.toContain(text);

    // And the clipboard was written to four times and read not once.
    expect(reads.count).toBe(0);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // -------------------------------------------------- what a log can hold

  test('hides a key and a secret in a log, shows markup in it as text, and records each page read as written', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-write']);
    const left = await engine.rpc<{ name: string }[]>('terminal.logs.list', {});
    if (left.length > 0) await engine.rpc('terminal.logs.delete', { names: left.map((log) => log.name) });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Log this session to disk' }).click();
    await expect(page.getByText('LOG', { exact: true })).toBeVisible();
    await typeInTerminal(page, 'show secret\r');
    await expectOnScreen(page, SECRETS.lines[1]);
    await typeInTerminal(page, 'show key\r');
    await expectOnScreen(page, 'END OPENSSH PRIVATE KEY');
    await typeInTerminal(page, 'show markup\r');
    // The start of it: the line is longer than the terminal is wide.
    await expectOnScreen(page, '<img src=x onerror=');
    // Enough for four pages of the log.
    await typeInTerminal(page, 'lines 2500\r');
    await expectOnScreen(page, 'line 02499');
    const ran = () => page.evaluate((flag) => (window as unknown as Record<string, unknown>)[flag], MARKUP.flag);
    // The terminal itself shows what a device prints as characters in cells.
    expect(await ran()).toBeUndefined();

    const [session] = await engine.rpc<{ id: string; logging: string }[]>('terminal.list');
    const name = session!.logging;
    await expect
      .poll(async () => {
        const size = (await engine.rpc<{ size: number }>('terminal.logs.read', { name, limit: 1 })).size;
        const tail = await engine.rpc<{ text: string }>('terminal.logs.read', { name, offset: Math.max(0, size - 4096) });
        return tail.text.includes('line 02499');
      })
      .toBe(true);
    // The log is complete; and one more line is put into the file by hand.
    // The log writer takes escape sequences out, so a log holds them only if
    // something else wrote there — a restored file, another program. What
    // reads it back must not care.
    await engine.rpc('terminal.logging', { session_id: session!.id, enabled: false });
    const byHand =
      '2026-11-23T03:00:00.000+00:00 \u001b[2J\u001b]0;renamed\u0007\u001b[31mBY-HAND\u001b[0m ' +
      `<img src=x onerror="window.${MARKUP.flag}=3">`;
    await appendFile(join(engine.backend.dataDir, 'sessions', name), `${byHand}\n`);

    await page.getByRole('button', { name: 'Session logs' }).click();
    const drawer = page.getByRole('dialog', { name: 'Session logs' });
    await drawer.getByRole('list', { name: 'Session logs' }).getByRole('button', { name: /SW1-LAB/ }).click();
    const reader = drawer.getByTestId('log-reader');
    const range = reader.getByTestId('log-range');
    const text = reader.locator('pre');

    // The secret that fits on a line, and the one that does not: the key's
    // first marker stays to say what was there, and no line of it after that.
    await expect(text).toContainText('enable secret <secret:');
    await expect(text).toContainText(PRIVATE_KEY.lines[0]);
    await expect(text).toContainText('<hidden>');
    for (const hidden of [...SECRETS.values, ...PRIVATE_KEY.body, 'END OPENSSH PRIVATE KEY']) {
      await expect(reader, hidden).not.toContainText(hidden);
    }

    // What the device printed as markup is on the page as the characters it is.
    await expect(text).toContainText(MARKUP.line);
    await expect(text.locator('img, script, b, iframe')).toHaveCount(0);
    await expect(reader.locator('.xterm')).toHaveCount(0);
    expect(await ran()).toBeUndefined();

    // A copy button that did not ask for a clip leaves none, here as anywhere.
    await reader.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(reader.getByText('Copied to the clipboard')).toBeAttached();
    await expect(page.getByRole('button', { name: 'Clips', exact: true })).toBeAttached();
    await expect(page.getByRole('button', { name: /^Clips, / })).toHaveCount(0);

    // The line written by hand, escape sequences and all: text, and inert.
    await reader.getByRole('button', { name: 'The end of the log' }).click();
    await expect(range).toContainText('the end of the log');
    await expect(text).toContainText('BY-HAND');
    await expect(text).toContainText('<img src=x onerror=');
    await expect(text.locator('img')).toHaveCount(0);
    expect(await page.title()).not.toContain('renamed');
    expect(await ran()).toBeUndefined();

    // Reading it as written: asked for once, with a warning, and from then
    // on every page that is read that way is one entry in the audit trail —
    // the read that only finds where a line starts is not one of them.
    await reader.getByRole('button', { name: 'The start of the log' }).click();
    await expect(range).toContainText('the start of the log');
    const viewedBefore = (await audit('terminal.view_unredacted')).length;
    await reader.getByRole('button', { name: 'Show unredacted…' }).click();
    const ask = page.getByRole('dialog', { name: /Read SW1-LAB without redaction\?/ });
    await expect(ask).toContainText('records every page read this way in the audit trail');
    expect(await audit('terminal.view_unredacted')).toHaveLength(viewedBefore);
    await ask.getByRole('button', { name: 'Show unredacted' }).click();
    await expect(reader.getByTestId('log-raw')).toBeVisible();
    await expect(text).toContainText(SECRETS.lines[0]);
    for (const line of PRIVATE_KEY.lines) await expect(text).toContainText(line);
    // What was revealed has no button that would carry it off.
    await expect(reader.getByRole('button', { name: 'Copy', exact: true })).toHaveCount(0);

    const pages: number[] = [0];
    const offsetShown = async () => Number(await range.getAttribute('data-offset'));
    await reader.getByRole('button', { name: 'The page after' }).click();
    await expect(range).not.toHaveAttribute('data-offset', '0');
    pages.push(await offsetShown());
    await reader.getByRole('button', { name: 'The end of the log' }).click();
    await expect(range).toContainText('the end of the log');
    pages.push(await offsetShown());
    const endStart = String(pages.at(-1));
    await reader.getByRole('button', { name: 'The page before' }).click();
    await expect(range).toHaveAttribute('data-next', endStart);
    pages.push(await offsetShown());

    const viewed = await audit('terminal.view_unredacted');
    expect(viewed).toHaveLength(viewedBefore + pages.length);
    const recorded = viewed.slice(0, pages.length).reverse();
    expect(recorded.map((entry) => entry.detail)).toEqual(
      pages.map((offset) => expect.objectContaining({ source: 'log', name, offset })),
    );
    for (const secret of [...SECRETS.values, ...PRIVATE_KEY.body]) {
      expect(JSON.stringify(viewed), secret).not.toContain(secret);
    }

    // Hidden again, paging writes nothing more; and it is for the open log
    // only: opened again, it starts out redacted.
    await reader.getByRole('button', { name: 'Hide secrets again' }).click();
    await expect(reader.getByTestId('log-raw')).toBeHidden();
    await reader.getByRole('button', { name: 'The start of the log' }).click();
    await expect(range).toContainText('the start of the log');
    await expect(text).toContainText('enable secret <secret:');
    await reader.getByRole('button', { name: 'Show unredacted…' }).click();
    await ask.getByRole('button', { name: 'Show unredacted' }).click();
    await expect(reader.getByTestId('log-raw')).toBeVisible();
    await reader.getByRole('button', { name: 'All logs' }).click();
    await drawer.getByRole('list', { name: 'Session logs' }).getByRole('button', { name: /SW1-LAB/ }).click();
    await expect(range).toContainText('the start of the log');
    await expect(reader.getByTestId('log-raw')).toHaveCount(0);
    await expect(text).toContainText('enable secret <secret:');
    for (const secret of SECRETS.values) await expect(reader).not.toContainText(secret);
    expect(await audit('terminal.view_unredacted')).toHaveLength(viewedBefore + pages.length + 1);

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------- whichever methods are missing

  test('draws the controls the engine can answer and no others, whichever of the newer methods are missing', async ({
    page,
    context,
  }) => {
    test.setTimeout(900_000);
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-write']);

    // A finished log to open, and two snippets: one without a variable, one with.
    const logged = await engine.rpc<{ id: string }>('terminal.open', {
      device_id: engine.deviceId,
      cols: 80,
      rows: 24,
      log: true,
    });
    await engine.rpc('terminal.input', { session_id: logged.id, data: 'say LOGGED\r' });
    await expect
      .poll(async () => (await engine.rpc<{ size: number }[]>('terminal.logs.list', {})).some((log) => log.size > 0))
      .toBe(true);
    await engine.rpc('terminal.close', { session_id: logged.id });
    await engine.rpc('terminal.snippets.save', { name: 'Gate plain', text: 'show version' });
    await engine.rpc('terminal.snippets.save', { name: 'Gate filled', text: 'say {{word}}' });

    /** Every control this work added, as it is on screen. */
    interface Drawn {
      /** The toolbar's Logs button. */
      logs: boolean;
      /** The entries of the toolbar's Capture menu; none when there is no menu. */
      capture: string[];
      /** "Keep" beside a selection. */
      keepButton: boolean;
      /** The capture entries of the terminal's context menu. */
      context: string[];
      /** The entries of the drawer's import-and-export menu; none when there is no menu. */
      snippetMenu: string[];
      /** A snippet with a variable opens the form with the engine's render. */
      snippetForm: boolean;
      /** A row offers "choose the sessions it goes to". */
      chooseSessions: boolean;
      /** What can be done with a clip, by the first word of each button. */
      clipActions: string[];
      /** What an open log offers besides reading it. */
      logReader: string[];
    }

    const KEEP_SELECTION = 'Keep the selection as evidence…';
    const KEEP_SCREEN = 'Keep the last screen as evidence…';
    const KEEP_200 = 'Keep the last 200 lines as evidence…';
    const KEEP_LINES = 'Keep the last lines as evidence…';
    const SAVE = 'Save the last lines as a file…';
    const everything: Drawn = {
      logs: true,
      capture: [KEEP_SELECTION, KEEP_SCREEN, KEEP_200, SAVE],
      keepButton: true,
      context: [KEEP_SELECTION, KEEP_LINES, SAVE],
      snippetMenu: ['Import…', 'Export all'],
      snippetForm: true,
      chooseSessions: true,
      clipActions: ['Copy', 'Type', 'Keep', 'Save', 'Forget'],
      logReader: ['keep', 'unredacted', 'delete'],
    };

    const labels = async () =>
      (await page.getByRole('menuitem').allTextContents()).map((label) => label.trim());

    const drawn = async (): Promise<Drawn> => {
      await typeInTerminal(page, 'say GATE\r');
      await expectOnScreen(page, '[GATE]');

      const logs = (await page.getByRole('button', { name: 'Session logs', exact: true }).count()) > 0;
      let capture: string[] = [];
      const captureButton = page.getByRole('button', { name: 'Capture', exact: true });
      if ((await captureButton.count()) > 0) {
        await captureButton.click();
        await expect(page.getByRole('menu')).toBeVisible();
        capture = await labels();
        await page.keyboard.press('Escape');
        await expect(page.getByRole('menu')).toHaveCount(0);
      }

      await selectLine(page, '[GATE]');
      const copy = page.getByRole('button', { name: 'Copy', exact: true });
      await expect(copy).toBeVisible();
      const keepButton = (await page.getByRole('button', { name: 'Keep', exact: true }).count()) > 0;
      // Leaves a clip, for the panel further down.
      await copy.click();
      await page.locator('.xterm-screen:visible').first().click({ button: 'right' });
      await expect(page.getByRole('menuitem', { name: 'Copy' })).toBeVisible();
      const context = (await labels()).filter((label) => /evidence|Save the last/.test(label));
      await page.keyboard.press('Escape');
      await expect(page.getByRole('menu')).toHaveCount(0);

      await page.getByRole('button', { name: 'Snippets' }).click();
      const drawer = page.getByRole('dialog', { name: 'Snippets' });
      await expect(drawer.getByRole('button', { name: /Gate plain/ }).first()).toBeVisible();
      let snippetMenu: string[] = [];
      const menuButton = drawer.getByRole('button', { name: 'Import and export snippets' });
      if ((await menuButton.count()) > 0) {
        await menuButton.click();
        await expect(page.getByRole('menu')).toBeVisible();
        snippetMenu = await labels();
        await page.keyboard.press('Escape');
        await expect(page.getByRole('menu')).toHaveCount(0);
      }
      const chooseSessions = (await drawer.getByRole('button', { name: /^Choose where to send/ }).count()) > 0;
      // A snippet with a variable: the form with the engine's render — or,
      // down the old road, its text as it stands and the paste's question.
      await drawer.getByLabel('Search snippets').fill('Gate filled');
      await drawer.getByRole('button', { name: /Gate filled/ }).first().click();
      const form = drawer.getByTestId('snippet-use');
      const question = page.getByRole('dialog', { name: 'Send this snippet into SW1-LAB?' });
      await expect(form.or(question)).toBeVisible();
      const snippetForm = (await form.count()) > 0;
      // Out of the question or the form, and out of the drawer, typing nothing.
      await page.keyboard.press('Escape');
      if (await drawer.isVisible()) await page.keyboard.press('Escape');
      await expect(drawer).toBeHidden();
      await expect(question).toBeHidden();

      await page.getByRole('button', { name: /^Clips/ }).click();
      const clips = page.getByRole('dialog', { name: 'Clips' });
      await expect(clips.getByRole('listitem')).toHaveCount(1);
      const clipActions = await clips
        .getByRole('listitem')
        .getByRole('button')
        .evaluateAll((buttons) => buttons.map((button) => (button.getAttribute('aria-label') ?? '').split(' ')[0] ?? ''));
      await page.keyboard.press('Escape');
      await expect(clips).toBeHidden();

      const logReader: string[] = [];
      if (logs) {
        await page.getByRole('button', { name: 'Session logs', exact: true }).click();
        const logsDrawer = page.getByRole('dialog', { name: 'Session logs' });
        await logsDrawer.getByRole('list', { name: 'Session logs' }).getByRole('button').first().click();
        const reader = logsDrawer.getByTestId('log-reader');
        await expect(reader.getByTestId('log-range')).toBeVisible();
        if ((await reader.getByRole('button', { name: 'Keep these lines as evidence…' }).count()) > 0) logReader.push('keep');
        if ((await reader.getByRole('button', { name: 'Show unredacted…' }).count()) > 0) logReader.push('unredacted');
        if ((await reader.getByRole('button', { name: /^Delete the log of/ }).count()) > 0) logReader.push('delete');
        await page.keyboard.press('Escape');
        await expect(logsDrawer).toBeHidden();
      }

      return { logs, capture, keepButton, context, snippetMenu, snippetForm, chooseSessions, clipActions, logReader };
    };

    const without = (...names: string[]) => (method: string) =>
      names.some((name) => (name.endsWith('.') ? method.startsWith(name) : method === name));
    const noKeep = everything.clipActions.filter((action) => action !== 'Keep');

    const builds: { name: string; withheld: (method: string) => boolean; differs: Partial<Drawn> }[] = [
      { name: 'every method', withheld: () => false, differs: {} },
      {
        name: 'no terminal.snippets.render',
        withheld: without('terminal.snippets.render'),
        differs: { snippetForm: false, chooseSessions: false },
      },
      {
        name: 'no terminal.snippets.send',
        withheld: without('terminal.snippets.send'),
        differs: { snippetForm: false, chooseSessions: false },
      },
      { name: 'no terminal.snippets.export', withheld: without('terminal.snippets.export'), differs: { snippetMenu: ['Import…'] } },
      { name: 'no terminal.snippets.import', withheld: without('terminal.snippets.import'), differs: { snippetMenu: ['Export all'] } },
      {
        name: 'neither import nor export',
        withheld: without('terminal.snippets.import', 'terminal.snippets.export'),
        differs: { snippetMenu: [] },
      },
      { name: 'no terminal.logs.*', withheld: without('terminal.logs.'), differs: { logs: false, logReader: [] } },
      { name: 'no terminal.logs.read', withheld: without('terminal.logs.read'), differs: { logs: false, logReader: [] } },
      { name: 'no terminal.logs.delete', withheld: without('terminal.logs.delete'), differs: { logReader: ['keep', 'unredacted'] } },
      {
        name: 'no evidence.*',
        withheld: without('evidence.'),
        differs: { capture: [SAVE], keepButton: false, context: [SAVE], clipActions: noKeep, logReader: ['unredacted', 'delete'] },
      },
      {
        name: 'no evidence.add_text',
        withheld: without('evidence.add_text'),
        differs: { capture: [KEEP_SCREEN, KEEP_200, SAVE], keepButton: false, context: [KEEP_LINES, SAVE], clipActions: noKeep },
      },
      {
        name: 'no evidence.capture_terminal',
        withheld: without('evidence.capture_terminal'),
        differs: { capture: [KEEP_SELECTION, SAVE], context: [KEEP_SELECTION, SAVE] },
      },
      { name: 'no evidence.capture_log', withheld: without('evidence.capture_log'), differs: { logReader: ['unredacted', 'delete'] } },
      {
        name: 'no terminal.snapshot',
        withheld: without('terminal.snapshot'),
        differs: { capture: [KEEP_SELECTION, KEEP_SCREEN, KEEP_200], context: [KEEP_SELECTION, KEEP_LINES] },
      },
      {
        name: 'no evidence.* and no terminal.snapshot',
        withheld: without('evidence.', 'terminal.snapshot'),
        differs: { capture: [], keepButton: false, context: [], clipActions: noKeep, logReader: ['unredacted', 'delete'] },
      },
      {
        name: 'none of them',
        withheld: without(
          'evidence.',
          'terminal.logs.',
          'terminal.snapshot',
          'terminal.snippets.render',
          'terminal.snippets.send',
          'terminal.snippets.import',
          'terminal.snippets.export',
        ),
        differs: {
          logs: false,
          capture: [],
          keepButton: false,
          context: [],
          snippetMenu: [],
          snippetForm: false,
          chooseSessions: false,
          clipActions: noKeep,
          logReader: [],
        },
      },
    ];

    for (const build of builds) {
      await test.step(build.name, async () => {
        // A fresh start each time: no session, no tab remembered from the build before.
        for (const session of await engine.rpc<{ id: string }[]>('terminal.list')) {
          await engine.rpc('terminal.close', { session_id: session.id });
        }
        await page.unrouteAll({ behavior: 'wait' });
        const calls = await withholdMethods(page, build.withheld);
        await openApp(page, '/terminal', { backend: engine.backend });
        await page.evaluate(() => window.localStorage.removeItem('nocturne.terminal.layout'));
        await openApp(page, '/terminal', { backend: engine.backend });
        // Nothing is drawn on a guess, so nothing can be counted before the list of methods is in.
        await expect.poll(() => calls.called.includes('app.methods')).toBe(true);
        await openDeviceSession(page);

        expect(await drawn(), build.name).toEqual({ ...everything, ...build.differs });
        // And nothing that was looked at or opened asked the engine for a method it does not have.
        expect(calls.called.filter(build.withheld), build.name).toEqual([]);
      });
    }

    expect(console.messages, engine.backend.log()).toEqual([]);
  });

  // ------------------------------------------------------------ the keyboard

  /**
   * Presses Tab (or Shift+Tab) until `target` has the focus, and fails if it
   * never gets it. Generous: every snippet an earlier test saved is in the
   * drawer, four stops a row.
   */
  async function tabTo(page: Page, target: Locator, back = false): Promise<void> {
    await expect(target).toBeVisible();
    for (let step = 0; step < 250; step += 1) {
      if (await target.evaluate((element) => element === document.activeElement)) return;
      await page.keyboard.press(back ? 'Shift+Tab' : 'Tab');
    }
    throw new Error(`${back ? 'Shift+Tab' : 'Tab'} never reached ${target.toString()}`);
  }

  /**
   * What the element with the focus shows of it: "shown", or what is wrong.
   * The design system draws the ring on the element itself, on the box
   * beside a checkbox or a radio button, on the frame around a text field,
   * and as the highlight of a menu item.
   */
  async function focusRing(page: Page): Promise<string> {
    return page.evaluate(() => {
      const element = document.activeElement;
      if (!(element instanceof HTMLElement) || element === document.body) return 'nothing has the focus';
      const what = `${element.tagName.toLowerCase()} “${(element.getAttribute('aria-label') ?? element.textContent ?? '').trim().slice(0, 40)}”`;
      const drawn = (node: Element | null) => {
        if (!node) return false;
        const style = getComputedStyle(node);
        return (style.outlineStyle !== 'none' && Number.parseFloat(style.outlineWidth) > 0) || style.boxShadow !== 'none';
      };
      if (element.getAttribute('role') === 'menuitem') {
        const menu = element.closest('[role="menu"]');
        const own = getComputedStyle(element).backgroundColor;
        return menu && own !== getComputedStyle(menu).backgroundColor ? 'shown' : `${what} is not highlighted`;
      }
      if (!element.matches(':focus-visible')) return `${what} has the focus without showing it`;
      const shown = drawn(element) || drawn(element.nextElementSibling) || drawn(element.closest('[data-control]'));
      return shown ? 'shown' : `${what} draws no ring`;
    });
  }

  test('can be worked from the keyboard alone, with the focus always in sight and handed back', async ({
    page,
    context,
  }) => {
    const console = watchConsole(page);
    await context.grantPermissions(['clipboard-write']);
    const left = await engine.rpc<{ name: string }[]>('terminal.logs.list', {});
    if (left.length > 0) await engine.rpc('terminal.logs.delete', { names: left.map((log) => log.name) });
    await engine.rpc('terminal.snippets.save', { name: 'Keys pair', text: 'say KEYS-ONE\nsay KEYS-TWO' });
    await engine.rpc('terminal.snippets.save', { name: 'Keys form', text: 'say KEYS {{word}}' });
    await openApp(page, '/terminal', { backend: engine.backend });
    await openDeviceSession(page);
    await page.getByRole('button', { name: 'Log this session to disk' }).click();
    await expect(page.getByText('LOG', { exact: true })).toBeVisible();
    await typeInTerminal(page, 'say KEYBOARD\r');
    await expectOnScreen(page, '[KEYBOARD]');
    // The one thing done with the mouse: xterm has no keyboard selection. The
    // copy itself is the terminal's own chord.
    await selectLine(page, '[KEYBOARD]');
    await page.keyboard.press('Control+Shift+C');
    await expect(page.getByRole('button', { name: 'Clips, 1' })).toBeVisible();
    engine.device.forget();
    const sentBefore = (await audit('terminal.snippet.send')).length;

    const snippetsButton = page.getByRole('button', { name: 'Snippets' });
    const drawer = page.getByRole('dialog', { name: 'Snippets' });
    const sheet = page.getByRole('dialog', { name: 'Send this snippet into SW1-LAB?' });
    // A terminal keeps every key, Tab included, so a walk from the keyboard
    // starts on the toolbar: put there, then moved by a key, as a key would.
    await snippetsButton.focus();
    await page.keyboard.press('Shift+Tab');
    await tabTo(page, snippetsButton);
    expect(await focusRing(page)).toBe('shown');

    // A snippet without variables, from its row: the question, and nothing typed before it or after a no.
    await page.keyboard.press('Enter');
    await expect(drawer).toBeVisible();
    expect(await focusRing(page)).toBe('shown');
    const pair = drawer.getByRole('button', { name: /^Keys pair/ });
    await tabTo(page, pair);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole('listitem')).toHaveCount(2);
    await expect(sheet.getByRole('button', { name: /Send anyway/ })).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    await page.waitForTimeout(400);
    expect(engine.device.receivedWith('KEYS-ONE')).toBeNull();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    // Given back to the row it came from, which can be pressed again at once.
    await expect(pair).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('KEYS-ONE')).toBeNull();
    await page.keyboard.press('Enter');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expect.poll(() => engine.device.receivedWith('KEYS-ONE')).toBe('say KEYS-ONE\rsay KEYS-TWO');
    await page.waitForTimeout(300);
    expect(engine.device.everyReceivedWith('KEYS-ONE')).toEqual(['say KEYS-ONE\rsay KEYS-TWO']);
    expect(engine.device.said).toEqual(['say KEYS-ONE']);

    // A snippet with a variable: the form takes the focus, Enter in a field asks, Escape gives the field back.
    await snippetsButton.focus();
    await page.keyboard.press('Shift+Tab');
    await tabTo(page, snippetsButton);
    await page.keyboard.press('Enter');
    await expect(drawer).toBeVisible();
    await tabTo(page, drawer.getByRole('button', { name: /^Keys form/ }));
    await page.keyboard.press('Enter');
    const form = drawer.getByTestId('snippet-use');
    const word = form.getByLabel(/^word/);
    await expect(word).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.type('HELLO');
    await expect(form.getByRole('region', { name: 'What will be typed' })).toContainText('say KEYS HELLO');
    await page.keyboard.press('Enter');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    await expect(word).toBeFocused();
    await expect(word).toHaveValue('HELLO');
    // The session to send to is a checkbox, and the send a button: both on the way.
    const target = form.getByRole('checkbox', { name: 'SW1-LAB' });
    await tabTo(page, target);
    expect(await focusRing(page)).toBe('shown');
    const typeInto = form.getByRole('button', { name: 'Type into SW1-LAB' });
    await tabTo(page, typeInto);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    await expect(sheet).toBeVisible();
    // The last snippet's last line is still at the prompt, its Enter being the
    // person's: what the console was sent from here on is this one alone.
    engine.device.forget();
    await page.keyboard.press('Enter');
    await expect(drawer).toBeHidden();
    await expect.poll(() => engine.device.receivedWith('KEYS HELLO')).toBe('say KEYS HELLO');
    expect(await audit('terminal.snippet.send')).toHaveLength(sentBefore + 2);

    // Import, from the drawer's menu: the menu gives the focus back to its
    // button as it closes, and that button goes with the list it sat under.
    await snippetsButton.focus();
    await page.keyboard.press('Shift+Tab');
    await tabTo(page, snippetsButton);
    await page.keyboard.press('Enter');
    await expect(drawer).toBeVisible();
    // The footer is the end of the drawer, and the drawer keeps Tab inside
    // itself: back from the top is the short way there.
    await tabTo(page, drawer.getByRole('button', { name: 'Import and export snippets' }), true);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('menuitem', { name: 'Import…' })).toBeFocused();
    await page.keyboard.press('Enter');
    const importing = drawer.getByTestId('snippet-import');
    await expect(importing).toBeVisible();
    await expect(importing.getByRole('button', { name: 'Choose a file' })).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    // A radio group is one stop for Tab, on the option that is chosen; the arrows choose another.
    await tabTo(page, importing.getByRole('radio', { name: /Keep the one here/ }));
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('ArrowDown');
    await expect(importing.getByRole('radio', { name: /Keep both/ })).toBeFocused();
    await expect(importing.getByRole('radio', { name: /Keep both/ })).toBeChecked();
    expect(await focusRing(page)).toBe('shown');
    await tabTo(page, importing.getByRole('button', { name: 'All snippets' }), true);
    await page.keyboard.press('Enter');
    // Nothing was opened from the list in this visit, so the list starts at its search.
    await expect(drawer.getByLabel('Search snippets')).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();
    await expect(snippetsButton).toBeFocused();

    // Capture: the menu takes the focus, and the dialog gives it back to the button.
    const capture = page.getByRole('button', { name: 'Capture', exact: true });
    await capture.focus();
    await page.keyboard.press('Shift+Tab');
    await tabTo(page, capture);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    await expect(page.getByRole('menu')).toBeVisible();
    await expect(page.getByRole('menuitem').first()).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    const lastLines = page.getByRole('menuitem', { name: 'Keep the last 200 lines as evidence…' });
    for (let step = 0; step < 6 && !(await lastLines.evaluate((item) => item === document.activeElement)); step += 1) {
      await page.keyboard.press('ArrowDown');
    }
    await expect(lastLines).toBeFocused();
    await page.keyboard.press('Enter');
    const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
    await expect(keep).toBeVisible();
    await expect(keep.getByTestId('keep-preview')).toContainText('[KEYBOARD]');
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Escape');
    await expect(keep).toBeHidden();
    await expect(capture).toBeFocused();

    // Session logs: the drawer, a log, the question before an unredacted read, and back.
    const logsButton = page.getByRole('button', { name: 'Session logs', exact: true });
    await tabTo(page, logsButton, true);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    const logs = page.getByRole('dialog', { name: 'Session logs' });
    await expect(logs).toBeVisible();
    const logRow = logs.getByRole('list', { name: 'Session logs' }).getByRole('button', { name: /SW1-LAB/ });
    await tabTo(page, logRow);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    const reader = logs.getByTestId('log-reader');
    await expect(reader.getByTestId('log-range')).toBeVisible();
    // Opening a log does not drop the focus out of the drawer.
    expect(await logs.evaluate((element) => element.contains(document.activeElement))).toBe(true);
    const reveal = reader.getByRole('button', { name: 'Show unredacted…' });
    await tabTo(page, reveal);
    expect(await focusRing(page)).toBe('shown');
    const viewedBefore = (await audit('terminal.view_unredacted')).length;
    await page.keyboard.press('Enter');
    const ask = page.getByRole('dialog', { name: /without redaction\?/ });
    await expect(ask).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(ask).toBeHidden();
    await expect(reveal).toBeFocused();
    expect(await audit('terminal.view_unredacted')).toHaveLength(viewedBefore);
    await tabTo(page, reader.getByRole('button', { name: 'All logs' }), true);
    await page.keyboard.press('Enter');
    await expect(logRow).toBeVisible();
    expect(await logs.evaluate((element) => element.contains(document.activeElement))).toBe(true);
    await page.keyboard.press('Escape');
    await expect(logs).toBeHidden();
    await expect(logsButton).toBeFocused();

    // Clips: the panel takes the focus; typing a clip in is the paste's question, and no is no.
    const clipsButton = page.getByRole('button', { name: 'Clips, 1' });
    await tabTo(page, clipsButton);
    expect(await focusRing(page)).toBe('shown');
    await page.keyboard.press('Enter');
    const clips = page.getByRole('dialog', { name: 'Clips' });
    await expect(clips).toBeVisible();
    // Into the panel — it is drawn at the end of the page, so from the button
    // Tab alone would never reach it — and onto the newest clip, not onto
    // Clear, which a second Enter would press.
    await expect(clips.getByRole('button', { name: 'Copy “Selection from SW1-LAB” again' })).toBeFocused();
    expect(await focusRing(page)).toBe('shown');
    const typeClip = clips.getByRole('button', { name: /^Type “Selection from SW1-LAB” into SW1-LAB/ });
    await tabTo(page, typeClip);
    expect(await focusRing(page)).toBe('shown');
    engine.device.forget();
    await page.keyboard.press('Enter');
    const clipSheet = page.getByRole('dialog', { name: 'Type this clip into SW1-LAB?' });
    await expect(clipSheet).toBeVisible();
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('[KEYBOARD]')).toBeNull();
    await page.keyboard.press('Escape');
    await expect(clipSheet).toBeHidden();
    await page.waitForTimeout(300);
    expect(engine.device.receivedWith('[KEYBOARD]')).toBeNull();

    expect(console.messages, engine.backend.log()).toEqual([]);
  });
});
