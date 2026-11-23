import { describe, expect, it } from 'vitest';
import {
  blamed,
  choicesOf,
  codecOption,
  fieldShown,
  initialValues,
  leadOf,
  leadValues,
  linkOption,
  listOf,
  parseHeaders,
  parseSizes,
  pickerOf,
  sourcesOf,
  SPECS,
  type Values,
} from './specs';

const spec = (id: string) => {
  const found = SPECS[id];
  if (!found) throw new Error(`no spec for ${id}`);
  return found;
};

/** The form as it starts, plus whatever the test types into it. */
function form(id: string, typed: Values = {}): Values {
  return { ...initialValues(SPECS[id]), ...typed };
}

const params = (id: string, typed: Values = {}) => spec(id).params(form(id, typed));

describe('a calculator is not called until it can be answered', () => {
  it('asks nothing while the required field is empty', () => {
    expect(params('tools.subnet')).toBeNull();
    expect(params('tools.subnet', { input: '  ' })).toBeNull();
    expect(params('tools.contains', { a: '10.0.0.0/8' })).toBeNull();
    expect(params('tools.subnet.split', { cidr: '10.0.0.0/24' })).toBeNull();
  });

  it('asks as soon as it can, and leaves judging the value to the engine', () => {
    expect(params('tools.subnet', { input: '10.0.300.1/24' })).toEqual({ input: '10.0.300.1/24' });
  });

  it('holds back a two-field tool until both are there', () => {
    expect(params('tools.ip.range', { first: '10.0.0.5' })).toBeNull();
    expect(params('tools.ip.range', { first: '10.0.0.5', last: '10.0.0.70' })).toEqual({
      first: '10.0.0.5',
      last: '10.0.0.70',
    });
  });

  it('needs two blocks before it can say whether any overlap', () => {
    expect(params('tools.overlaps', { cidrs: '10.0.0.0/24' })).toBeNull();
    expect(params('tools.overlaps', { cidrs: '10.0.0.0/23\n10.0.1.0/24' })).toEqual({
      cidrs: ['10.0.0.0/23', '10.0.1.0/24'],
    });
  });
});

describe('an empty optional field is left out of the call', () => {
  it('never sends an empty count, because 0 means "until cancelled" for a ping', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: '' })).toEqual({
      host: '10.0.0.1',
    });
  });

  it('sends a count of 0 when that is what was asked for', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: '0' })).toEqual({
      host: '10.0.0.1',
      count: 0,
    });
  });

  it('leaves out a number that is not one, rather than sending NaN', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: 'five' })).toEqual({
      host: '10.0.0.1',
    });
  });

  it('omits an optional text field but keeps a switch that is off, which the backend reads as off', () => {
    expect(params('probe.ports', { aim: 'host', host: '10.0.0.1', ports: '', banners: '' })).toEqual({
      host: '10.0.0.1',
      banners: false,
    });
  });
});

describe('a probe aims at one thing', () => {
  it('sends a host, never a host and a device together (the backend refuses both)', () => {
    const both = form('probe.ping', { aim: 'host', host: '10.0.0.1', device: 'core-sw1' });
    expect(spec('probe.ping').params(both)).toEqual({ host: '10.0.0.1', count: 5 });
  });

  it('sends the device when that is what is aimed at', () => {
    expect(params('probe.ping', { aim: 'device', device: 'core-sw1' })).toEqual({
      device: 'core-sw1',
      count: 5,
    });
  });

  it('asks nothing while the chosen kind of target is empty', () => {
    expect(params('probe.ping', { aim: 'device', host: '10.0.0.1' })).toBeNull();
  });

  it('shows the host field only while a host is what is aimed at', () => {
    const host = spec('probe.ping').fields.find((field) => field.name === 'host');
    const device = spec('probe.ping').fields.find((field) => field.name === 'device');
    const values = form('probe.ping', { aim: 'device' });
    expect(host && fieldShown(host, values)).toBe(false);
    expect(device && fieldShown(device, values)).toBe(true);
  });
});

