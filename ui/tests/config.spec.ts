/**
 * The Configuration screen, against a real engine and real devices.
 *
 * A `nocturne-server` is started on its own empty data directory and the
 * devices it reads are simulated IOS switches in a Nocturne lab — a lab node
 * is an inventory device with `transport: sim`, so `config.backup` opens a
 * real session, the profile's `show running-config` really runs, the stored
 * text really is what the device printed, and the diff between two versions
 * really is the engine's.
 *
 * Nothing here is stubbed. The second version of a configuration is made by
 * changing the device at its own console (`labs.node.config_set`), exactly as
 * an engineer bypassing Nocturne would, and the screen then has to show what
 * changed. The secrets that get redacted are secrets the simulator really
 * holds, and the compliance findings are the built-in hardening rules'
 * verdicts on a real switch.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 120_000 });

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

// ----------------------------------------------------------------- the lab

/** The running lab, so each test can stop the one before it (only eight may run). */
let running: string | null = null;

interface Lab {
  id: string;
  /** Inventory ids by node name. */
  devices: Record<string, string>;
}

/**
 * A lab with simulated IOS switches, started, and the inventory ids of its
 * nodes. The simulator speaks IOS, so `cisco_ios` drives it and everything
 * the screen shows — the configuration text, the diff, the compliance
 * verdicts — is the real pipeline's work.
 */
async function startLab(rpc: Rpc, name: string, nodes: string[]): Promise<Lab> {
  if (running) await rpc('labs.stop', { id: running });
  const lab = await rpc<{ id: string }>('labs.create', { name });
  for (const node of nodes) {
    await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: node });
  }
  await rpc('labs.start', { id: lab.id });
  running = lab.id;

  const devices = await rpc<{ id: string; name: string }[]>('devices.list', {});
  const ids: Record<string, string> = {};
  for (const node of nodes) {
    const found = devices.find((device) => device.name === `${name}/${node}`);
    if (!found) throw new Error(`the lab did not produce a device for ${node}`);
    ids[node] = found.id;
  }
  return { id: lab.id, devices: ids };
}

/** Backs the devices up through the API, for tests that start from "captured". */
async function backUp(rpc: Rpc, deviceIds: string[]): Promise<void> {
  const started = await rpc<{ job_id: string }>('config.backup', { device_ids: deviceIds });
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: started.job_id })).state, {
      timeout: 60_000,
    })
    .not.toBe('running');
}

/** Replaces a lab node's configuration at its own console, and reboots it onto it. */
async function atTheConsole(rpc: Rpc, lab: string, node: string, startup: string): Promise<void> {
  const result = await rpc<{ rejected: { line: string; error: string }[] }>('labs.node.config_set', {
    lab_id: lab,
    node,
    startup,
  });
  expect(result.rejected, 'the simulator rejected a line of the configuration').toEqual([]);
}

/**
 * A switch configuration with two descriptions, a user password and an SNMP
 * community in it: changes a diff can show and secrets redaction must hide.
 * No `enable secret`, because setting one locks Nocturne's own credential
 * out of privileged mode and the next backup then fails — which is correct
 * of it, and not what these tests are about.
 */
const CONFIGURED = `!
version 15.2
no service password-encryption
!
hostname SW1
!
username netops privilege 15 password 0 sup3rsecret
no aaa new-model
!
snmp-server community public RO
!
spanning-tree mode pvst
!
interface FastEthernet0/1
 description uplink to dist-1
!
interface FastEthernet0/2
 description reception desk
!
interface Vlan1
 no ip address
 shutdown
!
line con 0
line vty 0 4
 login
!
end
`;

// --------------------------------------------------------------------- tests

