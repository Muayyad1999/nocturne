/**
 * The Automation screen, against a real engine and real (simulated) devices.
 *
 * A `nocturne-server` is started on its own empty data directory and the
 * devices it runs commands on are a Nocturne lab: two 24-port switches and a
 * four-port router, each an inventory device with `transport: sim`, so the
 * runner reaches them exactly as it reaches hardware. That is what makes the
 * comparison test worth having — two switches really do print the same
 * interface table, the router really does print a different one, and the
 * screen really has to group them.
 *
 * A fourth device is a real address nobody answers on, so "could not be
 * reached" is a real outcome rather than a fixture, and a playbook with a
 * `pause` step really holds a device's lease, so "skipped because another run
 * has it" is a real outcome too.
 *
 * Nothing here is mocked. Every test asserts the console is clean.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { goTo, openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = process.env.NOCTURNE_UI_DIST ?? (await mkdtemp(join(tmpdir(), 'nocturne-ui-')));
  if (process.env.NOCTURNE_UI_DIST === undefined) {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  backend = await startBackend({ uiDir });

  const rpc = rpcFor(request, backend);
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'AS1' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'AS2' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'router', name: 'EDGE' });
  await rpc('labs.start', { id: lab.id });
  // A real address with nothing behind it: "could not be reached" is earned.
  await rpc('devices.create', {
    name: 'dead-sw',
    address: '192.0.2.77',
    transport: 'ssh',
    profile: 'cisco_ios',
    site: 'HQ',
    role: 'access',
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) {
    await rm(uiDir, { recursive: true, force: true });
  }
});

// An action that cannot find what it is looking for should say so in seconds,
// not hold the whole suite for the length of the test timeout.
test.beforeEach(({ page }) => {
  page.setDefaultTimeout(20_000);
});

// ------------------------------------------------------------------ helpers

/** Types commands into the composer's editor (CodeMirror answers to `fill`). */
async function compose(page: Page, commands: string): Promise<void> {
  await page.getByRole('textbox', { name: 'Commands, one per line' }).fill(commands);
}

/** Presses Run and answers the confirmation. */
async function pressRun(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^Run \d+ command/ }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Only read-only commands are sent');
  await dialog.getByRole('button', { name: 'Run', exact: true }).click();
}

/** The header of the run that is open: its name, its tally and its actions. */
function runHeader(page: Page): Locator {
  return page.getByRole('region', { name: 'This run' });
}

/** Narrows the composer's filter to one device by name. */
async function only(page: Page, name: string): Promise<void> {
  await page.getByRole('textbox', { name: 'Name, address or tag contains' }).fill(name);
  await expect(page.getByText('1 device right now')).toBeVisible();
}

// -------------------------------------------------------------- the feature

