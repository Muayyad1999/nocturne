/**
 * The Terminal screen, against a real engine and a real device.
 *
 * Nothing here is mocked: `nocturne-server` on a fresh data directory, the
 * production UI bundle it serves, and a console the test itself answers for.
 * What is being proved is the part a terminal lives or dies by — that a
 * session survives leaving the screen and reloading the window, and comes
 * back with every byte, once.
 */

import { expect, test, type Page } from '@playwright/test';
import { goTo, openApp, pageLoaded, watchConsole } from './support/app';
import {
  announceLag,
  loseOutputOnRequest,
  losingOutput,
  openInDesktopShell,
  outputLossSettled,
  setShellClipboard,
  shellRecord,
  startSshd,
  startTerminalEngine,
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
});