test.describe('a device nothing has ever been captured from', () => {
  test('says so, and is one button away from being captured', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await startLab(rpc, 'Never', ['SW1']);

    await openApp(page, '/config', { backend });
    await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();

    // The coverage table knows the device and says what it has: nothing.
    const table = page.getByRole('table', { name: 'Backup coverage' });
    const row = table.getByRole('row').filter({ hasText: 'Never/SW1' });
    await expect(row).toBeVisible();
    await expect(row.getByText('Never captured')).toBeVisible();
    await expect(row.getByText('never', { exact: true })).toBeVisible();
    await expect(row.getByText('none', { exact: true })).toBeVisible();

    // Its history is an honest empty state, not an empty configuration.
    await openApp(page, `/config/history?device=${lab.devices.SW1}`, { backend });
    await expect(
      page.getByText('No running configuration has ever been captured from Never/SW1'),
    ).toBeVisible();
    await expect(page.getByText('which is different from having an empty one')).toBeVisible();

    // And the way out is the one action the empty state offers.
    await page.getByRole('button', { name: 'Back up this device now' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('read-only session');
    await expect(dialog).toContainText('Nothing is changed on any device');
    await dialog.getByRole('button', { name: 'Back it up' }).click();

    // The real device answers, the capture is stored, and the text appears
    // with the line numbers a configuration is discussed by.
    const editor = page.getByRole('textbox', { name: /Running configuration, version/ });
    await expect(editor).toBeVisible({ timeout: 60_000 });
    await expect(editor).toContainText('hostname SW1');
    await expect(page.getByTestId('version-meta')).toContainText('asked for by hand');
    await expect(page.getByTestId('version-row')).toHaveCount(1);

    // It really is what the device prints, not something the screen made up.
    const device = await rpc<{ text: string }>('labs.node.config', { lab_id: lab.id, node: 'SW1' });
    expect(device.text).toContain('hostname SW1');

    expect(console.messages).toEqual([]);
  });
});

test.describe('backing up many devices', () => {
  test('is a job with an outcome for every device, including what went wrong', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await startLab(rpc, 'Many', ['SW1', 'SW2', 'SW3']);
    // A device that is in the inventory and cannot be reached: its failure is
    // part of the outcome, with the engine's own reason.
    await rpc('devices.create', {
      name: 'unreachable-sw',
      address: '198.51.100.9',
      profile: 'cisco_ios',
      site: 'HQ',
      role: 'access',
    });

    await openApp(page, '/config/backups', { backend });

    // Devices from earlier tests are still in the inventory, so this picks
    // the four it is about rather than everything.
    const table = page.getByRole('table', { name: 'Backup coverage' });
    for (const name of ['Many/SW1', 'Many/SW2', 'Many/SW3', 'unreachable-sw']) {
      await table.getByRole('row').filter({ hasText: name }).getByRole('checkbox').check();
    }

    await page.getByRole('button', { name: /Back up 4 devices/ }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('4 devices');
    await expect(dialog).toContainText('unreachable-sw');
    await dialog.getByRole('button', { name: 'Back up 4' }).click();

    const outcomes = page.getByTestId('backup-outcome');
    await expect(outcomes).toHaveCount(4, { timeout: 120_000 });

    // Three simulated switches stored their first capture…
    for (const node of ['SW1', 'SW2', 'SW3']) {
      await expect(outcomes.filter({ hasText: `Many/${node}` })).toContainText('First capture');
      await expect(outcomes.filter({ hasText: `Many/${node}` })).toContainText('first backup stored');
    }
    // …and each one says, in the engine's words, that its startup-config
    // could not be read. The simulator has none, which is worth knowing and
    // is not a failure.
    await expect(outcomes.filter({ hasText: 'Many/SW1' })).toContainText(
      'could not read the startup-config',
    );

    // The unreachable device failed, with the reason the engine gave.
    const failed = outcomes.filter({ hasText: 'unreachable-sw' });
    await expect(failed).toContainText('Failed');
    await expect(failed).not.toContainText('First capture');
    const job = await rpc<{ result: { failed: { name: string; error: string }[] } }>('jobs.get', {
      id: (await rpc<{ id: string }[]>('jobs.list', { kind: 'config.backup' }))[0]!.id,
    });
    const reason = job.result.failed.find((entry) => entry.name === 'unreachable-sw')?.error ?? '';
    expect(reason).not.toEqual('');
    await expect(failed).toContainText(reason.slice(0, 40));

    // And the devices that could not say whether they have unsaved changes
    // are named rather than counted as saved.
    await expect(page.getByTestId('not-comparable')).toContainText('only one of the two');

    expect(console.messages).toEqual([]);
  });
});

test.describe('two versions of one device', () => {
  test('are compared by the engine, and the secrets stay hidden', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await startLab(rpc, 'Diff', ['SW1']);
    const device = lab.devices.SW1!;

    // First capture: the switch as the simulator boots it.
    await backUp(rpc, [device]);
    // Then somebody configures it at the console, behind Nocturne's back.
    await atTheConsole(rpc, lab.id, 'SW1', CONFIGURED);
    await backUp(rpc, [device]);

    const versions = await rpc<{ id: number }[]>('config.versions', { device_id: device });
    expect(versions, 'the console change should have produced a second version').toHaveLength(2);

    await openApp(page, `/config/history?device=${device}`, { backend });
    await expect(page.getByTestId('version-row')).toHaveCount(2);

    // The latest capture is shown, and the secrets in it are marked as
    // secrets — not drawn as if they were values.
    const note = page.getByTestId('redaction-note');
    await expect(note).toContainText('Secrets hidden');
    await expect(note).toContainText('2 secrets taken out in 2 places');
    await expect(note).toContainText('well-known default');
    const editor = page.getByRole('textbox', { name: /Running configuration, version/ });
    await expect(editor).toContainText('<secret:');
    await expect(editor).not.toContainText('sup3rsecret');

    // Searching inside the text is local and instant.
    await page.getByRole('searchbox', { name: 'Find in this configuration' }).fill('description');
    await expect(page.getByTestId('find-count')).toHaveText('1 of 2');

    // Tick both versions: the diff is the engine's, down to the line numbers.
    for (const version of versions) {
      await page.getByRole('checkbox', { name: `Compare version ${version.id}` }).check();
    }
    await expect(page.getByText(/Version \d+ compared with version \d+/)).toBeVisible();
    const added = page.getByTestId('diff-line').filter({ hasText: 'description uplink to dist-1' });
    await expect(added).toBeVisible();
    await expect(page.getByTestId('diff-line').filter({ hasText: 'username netops' })).toContainText(
      '<secret:',
    );

    // The engine's own section summary is on screen, so the change is
    // attributed to the part of the configuration it is in.
    await expect(page.getByText('interface FastEthernet0/1', { exact: false }).first()).toBeVisible();

    // Side by side is the same diff in two columns, not a second opinion.
    await page.getByRole('button', { name: 'Side by side' }).click();
    const rows = page.getByTestId('diff-row');
    await expect(rows.filter({ hasText: 'description reception desk' })).toBeVisible();
    const engine = await rpc<{ diff: { lines: { text: string }[] } }>('config.diff', {
      a: versions[1]!.id,
      b: versions[0]!.id,
      context: 3,
    });
    // Every line on screen is a line the engine gave us. The rows that stand
    // for skipped unchanged lines are the engine's `@` markers and say so.
    const shown = await page
      .locator('[data-testid="diff-row"]:not([data-kind="gap"])')
      .allInnerTexts();
    expect(shown.length).toBeGreaterThan(5);
    for (const text of shown) {
      if (text.trim() === '') continue;
      expect(
        engine.diff.lines.some((line) => line.text.trim() !== '' && text.includes(line.text.trim())),
        `“${text}” is not a line the engine gave us`,
      ).toBe(true);
    }

    expect(console.messages).toEqual([]);
  });
});

