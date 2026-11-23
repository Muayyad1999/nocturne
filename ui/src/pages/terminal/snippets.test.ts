/**
 * The arithmetic of the snippets drawer.
 *
 * Every engine answer in here — the lists, the refusals, the results of a
 * send and of an import — was recorded from a running `nocturne-server`
 * (ids shortened); none was written from the specification.
 */

import { describe, expect, it } from 'vitest';
import { RpcError } from '@/api/errors';
import type { SessionInfo, Snippet, SnippetImportResult, TerminalLineClass } from '@/api/types/terminal';
import {
  changedHash,
  exportFilename,
  importSummary,
  initialFields,
  initialTargets,
  isRequired,
  lineCount,
  mayApply,
  notChosenBecause,
  openTargets,
  refusedSessions,
  renderRefusal,
  sendReport,
  splitByChoice,
  tagCounts,
  toggleTag,
  unfilled,
  valuesFor,
  withTags,
  worstClasses,
} from './snippets';

const SAY: Snippet = {
  id: '2p4lig3fvw64ahel',
  name: 'Say',
  text: 'say {{what}} on {{port=Gi0/1}}\nsay done\n',
  profile: null,
  tags: ['daily', 'ports'],
  created_at: 1795400988532,
  updated_at: 1795400988532,
  variables: [{ name: 'what' }, { name: 'port', default: 'Gi0/1' }],
};

/** `terminal.snippets.list {session_id}` for a session to a `cisco_ios` device. */
const FOR_IOS: Snippet[] = [
  { id: '637vor23', name: 'Port bounce', text: 'say generic bounce {{port}}', profile: null, tags: [], created_at: 1, updated_at: 1, variables: [{ name: 'port' }], chosen: false, chosen_instead: 'mldeddkr' },
  { id: 'mldeddkr', name: 'Port bounce', text: 'say ios bounce {{port}}', profile: 'cisco_ios', tags: [], created_at: 1, updated_at: 1, variables: [{ name: 'port' }], chosen: true },
  { ...SAY, chosen: true },
  { id: '3zukjtz7', name: 'Version', text: 'show version', profile: null, tags: ['daily'], created_at: 1, updated_at: 1, variables: [], chosen: true },
];

/** The same library for a session whose device type nothing knows. */
const FOR_UNKNOWN: Snippet[] = [
  { ...FOR_IOS[0]!, chosen: true, chosen_instead: undefined },
  { ...FOR_IOS[1]!, chosen: false },
  { ...SAY, chosen: true },
  { ...FOR_IOS[3]! },
];

describe('the values a form sends', () => {
  it('starts with every default on screen and every required variable empty', () => {
    expect(initialFields(SAY.variables)).toEqual({ what: '', port: 'Gi0/1' });
    expect(isRequired({ name: 'what' })).toBe(true);
    // An empty default is a default: `{{descr=}}` may be left out.
    expect(isRequired({ name: 'descr', default: '' })).toBe(false);
    expect(initialFields([{ name: 'descr', default: '' }])).toEqual({ descr: '' });
  });

  it('leaves out a required variable that is empty, so the engine says it is missing', () => {
    expect(valuesFor(SAY.variables, { what: '', port: 'Gi0/1' })).toEqual({ port: 'Gi0/1' });
    expect(unfilled(SAY.variables, { what: '', port: 'Gi0/1' })).toEqual(['what']);
    expect(unfilled(SAY.variables, { what: 'x', port: '' })).toEqual([]);
  });

  it('sends a field exactly as it stands: nothing trimmed, nothing filtered', () => {
    // A line break, a tab and trailing spaces all reach the engine, which
    // is the one that says what a value may hold.
    const fields = { what: ' a\nb\t ', port: '' };
    expect(valuesFor(SAY.variables, fields)).toEqual({ what: ' a\nb\t ', port: '' });
  });

  it('sends an emptied default as empty, not as the default again', () => {
    expect(valuesFor(SAY.variables, { what: 'x', port: '' })).toEqual({ what: 'x', port: '' });
  });
});