test.describe('running commands across devices', () => {
  test('sends them to everything the filter matches and says what each device did', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation', { backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Automation' })).toBeVisible();
    // The filter is open, so it says what that means: the whole inventory.
    await expect(page.getByText('4 devices right now')).toBeVisible();
    await expect(page.getByText('Nothing is set in the filter')).toBeVisible();

    await compose(page, 'show ip interface brief');
    await pressRun(page);

    // The run opens on its own page, with the tally of the four outcomes.
    await expect(page.getByRole('heading', { level: 2, name: 'show ip interface brief' })).toBeVisible();
    await expect(runHeader(page).getByText('3 answered', { exact: true })).toBeVisible({ timeout: 30_000 });
    await expect(runHeader(page).getByText('1 not reached', { exact: true })).toBeVisible();
    // A plain command run is not an Ansible run: it carries no caveat.
    await expect(page.getByText('No backup, no drift check')).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('groups the answers so the odd one out is obvious', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation', { backend });
    await compose(page, 'show ip interface brief');
    await pressRun(page);
    await expect(runHeader(page).getByText('3 answered', { exact: true })).toBeVisible({ timeout: 30_000 });

    // Two switches print the same interface table; the router does not.
    await expect(page.getByText('2 say one thing, 1 differs')).toBeVisible();

    const majority = page.getByRole('listitem').filter({ hasText: 'Majority' }).first();
    await expect(majority).toContainText('Campus/AS1');
    await expect(majority).toContainText('Campus/AS2');

    const odd = page.getByRole('listitem').filter({ hasText: 'Differs' }).first();
    await expect(odd).toContainText('Campus/EDGE');
    await expect(odd).toContainText('lines differ');
    // The difference itself, as − and + rather than as colour alone.
    await expect(odd.getByText('GigabitEthernet0/0').first()).toBeVisible();
    await expect(odd.getByText('FastEthernet0/1').first()).toBeVisible();

    // The lines that were trimmed are named, so agreement is never a trick.
    await expect(page.getByText(/Lines matching \d+ patterns were left out/)).toBeVisible();

    // And the device with no output is accounted for, with the reason.
    const nothing = page.getByRole('region', { name: 'Devices with no output for this command' });
    await expect(nothing).toContainText('Not reached');
    await expect(nothing).toContainText('dead-sw');

    expect(console.messages).toEqual([]);
  });

  test('a device that was never reached does not read as a failure', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation', { backend });
    await compose(page, 'show version');
    await pressRun(page);
    await expect(runHeader(page).getByText('3 answered', { exact: true })).toBeVisible({ timeout: 30_000 });

    await page.getByRole('tab', { name: 'Devices' }).click();

    const unreached = page.getByRole('region', { name: /^Not reached/ });
    await expect(unreached).toContainText('1 of 4 devices');
    await expect(unreached).toContainText('No session could be opened, so nothing was sent at all');
    await expect(unreached).toContainText('This is not a failed command');
    await expect(unreached).toContainText('dead-sw');
    // The engine's own sentence, not a paraphrase of it.
    await expect(unreached).toContainText('has no credential with a user name');

    const answered = page.getByRole('region', { name: /^Answered/ });
    await expect(answered).toContainText('3 of 4 devices');
    await expect(answered).toContainText('Campus/AS1');
    // Simulated devices are labelled wherever they appear.
    await expect(answered.getByText('LAB').first()).toBeVisible();

    // Nothing failed, so there is no failure group at all.
    await expect(page.getByRole('region', { name: /^Failed/ })).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('a device another run is holding is skipped, and says which run has it', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);

    // A playbook that holds one device's lease for a while: the backend lets
    // one run at a time near a device, and this is how that is reached from
    // outside.
    const holder = await rpc<{ job_id: string }>('playbooks.run', {
      device_ids: ['Campus/AS1'],
      yaml: 'name: Holder\nsteps:\n  - name: Hold the device\n    pause: 20\n',
      name: 'Holder',
    });

    await openApp(page, '/automation', { backend });
    await only(page, 'AS1');
    await compose(page, 'show version');
    await pressRun(page);

    await expect(runHeader(page).getByText('1 skipped', { exact: true })).toBeVisible({ timeout: 30_000 });
    await page.getByRole('tab', { name: 'Devices' }).click();

    const skipped = page.getByRole('region', { name: /^Skipped/ });
    await expect(skipped).toContainText('Nocturne deliberately left these alone');
    await expect(skipped).toContainText('Nothing was sent and nothing failed');
    await expect(skipped).toContainText('is already being worked on by the run');

    // Give the device back, so the tests after this one are not skipped too.
    await rpc('jobs.cancel', { id: holder.job_id });

    expect(console.messages).toEqual([]);
  });

  test('a command that is not read-only is refused before anything is sent', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation', { backend });
    await only(page, 'AS1');
    await compose(page, 'reload');
    await pressRun(page);

    const refusal = page.getByRole('alert');
    await expect(refusal).toContainText('Nothing was sent');
    await expect(refusal).toContainText('is not a read-only command on Cisco IOS');
    await expect(refusal).toContainText('Commands that change a device go through a change');
    // It never left the composer: no run was opened.
    await expect(page.getByRole('heading', { level: 2, name: 'reload' })).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('the history keeps what was run, by what, and the per-device outcome', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation', { backend });

    const runs = page.getByRole('table', { name: 'Runs' });
    // Everything that has been run so far is here, with who started it and
    // what each device did.
    await expect(runs).toContainText('A person');
    await expect(runs).toContainText('answered');
    await expect(runs).toContainText('skipped');
    await expect(runs).toContainText('not reached');

    const target = runs.getByRole('row').filter({ hasText: 'show ip interface brief' }).first();
    await target.click();
    await target.press('Enter');
    await expect(page.getByText('Started by')).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Compare' })).toBeVisible();

    // Export hands the whole run over as text, without touching a device.
    await page.getByRole('button', { name: 'Export…' }).click();
    await page.getByRole('menuitem', { name: 'as CSV' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('device,state,command,error,output');
    await page.keyboard.press('Escape');

    expect(console.messages).toEqual([]);
  });
});

// -------------------------------------------------------------------- tasks

