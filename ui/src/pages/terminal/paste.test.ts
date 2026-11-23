import { describe, expect, it } from 'vitest';
import type { TerminalLineClass } from '@/api/types/terminal';
import {
  asTyped,
  bracketed,
  carriesText,
  droppedText,
  hasSeveralLines,
  needsConfirmation,
  normaliseLines,
  pasteConfirmLabel,
  pasteKeyRoute,
  pasteRisk,
  pasteSummary,
  pasteTitle,
  splitLines,
  TAB_DRAG_TYPE,
} from './paste';

const line = (text: string, cls: TerminalLineClass['class'], reason = ''): TerminalLineClass => ({
  line: text,
  class: cls,
  reason,
});

describe('splitLines', () => {
  it('keeps blank lines and takes any line ending', () => {
    expect(splitLines('a\r\nb\n\nc')).toEqual(['a', 'b', '', 'c']);
  });
});

describe('normaliseLines', () => {
  it('makes one line ending of the three there are', () => {
    // Windows, Unix, and the bare carriage return of a captured session.
    expect(normaliseLines('a\r\nb\nc\rd')).toBe('a\nb\nc\nd');
    expect(normaliseLines('a\r\n\r\nb')).toBe('a\n\nb');
  });
});

describe('asTyped', () => {
  it('sends one carriage return for each line ending, as the Enter key does', () => {
    expect(asTyped('conf t\r\nint gi0/1\r\n')).toBe('conf t\rint gi0/1\r');
    expect(asTyped('conf t\nint gi0/1\n')).toBe('conf t\rint gi0/1\r');
    expect(asTyped('conf t\rint gi0/1')).toBe('conf t\rint gi0/1');
  });

  it('never leaves a line feed for the device to take as a second Enter', () => {
    // `reload` asks `[confirm]`; the line feed of a CRLF would answer it.
    expect(asTyped('reload\r\n')).not.toContain('\n');
  });
});

describe('bracketed', () => {
  it('puts the paste between the markers the far end asked for', () => {
    expect(bracketed('ls\rpwd\r')).toBe('\u001b[200~ls\rpwd\r\u001b[201~');
  });

  it('does not let the text close the bracket itself', () => {
    // Otherwise what follows the marker would run as if it had been typed.
    expect(bracketed('echo hi\u001b[201~rm -rf /\r')).toBe('\u001b[200~echo hirm -rf /\r\u001b[201~');
    // Not even by hiding one marker inside another.
    expect(bracketed('\u001b[20\u001b[201~1~x')).toBe('\u001b[200~x\u001b[201~');
  });
});

describe('hasSeveralLines', () => {
  it('does not count the newline that ends a single command', () => {
    expect(hasSeveralLines('show version\n')).toBe(false);
    expect(hasSeveralLines('show version')).toBe(false);
    expect(hasSeveralLines('show version\nshow ip route')).toBe(true);
    expect(hasSeveralLines('show version\n\n')).toBe(true);
  });

  it('counts lines the same whatever ends them', () => {
    expect(hasSeveralLines('show version\r\n')).toBe(false);
    expect(hasSeveralLines('show version\r')).toBe(false);
    expect(hasSeveralLines('conf t\rreload')).toBe(true);
  });
});

describe('needsConfirmation', () => {
  it('asks about anything with more than one line', () => {
    expect(needsConfirmation('conf t\nint gi0/1\n', null)).toBe(true);
  });

  it('lets a read-only line through', () => {
    expect(needsConfirmation('show version', [line('show version', 'read_only')])).toBe(false);
  });

  it('asks about a single line the policy flags', () => {
    expect(needsConfirmation('reload', [line('reload', 'dangerous', 'disruptive')])).toBe(true);
    expect(needsConfirmation('conf t', [line('conf t', 'change', 'not read-only')])).toBe(true);
  });

  it('does not block a one-liner when the policy could not be asked', () => {
    expect(needsConfirmation('show version', null)).toBe(false);
  });
});

describe('pasteRisk', () => {
  it('counts the classes and keeps the worst reason', () => {
    expect(
      pasteRisk([
        line('show version', 'read_only', 'read-only command'),
        line('conf t', 'change', 'not on the read-only list'),
        line('reload', 'dangerous', '`reload` is disruptive'),
      ]),
    ).toEqual({ lines: 3, changes: 1, dangerous: 1, headline: '`reload` is disruptive' });
  });

  it('has no headline when everything is read-only', () => {
    expect(pasteRisk([line('show version', 'read_only', 'read-only command')]).headline).toBeNull();
  });
});