describe('a refused render', () => {
  it('puts the engine’s clause beside the field it is about', () => {
    const error = new RpcError(
      'invalid',
      '`Say` cannot be filled in: the value of `what` contains a line break; the value of `port` contains a control character (U+0009)',
      { variables: [{ name: 'what', problem: 'line_break' }, { name: 'port', problem: 'control_character' }] },
    );
    expect(renderRefusal(error, SAY.variables)).toEqual({
      fields: {
        what: { problem: 'line_break', sentence: 'The value of `what` contains a line break.' },
        port: { problem: 'control_character', sentence: 'The value of `port` contains a control character (U+0009).' },
      },
      general: null,
    });
  });

  it('marks a variable that only lacks a value as missing, in the engine’s words', () => {
    const error = new RpcError('invalid', '`Say` cannot be filled in: `what` has no value and no default', {
      variables: [{ name: 'what', problem: 'missing' }],
    });
    expect(renderRefusal(error, SAY.variables)).toEqual({
      fields: { what: { problem: 'missing', sentence: '`what` has no value and no default.' } },
      general: null,
    });
  });

  it('gives every named field the whole sentence when the clauses cannot be told apart', () => {
    // Not a recorded answer: a message shaped unlike today's, to show what happens then.
    const error = new RpcError('invalid', 'two values are wrong', {
      variables: [{ name: 'what', problem: 'too_long' }, { name: 'port', problem: 'too_long' }],
    });
    const refusal = renderRefusal(error, SAY.variables);
    expect(refusal.fields.what?.sentence).toBe('Two values are wrong.');
    expect(refusal.fields.port?.sentence).toBe('Two values are wrong.');
  });

  it('says a refusal that is about no field of the form where the preview is', () => {
    const notValues = new RpcError('invalid', '`Old` cannot be sent as it is: line 1, column 16: the default of `port` contains a control character (U+001B)');
    expect(renderRefusal(notValues, [])).toEqual({
      fields: {},
      general: '`Old` cannot be sent as it is: line 1, column 16: the default of `port` contains a control character (U+001B)',
    });

    const unknown = new RpcError('invalid', '`Say` cannot be filled in: `vlan` is not a variable of this snippet', {
      variables: [{ name: 'vlan', problem: 'unknown' }],
    });
    expect(renderRefusal(unknown, SAY.variables)).toEqual({
      fields: {},
      general: '`vlan` is not a variable of this snippet.',
    });
    expect(renderRefusal(new Error('the engine is not answering'), SAY.variables).general).toBe(
      'the engine is not answering',
    );
  });
});

describe('a refused send', () => {
  it('reads the hash the snippet renders to now out of a conflict', () => {
    const error = new RpcError(
      'conflict',
      '`Say` no longer renders to the text that was confirmed (the snippet or a value has changed); render it again and confirm what it shows now',
      { sha256: 'fffbddf1bd3ed2af219e53d16b6debc50ce27325084c010f19d971083c086c76' },
    );
    expect(changedHash(error)).toBe('fffbddf1bd3ed2af219e53d16b6debc50ce27325084c010f19d971083c086c76');
    expect(changedHash(new RpcError('invalid', 'no'))).toBeNull();
    expect(changedHash(new Error('no'))).toBeNull();
  });

  it('reads which sessions were refused, and why', () => {
    const error = new RpcError('not_found', 'nothing was sent to any session: terminal session `nope` was not found', {
      refused: [{ session_id: 'nope', code: 'not_found', message: 'terminal session `nope` was not found' }],
    });
    expect(refusedSessions(error)).toEqual([
      { session_id: 'nope', code: 'not_found', message: 'terminal session `nope` was not found' },
    ]);
    expect(refusedSessions(new RpcError('conflict', 'changed', { sha256: 'x' }))).toEqual([]);
  });
});