test.describe('searching the whole network', () => {
  test('finds the line, says how many devices were searched, and says who had nothing', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await startLab(rpc, 'Search', ['SW1', 'SW2']);
    await backUp(rpc, [lab.devices.SW1!, lab.devices.SW2!]);
    await atTheConsole(rpc, lab.id, 'SW1', CONFIGURED);
    await backUp(rpc, [lab.devices.SW1!]);
    // One device with nothing stored, so the coverage line has to say so.
    await rpc('devices.create', {
      name: 'never-read-sw',
      address: '198.51.100.11',
      profile: 'cisco_ios',
      site: 'HQ',
      role: 'access',
    });

    await openApp(page, '/config/search', { backend });
    const field = page.getByRole('searchbox', { name: "Search every device's configuration" });
    await field.fill('description');

    // The devices of the tests before this one are still in the inventory, so
    // the coverage line is checked against what the engine itself answers —
    // which is the claim being made: these are the engine's numbers, not the
    // screen's arithmetic.
    const engine = await rpc<{
      devices_searched: number;
      devices_matched: number;
      devices_without_backup: number;
      matches: unknown[];
    }>('config.search', { query: 'description', context: 1, kind: 'running', limit: 500 });
    expect(engine.devices_without_backup).toBeGreaterThan(0);

    const coverage = page.getByTestId('search-coverage');
    await expect(coverage).toContainText(`${engine.matches.length} lines`, { timeout: 30_000 });
    await expect(coverage).toContainText(`on ${engine.devices_matched} device`);
    await expect(coverage).toContainText(`of ${engine.devices_searched} searched`);
    await expect(coverage).toContainText(`${engine.devices_without_backup} had nothing to search`);

    // The hit is shown with the section it is in and the lines around it.
    const group = page.getByTestId('search-group').filter({ hasText: 'Search/SW1' });
    const hits = group.getByTestId('search-hit');
    await expect(hits.first()).toContainText('interface FastEthernet0/1');
    await expect(hits.first()).toContainText('description uplink to dist-1');

    // The query is in the address, so a result can be handed to someone else.
    expect(page.url()).toContain('q=description');

    // A line nobody has: the answer says what it is an answer about.
    await field.fill('ip helper-address 10.9.9.9');
    await expect(page.getByText('No device has a line matching')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('had no stored configuration to look in')).toBeVisible();

    // Secrets are redacted in search results too, always.
    await field.fill('snmp-server community');
    await expect(page.getByTestId('search-hit').first()).toContainText('<secret:', {
      timeout: 30_000,
    });
    await expect(page.getByTestId('search-hit').first()).not.toContainText('public RO');

    // And the other way round: who has *not* got a line.
    await field.fill('ip ssh version 2');
    await page.getByRole('button', { name: 'Who hasn’t got it' }).click();
    await expect(page.getByText('have no line matching')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('without-backup')).toContainText('never-read-sw');

    expect(console.messages).toEqual([]);
  });
});