describe('the lines a person types', () => {
  it('reads a VLSM request whichever way round the name and the number are', () => {
    expect(parseSizes('Staff 200\n50 Voice\n2\n\n  ')).toEqual([
      { name: 'Staff', hosts: 200 },
      { name: 'Voice', hosts: 50 },
      { hosts: 2 },
    ]);
  });

  it('keeps a name made of several words', () => {
    expect(parseSizes('Guest wireless 120')).toEqual([{ name: 'Guest wireless', hosts: 120 }]);
  });

  it('ignores a line with no number in it instead of guessing one', () => {
    expect(parseSizes('Staff\nVoice 10')).toEqual([{ name: 'Voice', hosts: 10 }]);
  });

  it('reads headers as a device’s documentation writes them, and ignores a line that is not one', () => {
    expect(parseHeaders('Authorization: Bearer abc\nX-Trace:  1 \nnot a header\n: nothing')).toEqual([
      ['Authorization', 'Bearer abc'],
      ['X-Trace', '1'],
    ]);
  });

  it('splits a list on commas, semicolons and lines alike', () => {
    expect(listOf('22,80 ; 443\n8080')).toEqual(['22', '80', '443', '8080']);
  });

  it('sends the encapsulations as a list, outermost first', () => {
    expect(params('tools.mtu', { link_mtu: '1500', encapsulations: 'gre, ipsec-esp-tunnel' })).toEqual({
      link_mtu: 1500,
      encapsulations: ['gre', 'ipsec-esp-tunnel'],
    });
  });
});

describe('whitespace is part of the input where it is part of the answer', () => {
  it('keeps the text of a digest exactly as typed, trailing spaces and all', () => {
    expect(params('tools.hash', { mode: 'text', text: 'nocturne ' })).toEqual({ text: 'nocturne ' });
  });

  it('sends Base64 bytes trimmed, because a pasted blob carries a newline', () => {
    expect(params('tools.hash', { mode: 'base64', text: ' bm9jdHVybmU=\n' })).toEqual({
      base64: 'bm9jdHVybmU=',
    });
  });
});

describe('the tools whose input is a secret', () => {
  it('are marked so their inputs are never written to this computer', () => {
    for (const id of [
      'tools.secret.decode',
      'tools.secret.encode',
      'tools.secret.identify',
      'tools.password',
      'tools.keys.generate',
    ]) {
      expect(spec(id).secret, id).toBe(true);
    }
  });

  it('are the only ones that are', () => {
    const secret = Object.entries(SPECS)
      .filter(([, entry]) => entry.secret)
      .map(([id]) => id)
      .sort();
    expect(secret).toEqual([
      'tools.keys.generate',
      'tools.password',
      'tools.secret.decode',
      'tools.secret.encode',
      'tools.secret.identify',
    ]);
  });

  it('asks for a decode as soon as there is a line, and lets the engine recognise the encoding', () => {
    expect(params('tools.secret.decode', { text: 'password 7 0822455D0A16' })).toEqual({
      text: 'password 7 0822455D0A16',
    });
    expect(params('tools.secret.decode', { text: 'x', kind: 'junos9' })).toEqual({
      text: 'x',
      kind: 'junos9',
    });
  });

  it('asks for a password with the classes as switches, not as absent fields', () => {
    expect(params('tools.password')).toEqual({
      length: 20,
      count: 5,
      upper: true,
      lower: true,
      digits: true,
      symbols: true,
      avoid_ambiguous: false,
    });
  });
});

describe('the servers', () => {
  it('can be started with nothing said, which uses the settings’ own defaults', () => {
    expect(params('tftp.start')).toEqual({ allow_write: false, allow_overwrite: false });
  });

  it('never asks to overwrite when uploads themselves are off', () => {
    expect(params('tftp.start', { root: '/srv/tftp', allow_overwrite: 'on' })).toEqual({
      root: '/srv/tftp',
      allow_write: false,
      allow_overwrite: false,
    });
  });

  it('allows overwriting only when both switches are on', () => {
    expect(params('tftp.start', { allow_write: 'on', allow_overwrite: 'on' })).toEqual({
      allow_write: true,
      allow_overwrite: true,
    });
  });
});

describe('a device probe', () => {
  it('waits for both the device and the target: this text ends up on a command line', () => {
    expect(params('probe.device.ping', { device_id: 'core-sw1' })).toBeNull();
    expect(params('probe.device.ping', { target: '10.0.0.1' })).toBeNull();
    expect(params('probe.device.ping', { device_id: 'core-sw1', target: '10.0.0.1' })).toEqual({
      device_id: 'core-sw1',
      target: '10.0.0.1',
    });
  });

  it('passes an injection attempt through untouched, for the backend to refuse by name', () => {
    expect(params('probe.device.ping', { device_id: 'sw1', target: '8.8.8.8; reload' })).toEqual({
      device_id: 'sw1',
      target: '8.8.8.8; reload',
    });
  });
});

// ---------------------------------------------------------------------- qos
//
// Every row and every refusal below is the engine's own, copied from a
// running `nocturne-server` (`tools.reference`, `tools.qos.marking`,
// `tools.qos.voice`) on 2026-11-22 — none is written from the specification.