describe('pasteTitle', () => {
  it('names where the text came from and where it is going', () => {
    expect(pasteTitle({ source: 'paste', title: 'SW1', targets: 1 })).toBe('Paste into SW1?');
    expect(pasteTitle({ source: 'snippet', title: 'SW1', targets: 1 })).toBe('Send this snippet into SW1?');
    expect(pasteTitle({ source: 'drop', title: 'SW1', targets: 1 })).toBe('Drop this text into SW1?');
    expect(pasteTitle({ source: 'paste', title: 'SW1', targets: 3 })).toBe('Paste into 3 sessions?');
  });
});

describe('pasteKeyRoute', () => {
  const ctrlShiftV = { ctrlKey: true, shiftKey: true, metaKey: false };
  const cmdV = { ctrlKey: false, shiftKey: false, metaKey: true };
  const cmdShiftV = { ctrlKey: false, shiftKey: true, metaKey: true };

  it('leaves Ctrl+Shift+V to the browser everywhere it is the browser\'s own paste', () => {
    // No clipboard is read, so it works in the desktop shell and over plain HTTP.
    expect(pasteKeyRoute(ctrlShiftV, { mac: false, desktop: false })).toBe('browser');
    expect(pasteKeyRoute(ctrlShiftV, { mac: false, desktop: true })).toBe('browser');
  });

  it('leaves the Mac its own paste key', () => {
    expect(pasteKeyRoute(cmdV, { mac: true, desktop: false })).toBe('browser');
    expect(pasteKeyRoute(cmdV, { mac: true, desktop: true })).toBe('browser');
    expect(pasteKeyRoute(cmdShiftV, { mac: true, desktop: false })).toBe('browser');
  });

  it('reads the clipboard for a chord that nothing on the platform is bound to', () => {
    expect(pasteKeyRoute(ctrlShiftV, { mac: true, desktop: false })).toBe('clipboard');
    expect(pasteKeyRoute(ctrlShiftV, { mac: true, desktop: true })).toBe('clipboard');
    // The shell's Edit menu has Paste, but no "Paste and Match Style".
    expect(pasteKeyRoute(cmdShiftV, { mac: true, desktop: true })).toBe('clipboard');
  });
});

describe('droppedText', () => {
  const transfer = (data: Record<string, string>) => ({
    types: Object.keys(data),
    getData: (type: string) => data[type] ?? '',
  });

  it('takes plain text', () => {
    expect(droppedText(transfer({ 'text/plain': 'show version', 'text/html': '<b>show version</b>' }))).toBe(
      'show version',
    );
  });

  it('is not fooled by a tab being dragged, whose text is a session id', () => {
    expect(droppedText(transfer({ 'text/plain': 'sess-1', [TAB_DRAG_TYPE]: 'sess-1' }))).toBeNull();
  });

  it('can tell from the types alone, before the drop, whether it would take it', () => {
    expect(carriesText(['text/plain', 'text/html'])).toBe(true);
    expect(carriesText(['text/plain', TAB_DRAG_TYPE])).toBe(false);
    expect(carriesText(['Files'])).toBe(false);
  });

  it('leaves files and empty drags alone', () => {
    expect(droppedText({ types: ['Files'], getData: () => '' })).toBeNull();
    expect(droppedText(transfer({ 'text/plain': '' }))).toBeNull();
    expect(droppedText(transfer({ 'text/uri-list': 'https://example.com' }))).toBeNull();
  });
});

describe('pasteTitle', () => {
  it('names what is being done by the door the text came through', () => {
    expect(pasteTitle({ source: 'paste', title: 'SW1', targets: 1 })).toBe('Paste into SW1?');
    expect(pasteTitle({ source: 'snippet', title: 'SW1', targets: 1 })).toBe('Send this snippet into SW1?');
    expect(pasteTitle({ source: 'clip', title: 'SW1', targets: 1 })).toBe('Type this clip into SW1?');
    // Several sessions are counted, not named after the first of them.
    expect(pasteTitle({ source: 'snippet', title: 'SW1', targets: 3 })).toBe('Send this snippet into 3 sessions?');
  });

  it('calls the yes by the verb of the question', () => {
    expect(pasteConfirmLabel('paste')).toBe('Paste anyway');
    expect(pasteConfirmLabel('drop')).toBe('Paste anyway');
    expect(pasteConfirmLabel('snippet')).toBe('Send anyway');
    expect(pasteConfirmLabel('clip')).toBe('Type anyway');
  });
});

describe('pasteSummary', () => {
  it('says what will happen in one sentence', () => {
    expect(pasteSummary('show version', null)).toMatch(/^one line will be sent/);
    expect(pasteSummary('a\nb\nc', { lines: 3, changes: 2, dangerous: 1, headline: null })).toBe(
      '3 lines, 1 disruptive and 2 that change the configuration.',
    );
  });
});