test.describe('tasks', () => {
  test('a task keeps its filter as a filter, and says what it means right now', async ({ page, request }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/tasks', { backend });

    await page.getByRole('button', { name: 'New task' }).first().click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Lab health');
    await page.getByRole('combobox', { name: 'Lab devices' }).selectOption('only');
    // The backend answers for the selection as it is on screen, before saving.
    await expect(page.getByText('3 devices right now')).toBeVisible();

    await page.getByRole('textbox', { name: 'Commands, one per line' }).fill('show version');
    await page.getByRole('button', { name: 'Create the task' }).click();

    const row = page.getByRole('row').filter({ hasText: 'Lab health' });
    await expect(row).toContainText('Every lab device');
    await expect(row).toContainText('Commands');
    await expect(row).toContainText('Never');

    // It really is a filter: the engine resolves it when the task runs.
    const rpc = rpcFor(request, backend);
    const tasks = await rpc<{ id: string; select: { filter?: { lab?: boolean } } }[]>('tasks.list');
    expect(tasks[0]?.select.filter?.lab).toBe(true);

    expect(console.messages).toEqual([]);
  });

  test('running a task opens its run', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/tasks', { backend });

    await page.getByRole('row').filter({ hasText: 'Lab health' }).getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Run now…' }).click();
    const confirm = page.getByRole('dialog');
    await expect(confirm).toContainText('Every lab device');
    await confirm.getByRole('button', { name: 'Run now' }).click();

    await expect(runHeader(page).getByText('3 answered', { exact: true })).toBeVisible({ timeout: 30_000 });
    expect(console.messages).toEqual([]);
  });
});

// ---------------------------------------------------------------- templates

test.describe('templates', () => {
  test('renders against a real device and says that applying drafts a change', async ({ page, request }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/templates', { backend });

    await page.getByRole('cell', { name: 'NTP servers (IOS)' }).dblclick();

    await expect(page.getByText('Built in, read-only')).toBeVisible();
    // A `list` variable is a TagInput: a plain text box, one value at a time.
    const servers = page.getByRole('textbox', { name: 'NTP servers' });
    await servers.fill('192.0.2.1');
    await servers.press('Enter');

    await page.getByRole('combobox', { name: 'Preview against' }).click();
    await page.getByRole('option', { name: 'Campus/AS1' }).click();
    await expect(page.getByText('ntp server 192.0.2.1')).toBeVisible({ timeout: 20_000 });

    // The promise: this drafts a change, it does not configure anything.
    await expect(page.getByText('This drafts a change and touches nothing')).toBeVisible();

    await page.getByRole('textbox', { name: 'Name, address or tag contains' }).fill('AS1');
    await page.getByRole('button', { name: 'Draft a change…' }).click();
    const confirm = page.getByRole('dialog');
    await expect(confirm).toContainText('Nothing is sent to a device');
    await confirm.getByRole('button', { name: 'Draft the change' }).click();

    await expect(page.getByText('A change was drafted')).toBeVisible({ timeout: 20_000 });

    // And the change really is only a draft: nothing was applied.
    const rpc = rpcFor(request, backend);
    const changes = await rpc<{ id: string; state: string }[]>('changes.list');
    expect(changes.length).toBeGreaterThan(0);
    expect(changes.map((change) => change.state)).not.toContain('applied');

    expect(console.messages).toEqual([]);
  });
});

// ------------------------------------------------------ schedules, triggers

test.describe('schedules and triggers', () => {
  test('a schedule shows the next five firings in local time', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/schedules', { backend });

    await page.getByRole('button', { name: 'New schedule' }).first().click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Nightly health');
    await page.getByRole('textbox', { name: 'At' }).fill('02:30');

    const preview = page.getByRole('list').filter({ hasText: '02:30' }).first();
    await expect(preview.getByRole('listitem')).toHaveCount(5);
    await expect(page.getByText('Every day at 02:30, in this machine')).toBeVisible();

    await page.getByRole('combobox', { name: 'Task' }).selectOption({ label: 'Lab health' });
    await page.getByRole('button', { name: 'Create the schedule' }).click();

    const row = page.getByRole('row').filter({ hasText: 'Nightly health' });
    await expect(row).toContainText('Every day at 02:30');
    await expect(row).toContainText('tasks.run');
    await expect(row).toContainText('Never');

    expect(console.messages).toEqual([]);
  });

  test('a trigger may not watch what automation publishes itself', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/schedules', { backend });

    await page.getByRole('button', { name: 'New trigger' }).first().click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('A loop');
    await page.getByRole('combobox', { name: 'Event topic' }).fill('tasks.changed');
    await page.getByRole('combobox', { name: 'Task' }).selectOption({ label: 'Lab health' });
    await page.getByRole('button', { name: 'Create the trigger' }).click();

    // The engine's reason, as it said it.
    await expect(page.getByText('would run in a loop')).toBeVisible();

    await page.keyboard.press('Escape');
    expect(console.messages).toEqual([]);
  });

  test('a trigger on a real event topic is saved and can be fired by hand', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/schedules', { backend });

    await page.getByRole('button', { name: 'New trigger' }).first().click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('On a device going down');
    await page.getByRole('combobox', { name: 'Event topic' }).fill('devices.state');
    await page.getByRole('combobox', { name: 'Task' }).selectOption({ label: 'Lab health' });
    await page.getByRole('button', { name: 'Create the trigger' }).click();

    const row = page.getByRole('row').filter({ hasText: 'On a device going down' });
    await expect(row).toContainText('devices.state');
    await expect(row).toContainText('Collects events for 30s, then fires once');
    await expect(row).toContainText('Never');

    await row.getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Fire it now…' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Fire now' }).click();
    await expect(page.getByText('On a device going down fired')).toBeVisible({ timeout: 20_000 });

    expect(console.messages).toEqual([]);
  });
});

