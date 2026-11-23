/**
 * The Changes screen, against a real engine and a real device.
 *
 * A `nocturne-server` is started on its own empty data directory, and the
 * device it plans against is a simulated IOS switch inside a Nocturne lab —
 * a lab node is an inventory device with `transport: sim`, so the change
 * pipeline reaches it exactly as it reaches a real switch. That is what makes
 * the apply test worth anything: the commands really are sent, the running
 * configuration really is read back, verification really runs, and a rollback
 * really puts the device back.
 *
 * Nothing here is mocked except two deliberate fault injections, each marked
 * where it happens: a plan hash that no longer matches its approval, and an
 * apply the engine refuses. Both exist to prove what the screen does when it
 * is told "no", which cannot otherwise be reached from the outside.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async () => {
  test.setTimeout(300_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  backend = await startBackend({ uiDir });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ----------------------------------------------------------------- the device

/**
 * A running lab with one IOS switch, and the inventory name of that switch.
 * The simulator speaks IOS, so `cisco_ios` drives it and the whole pipeline —
 * backup, hierarchy diff, verification, rollback — is the real one.
 */
let running: string | null = null;

async function startLab(rpc: Rpc, name: string): Promise<string> {
  // Only eight labs may run at once, and every test here wants a device of
  // its own, so the one before it is stopped. Its devices and its changes
  // stay in the inventory, which is what the list test reads.
  if (running) await rpc('labs.stop', { id: running });
  const lab = await rpc<{ id: string }>('labs.create', { name });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });
  running = lab.id;
  return `${name}/SW1`;
}

/** Creates a change and waits for the engine to finish planning it. */
async function planned(
  rpc: Rpc,
  title: string,
  device: string,
  lines: string[],
  extra: Record<string, unknown> = {},
): Promise<string> {
  const created = await rpc<{ change: { id: string } }>('changes.create', {
    title,
    targets: [{ device_id: device, intent: { type: 'lines', lines } }],
    plan: true,
    ...extra,
  });
  const id = created.change.id;
  await expect
    .poll(async () => (await rpc<{ state: string }>('changes.get', { id })).state, {
      timeout: 60_000,
    })
    .toBe('planned');
  return id;
}

/** The plan hash of a change's only target. */
async function hashOf(rpc: Rpc, id: string): Promise<string> {
  const change = await rpc<{ targets: { plan: { plan_hash: string } }[] }>('changes.get', { id });
  const first = change.targets[0];
  if (!first) throw new Error('the change has no targets');
  return first.plan.plan_hash;
}

/** The running configuration of a lab node, as the simulator prints it. */
async function configOf(rpc: Rpc, lab: string, node = 'SW1'): Promise<string> {
  const labs = await rpc<{ id: string; name: string }[]>('labs.list');
  const found = labs.find((entry) => entry.name === lab);
  if (!found) throw new Error(`no lab named ${lab}`);
  const config = await rpc<{ text: string }>('labs.node.config', { lab_id: found.id, node });
  return config.text;
}

function cards(page: Page) {
  return page.getByTestId('device-card');
}

/** The "what will be sent" list of one device — not its rollback list. */
function sent(page: Page, device: string) {
  // `exact`: the rollback list's name ends in the same words.
  return page
    .getByRole('list', { name: `Commands for ${device}`, exact: true })
    .getByTestId('command-line');
}

// --------------------------------------------------------------------- tests