test.describe('compliance', () => {
  test('judges a real switch, shows the rule’s own reasoning and its evidence', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await startLab(rpc, 'Policy', ['SW1']);
    await backUp(rpc, [lab.devices.SW1!]);
    await atTheConsole(rpc, lab.id, 'SW1', CONFIGURED);
    await backUp(rpc, [lab.devices.SW1!]);
    // A device with no capture: its rules must read "not evaluated", never "pass".
    await rpc('devices.create', {
      name: 'unjudged-sw',
      address: '198.51.100.12',
      profile: 'cisco_ios',
      site: 'HQ',
      role: 'access',
    });

    await openApp(page, '/config/compliance', { backend });

    // The built-in policies, and what each rule looks for. The list opens
    // collapsed, because the findings are what the tab is for.
    await page.getByRole('button', { name: 'Policies', exact: true }).click();
    await expect(page.getByTestId('policy').first()).toContainText('Cisco IOS hardening baseline');
    const chip = page.getByTestId('rule-chip').first();
    await expect(chip).toBeVisible();
    await expect(chip).toContainText('The configuration must contain the line');

    // Nothing has been run yet. The backend says `not_found` to that, which
    // is an empty state and not an error.
    await expect(page.getByText('No policy has been run yet')).toBeVisible();
    await expect(page.getByText('It touches no device')).toBeVisible();

    await page.getByRole('button', { name: 'Run every enabled policy' }).first().click();

    const findings = page.getByRole('table', { name: 'Compliance findings' });
    await expect(findings.getByRole('row')).not.toHaveCount(1, { timeout: 60_000 });

    // The SNMP rule fails on this switch, and its evidence is the device's own
    // line — with the community string redacted.
    const snmp = findings
      .getByRole('row')
      .filter({ hasText: 'No SNMP community is named' })
      .first();
    await expect(snmp).toContainText('Fail');
    await expect(snmp).toContainText('<secret:');
    // The rule's own title says `public`; the device's line does not get to.
    await expect(snmp).not.toContainText('community public');

    // A device with nothing stored is not a passing device.
    await page.getByRole('combobox', { name: 'Finding status' }).selectOption('not_evaluated');
    const unjudged = findings.getByRole('row').filter({ hasText: 'unjudged-sw' });
    await expect(unjudged.first()).toContainText('Not evaluated');
    await expect(page.getByText('had no stored configuration, so')).toBeVisible();

    // The rule itself: what it checks, why it matters, what would fix it, and
    // what it concludes about a device chosen here and now.
    await page.getByRole('combobox', { name: 'Finding status' }).selectOption('fail');
    await findings.getByRole('button', { name: 'Only SSH version 2 is accepted' }).first().click();
    const drawer = page.getByRole('dialog').filter({ hasText: 'Why it matters' });
    await expect(drawer).toContainText('ip ssh version 2');
    await expect(drawer).toContainText('Checked:');
    await expect(drawer).toContainText('What would be sent to fix it');

    await drawer.getByRole('combobox', { name: 'Device to test the rule on' }).fill('Policy/SW1');
    await page.getByRole('option', { name: /Policy\/SW1/ }).first().click();
    await drawer.getByRole('button', { name: 'Test' }).click();
    await expect(page.getByTestId('rule-test')).toContainText('Fail', { timeout: 30_000 });
    await page.keyboard.press('Escape');

    expect(console.messages).toEqual([]);
  });
});

