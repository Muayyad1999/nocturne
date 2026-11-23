import { describe, expect, it } from 'vitest';
import type { Forwarder, ForwarderStatus } from '@/api/types/logs';
import { FORWARDERS, STATUS, STATUS_WITH_QUEUE_BYTES } from './fixtures';
import {
  destinationOf,
  draftOfForwarder,
  fieldOfForwardError,
  floorPhrase,
  rowsOf,
  saveParams,
  sendsAudit,
  sentMeaning,
  stateView,
  tokenFate,
  waitingOf,
  willHaveDestination,
} from './forward';

const [SYSLOG, HTTPS] = FORWARDERS as [Forwarder, Forwarder];

describe('a destination that can be written and never read', () => {
  it('has no URL to show for a stored HTTPS forwarder: only that one is stored', () => {
    expect(HTTPS.has_secret).toBe(true);
    expect(destinationOf(HTTPS)).toBe('A URL in the vault (never shown)');
    // The run that produced this fixture saved a URL and a token with it.
    // Neither is anywhere in what `logs.forward.list` answered.
    expect(JSON.stringify(HTTPS)).not.toContain('://');
    expect(JSON.stringify(HTTPS)).not.toContain('11111111');
    expect(destinationOf({ ...HTTPS, has_secret: false })).toBe('No destination stored');
    expect(destinationOf(SYSLOG)).toBe('127.0.0.1:59849');
  });

  it('sends neither a URL nor a token when neither was typed, so what is stored stays', () => {
    const draft = { ...draftOfForwarder(HTTPS), name: 'Splunk (renamed)' };
    const { params } = saveParams(draft);
    expect(params).not.toHaveProperty('url');
    expect(params).not.toHaveProperty('token');
    expect(params).toMatchObject({ id: HTTPS.id, name: 'Splunk (renamed)', kind: 'https_json' });
  });

  it('starts every edit with nothing typed, whatever the vault holds', () => {
    const draft = draftOfForwarder(HTTPS);
    expect(draft.url).toBeUndefined();
    expect(draft.token).toBeUndefined();
  });

  it('sends what was typed, and only that', () => {
    const { params } = saveParams({
      ...draftOfForwarder(),
      name: 'Splunk',
      kind: 'https_json',
      url: ' https://collector.example.net:8088/services/collector/event ',
      token: ' 11111111-2222-3333-4444-555555555555 ',
    });
    expect(params.url).toBe('https://collector.example.net:8088/services/collector/event');
    expect(params.token).toBe('11111111-2222-3333-4444-555555555555');
    // An HTTPS forwarder has no host, port or format to send.
    expect(params).not.toHaveProperty('host');
    expect(params).not.toHaveProperty('port');
    expect(params).not.toHaveProperty('format');
  });

  it('never sends an empty URL, which the engine reads as "forget the destination"', () => {
    const { params } = saveParams({ ...draftOfForwarder(HTTPS), url: '   ' });
    expect(params).not.toHaveProperty('url');
  });

  it('removes the token by sending it empty, and says that is what will happen', () => {
    const draft = { ...draftOfForwarder(HTTPS), token: '' };
    expect(saveParams(draft).params.token).toBe('');
    expect(tokenFate(draft, HTTPS)).toBe('removed');
  });

  it('warns that a new URL does not carry the stored token with it', () => {
    const moved = { ...draftOfForwarder(HTTPS), url: 'https://new.example.net/collector' };
    expect(tokenFate(moved, HTTPS)).toBe('dropped-with-new-url');
    // With the token typed again there is nothing to warn about.
    expect(tokenFate({ ...moved, token: 'again' }, HTTPS)).toBe('replaced');
  });

  it('says the stored token is kept when nothing was typed, and has nothing to say where none can be', () => {
    expect(tokenFate(draftOfForwarder(HTTPS), HTTPS)).toBe('kept');
    expect(tokenFate(draftOfForwarder(SYSLOG), SYSLOG)).toBe('none');
    expect(tokenFate({ ...draftOfForwarder(), kind: 'https_json' }, undefined)).toBe('none');
    expect(tokenFate({ ...draftOfForwarder(), kind: 'https_json', url: 'https://x.example' }, undefined)).toBe('none');
  });

  it('will not offer to save an HTTPS forwarder that would have no destination', () => {
    const fresh = { ...draftOfForwarder(), kind: 'https_json' as const, name: 'x' };
    expect(willHaveDestination(fresh, undefined)).toBe(false);
    expect(willHaveDestination({ ...fresh, url: 'https://x.example' }, undefined)).toBe(true);
    expect(willHaveDestination(draftOfForwarder(HTTPS), HTTPS)).toBe(true);
    // A syslog forwarder turned into an HTTPS one has no stored URL to keep.
    expect(willHaveDestination({ ...draftOfForwarder(SYSLOG), kind: 'https_json' }, SYSLOG)).toBe(false);
  });
});