test.describe('drafting a change', () => {
  test('picks devices, takes the lines as typed, and plans them', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Draft');

    await openApp(page, '/changes', { backend });
    await expect(page.getByRole('heading', { level: 1, name: 'Changes' })).toBeVisible();
    // Nothing yet, and the empty state says what the screen is for.
    await expect(page.getByText('Nothing is in flight')).toBeVisible();

    await page.getByRole('button', { name: 'New change' }).first().click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();

    // Step 1: the device. The checkbox's label is the device's name.
    await dialog.getByText(device, { exact: true }).click();
    await expect(dialog.getByText('1 device chosen')).toBeVisible();
    await dialog.getByRole('button', { name: 'Continue' }).click();

    // Step 2: what to do. CodeMirror's document is a textbox with a name.
    await expect(dialog.getByText('Step 2 of 3')).toBeVisible();
    await dialog
      .getByRole('textbox', { name: 'Configuration lines' })
      .fill('interface FastEthernet0/4\ndescription drafted from the screen\nexit');

    // Preview asks the engine what it would do, on that one device, and
    // creates nothing.
    await dialog.getByRole('button', { name: `Preview on ${device}` }).click();
    await expect(dialog.getByTestId('command-line').first()).toContainText(
      'interface FastEthernet0/4',
    );
    await expect(dialog.getByText('Low risk').first()).toBeVisible();
    expect(await rpc<unknown[]>('changes.list', {})).toHaveLength(0);

    await dialog.getByRole('button', { name: 'Continue' }).click();

    // Step 3: name it and plan it.
    await dialog.getByRole('textbox', { name: 'Title' }).fill('Describe Fa0/4');
    await dialog.getByRole('textbox', { name: 'Change reference' }).fill('CHG-7');
    await dialog.getByRole('button', { name: 'Plan on 1 device' }).click();

    // Landing on the change page, with the commands the engine worked out.
    await expect(page.getByRole('heading', { level: 1, name: 'Describe Fa0/4' })).toBeVisible();
    await expect(page.getByText('CHG-7')).toBeVisible();
    await expect(page.getByRole('button', { name: /Approve & apply to 1 device/ })).toBeVisible({
      timeout: 60_000,
    });

    const lines = sent(page, device);
    await expect(lines).toHaveCount(3);
    await expect(lines.nth(0)).toContainText('interface FastEthernet0/4');
    await expect(lines.nth(1)).toContainText('description drafted from the screen');
    await expect(lines.nth(2)).toContainText('exit');

    expect(console.messages).toEqual([]);
  });
});