const G711 = {
  id: 'g711',
  name: 'G.711',
  bit_rate_bps: 64000,
  sample_bytes: 80,
  sample_ms: 10,
  packet_ms: 20,
  payload_bytes: 160,
  mos: 4.1,
};
const G726_24 = {
  id: 'g726-24',
  name: 'G.726 (24 kbps)',
  bit_rate_bps: 24000,
  sample_bytes: 15,
  sample_ms: 5,
  packet_ms: 20,
  payload_bytes: 60,
  mos: null,
};
const ETHERNET = {
  id: 'ethernet',
  name: 'Ethernet',
  overhead_bytes: 18,
  parts: 'Ethernet header 14 + Ethernet FCS 4',
  crtp: false,
  source: 'Cisco 7934',
  note: 'The frame as Cisco counts it: without the preamble and the inter-frame gap. The Cisco SRND table counts the 14-byte header alone.',
};
const PPP = {
  id: 'ppp',
  name: 'PPP',
  overhead_bytes: 6,
  parts: 'PPP header 6',
  crtp: true,
  source: 'Cisco SRND 12.x, table 3-8',
  note: '',
};

const field = (id: string, name: string) => {
  const found = spec(id).fields.find((entry) => entry.name === name);
  if (!found) throw new Error(`${id} has no field ${name}`);
  return found;
};

describe('a marking is one value and what it is', () => {
  it('chooses no form for the reader: the same number is a different marking as each', () => {
    // 46 is EF as a DSCP and DSCP 11 with ECN bits set as a ToS byte. The
    // engine refuses to guess, so the form starts with nothing chosen.
    expect(initialValues(SPECS['tools.qos.marking'])['as']).toBe('');
    expect(params('tools.qos.marking', { value: '46' })).toBeNull();
    expect(params('tools.qos.marking', { as: 'dscp' })).toBeNull();
  });

  it('asks once both are there, and sends the value exactly as it was typed', () => {
    expect(params('tools.qos.marking', { value: ' 0xb8 ', as: 'tos' })).toEqual({
      value: '0xb8',
      as: 'tos',
    });
    // 184 as a DSCP is wrong, and saying so is the engine's job, in its words.
    expect(params('tools.qos.marking', { value: '184', as: 'dscp' })).toEqual({
      value: '184',
      as: 'dscp',
    });
  });

  it('offers the engine’s four forms after "nothing chosen yet", and says what each takes', () => {
    const choices = choicesOf(field('tools.qos.marking', 'as')) ?? [];
    expect(choices.map((choice) => choice.value)).toEqual(['', 'dscp', 'tos', 'precedence', 'name']);
    expect(choices.slice(1).every((choice) => (choice.hint ?? '').length > 0)).toBe(true);
  });
});

describe('a voice calculation is not asked for until it can be answered', () => {
  const chosen = { codec: 'g729', link: 'ppp', calls: '10' };

  it('has no default number of calls: one call for a link sized for sixty looks like an answer', () => {
    expect(initialValues(SPECS['tools.qos.voice'])['calls']).toBe('');
    expect(params('tools.qos.voice', { codec: 'g729', link: 'ppp' })).toBeNull();
  });

  it('waits for a codec and a link as well, and chooses neither itself', () => {
    expect(params('tools.qos.voice', { link: 'ppp', calls: '10' })).toBeNull();
    expect(params('tools.qos.voice', { codec: 'g729', calls: '10' })).toBeNull();
  });

  it('sends the switches as off rather than leaving them out, and nothing it was not given', () => {
    expect(params('tools.qos.voice', chosen)).toEqual({
      codec: 'g729',
      calls: 10,
      link: 'ppp',
      crtp: false,
      vad: false,
    });
  });

  it('sends compressed RTP, VAD and the voice per packet when they are asked for', () => {
    expect(params('tools.qos.voice', { ...chosen, crtp: 'on', vad: 'on', packet_ms: '30' })).toEqual({
      codec: 'g729',
      calls: 10,
      link: 'ppp',
      crtp: true,
      vad: true,
      packet_ms: 30,
    });
  });

  it('passes compressed RTP on Ethernet through, for the engine to refuse in its own words', () => {
    expect(params('tools.qos.voice', { ...chosen, link: 'ethernet', crtp: 'on' })).toMatchObject({
      link: 'ethernet',
      crtp: true,
    });
  });

  it('waits for the layer-2 bytes of a custom link, which has no figure of its own', () => {
    expect(params('tools.qos.voice', { ...chosen, link: 'custom' })).toBeNull();
    expect(params('tools.qos.voice', { ...chosen, link: 'custom', l2_bytes: '26' })).toEqual({
      codec: 'g729',
      calls: 10,
      link: 'custom',
      crtp: false,
      vad: false,
      l2_bytes: 26,
    });
  });

  it('counts zero layer-2 bytes as an answer, not as an empty field', () => {
    expect(params('tools.qos.voice', { ...chosen, link: 'custom', l2_bytes: '0' })).toMatchObject({
      l2_bytes: 0,
    });
  });

  it('never sends a layer-2 figure left over from a custom link beside a named one', () => {
    // The field is hidden once the link is PPP again, but what was typed in it
    // is still in the form; the engine refuses `l2_bytes` beside a named link.
    const sent = params('tools.qos.voice', { ...chosen, link: 'ppp', l2_bytes: '26' });
    expect(sent).not.toHaveProperty('l2_bytes');
    const l2 = field('tools.qos.voice', 'l2_bytes');
    expect(fieldShown(l2, form('tools.qos.voice', { link: 'ppp' }))).toBe(false);
    expect(fieldShown(l2, form('tools.qos.voice', { link: 'custom' }))).toBe(true);
  });
});