describe('the variant the engine chose', () => {
  it('offers what the engine chose and keeps the rest as the others', () => {
    const rows = splitByChoice(FOR_IOS);
    expect(rows.offered.map((row) => row.id)).toEqual(['mldeddkr', SAY.id, '3zukjtz7']);
    expect(rows.others.map((row) => row.id)).toEqual(['637vor23']);
  });

  it('makes no choice of its own when the engine was asked for no session', () => {
    const plain = FOR_IOS.map(({ chosen: _chosen, chosen_instead: _instead, ...row }) => row);
    expect(splitByChoice(plain)).toEqual({ offered: plain, others: [] });
  });

  it('says which variant stands in, as the engine named it', () => {
    expect(notChosenBecause(FOR_IOS[0]!, FOR_IOS)).toBe('Not used in this session: the cisco_ios variant is.');
    expect(notChosenBecause(FOR_IOS[1]!, FOR_IOS)).toBe('');
  });

  it('does not say a variant is wrong for a session whose device type nothing knows', () => {
    const rows = splitByChoice(FOR_UNKNOWN);
    expect(rows.others.map((row) => row.id)).toEqual(['mldeddkr']);
    expect(notChosenBecause(FOR_UNKNOWN[1]!, FOR_UNKNOWN)).toBe(
      'For cisco_ios devices. This session’s device type is not known, so it is not offered on a guess.',
    );
  });
});

describe('tags', () => {
  it('counts each tag once per snippet, most used first', () => {
    expect(tagCounts(FOR_IOS)).toEqual([
      { tag: 'daily', count: 2 },
      { tag: 'ports', count: 1 },
    ]);
    expect(tagCounts([{ ...SAY, tags: ['a', 'a'] }])).toEqual([{ tag: 'a', count: 1 }]);
  });

  it('keeps the snippets that carry every picked tag', () => {
    expect(withTags(FOR_IOS, []).length).toBe(4);
    expect(withTags(FOR_IOS, ['daily']).map((row) => row.name)).toEqual(['Say', 'Version']);
    expect(withTags(FOR_IOS, ['daily', 'ports']).map((row) => row.name)).toEqual(['Say']);
    expect(toggleTag(['daily'], 'ports')).toEqual(['daily', 'ports']);
    expect(toggleTag(['daily', 'ports'], 'daily')).toEqual(['ports']);
  });
});

describe('where a snippet goes', () => {
  const session = (id: string, state: SessionInfo['state'], device: string | null = null): SessionInfo => ({
    id,
    title: id.toUpperCase(),
    kind: 'telnet',
    device_id: device,
    lab: false,
    opened_at: 1,
    state,
    cols: 80,
    rows: 24,
    info: { peer: '10.0.0.1:23', legacy_crypto: false, via: [] },
    bytes_in: 0,
    bytes_out: 0,
  });
  const open = openTargets([session('a', 'open', 'dev-a'), session('b', 'closed'), session('c', 'open')]);

  it('offers the sessions that are open, with the device whose policy judges them', () => {
    expect(open).toEqual([
      { sessionId: 'a', title: 'A', deviceId: 'dev-a' },
      { sessionId: 'c', title: 'C', deviceId: null },
    ]);
  });

  it('starts with the session in front, and with the broadcast when it is part of one', () => {
    expect(initialTargets(open, 'a', [])).toEqual(['a']);
    expect(initialTargets(open, 'a', ['a', 'c'])).toEqual(['a', 'c']);
    // The session in front is held back from the broadcast: it alone.
    expect(initialTargets(open, 'a', ['c', 'x'])).toEqual(['a']);
    // Closed, or none in front: nothing is ticked for the person.
    expect(initialTargets(open, 'b', [])).toEqual([]);
    expect(initialTargets(open, null, ['a', 'c'])).toEqual([]);
    expect(initialTargets(open, 'a', ['a', 'b'])).toEqual(['a']);
  });

  it('judges a line by the worst any of the devices thinks of it', () => {
    const reads = (line: string): TerminalLineClass => ({ line, class: 'read_only', reason: 'reads' });
    const one = [reads('show clock'), reads('reload')];
    const other: TerminalLineClass[] = [reads('show clock'), { line: 'reload', class: 'dangerous', reason: 'restarts the device' }];
    expect(worstClasses([one, other])).toEqual([reads('show clock'), other[1]]);
    expect(worstClasses([other, one])).toEqual([reads('show clock'), other[1]]);
    expect(worstClasses([])).toEqual([]);
  });

  it('reports a send session by session', () => {
    const whole = sendReport(
      { sent: [{ session_id: 'a', lines: 2 }, { session_id: 'c', lines: 2 }], failed: [] },
      open,
    );
    expect(whole).toMatchObject({ complete: true, headline: 'Typed into 2 sessions' });
    expect(sendReport({ sent: [{ session_id: 'a', lines: 1 }], failed: [] }, open)).toMatchObject({
      complete: true,
      headline: 'Typed into A',
      sessions: [{ sessionId: 'a', title: 'A', ok: true, says: 'Typed 1 line; the Enter is yours.' }],
    });
  });

  it('names who got it and who did not when a connection died under the send', () => {
    const partial = sendReport(
      {
        sent: [{ session_id: 'a', lines: 1 }],
        failed: [{ session_id: 'c', code: 'closed', message: 'the session to DEAD has ended (connection closed); open a new one' }],
      },
      open,
    );
    expect(partial.complete).toBe(false);
    expect(partial.headline).toBe('Typed into 1 of 2 sessions');
    expect(partial.sessions).toEqual([
      { sessionId: 'a', title: 'A', ok: true, says: 'Typed 1 line; the Enter is yours.' },
      { sessionId: 'c', title: 'C', ok: false, says: 'the session to DEAD has ended (connection closed); open a new one' },
    ]);
    const none = sendReport({ sent: [], failed: partial.sessions.slice(1).map(() => ({ session_id: 'c', code: 'closed', message: 'gone' })) }, open);
    expect(none).toMatchObject({ complete: false, headline: 'Not typed into the session' });
  });
});

