import { describe, expect, it } from 'vitest';
import {
  SEVERITY_BANDS,
  bandKept,
  bandOf,
  facilityName,
  filterFrom,
  isNarrowed,
  parseQuery,
  severityName,
  severityTone,
  span,
  withoutSeverity,
} from './query';

describe('the query bar', () => {
  it('hands words, phrases and exclusions to the backend untouched', () => {
    // The backend already understands all three, and in the message, the
    // source and the mnemonic. Parsing them again here would be a second
    // language that disagrees with the first.
    const parsed = parseQuery('LINK-3-UPDOWN "changed state to down" -administratively');
    expect(parsed.text).toBe('LINK-3-UPDOWN "changed state to down" -administratively');
    expect(parsed.unknown).toEqual([]);
  });

  it('lifts out the fields that are columns, and leaves the rest as text', () => {
    const parsed = parseQuery('if:Gi1/0/3 mnemonic:LINK-3-UPDOWN from:10.0.0.9 flapping');
    expect(parsed).toMatchObject({
      interface: 'Gi1/0/3',
      mnemonic: 'LINK-3-UPDOWN',
      source: '10.0.0.9',
      text: 'flapping',
    });
  });

  it('takes a facility by name or by number, and nothing else', () => {
    expect(parseQuery('facility:local7').facility).toBe(23);
    expect(parseQuery('fac:4').facility).toBe(4);
    expect(parseQuery('facility:0').facility).toBe(0);
    const wrong = parseQuery('facility:loud');
    expect(wrong.facility).toBeUndefined();
    expect(wrong.unknown).toEqual(['facility:loud']);
  });

  it('does not mistake an address or an exclusion for a field', () => {
    // A colon inside a word is not a field, and `-up` is the backend's
    // exclusion marker, not a key with an empty name.
    const parsed = parseQuery('fe80::1 -up 45:');
    expect(parsed.text).toBe('fe80::1 -up 45:');
    expect(parsed.unknown).toEqual([]);
  });

  it('says when a field does not exist instead of searching for the whole token', () => {
    const parsed = parseQuery('severity:3 switch');
    expect(parsed.unknown).toEqual(['severity']);
    expect(parsed.text).toBe('switch');
  });

  it('keeps a quoted field value in one piece, without its quotes', () => {
    const parsed = parseQuery('if:"GigabitEthernet 1/0/3" down');
    expect(parsed.interface).toBe('GigabitEthernet 1/0/3');
    expect(parsed.text).toBe('down');
  });

  it('accepts is:unattributed, which is a column and not a word to search for', () => {
    expect(parseQuery('is:unattributed').unattributed).toBe(true);
    expect(parseQuery('is:unattributed').text).toBe('');
    expect(parseQuery('is:lost').unknown).toEqual(['is:lost']);
  });
});

describe('the filter a search becomes', () => {
  it('sends only the fields that were asked for', () => {
    const filter = filterFrom({
      query: '  ',
      stream: 'syslog',
      deviceId: null,
      maxSeverity: undefined,
      since: 1_000,
    });
    expect(filter).toEqual({ stream: 'syslog', since: 1_000 });
    expect(isNarrowed(filter)).toBe(false);
  });

  it('counts a severity cut as narrowing, because it hides lines', () => {
    const filter = filterFrom({
      query: '',
      stream: 'syslog',
      deviceId: null,
      maxSeverity: 3,
      since: 1_000,
    });
    expect(isNarrowed(filter)).toBe(true);
    expect(withoutSeverity(filter)).toEqual({ stream: 'syslog', since: 1_000 });
  });

  it('passes the device and the time range through', () => {
    expect(
      filterFrom({
        query: 'if:Gi0/1',
        stream: '*',
        deviceId: 'dev-1',
        maxSeverity: 4,
        since: 10,
        until: 20,
      }),
    ).toEqual({
      stream: '*',
      device_id: 'dev-1',
      max_severity: 4,
      interface: 'Gi0/1',
      since: 10,
      until: 20,
    });
  });
});

describe('severity, which counts down and can be absent', () => {
  it('names the scale the way the backend does', () => {
    expect(severityName(0)).toBe('emergency');
    expect(severityName(7)).toBe('debug');
    expect(severityName(null)).toBe('unknown');
    expect(severityName(9)).toBe('unknown');
    expect(facilityName(23)).toBe('local7');
    expect(facilityName(null)).toBe('unknown');
  });

  it('puts a line with no severity in its own band, never in debug', () => {
    expect(bandOf(3)).toBe('error');
    expect(bandOf(4)).toBe('warning');
    expect(bandOf(6)).toBe('notice');
    expect(bandOf(7)).toBe('debug');
    expect(bandOf(null)).toBe('unsaid');
    expect(severityTone(null)).toBe('idle');
  });

  it('hides every line with no severity as soon as any severity filter is set', () => {
    // SQL's `severity <= 3` is false for NULL, so the lines that never said
    // disappear under any cut. The screen has to say so.
    const unsaid = SEVERITY_BANDS.find((band) => band.id === 'unsaid');
    expect(unsaid && bandKept(unsaid, undefined)).toBe(true);
    expect(unsaid && bandKept(unsaid, 6)).toBe(false);
  });
});

describe('time ranges', () => {
  it('falls back to the last hour for a range nobody offers', () => {
    expect(span('nonsense').id).toBe('1h');
    expect(span(null).id).toBe('1h');
    expect(span('7d').ms).toBe(7 * 86_400_000);
  });
});