describe('a choice that is the engine’s table', () => {
  it('is not known, rather than empty, until the table has been read', () => {
    expect(choicesOf(field('tools.qos.voice', 'codec'))).toBeUndefined();
    expect(choicesOf(field('tools.qos.voice', 'codec'), [])).toEqual([
      { value: '', label: 'Choose a codec…' },
    ]);
  });

  it('offers the rows the engine has, in its order, under "nothing chosen yet"', () => {
    const choices = choicesOf(field('tools.qos.voice', 'codec'), [G711, G726_24]) ?? [];
    expect(choices.map((choice) => [choice.value, choice.label])).toEqual([
      ['', 'Choose a codec…'],
      ['g711', 'G.711'],
      ['g726-24', 'G.726 (24 kbps)'],
    ]);
  });

  it('adds the custom link after the engine’s rows, because it is not one of them', () => {
    const choices = choicesOf(field('tools.qos.voice', 'link'), [ETHERNET, PPP]) ?? [];
    expect(choices.map((choice) => choice.value)).toEqual(['', 'ethernet', 'ppp', 'custom']);
  });

  it('leaves out a row it could not ask the engine about', () => {
    const choices = choicesOf(field('tools.qos.voice', 'codec'), [{ name: 'No id' }, G711]) ?? [];
    expect(choices.map((choice) => choice.value)).toEqual(['', 'g711']);
  });

  it('leaves a choice written in the spec exactly as it was', () => {
    const kinds = field('probe.dns', 'kind');
    expect(choicesOf(kinds)).toEqual(kinds.options);
  });

  it('waits while the table is being read, and offers nothing meanwhile', () => {
    const picker = pickerOf(field('tools.qos.voice', 'codec'), '', {});
    expect(picker.ready).toBe(false);
    expect(picker.options).toEqual([{ value: '', label: 'Reading the engine’s table…' }]);
    expect(picker.error).toBeUndefined();
  });

  it('says a table could not be read in the engine’s words, and stops saying it is being read', () => {
    const picker = pickerOf(field('tools.qos.voice', 'codec'), '', {
      error: 'unknown reference table `voice_codecs`',
    });
    // Nothing is offered in the table's place: a list kept here could
    // disagree with the calculator about what there is.
    expect(picker.ready).toBe(false);
    expect(picker.options).toEqual([{ value: '', label: 'Nothing to choose from' }]);
    expect(picker.error).toBe(
      'The engine’s table of these could not be read, so there is nothing to choose from: unknown reference table `voice_codecs`',
    );
  });

  it('shows the codec the form holds while its table is unread, because that codec is what is asked about', () => {
    // A form remembered from last time asks the engine at once. A list that
    // read "Reading…" or "Nothing to choose from" above the answer for G.729
    // would be showing the answer to a question that is nowhere on screen.
    const codec = field('tools.qos.voice', 'codec');
    const reading = pickerOf(codec, 'g729', {});
    expect(reading.ready).toBe(false);
    expect(reading.options).toEqual([{ value: 'g729', label: 'g729' }]);
    expect(reading.hint).toBe('Reading the engine’s table…');

    const unreadable = pickerOf(codec, 'g729', { error: 'Nocturne is not reachable.' });
    expect(unreadable.ready).toBe(false);
    expect(unreadable.options).toEqual([{ value: 'g729', label: 'g729' }]);
    expect(unreadable.error).toContain('could not be read');
    expect(unreadable.error).toContain('Nocturne is not reachable.');
  });

  it('shows a value the form holds and the list does not have, not the first choice in its place', () => {
    // Remembered from a build that had it, or typed into storage by hand: it
    // is still what is sent, and the engine's refusal goes beside it.
    const codec = field('tools.qos.voice', 'codec');
    const stray = pickerOf(codec, 'opus', { rows: [G711] });
    expect(stray.ready).toBe(true);
    expect(stray.options.map((option) => [option.value, option.label])).toEqual([
      ['', 'Choose a codec…'],
      ['g711', 'G.711'],
      ['opus', 'opus — not one of the choices'],
    ]);
    // A value that is a row is not listed twice, and nothing is added for none.
    expect(pickerOf(codec, 'g711', { rows: [G711] }).options.map((option) => option.value)).toEqual(['', 'g711']);
    expect(pickerOf(codec, '', { rows: [G711] }).options.map((option) => option.value)).toEqual(['', 'g711']);
    const as = pickerOf(field('tools.qos.marking', 'as'), 'cos');
    expect(as.options.at(-1)).toEqual({ value: 'cos', label: 'cos — not one of the choices' });
  });

  it('opens no form on a choice that is not one of its own', () => {
    for (const [id, entry] of Object.entries(SPECS)) {
      const initial = initialValues(entry);
      for (const one of entry.fields) {
        if (one.kind !== 'select' || one.source) continue;
        const labels = pickerOf(one, initial[one.name] ?? '').options.map((option) => option.label);
        expect(labels.join(' | '), `${id}: ${one.name}`).not.toContain('not one of the choices');
      }
    }
  });

  it('says in words that a table which was read is empty', () => {
    const codec = pickerOf(field('tools.qos.voice', 'codec'), '', { rows: [] });
    expect(codec.ready).toBe(true);
    expect(codec.error).toBeUndefined();
    expect(codec.hint).toBe('The engine’s table of these is empty: there is nothing of its own to choose.');
    // The choice the spec writes is still there: a custom link needs no row.
    const link = pickerOf(field('tools.qos.voice', 'link'), '', { rows: [] });
    expect(link.options.map((option) => option.value)).toEqual(['', 'custom']);
  });

  it('puts the chosen row’s figures where the hint was', () => {
    const codec = field('tools.qos.voice', 'codec');
    expect(pickerOf(codec, 'g711', { rows: [G711, G726_24] }).hint).toBe(codecOption(G711)?.hint);
    expect(pickerOf(codec, '', { rows: [G711, G726_24] }).hint).toBeUndefined();
    const as = field('tools.qos.marking', 'as');
    expect(pickerOf(as, '').hint).toBe(as.hint);
    expect(pickerOf(as, 'tos').hint).toBe('0 to 255. Its bottom two bits come back as the ECN field.');
  });

  it('always says when the engine has more rows than the picker offers', () => {
    const codec = field('tools.qos.voice', 'codec');
    const partial = 'The engine has 12 of these; the first 2 are offered here.';
    const rows = [G711, G726_24];
    expect(pickerOf(codec, '', { rows, partial }).hint).toBe(partial);
    expect(pickerOf(codec, 'g711', { rows, partial }).hint).toBe(`${codecOption(G711)?.hint} ${partial}`);
  });

  it('names each table a form reads once, and none for a form that reads none', () => {
    expect(sourcesOf(spec('tools.qos.voice').fields)).toEqual(['voice_codecs', 'voice_links']);
    expect(sourcesOf(spec('tools.qos.marking').fields)).toEqual([]);
    expect(sourcesOf(spec('tools.subnet').fields)).toEqual([]);
  });

  it('says what a codec is in the row’s own figures', () => {
    expect(codecOption(G711)).toEqual({
      value: 'g711',
      label: 'G.711',
      hint: '64 000 bps · 80 bytes every 10 ms · 20 ms (160 bytes) per packet by default · MOS 4.1.',
    });
  });

  it('leaves a score the source does not give out of the sentence, rather than writing none', () => {
    const hint = codecOption(G726_24)?.hint ?? '';
    expect(hint).toContain('15 bytes every 5 ms');
    expect(hint).not.toMatch(/MOS|null/);
  });

  it('says what a link adds, where the figure is from, and whether cRTP is counted on it', () => {
    expect(linkOption(ETHERNET)).toEqual({
      value: 'ethernet',
      label: 'Ethernet — 18 bytes',
      hint: '18 bytes on every packet: Ethernet header 14 + Ethernet FCS 4. Figure from Cisco 7934. Compressed RTP is not counted on it.',
    });
    expect(linkOption(PPP)?.hint).toBe(
      '6 bytes on every packet: PPP header 6. Figure from Cisco SRND 12.x, table 3-8.',
    );
  });
});