describe('the document is replaced by what is sent', () => {
  it('sends a syslog forwarder whole, so an edit of the name does not reset the rest', () => {
    const { params, errors } = saveParams({ ...draftOfForwarder(SYSLOG), name: 'SIEM (prod)' });
    expect(errors).toEqual({});
    expect(params).toEqual({
      id: SYSLOG.id,
      name: 'SIEM (prod)',
      kind: 'syslog_tcp',
      enabled: true,
      host: '127.0.0.1',
      port: 59849,
      format: 'rfc5424',
      streams: ['syslog', 'trap', 'event'],
      min_severity: 6,
      lab: true,
    });
  });

  it('leaves the port out when none is typed, so the kind’s own port applies', () => {
    const { params } = saveParams({ ...draftOfForwarder(), name: 'x', kind: 'syslog_tls', host: 'siem.example.net' });
    expect(params).not.toHaveProperty('port');
  });

  it('refuses a port that is not one, beside the port', () => {
    for (const port of ['0', '70000', 'syslog', '51.4']) {
      const { errors } = saveParams({ ...draftOfForwarder(SYSLOG), port });
      expect(errors.port, port).toBeDefined();
    }
  });

  it('never makes the audit trail a default', () => {
    const fresh = saveParams({ ...draftOfForwarder(), name: 'x', host: 'h' }).params;
    expect(fresh.streams).toEqual(['syslog', 'trap', 'event']);
    expect(sendsAudit(fresh.streams ?? [])).toBe(false);
    expect(sendsAudit(HTTPS.streams)).toBe(true);
  });

  it('sends the streams in the engine’s order whatever order they were ticked in', () => {
    const { params } = saveParams({ ...draftOfForwarder(SYSLOG), streams: ['audit', 'event', 'syslog'] });
    expect(params.streams).toEqual(['syslog', 'event', 'audit']);
  });

  it('gives an authority only to a kind that checks a certificate', () => {
    const pem = '-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----';
    expect(saveParams({ ...draftOfForwarder(SYSLOG), kind: 'syslog_tls', caPem: pem }).params.ca_pem).toBe(pem);
    expect(saveParams({ ...draftOfForwarder(SYSLOG), kind: 'syslog_udp', caPem: pem }).params).not.toHaveProperty(
      'ca_pem',
    );
  });
});