test.describe('drift from the intended configuration', () => {
  test('is a diff, and fixing it drafts a change without touching the device', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await startLab(rpc, 'Drift', ['SW1']);
    const device = lab.devices.SW1!;
    await atTheConsole(rpc, lab.id, 'SW1', CONFIGURED);
    await backUp(rpc, [device]);
    const before = await rpc<{ text: string }>('labs.node.config', { lab_id: lab.id, node: 'SW1' });

    await openApp(page, `/config/intended?device=${device}`, { backend });

    // Writing down what the device should have. The editor is CodeMirror, so
    // its document is a text box with a name.
    await page
      .getByRole('textbox', { name: 'Intended configuration' })
      .fill('service password-encryption\nip ssh version 2\nntp server 10.0.0.1\n');
    await page.getByRole('button', { name: 'Save the intended configuration' }).click();

    // The drift is the engine's: the lines it would send, in order.
    await expect(page.getByText('3 lines out of step')).toBeVisible({ timeout: 30_000 });
    const commands = page.getByTestId('drift-command');
    await expect(commands).toHaveCount(3);
    await expect(commands.nth(0)).toContainText('service password-encryption');
    await expect(commands.nth(2)).toContainText('ntp server 10.0.0.1');

    // Handing it on drafts a change and says so before it does anything.
    await page.getByRole('button', { name: 'Fix the drift…' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('nothing is sent to Drift/SW1 until somebody approves');
    await dialog.getByRole('button', { name: 'Draft the change' }).click();
    await expect(page.getByText('A change was drafted')).toBeVisible({ timeout: 30_000 });

    // A change exists, it carries those lines, and the device is untouched.
    const changes = await rpc<{ id: string; state: string; title: string }[]>('changes.list', {});
    const drafted = changes.find((change) => change.title.includes('Drift/SW1'));
    expect(drafted, 'the hand-over should have created a change').toBeTruthy();
    expect(drafted!.state).not.toBe('applied');
    const after = await rpc<{ text: string }>('labs.node.config', { lab_id: lab.id, node: 'SW1' });
    expect(after.text).toBe(before.text);
    expect(after.text).not.toContain('ip ssh version 2');

    // And the screen can walk there.
    await page.getByRole('button', { name: 'Open the change' }).click();
    await pageLoaded(page);
    await expect(page.getByRole('heading', { level: 1 })).toContainText('drift', {
      timeout: 30_000,
    });

    expect(console.messages).toEqual([]);
  });
});