// Every sentence either method refused with, one of each reason, when called
// on a running `nocturne-server` on 2026-11-22 with each form of input it
// takes and each way of getting one wrong — including the ones no form here
// can produce (an empty value, an alias, a missing parameter), because the
// table is the list of what the engine can say, not of what this screen sends.
// The field beside each was decided by reading the sentence, not by running
// `blamed` on it.
const MARKING_REFUSALS: readonly (readonly [string, string])[] = [
  ['invalid DSCP `184`: a DSCP is six bits, 0 to 63; write binary with a prefix, as in 0b101110, and for the whole byte choose `tos`', 'value'],
  ['invalid DSCP `voice`: neither a number from 0 to 63 nor a code point name such as ef, af41 or cs3', 'value'],
  ['invalid DSCP name `46`: that is a number, not a name; say whether it is a `dscp`, a `tos` byte or a `precedence`', 'value'],
  ['invalid DSCP name `gold`: not a code point name; the names are CS0 to CS7, AF11 to AF43, EF, VOICE-ADMIT, NQB, LE and DF (also `default` and `be`)', 'value'],
  ['invalid ToS byte `300`: the byte is eight bits, 0 to 255; write binary with a prefix, as in 0b10111000', 'value'],
  ['invalid ToS byte `ef`: not a number; write the byte in decimal (184), hexadecimal (0xb8) or binary (0b10111000)', 'value'],
  ['invalid IP precedence `9`: IP precedence is three bits, 0 to 7', 'value'],
  ['invalid IP precedence `urgent`: neither a number from 0 to 7 nor one of routine, priority, immediate, flash, flash-override, critical, internet, network', 'value'],
  ['invalid kind of marking `cos`: say what the value is: `dscp`, `tos`, `precedence` or `name`', 'as'],
  ['invalid DSCP `xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`: too long to be a marking', 'value'],
  ['invalid ToS byte `xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`: too long to be a marking', 'value'],
  ['say what the value is in `as`: `dscp`, `tos`, `precedence` or `name`. The same number is a different marking as each', 'as'],
  ['give the marking in `value`: a DSCP number or name, a ToS byte or an IP precedence', 'value'],
  ['invalid DSCP: nothing was entered', 'value'],
  ['invalid ToS byte: nothing was entered', 'value'],
  ['`value` is one marking, as text or a number', 'value'],
];