describe('the engine’s refusals, beside the field they are about', () => {
  // Sentences `logs.forward.save` answers; none repeats the host, URL or token.
  it.each([
    ['give the forwarder a name (at most 100 characters)', 'name'],
    ['an HTTPS forwarder needs the URL it posts to (`url`)', 'url'],
    ["the collector's URL has to start with https:// (or http:// on your own network)", 'url'],
    ["the collector's URL is not a complete URL (https://host[:port]/path)", 'url'],
    ['the token cannot be sent in an HTTP header: it is too long or holds a character a header may not', 'token'],
    ['a token needs the URL it is sent to: give `url` as well', 'token'],
    [
      'a syslog forwarder needs the collector\'s host name or IP address (`host`), without a scheme, a path or spaces',
      'host',
    ],
    [
      "that is this application's own syslog receiver: it would be sent its own lines, store them and send them again, for ever",
      'host',
    ],
    ['choose at least one stream for the forwarder to send', 'streams'],
    ['there is no stream called `flows`: a forwarder can send syslog, trap, event and audit', 'streams'],
    ['a severity is a number from 0 (emergency) to 7 (debug)', 'min_severity'],
    [
      '`ca_pem` holds something that is not a certificate (a private key, perhaps). Give only the authority\'s certificate: this text is stored with the forwarder and shown again',
      'ca_pem',
    ],
    ['invalid input: the certificate is not valid PEM', 'ca_pem'],
    ['only a TLS or an HTTPS forwarder checks a certificate: leave `ca_pem` out for this kind', 'ca_pem'],
    ['an HTTPS forwarder posts JSON; RFC 5424 is for the syslog kinds', 'format'],
  ] as const)('places “%s” beside %s', (message, field) => {
    expect(fieldOfForwardError(message)).toBe(field);
  });

  it('puts what it does not recognise above the form', () => {
    expect(fieldOfForwardError('there are already 16 forwarders, which is as many as there can be')).toBe('form');
    expect(fieldOfForwardError('a syslog forwarder has a host and a port, not a URL and a token')).toBe('form');
  });
});

describe('what waits in a forwarder’s queue', () => {
  it('reads the entries waiting, what they take up and the budget, where the engine says', () => {
    const [first] = STATUS_WITH_QUEUE_BYTES.forwarders;
    expect(waitingOf(first)).toEqual({ entries: 0, bytes: 0, budget: 16_777_216 });
  });

  it('says nothing — not an empty queue — for a build that does not report it', () => {
    const [first] = STATUS.forwarders;
    expect(waitingOf(first)).toBeNull();
    expect(waitingOf(undefined)).toBeNull();
  });
});

describe('what a forwarder has done', () => {
  it('joins the forwarders with the status by id', () => {
    const rows = rowsOf(FORWARDERS, STATUS.forwarders);
    expect(rows[0]?.status).toMatchObject({ state: 'failing', dropped: 2 });
    expect(rows[0]?.status?.last_error).toBe('connection refused by the destination');
  });

  it('shows a forwarder the status does not list as not reported, never as zero', () => {
    const rows = rowsOf(FORWARDERS, STATUS.forwarders);
    expect(rows[1]?.status).toBeUndefined();
    expect(stateView(rows[1]?.status).label).toBe('Not reported');
    expect(rowsOf(FORWARDERS, undefined).every((row) => row.status === undefined)).toBe(true);
  });

  it('names the four states the engine has, and tells idle from sending', () => {
    const state = (value: ForwarderStatus['state']) =>
      stateView({ id: 'x', name: 'x', state: value, sent: 0, dropped: 0 });
    expect(state('disabled')).toMatchObject({ tone: 'idle', label: 'Switched off' });
    expect(state('idle')).toMatchObject({ tone: 'info', label: 'Nothing sent yet' });
    expect(state('ok')).toMatchObject({ tone: 'ok', label: 'Sending' });
    expect(state('failing')).toMatchObject({ tone: 'crit', label: 'Failing' });
  });

  it('does not let "sent" be read as "arrived" for a syslog transport', () => {
    expect(sentMeaning('syslog_tcp')).toContain('does not mean “arrived”');
    expect(sentMeaning('syslog_udp')).toContain('does not mean “arrived”');
    expect(sentMeaning('https_json')).toBe('Sent is what the collector answered 2xx to.');
  });

  it('says what a severity floor lets through', () => {
    expect(floorPhrase(6)).toBe('informational and worse');
    expect(floorPhrase(null)).toBe('Everything');
    expect(floorPhrase(undefined)).toBe('Everything');
  });
});