describe('an import, told to the person', () => {
  const result = (over: Partial<SnippetImportResult>): SnippetImportResult => ({
    created: 0,
    replaced: 0,
    skipped: 0,
    renamed: 0,
    problems: [],
    applied: false,
    dry_run: true,
    ...over,
  });

  it('says what a dry run would do, and afterwards what was done', () => {
    expect(importSummary(result({ created: 2, renamed: 1 }))).toBe('2 snippets would be added (1 under a new name).');
    expect(importSummary(result({ created: 1, replaced: 1 }))).toBe('1 snippet would be added, 1 would be replaced.');
    expect(importSummary(result({ created: 1, skipped: 1 }))).toBe(
      '1 snippet would be added, 1 would be left as they are here.',
    );
    expect(importSummary(result({ created: 2, renamed: 1, applied: true, dry_run: false }))).toBe(
      '2 snippets were added (1 under a new name).',
    );
    expect(importSummary(result({}))).toBe('Nothing would change.');
    expect(importSummary(result({ skipped: 3, applied: true, dry_run: false }))).toBe('3 were left as they are here.');
  });

  it('says that nothing is imported while one entry is wrong, and does not allow it', () => {
    const bad = result({
      problems: [{ index: 2, name: 'Bad', problem: 'the text of `Bad`, line 1, column 6: `{{` opens a variable that is not closed on this line' }],
    });
    expect(importSummary(bad)).toBe(
      '1 entry cannot be imported, so nothing would be: an import is the whole document or none of it.',
    );
    expect(mayApply(bad)).toBe(false);
    expect(mayApply(result({ created: 1 }))).toBe(true);
    // Not before a dry run, and not on the strength of an import that already ran.
    expect(mayApply(null)).toBe(false);
    expect(mayApply(result({ created: 1, applied: true, dry_run: false }))).toBe(false);
  });

  it('names the exported file by the day', () => {
    expect(exportFilename(new Date(2026, 10, 3, 9, 5))).toBe('nocturne-snippets-20261103.json');
  });
});

describe('lineCount', () => {
  it('does not count the line break that ends a snippet as a line', () => {
    expect(lineCount('show version')).toBe(1);
    expect(lineCount('show version\n')).toBe(1);
    expect(lineCount('a\r\nb\r\n')).toBe(2);
    expect(lineCount('a\n\nb')).toBe(3);
  });
});