/** The same for the voice calculator, with the link that was chosen when it was refused. */
const VOICE_REFUSALS: readonly (readonly [string, string, string | undefined])[] = [
  ['invalid link type `ethernet`: compressed RTP is not counted on Ethernet: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` and give its layer-2 bytes', 'ethernet', 'link'],
  ['invalid link type `dot1q`: compressed RTP is not counted on Ethernet with an 802.1Q tag: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` and give its layer-2 bytes', 'dot1q', 'link'],
  ['invalid link type `mpls`: compressed RTP is not counted on MPLS: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` and give its layer-2 bytes', 'mpls', 'link'],
  ['invalid link type `wlan`: compressed RTP is not counted on 802.11 wireless LAN: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` and give its layer-2 bytes', 'wlan', 'link'],
  ['invalid number of calls `0`: there has to be at least one call to work out', 'mlppp', 'calls'],
  ['1000001 simultaneous calls is more than this calculator works out; the most is 1000000', 'mlppp', 'calls'],
  ['give the number of simultaneous calls in `calls`', 'mlppp', 'calls'],
  ['invalid voice per packet `25 ms`: G.729 produces 10 bytes every 10 ms, so a packet holds a multiple of 10 ms; its default is 20 ms', 'mlppp', 'packet_ms'],
  ['invalid voice per packet `0 ms`: a packet has to hold some voice', 'mlppp', 'packet_ms'],
  ['invalid voice per packet `250 ms`: more than 240 ms of voice in one packet is not something a phone or a gateway sends', 'mlppp', 'packet_ms'],
  ['invalid link type `custom`: a custom link needs its layer-2 bytes per packet; give them in `l2_bytes`', 'custom', 'link'],
  ['invalid layer-2 overhead `300 bytes`: more than 256 bytes in front of every packet is not a link header', 'custom', 'l2_bytes'],
  ['invalid layer-2 overhead `26 bytes`: `l2_bytes` goes with the `custom` link only; `ppp` already counts 6 bytes', 'ppp', undefined],
  ['invalid codec `opus`: not one this calculator has figures for; it has g711, g729, g723.1-6.3, g723.1-5.3, g726-32, g726-24, g728, g722, ilbc-20, ilbc-30', 'mlppp', 'codec'],
  ['invalid codec: say which one: g711, g729, g723.1-6.3, g723.1-5.3, g726-32, g726-24, g728, g722, ilbc-20, ilbc-30', 'mlppp', 'codec'],
  ['invalid codec `g723`: that name covers two bit rates; say which: `g723.1-6.3` or `g723.1-5.3`', 'mlppp', 'codec'],
  ['invalid codec `g726`: that name covers two bit rates; say which: `g726-32` or `g726-24`', 'mlppp', 'codec'],
  ['invalid codec `ilbc`: that name covers two bit rates; say which: `ilbc-20` or `ilbc-30`', 'mlppp', 'codec'],
  ['invalid codec `xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`: too long to be a codec name', 'mlppp', 'codec'],
  ['invalid link type `atm`: not one this calculator has figures for; it has ethernet, dot1q, ppp, mlppp, frame-relay, frf12, mpls, wlan, none, custom', 'atm', 'link'],
  ['invalid link type: say which one: ethernet, dot1q, ppp, mlppp, frame-relay, frf12, mpls, wlan, none, custom', 'ppp', 'link'],
  ['invalid link type `xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`: too long to be a link type', 'ppp', 'link'],
];