test.describe('a plan', () => {
  test('shows the commands in order, the predicted diff, the risk and the way back', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Read');
    const id = await planned(rpc, 'Guest port', device, [
      'interface FastEthernet0/6',
      'description guest',
      'switchport mode access',
      'exit',
    ]);

    await openApp(page, `/changes/${id}`, { backend });
    const card = cards(page).first();

    // Opened by itself when there is only one device: this screen's whole
    // job is that the commands are read.
    await expect(card.getByText('What will be sent')).toBeVisible();
    await expect(sent(page, device)).toHaveCount(4);

    // The predicted configuration is the engine's own diff, down to the
    // line numbers — the screen does not diff anything itself.
    await expect(card.getByText('Predicted configuration')).toBeVisible();
    await expect(card.getByTestId('diff-line').filter({ hasText: 'description guest' })).toBeVisible();

    await expect(card.getByText('Low risk').first()).toBeVisible();
    await expect(
      card.getByText('it only adds descriptions, VLAN names, access-port settings or management lines'),
    ).toBeVisible();

    // What undoes it, computed from the same diff.
    await expect(card.getByText('If it has to be undone')).toBeVisible();
    await expect(card.getByText('no description')).toBeVisible();

    // A low-risk change arms no dead-man switch, and the card says exactly
    // that rather than claiming the platform has none.
    await expect(card.getByRole('heading', { name: 'Dead-man switch' })).toBeVisible();
    await expect(card.getByText('None will be armed for this change')).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

test.describe('protected resources', () => {
  test('are confirmed by typing the device’s name, and by nothing else', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Protect');
    const id = await planned(rpc, 'Lock down the login lines', device, [
      'line vty 0 4',
      'transport input ssh',
      'login local',
      'exit',
    ]);

    await openApp(page, `/changes/${id}`, { backend });

    // The engine found them; the screen explains each one in its words.
    const card = cards(page).first();
    await expect(card.getByTestId('protected-hit').first()).toContainText('Login lines');
    await expect(card.getByTestId('protected-hit').first()).toContainText(
      'is how Nocturne (and everybody else) logs in',
    );
    await expect(page.getByText('Critical risk').first()).toBeVisible();

    // Critical risk, so the engine arms the platform's dead-man switch and
    // the card shows what it is and what it would do.
    await expect(card.getByText('reload timer')).toBeVisible();
    await expect(card.getByText('reload in 10')).toBeVisible();

    const approve = page.getByRole('button', { name: /Approve & apply to 1 device/ });
    await expect(approve).toBeDisabled();

    const confirmation = page.getByRole('textbox', { name: `Type ${device} to confirm` });

    // "yes" is not the device's name, and neither is anything but the name.
    await confirmation.fill('yes');
    await expect(page.getByText(`That is not ${device}.`)).toBeVisible();
    await expect(approve).toBeDisabled();

    await confirmation.fill(device.slice(0, -1));
    await expect(approve).toBeDisabled();

    await confirmation.fill(device);
    await expect(approve).toBeEnabled();

    // Approving without applying records the signature and the name typed.
    await page.getByRole('button', { name: 'Approve without applying' }).click();
    await expect(page.getByText('Approved', { exact: false }).first()).toBeVisible();

    const change = await rpc<{
      state: string;
      approval: { confirmed_devices: string[]; plan_hashes: string[] };
    }>('changes.get', { id });
    expect(change.state).toBe('approved');
    expect(change.approval.confirmed_devices).toEqual([device]);
    expect(change.approval.plan_hashes).toEqual([await hashOf(rpc, id)]);

    expect(console.messages).toEqual([]);
  });
});

test.describe('applying', () => {
  test('sends the commands to a simulated device, verifies them, and can undo them', async ({
    page,
    request,
  }) => {
    test.setTimeout(300_000);
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Apply');
    const id = await planned(rpc, 'Describe the uplink', device, [
      'interface GigabitEthernet0/1',
      'description uplink to core',
      'exit',
    ]);

    // Before: the device says nothing about that port.
    // The record is written a moment before the session is closed, so poll
    // the device itself rather than read it once.
    await expect
      .poll(() => configOf(rpc, 'Apply'), { timeout: 60_000 })
      .not.toContain('description uplink to core');

    await openApp(page, `/changes/${id}`, { backend });
    await page.getByRole('checkbox', { name: 'I have reviewed the plans' }).check();
    await page.getByRole('button', { name: /Approve & apply to 1 device/ }).click();

    // The live stepper, as the engine reports each stage.
    const card = cards(page).first();
    await expect(card.getByTestId('apply-step').filter({ hasText: 'Connect' })).toBeVisible({
      timeout: 60_000,
    });
    await expect(page.getByText('Applied', { exact: false }).first()).toBeVisible({
      timeout: 120_000,
    });

    // Every stage of the pipeline happened, in the engine's own words.
    for (const step of ['Connect', 'Drift check', 'Backup', 'Send', 'Verify', 'Save']) {
      // `first`: "Backup" is also the start of "Backup afterwards".
      await expect(card.getByTestId('apply-step').filter({ hasText: step }).first()).toBeVisible();
    }
    await expect(
      card.getByTestId('verify-check').filter({ hasText: 'configuration' }),
    ).toContainText('exactly what the plan predicted');

    // And the device really has it.
    await expect
      .poll(() => configOf(rpc, 'Apply'), { timeout: 60_000 })
      .toContain('description uplink to core');

    // Undoing a change that succeeded, from the screen. The way back is a
    // change of its own, worked out from what the device holds now, and
    // nothing is sent until a person approves and applies it.
    await page.getByRole('button', { name: 'Roll back…' }).click();
    const question = page.getByRole('dialog');
    await expect(question).toContainText(device);
    await expect(question).toContainText('nothing is sent until it is approved and applied');
    await question.getByRole('button', { name: 'Plan the undo' }).click();

    // It opens on its own page and says, in plain words, what it is.
    await expect(
      page.getByRole('heading', { level: 1, name: 'Undo: Describe the uplink' }),
    ).toBeVisible({ timeout: 30_000 });
    const undoes = page.getByTestId('undoes');
    await expect(undoes).toContainText('This change undoes Describe the uplink');
    await expect(undoes).toContainText('Nothing has been sent');
    await expect(page.getByRole('button', { name: /Approve & apply to 1 device/ })).toBeVisible({
      timeout: 60_000,
    });
    await expect(undoes).toContainText('approve and apply it like any other change');

    // Its lines are the way back (the port's description as it was before,
    // or none), and the device has not been sent them.
    const back = sent(page, device);
    await expect(back.first()).toContainText('interface GigabitEthernet0/1');
    await expect(back.filter({ hasText: 'description' })).toHaveCount(1);
    await expect(cards(page).first().getByText('Predicted configuration')).toBeVisible();
    expect(await configOf(rpc, 'Apply')).toContain('description uplink to core');

    // Approved and applied like any change: backup, send, verify.
    await page.getByRole('checkbox', { name: 'I have reviewed the plans' }).check();
    await page.getByRole('button', { name: /Approve & apply to 1 device/ }).click();
    await expect(page.getByTestId('undoes')).toContainText('Applied:', { timeout: 120_000 });
    const undoCard = cards(page).first();
    for (const step of ['Backup', 'Send', 'Verify']) {
      await expect(undoCard.getByTestId('apply-step').filter({ hasText: step }).first()).toBeVisible();
    }
    await expect
      .poll(() => configOf(rpc, 'Apply'), { timeout: 60_000 })
      .not.toContain('description uplink to core');
    // An undo is not undone in turn: its page says how to have the change
    // back, and offers no "Roll back…" of its own.
    await expect(undoes).toContainText('plan it again');
    await expect(page.getByRole('button', { name: 'Roll back…' })).toHaveCount(0);

    // The change it undid now reads as rolled back, and names its undo.
    await undoes.getByRole('link', { name: 'Describe the uplink' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Describe the uplink' })).toBeVisible();
    await expect(page.getByText('Rolled back', { exact: true }).first()).toBeVisible({
      timeout: 30_000,
    });
    const undoneBy = page.getByTestId('undone-by');
    await expect(undoneBy).toContainText('Undone by Undo: Describe the uplink');
    // Nothing is left to undo, so it is not offered again.
    await expect(page.getByRole('button', { name: 'Roll back…' })).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('is refused by the engine when the device drifted, in the engine’s own words', async ({
    page,
    request,
  }) => {
    test.setTimeout(240_000);
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Drift');

    const stale = await planned(rpc, 'Planned before the drift', device, [
      'interface FastEthernet0/7',
      'description seven',
      'exit',
    ]);
    const other = await planned(rpc, 'What caused the drift', device, [
      'interface FastEthernet0/8',
      'description eight',
      'exit',
    ]);

    // Approve the first one against the configuration as it is now…
    await rpc('changes.approve', { id: stale, plan_hashes: [await hashOf(rpc, stale)] });
    // …then change the device underneath it.
    await rpc('changes.approve', { id: other, plan_hashes: [await hashOf(rpc, other)] });
    await rpc('changes.apply', { id: other });
    await expect
      .poll(async () => (await rpc<{ state: string }>('changes.get', { id: other })).state, {
        timeout: 120_000,
      })
      .toBe('applied');

    await openApp(page, `/changes/${stale}`, { backend });
    await page.getByRole('button', { name: 'Apply to 1 device', exact: true }).click();

    // Nothing is sent: the engine's drift check stops it, and the screen
    // repeats what the engine said rather than explaining it away.
    await expect(page.getByTestId('target-error')).toContainText(
      'changed since this was planned',
      { timeout: 120_000 },
    );
    const banner = page.getByRole('region', { name: 'Stale plan' });
    await expect(banner).toContainText('The plan was out of date');
    await expect(banner.getByRole('button', { name: 'Plan again' })).toBeVisible();

    const after = await configOf(rpc, 'Drift');
    expect(after).not.toContain('description seven');
    expect(after).toContain('description eight');

    expect(console.messages).toEqual([]);
  });
});

test.describe('an approval is bound to the plan it signed', () => {
  test('a plan that changed since it was approved is refused by the screen', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Bound');
    const id = await planned(rpc, 'Bound to its plan', device, [
      'interface FastEthernet0/9',
      'description nine',
      'exit',
    ]);
    await rpc('changes.approve', { id, plan_hashes: [await hashOf(rpc, id)] });

    // Fault injection. The engine never hands out an approved change whose
    // plan has moved on — a re-plan withdraws the approval — so the race the
    // screen guards against is produced here: the same approved change, with
    // one character of the stored plan's hash changed. Everything else is the
    // engine's own answer.
    let applied = 0;
    await page.route('**/api/rpc', async (route) => {
      const body = route.request().postDataJSON() as { method: string; params?: { id?: string } };
      if (body.method === 'changes.apply') applied += 1;
      if (body.method !== 'changes.get' || body.params?.id !== id) return route.continue();
      const response = await route.fetch();
      const payload = (await response.json()) as {
        ok: boolean;
        result?: { targets: { plan: { plan_hash: string } | null }[] };
      };
      const plan = payload.result?.targets[0]?.plan;
      if (plan) plan.plan_hash = `${plan.plan_hash.slice(0, -1)}0`;
      await route.fulfill({ response, json: payload });
    });

    await openApp(page, `/changes/${id}`, { backend });

    await expect(page.getByTestId('stale-approval')).toContainText(
      'not the one that was approved',
    );
    await expect(page.getByTestId('stale-approval')).toContainText(device);
    await expect(page.getByRole('button', { name: 'Apply to 1 device', exact: true })).toBeDisabled();
    await expect(
      page.getByTestId('stale-approval').getByRole('button', { name: 'Plan again' }),
    ).toBeVisible();

    // The screen never asked the engine to apply it, and never quietly
    // planned it again either.
    await page.waitForTimeout(500);
    expect(applied).toBe(0);
    expect((await rpc<{ state: string }>('changes.get', { id })).state).toBe('approved');

    expect(console.messages).toEqual([]);
  });

  test('a new plan withdraws the approval and the review that was given for the old one', async ({
    page,
    request,
  }) => {
    test.setTimeout(240_000);
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Replan');
    const id = await planned(rpc, 'Replanned while it was read', device, [
      'interface FastEthernet0/10',
      'description ten',
      'exit',
    ]);

    await openApp(page, `/changes/${id}`, { backend });
    await page.getByRole('checkbox', { name: 'I have reviewed the plans' }).check();
    await page.getByRole('button', { name: 'Approve without applying' }).click();
    await expect(page.getByRole('button', { name: 'Apply to 1 device', exact: true })).toBeVisible();

    // Somebody changes the device, and the change is planned again against
    // what it says now. The engine withdraws the approval; the plan is a new
    // one (it was computed from a different configuration), so the tick the
    // person gave the old plan must go with it.
    const other = await planned(rpc, 'Someone else was here', device, [
      'interface FastEthernet0/11',
      'description eleven',
      'exit',
    ]);
    await rpc('changes.approve', { id: other, plan_hashes: [await hashOf(rpc, other)] });
    await rpc('changes.apply', { id: other });
    await expect
      .poll(async () => (await rpc<{ state: string }>('changes.get', { id: other })).state, {
        timeout: 120_000,
      })
      .toBe('applied');

    const before = await hashOf(rpc, id);
    await rpc('changes.plan', { id });
    await expect
      .poll(async () => (await rpc<{ state: string }>('changes.get', { id })).state, {
        timeout: 60_000,
      })
      .toBe('planned');
    expect(await hashOf(rpc, id)).not.toBe(before);

    // The page says the plans were replaced, and goes back to asking.
    await expect(page.getByTestId('plans-replaced')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('approve-blockers')).toContainText('read the commands');
    await expect(page.getByRole('checkbox', { name: 'I have reviewed the plans' })).not.toBeChecked();
    await expect(page.getByRole('button', { name: /Approve & apply to 1 device/ })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Apply to 1 device', exact: true })).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });
});

test.describe('nothing is applied from the browser', () => {
  test('a refusal is shown in the engine’s own words, not the screen’s', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Refused');
    const id = await planned(rpc, 'Refused by the engine', device, [
      'interface FastEthernet0/11',
      'description eleven',
      'exit',
    ]);
    await rpc('changes.approve', { id, plan_hashes: [await hashOf(rpc, id)] });

    // Fault injection: the engine's own `forbidden` for an apply it will not
    // run. The sentence is the one the autonomy gate produces; the screen
    // must show it as it is, because only the engine knows why.
    const sentence =
      'autonomy is set to `draft`, so the assistant never applies changes by itself. ' +
      'Ask the person you are working with to look at the change `Refused by the engine` in the ' +
      'Changes screen and approve it there; they can see the exact commands, the predicted ' +
      'difference and the risk.';
    await page.route('**/api/rpc', async (route) => {
      const body = route.request().postDataJSON() as { method: string };
      if (body.method !== 'changes.apply') return route.continue();
      await route.fulfill({
        status: 200,
        json: { ok: false, error: { code: 'forbidden', message: sentence } },
      });
    });

    await openApp(page, `/changes/${id}`, { backend });
    await page.getByRole('button', { name: 'Apply to 1 device', exact: true }).click();

    await expect(page.getByTestId('apply-error')).toHaveText(new RegExp(escapeRegExp(sentence)));
    // The device was not touched.
    expect(await configOf(rpc, 'Refused')).not.toContain('description eleven');

    expect(console.messages).toEqual([]);
  });
});

test.describe('the list', () => {
  test('shows every change with its state, risk and devices, and filters', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const device = await startLab(rpc, 'Listed');
    await planned(rpc, 'A listed change', device, [
      'interface FastEthernet0/12',
      'description twelve',
      'exit',
    ]);

    await openApp(page, '/changes', { backend });
    const table = page.getByRole('table', { name: 'Changes' });
    const row = table.locator('tbody tr').filter({ hasText: 'A listed change' });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('Waiting for approval');
    await expect(row).toContainText('Low risk');
    await expect(row).toContainText(device);

    await page.getByRole('searchbox', { name: 'Search changes' }).fill('A listed change');
    await expect(table.locator('tbody tr')).toHaveCount(1);

    await page.getByRole('searchbox', { name: 'Search changes' }).fill('nothing matches this');
    await expect(page.getByText('No change matches')).toBeVisible();
    await page.getByRole('button', { name: 'Clear the search' }).click();

    // The row is a way in.
    await page.getByRole('link', { name: 'A listed change' }).click();
    await pageLoaded(page);
    await expect(page.getByRole('heading', { level: 1, name: 'A listed change' })).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