// ------------------------------------------------------------------ ansible

test.describe('ansible', () => {
  test('leads with what it cannot promise, and says so again where a run starts', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/ansible', { backend });

    const caveat = page.getByRole('note', { name: 'Outside the change pipeline' });
    await expect(caveat).toBeVisible();
    // The five guarantees a change gives and this does not, as five words.
    for (const missing of ['Backup', 'Drift check', 'Approved plan', 'Verification', 'Rollback']) {
      await expect(caveat.getByText(missing, { exact: true })).toBeVisible();
    }
    await expect(caveat).toContainText('nothing can be rolled back');
    // Not dismissible: there is nothing to press.
    await expect(caveat.getByRole('button')).toHaveCount(0);

    // And the honest state of this machine, with the backend's own reason.
    await expect(page.getByText('Not installed')).toBeVisible();
    await expect(page.getByText('running a playbook needs a local process')).toBeVisible();
    await expect(page.getByText('ansible-galaxy could not be asked')).toBeVisible();
    await expect(page.getByText('Not from here')).toBeVisible();

    expect(console.messages).toEqual([]);
  });

  test('the same caveats are in the task editor when a task would run a playbook', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/tasks', { backend });

    await page.getByRole('button', { name: 'New task' }).first().click();
    await page.getByRole('combobox', { name: 'Action' }).selectOption('ansible');
    await expect(page.getByRole('note', { name: 'Outside the change pipeline' })).toBeVisible();
    await expect(page.getByText('This is the one path with none of the change pipeline')).toBeVisible();

    await page.keyboard.press('Escape');
    expect(console.messages).toEqual([]);
  });
});

// ---------------------------------------------------------------- playbooks

test.describe('playbooks', () => {
  test('validates as it is written and reports a problem by step', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/automation/playbooks', { backend });

    await page.getByRole('button', { name: 'New playbook' }).first().click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Health check');
    await expect(page.getByText('3 steps, all valid')).toBeVisible({ timeout: 20_000 });

    await page
      .getByRole('textbox', { name: 'Steps, as YAML' })
      .fill('name: Broken\nsteps:\n  - name: Nothing at all\n');
    await expect(page.getByText('Step 1: a step does exactly one thing')).toBeVisible({ timeout: 20_000 });
    // A broken playbook cannot be run.
    await expect(page.getByRole('button', { name: 'Run…' })).toBeDisabled();

    expect(console.messages).toEqual([]);
  });
});

// ------------------------------------------------------------------- the rest

test('the palette reaches the screen, and the screen\'s own commands', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/', { backend });

  // The shell registers every destination; the screen is one of them.
  await page.keyboard.press('Control+k');
  await page.getByRole('combobox', { name: 'Search commands and screens' }).fill('Automation');
  await page.getByRole('option', { name: 'Go to Automation' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Automation' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Run', exact: true })).toHaveAttribute('aria-selected', 'true');

  // The screen's own commands are registered while it is open. (They only
  // exist while it is mounted — see NOTES.md.)
  await page.keyboard.press('Control+k');
  await page.getByRole('combobox', { name: 'Search commands and screens' }).fill('New schedule');
  await page.getByRole('option', { name: 'New schedule' }).first().click();
  await expect(page.getByRole('dialog', { name: 'New schedule' })).toBeVisible();
  await page.keyboard.press('Escape');

  await goTo(page, '/automation/templates');
  await expect(page.getByRole('tab', { name: 'Templates' })).toHaveAttribute('aria-selected', 'true');

  expect(console.messages).toEqual([]);
});