/** What the RPC router says when a number is not one the method's type can hold. */
const ROUTER_REFUSALS: readonly string[] = [
  'invalid parameters for `tools.qos.voice`: invalid value: integer `-3`, expected u32',
  'invalid parameters for `tools.qos.voice`: invalid type: floating point `1.5`, expected u32',
  'invalid parameters for `tools.qos.voice`: invalid value: integer `5000000000`, expected u32',
  'invalid parameters for `tools.qos.voice`: invalid type: string "10", expected u32',
  'invalid parameters for `tools.qos.voice`: invalid value: integer `-20`, expected u32',
  'invalid parameters for `tools.qos.voice`: invalid type: floating point `20.5`, expected u32',
  'invalid parameters for `tools.qos.voice`: invalid value: integer `-1`, expected u32',
];

describe('a refusal is put beside the field the engine says it is about', () => {
  const marking = (message: string, typed: Values = {}) =>
    blamed(spec('tools.qos.marking'), message, form('tools.qos.marking', typed));
  const voice = (message: string, typed: Values = {}) =>
    blamed(spec('tools.qos.voice'), message, form('tools.qos.voice', typed));

  it('puts a value outside its form beside the value', () => {
    for (const message of [
      'invalid DSCP `184`: a DSCP is six bits, 0 to 63; write binary with a prefix, as in 0b101110, and for the whole byte choose `tos`',
      'invalid DSCP name `46`: that is a number, not a name; say whether it is a `dscp`, a `tos` byte or a `precedence`',
      'invalid ToS byte `300`: the byte is eight bits, 0 to 255; write binary with a prefix, as in 0b10111000',
      'invalid IP precedence `9`: IP precedence is three bits, 0 to 7',
    ]) {
      expect(marking(message), message).toBe('value');
    }
  });

  it('puts an unknown kind of marking beside the choice of kind', () => {
    expect(
      marking('invalid kind of marking `cos`: say what the value is: `dscp`, `tos`, `precedence` or `name`'),
    ).toBe('as');
  });

  it('puts compressed RTP on a link that cannot carry it beside the link, which is what the engine calls wrong', () => {
    expect(
      voice(
        'invalid link type `ethernet`: compressed RTP is not counted on Ethernet: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` and give its layer-2 bytes',
      ),
    ).toBe('link');
  });

  it('tells the codec, the calls and the voice per packet apart', () => {
    expect(
      voice(
        'invalid codec `opus`: not one this calculator has figures for; it has g711, g729, g723.1-6.3, g723.1-5.3, g726-32, g726-24, g728, g722, ilbc-20, ilbc-30',
      ),
    ).toBe('codec');
    expect(voice('invalid number of calls `0`: there has to be at least one call to work out')).toBe('calls');
    expect(
      voice('1000001 simultaneous calls is more than this calculator works out; the most is 1000000'),
    ).toBe('calls');
    expect(
      voice(
        'invalid voice per packet `25 ms`: G.729 produces 10 bytes every 10 ms, so a packet holds a multiple of 10 ms; its default is 20 ms',
      ),
    ).toBe('packet_ms');
  });

  it('puts too many layer-2 bytes beside that field while it is on screen, and nowhere once it is not', () => {
    const message =
      'invalid layer-2 overhead `300 bytes`: more than 256 bytes in front of every packet is not a link header';
    expect(voice(message, { link: 'custom' })).toBe('l2_bytes');
    // With a named link the field is hidden: the sentence has no field to sit
    // beside, so it is shown whole where the answer would be.
    expect(voice(message, { link: 'ppp' })).toBeUndefined();
  });

  it('leaves a refusal that names no input where the answer would have been', () => {
    expect(
      voice('invalid parameters for `tools.qos.voice`: invalid value: integer `-3`, expected u32'),
    ).toBeUndefined();
    expect(voice('the engine is busy')).toBeUndefined();
  });

  it('finds the field in every sentence the marking engine can refuse with', () => {
    for (const [message, beside] of MARKING_REFUSALS) expect(marking(message), message).toBe(beside);
  });

  it('finds the field in every sentence the voice engine can refuse with, while that field is on screen', () => {
    for (const [message, link, beside] of VOICE_REFUSALS) {
      expect(voice(message, { link }), message).toBe(beside);
    }
  });

  it('puts a parameter that is missing beside the field it would have come from', () => {
    // Worded by the RPC method, not the calculator: the parameter is named,
    // not what it was supposed to be.
    expect(marking('give the marking in `value`: a DSCP number or name, a ToS byte or an IP precedence')).toBe(
      'value',
    );
    expect(
      marking(
        'say what the value is in `as`: `dscp`, `tos`, `precedence` or `name`. The same number is a different marking as each',
      ),
    ).toBe('as');
    expect(voice('give the number of simultaneous calls in `calls`')).toBe('calls');
  });

  it('does not guess which number the router could not read', () => {
    // The calls, the voice per packet and the layer-2 bytes are one type on
    // the wire, and the router's sentence names none of them. With all three
    // on screen it stays where the answer would have been, whole.
    expect(ROUTER_REFUSALS.length).toBeGreaterThan(3);
    for (const message of ROUTER_REFUSALS) {
      expect(voice(message, { link: 'custom' }), message).toBeUndefined();
    }
  });

  it('goes by what the engine says the input was, not by a word in its reason', () => {
    // A reason that happens to mention a codec is still about the link.
    expect(voice('invalid link type `x`: not a codec, and not a link either')).toBe('link');
  });

  it('does not move a refusal about for the tools that never said which field', () => {
    expect(
      blamed(spec('tools.subnet'), 'invalid IPv4 address `10.0.300.1`: 300 is not a valid octet (0-255)', {
        input: '10.0.300.1/24',
      }),
    ).toBeUndefined();
  });

  it('only ever names a field the form has', () => {
    for (const [id, entry] of Object.entries(SPECS)) {
      const names = new Set(entry.fields.map((one) => one.name));
      for (const blame of entry.blame ?? []) expect(names.has(blame.field), `${id}: ${blame.field}`).toBe(true);
    }
  });
});

