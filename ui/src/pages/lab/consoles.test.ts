import { describe, expect, it } from 'vitest';
import type { SessionInfo, SessionState } from '@/api/types/terminal';
import { neighbourTab, openSessionFor, tabsForLab } from './consoles';

function session(id: string, deviceId: string | null, state: SessionState = 'open', openedAt = 1): SessionInfo {
  return {
    id,
    title: id,
    kind: 'sim',
    device_id: deviceId,
    lab: true,
    opened_at: openedAt,
    state,
    cols: 120,
    rows: 40,
    info: { kind: 'sim', peer: 'lab:X', legacy_crypto: false, via: [] },
    bytes_in: 0,
    bytes_out: 0,
  };
}

const lab = { devices: { AS1: 'dev-as1', DS1: 'dev-ds1' } };

describe('tabsForLab', () => {
  it('is the backend’s sessions, not this window’s memory', () => {
    const tabs = tabsForLab(
      [session('s1', 'dev-as1'), session('s2', 'dev-other'), session('s3', 'dev-ds1', 'closed')],
      lab,
    );
    expect(tabs.map((tab) => tab.node)).toEqual(['AS1', 'DS1']);
    expect(tabs[1]?.closed).toBe(true);
  });

  it('ignores a session with no device, such as a local shell', () => {
    expect(tabsForLab([session('s1', null)], lab)).toEqual([]);
  });

  it('is empty for a lab whose nodes have no sessions', () => {
    expect(tabsForLab([session('s1', 'dev-elsewhere')], lab)).toEqual([]);
  });
});

describe('openSessionFor', () => {
  it('finds the console already open on a device, so a second click does not open a second tab', () => {
    const found = openSessionFor([session('s1', 'dev-as1')], 'dev-as1');
    expect(found?.id).toBe('s1');
  });

  it('does not reuse a session whose far end has gone', () => {
    expect(openSessionFor([session('s1', 'dev-as1', 'closed')], 'dev-as1')).toBeUndefined();
  });
});

describe('neighbourTab', () => {
  const tabs = tabsForLab(
    [session('s1', 'dev-as1', 'open', 1), session('s2', 'dev-ds1', 'open', 2)],
    lab,
  );

  it('falls back to the tab before the one closing', () => {
    expect(neighbourTab(tabs, 's2')).toBe('s1');
  });

  it('falls back to the one after when the first is closing', () => {
    expect(neighbourTab(tabs, 's1')).toBe('s2');
  });

  it('has nothing left when the last tab closes', () => {
    expect(neighbourTab([tabs[0]!], 's1')).toBeNull();
  });
});