describe('a reference table leads to the calculator that takes its rows', () => {
  it('opens the marking calculator on a DSCP row, by number and as a DSCP', () => {
    expect(leadOf('dscp')).toBe('tools.qos.marking');
    const ef = {
      name: 'EF',
      dscp: 46,
      binary: '101110',
      tos: 184,
      precedence: 5,
      phb: 'Expedited Forwarding (RFC 3246)',
      typical_use: 'Voice bearer (RTP): low loss, low delay, low jitter, strictly policed',
    };
    expect(leadValues('dscp', ef)).toEqual({ value: '46', as: 'dscp' });
  });

  it('does not hand over the label of a value the registry has not named', () => {
    // Searching the table for 47 gives a row whose name is "DSCP 47": a label,
    // not a code point name the engine would accept.
    const unnamed = {
      name: 'DSCP 47',
      dscp: 47,
      binary: '101111',
      tos: 188,
      precedence: 5,
      phb: 'No standard per-hop behaviour: pool 2, reserved for experimental or local use',
      typical_use: '',
    };
    expect(leadValues('dscp', unnamed)).toEqual({ value: '47', as: 'dscp' });
  });

  it('keeps DSCP 0 as a value: best effort is a marking, not a missing one', () => {
    expect(leadValues('dscp', { name: 'DF', dscp: 0 })).toEqual({ value: '0', as: 'dscp' });
  });

  it('opens the voice calculator with the codec or the link of the row chosen, and nothing else', () => {
    expect(leadOf('voice_codecs')).toBe('tools.qos.voice');
    expect(leadOf('voice_links')).toBe('tools.qos.voice');
    expect(leadValues('voice_codecs', G711)).toEqual({ codec: 'g711' });
    expect(leadValues('voice_links', PPP)).toEqual({ link: 'ppp' });
  });

  it('leads nowhere from a table no calculator takes, or from a row that is not one', () => {
    expect(leadOf('ports')).toBeUndefined();
    expect(leadValues('ports', { port: 22 })).toBeNull();
    expect(leadValues('dscp', { name: 'EF' })).toBeNull();
    expect(leadValues('voice_codecs', { name: 'G.711' })).toBeNull();
  });

  it('only ever fills a field the calculator’s form has', () => {
    for (const [table, row] of [
      ['dscp', { dscp: 46 }],
      ['voice_codecs', G711],
      ['voice_links', PPP],
    ] as const) {
      const names = new Set(spec(leadOf(table) ?? '').fields.map((one) => one.name));
      for (const name of Object.keys(leadValues(table, row) ?? {})) {
        expect(names.has(name), `${table}: ${name}`).toBe(true);
      }
    }
  });
});
