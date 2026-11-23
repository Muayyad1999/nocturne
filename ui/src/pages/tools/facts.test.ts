import { describe, expect, it } from 'vitest';
import type {
  Base64Result,
  DeviceProbeResult,
  DnsResult,
  MacInfo,
  Marking,
  MaskInfo,
  MtuResult,
  OidResult,
  PingResult,
  PortsResult,
  SubnetInfo,
  TlsResult,
  VoiceBandwidth,
} from '@/api/types/toolbox';
import { formatCount } from '@/lib/format';
import {
  base64Sections,
  deviceProbeSections,
  dnsSections,
  fact,
  lossFact,
  macSections,
  markingSections,
  maskSections,
  ms,
  mtuSections,
  oidSections,
  phbParts,
  pingSections,
  portSections,
  sectionsFor,
  subnetSections,
  tlsSections,
  voiceSections,
  type Fact,
  type Section,
} from './facts';

/** The facts of a result, flattened, for asserting on what is shown. */
function flat(sections: readonly Section[]): Fact[] {
  return sections.flatMap((section) => (section.kind === 'facts' ? section.facts : []));
}

const labelled = (sections: readonly Section[], label: string): Fact | undefined =>
  flat(sections).find((entry) => entry.label === label);

describe('a value that does not exist is left out, not printed as nothing', () => {
  it('drops an empty string, which is how the backend says "there is none"', () => {
    expect(fact('Netmask', '')).toEqual([]);
    expect(fact('Netmask', '   ')).toEqual([]);
    expect(fact('Netmask', null)).toEqual([]);
    expect(fact('Netmask', undefined)).toEqual([]);
    expect(fact('Netmask', '255.255.255.0')).toEqual([
      { label: 'Netmask', value: '255.255.255.0' },
    ]);
  });

  it('keeps a zero, which is a number and not an absence', () => {
    expect(fact('Days left', 0)).toEqual([{ label: 'Days left', value: '0' }]);
  });

  it('shows no netmask or broadcast for an IPv6 prefix, because there are none', () => {
    const sections = subnetSections(v6);
    expect(labelled(sections, 'Netmask')).toBeUndefined();
    expect(labelled(sections, 'Broadcast')).toBeUndefined();
    expect(labelled(sections, 'Network')?.value).toBe('2001:db8::/64');
    // IPv6 is where expanded and compressed earn their place.
    expect(labelled(sections, 'Expanded')?.value).toBe('2001:0db8:0000:0000:0000:0000:0000:0000');
  });

  it('shows no broadcast for a /31, and still shows the host range', () => {
    const sections = subnetSections({ ...v4, prefix: 31, cidr: '10.0.0.0/31', broadcast: '' });
    expect(labelled(sections, 'Broadcast')).toBeUndefined();
    expect(labelled(sections, 'Host range')?.value).toBe('10.1.2.1 – 10.1.2.62');
  });

  it('names a mask with gaps as having no prefix length at all', () => {
    const odd: MaskInfo = {
      prefix: null,
      netmask: '255.0.255.0',
      wildcard: '0.255.0.255',
      contiguous: false,
      addresses: '65536',
    };
    expect(labelled(maskSections(odd), 'Prefix length')?.value).toContain('not contiguous');
    expect(labelled(maskSections(odd), 'Contiguous')?.value).toContain('No');
  });
});

describe('loss is a reading, with a tone', () => {
  it('says none, some and all with three different tones', () => {
    expect(lossFact(0, 5, 5)).toMatchObject({ tone: 'ok', value: '0% — 5 of 5 answered' });
    expect(lossFact(40, 3, 5).tone).toBe('warn');
    expect(lossFact(100, 0, 5)).toMatchObject({ tone: 'crit', value: '100% — 0 of 5 answered' });
  });

  it('counts the echoes in the figure, so 100 % always has a denominator', () => {
    expect(lossFact(100, 0, 3).value).toContain('of 3');
  });
});

describe('round-trip times', () => {
  it('keeps the decimals that matter at each magnitude', () => {
    expect(ms(0.083)).toBe('0.08 ms');
    expect(ms(9.99)).toBe('9.99 ms');
    expect(ms(42.63)).toBe('42.6 ms');
    expect(ms(412.6)).toBe('413 ms');
  });

  it('has nothing to say about a time that does not exist', () => {
    expect(ms(null)).toBeNull();
    expect(ms(undefined)).toBeNull();
  });
});

describe('a ping report', () => {
  it('shows the loss, the method in words, and every echo including the lost ones', () => {
    const sections = pingSections(ping);
    expect(labelled(sections, 'Loss')?.value).toBe('50% — 1 of 2 answered');
    expect(labelled(sections, 'Sent as')?.value).toBe('unprivileged ICMP sockets');
    const table = sections.find((section) => section.kind === 'table');
    expect(table?.kind === 'table' && table.rows).toEqual([
      { seq: 1, rtt: '0.08 ms', ttl: 64, error: null },
      { seq: 2, rtt: '—', ttl: null, error: 'no reply' },
    ]);
  });

  it('says how many echoes a long run dropped, so the table is not mistaken for all of them', () => {
    const sections = pingSections({ ...ping, echoes_dropped: 1200 });
    // Grouped the way `lib/format` groups every count on every screen.
    expect(labelled(sections, 'Echoes left out')?.value).toBe(
      `${formatCount(1200)} older ones, to keep the newest`,
    );
  });

  it('names the device an address belongs to, and whether it is a lab device', () => {
    const sections = pingSections({
      ...ping,
      device: {
        id: 'd1',
        name: 'SW1',
        address: '10.0.0.2',
        role: 'access',
        site: 'HQ',
        lab: true,
        matched: 'the device’s address in the inventory',
      },
    });
    expect(labelled(sections, 'That address is')?.value).toBe(
      'SW1 (a lab device) — matched by the device’s address in the inventory',
    );
  });
});

describe('a port check', () => {
  it('shows every port that was tried, so closed and not-tried cannot look the same', () => {
    const sections = portSections(ports);
    const table = sections.find((section) => section.kind === 'table');
    expect(table?.kind === 'table' && table.rows).toEqual([
      { port: 22, state: 'open', service: 'ssh', rtt: '0.08 ms', banner: 'SSH-2.0-Test' },
      { port: 23, state: 'closed', service: 'telnet', rtt: '1.12 ms', banner: null },
    ]);
    expect(table?.kind === 'table' && table.note).toContain('says nothing about this host');
  });

  it('says plainly when none of the ports tried was open', () => {
    const sections = portSections({ ...ports, open: [] });
    expect(labelled(sections, 'Open')).toMatchObject({
      value: 'none of the 2 ports tried',
      tone: 'idle',
    });
  });
});

describe('a certificate', () => {
  it('describes the chain even when it does not verify, and keeps the verdict separate', () => {
    const sections = tlsSections(tls);
    expect(labelled(sections, 'Trusted')).toMatchObject({
      value: 'No — self-signed certificate',
      tone: 'crit',
    });
    expect(labelled(sections, 'Common name')?.value).toBe('router.example.net');
    expect(labelled(sections, 'Days left')).toMatchObject({ value: '12', tone: 'warn' });
  });

  it('calls an expired certificate critical rather than merely warning', () => {
    const expired = tlsSections({
      ...tls,
      expires_in_days: -3,
      chain: [{ ...cert, days_left: -3 }],
    });
    expect(labelled(expired, 'Days left')?.tone).toBe('crit');
    expect(labelled(expired, 'Expires in')?.tone).toBe('crit');
  });
});

describe('a device probe', () => {
  it('shows the command, the family and the device’s own words', () => {
    const sections = deviceProbeSections(deviceProbe);
    expect(labelled(sections, 'Command')?.value).toBe('ping 10.0.0.1 repeat 3');
    expect(labelled(sections, 'Ran on')?.value).toBe('SW1 itself');
    const code = sections.find((section) => section.kind === 'code');
    expect(code?.kind === 'code' && code.text).toContain('Success rate is 0 percent');
  });

  it('reports a real 100 % loss from the device as a reading', () => {
    expect(labelled(deviceProbeSections(deviceProbe), 'Loss')).toMatchObject({
      value: '100% — 0 of 3 answered',
      tone: 'crit',
    });
  });

  it('says there are no numbers when the output was not recognised, instead of inventing them', () => {
    const sections = deviceProbeSections({ ...deviceProbe, summary: undefined });
    expect(labelled(sections, 'Loss')).toBeUndefined();
    const note = sections.find((section) => section.kind === 'note');
    expect(note?.kind === 'note' && note.text).toContain('not in a format Nocturne reads');
  });

  it('shows the device’s refusal when it rejected the command', () => {
    const sections = deviceProbeSections({ ...deviceProbe, error: '% Invalid input detected' });
    const note = sections.find(
      (section) => section.kind === 'note' && section.text.includes('refused'),
    );
    expect(note?.kind === 'note' && note.tone).toBe('crit');
  });
});

describe('an answer that is empty but not missing', () => {
  it('tells an NXDOMAIN apart from a name with no record of that type', () => {
    const nothing = dnsSections({ ...dns, rcode: 'NXDOMAIN', records: [] });
    const table = nothing.find((section) => section.kind === 'table');
    expect(table?.kind === 'table' && table.empty).toBe('The server answered NXDOMAIN: no record.');
    expect(labelled(nothing, 'Response code')).toMatchObject({ tone: 'warn' });

    const noRecord = dnsSections({ ...dns, rcode: 'NOERROR', records: [] });
    const other = noRecord.find((section) => section.kind === 'table');
    expect(other?.kind === 'table' && other.empty).toBe(
      'The server answered, with no record of that type.',
    );
  });

  it('says when decoded bytes are not text, so an empty box is not a mystery', () => {
    const binary: Base64Result = {
      base64: 'AAEC',
      bytes: 3,
      text: null,
      note: 'The bytes are not valid UTF-8, so there is no text to show.',
    };
    const sections = base64Sections(binary);
    expect(sections.some((section) => section.kind === 'code')).toBe(false);
    const note = sections.find((section) => section.kind === 'note');
    expect(note?.kind === 'note' && note.text).toContain('not valid UTF-8');
  });

  it('says an OID is not in the dictionary rather than showing empty fields', () => {
    const unknown: OidResult = {
      input: '1.2.3.4',
      oid: null,
      name: null,
      notification: null,
      known: false,
    };
    const sections = oidSections(unknown);
    expect(sections).toHaveLength(1);
    expect(sections[0]?.kind).toBe('note');
  });

  it('says a MAC’s block is not in the registry rather than leaving the vendor blank', () => {
    const sections = macSections({ ...mac, vendor: null });
    expect(labelled(sections, 'Vendor')).toMatchObject({
      value: 'not in the bundled IEEE registry',
      tone: 'idle',
    });
  });
});

describe('an MTU stack', () => {
  it('shows what is left and where the bytes went', () => {
    const result: MtuResult = {
      payload_mtu: 1422,
      tcp_mss_v4: 1382,
      tcp_mss_v6: 1362,
      overhead: 78,
      breakdown: [
        ['gre (IPv4 20 + GRE 4)', 24],
        ['ipsec-esp-tunnel', 54],
      ],
    };
    const sections = mtuSections(result);
    expect(labelled(sections, 'Payload MTU')?.value).toBe('1422');
    const table = sections.find((section) => section.kind === 'table');
    expect(table?.kind === 'table' && table.rows).toEqual([
      { name: 'gre (IPv4 20 + GRE 4)', bytes: 24 },
      { name: 'ipsec-esp-tunnel', bytes: 54 },
    ]);
  });
});

describe('the dispatcher', () => {
  it('sends each method’s result to the shape that describes it', () => {
    expect(sectionsFor('tools.subnet', v4).length).toBeGreaterThan(0);
    expect(sectionsFor('probe.ping', ping).length).toBeGreaterThan(0);
  });

  it('returns nothing for a method it does not know, instead of guessing a layout', () => {
    expect(sectionsFor('tools.something.new', { anything: true })).toEqual([]);
  });

  it('knows both QoS calculators, so neither answers with an empty panel', () => {
    expect(sectionsFor('tools.qos.marking', ef)).toEqual(markingSections(ef));
    expect(sectionsFor('tools.qos.voice', g729OverMlppp)).toEqual(voiceSections(g729OverMlppp));
    expect(sectionsFor('tools.qos.marking', ef).length).toBeGreaterThan(0);
    expect(sectionsFor('tools.qos.voice', g729OverMlppp).length).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------- qos

/** The sentences of one kind, in order. */
const notes = (sections: readonly Section[]): string[] =>
  sections.flatMap((section) => (section.kind === 'note' ? [section.text] : []));

const lines = (sections: readonly Section[], title: string) => {
  const found = sections.find((section) => section.kind === 'lines' && section.title === title);
  return found?.kind === 'lines' ? found : undefined;
};

/** The facts under one heading; `undefined` heading is the block at the top. */
const block = (sections: readonly Section[], title: string | undefined): Fact[] =>
  sections.flatMap((section) => (section.kind === 'facts' && section.title === title ? section.facts : []));

/**
 * Everything a list of sections puts on screen, as one text, with the thin
 * spaces that group digits taken out so a figure reads as the engine sent it.
 */
const onScreen = (sections: unknown): string => JSON.stringify(sections).replace(/\u2009/g, '');

describe('the engine’s per-hop-behaviour sentence, laid out', () => {
  it('reads the behaviour, the drop precedence and the source out of an assured-forwarding sentence', () => {
    expect(phbParts('Assured Forwarding class 4, low drop precedence (RFC 2597)')).toEqual({
      behaviour: 'Assured Forwarding class 4',
      drop: 'low',
      source: 'RFC 2597',
    });
    expect(phbParts('Assured Forwarding class 1, high drop precedence (RFC 2597)').drop).toBe('high');
    expect(phbParts('Assured Forwarding class 2, medium drop precedence (RFC 2597)').drop).toBe('medium');
  });

  it('gives no drop precedence to a behaviour the engine gave none', () => {
    expect(phbParts('Expedited Forwarding (RFC 3246)')).toEqual({
      behaviour: 'Expedited Forwarding',
      drop: undefined,
      source: 'RFC 3246',
    });
    expect(phbParts('Class Selector 5 (RFC 2474)').drop).toBeUndefined();
  });

  it('keeps a comma that is part of the sentence, not the start of a drop precedence', () => {
    expect(
      phbParts('Default Forwarding, best effort; the same code point as CS0 (RFC 2474)'),
    ).toEqual({
      behaviour: 'Default Forwarding, best effort; the same code point as CS0',
      drop: undefined,
      source: 'RFC 2474',
    });
  });

  it('keeps an unassigned value’s sentence whole, and invents no source for it', () => {
    for (const pool of [
      'No standard per-hop behaviour: pool 2, reserved for experimental or local use',
      'No standard per-hop behaviour: pool 1, unassigned (standards action)',
      'No standard per-hop behaviour: pool 3, unassigned (standards action, RFC 8436)',
    ]) {
      expect(phbParts(pool), pool).toEqual({ behaviour: pool, drop: undefined, source: undefined });
    }
  });

  it('loses no word of any sentence in the engine’s table', () => {
    // Every `phb` of `tools.reference {table: "dscp"}`, as the engine returned
    // them. Put back together, the parts are the sentence.
    for (const phb of PHB_SENTENCES) {
      const { behaviour, drop, source } = phbParts(phb);
      const again = `${behaviour}${drop ? `, ${drop} drop precedence` : ''}${source ? ` (${source})` : ''}`;
      expect(again).toBe(phb);
      expect(source, phb).toMatch(/^RFC \d+$/);
    }
  });
});

describe('a marking, in every form', () => {
  it('shows the name, the DSCP three ways and the byte three ways, each one copyable', () => {
    const sections = markingSections(ef);
    expect(labelled(sections, 'Name')).toMatchObject({ value: 'EF', copy: true });
    expect(labelled(sections, 'DSCP')).toMatchObject({ value: '46', copy: true });
    expect(labelled(sections, 'DSCP, binary')?.value).toBe('101110');
    expect(labelled(sections, 'DSCP, hex')?.value).toBe('0x2E');
    expect(labelled(sections, 'ToS byte')).toMatchObject({ value: '184', copy: true });
    expect(labelled(sections, 'ToS byte, binary')?.value).toBe('10111000');
    expect(labelled(sections, 'ToS byte, hex')?.value).toBe('0xB8');
    expect(labelled(sections, 'IP precedence')?.value).toBe('5 — CRITIC/ECP');
  });

  it('gives the per-hop behaviour and the source the engine cites, and no drop precedence it did not', () => {
    const sections = markingSections(ef);
    expect(labelled(sections, 'Per-hop behaviour')?.value).toBe('Expedited Forwarding');
    expect(labelled(sections, 'Defined in')?.value).toBe('RFC 3246');
    expect(labelled(sections, 'Drop precedence')).toBeUndefined();
    expect(labelled(sections, 'Typically for')?.value).toBe(ef.typical_use);
  });

  it('gives an assured-forwarding class its drop precedence', () => {
    const sections = markingSections(af41);
    expect(labelled(sections, 'Per-hop behaviour')?.value).toBe('Assured Forwarding class 4');
    expect(labelled(sections, 'Drop precedence')?.value).toBe('low');
    expect(labelled(sections, 'Defined in')?.value).toBe('RFC 2597');
  });

  it('shows the ECN bits a ToS byte carried, and what they say', () => {
    const sections = markingSections(tosB9);
    expect(labelled(sections, 'ToS byte')?.value).toBe('185');
    // The field's number is beside its bits: ECT(1) is 1, and ECT(0) is 2.
    expect(labelled(sections, 'ECN bits')?.value).toBe('01 (1) — ECT(1)');
    expect(labelled(sections, 'What ECN says')?.value).toBe(tosB9.ecn_meaning);
    // The engine's note about the byte with the ECN bits clear is on screen.
    expect(notes(sections)).toEqual(tosB9.notes);
  });

  it('says a value has no name and no standard use, instead of leaving the rows out', () => {
    const sections = markingSections(dscp47);
    expect(labelled(sections, 'Name')).toMatchObject({ tone: 'idle' });
    expect(labelled(sections, 'Name')?.value).toContain('not named');
    expect(labelled(sections, 'Name')?.copy).toBeUndefined();
    expect(labelled(sections, 'Typically for')).toMatchObject({ tone: 'idle' });
    // The registry pool is the whole of what the engine says about it.
    expect(labelled(sections, 'Per-hop behaviour')?.value).toBe(dscp47.phb);
    expect(labelled(sections, 'Defined in')).toBeUndefined();
    // The number itself is as good as any other.
    expect(labelled(sections, 'DSCP')?.value).toBe('47');
  });

  it('keeps DSCP 0 and precedence 0 as values', () => {
    const df: Marking = { ...ef, dscp: 0, tos: 0, precedence: 0, precedence_name: 'Routine' };
    const sections = markingSections(df);
    expect(labelled(sections, 'DSCP')?.value).toBe('0');
    expect(labelled(sections, 'ToS byte')?.value).toBe('0');
    expect(labelled(sections, 'IP precedence')?.value).toBe('0 — Routine');
  });

  it('passes on every note the engine attached, in its order', () => {
    // A precedence fixes three bits only; the engine says which code point it
    // answered with and why, and that sentence must reach the reader.
    expect(notes(markingSections(precedence5))).toEqual(precedence5.notes);
    expect(precedence5.notes).toHaveLength(2);
  });

  it('leaves no field of the engine’s answer off the screen', () => {
    // Not a marking: every field holds a value no other field has, so that
    // finding the value on screen proves that field is shown.
    const marked: Marking = {
      dscp: 1101,
      name: 'F-NAME',
      dscp_binary: 'F-DSCP-BINARY',
      dscp_hex: 'F-DSCP-HEX',
      tos: 1102,
      tos_hex: 'F-TOS-HEX',
      tos_binary: 'F-TOS-BINARY',
      precedence: 1103,
      precedence_name: 'F-PRECEDENCE-NAME',
      ecn: 1104,
      ecn_binary: 'F-ECN-BINARY',
      ecn_name: 'CE',
      ecn_meaning: 'F-ECN-MEANING',
      phb: 'F-PHB',
      typical_use: 'F-TYPICAL-USE',
      notes: ['F-NOTE-1', 'F-NOTE-2'],
    };
    const screen = onScreen(markingSections(marked));
    for (const [field, value] of Object.entries(marked)) {
      for (const one of [value].flat()) expect(screen, field).toContain(String(one));
    }
  });
});

describe('what voice calls cost', () => {
  it('shows the per-call and total figures as the engine wrote them, with the exact bits beside', () => {
    const top = block(voiceSections(g729OverMlppp), undefined);
    expect(top.find((entry) => entry.label === 'Per call')?.value).toBe('11.6 kbps (11\u2009600 bps)');
    expect(top.find((entry) => entry.label === 'Total')?.value).toBe(
      '116 kbps (116\u2009000 bps) for 10 calls',
    );
    expect(top.find((entry) => entry.label === 'Each packet')?.value).toBe('29 bytes, 50 of them a second');
  });

  it('formats the engine’s figures and never works one out', () => {
    // A result whose total is not calls × per call. The engine is the only
    // one that does this sum; if the screen did it too, 116 000 would appear.
    const odd: VoiceBandwidth = { ...g729OverMlppp, total_bps: 99, total: '99 bps', packet_bytes: 31 };
    const sections = voiceSections(odd);
    expect(block(sections, undefined).find((entry) => entry.label === 'Total')?.value).toBe(
      '99 bps (99 bps) for 10 calls',
    );
    expect(JSON.stringify(sections)).not.toContain('116\u2009000');
    const parts = sections.find((section) => section.kind === 'table');
    expect(parts?.kind === 'table' && parts.note).toBe('31 bytes in all.');
  });

  it('says one call in the singular', () => {
    const one: VoiceBandwidth = { ...g729OverMlppp, calls: 1 };
    expect(block(voiceSections(one), undefined).find((entry) => entry.label === 'Total')?.value).toMatch(
      / for 1 call$/,
    );
  });

  it('names the codec, the link and where the link’s figure is from', () => {
    const top = block(voiceSections(g729OverMlppp), undefined);
    expect(top.find((entry) => entry.label === 'Codec')?.value).toBe(
      'G.729: 20 ms of voice, 20 bytes, in a packet',
    );
    expect(top.find((entry) => entry.label === 'Link')?.value).toBe(
      'Multilink PPP: 7 bytes (MP header 6 + End-of-frame flag 1)',
    );
    expect(top.find((entry) => entry.label === 'Link figure from')?.value).toBe('Cisco 7934');
    expect(top.find((entry) => entry.label === 'Compressed RTP')?.value).toBe('Counted');
    expect(top.find((entry) => entry.label === 'Codec MOS')?.value).toBe('3.92');
  });

  it('describes the packet that was worked out, not the codec’s default one', () => {
    // G.726 at 24 kbps defaults to 20 ms and 60 bytes; this was asked at 40 ms,
    // which is 120. The codec's own default is under its picker, not here.
    expect(g726OverCustom.codec).toMatchObject({ packet_ms: 20, payload_bytes: 60 });
    const top = block(voiceSections(g726OverCustom), undefined);
    expect(top.find((entry) => entry.label === 'Codec')?.value).toBe(
      'G.726 (24 kbps): 40 ms of voice, 120 bytes, in a packet',
    );
    expect(top.find((entry) => entry.label === 'Each packet')?.value).toBe('186 bytes, 25 of them a second');
  });

  it('says the source gives no score for a codec, rather than showing none', () => {
    const mos = block(voiceSections(g726OverCustom), undefined).find((entry) => entry.label === 'Codec MOS');
    expect(mos).toMatchObject({ value: 'none given in the source', tone: 'idle' });
  });

  it('puts the voice-activity estimate beside the full figure, never in its place', () => {
    const sections = voiceSections(g729OverMlppp);
    const estimate = sections.find(
      (section) => section.kind === 'facts' && (section.title ?? '').includes('voice-activity detection'),
    );
    expect(estimate?.kind === 'facts' && estimate.title).toContain('an estimate');
    expect(estimate?.kind === 'facts' && estimate.facts.map((entry) => entry.value)).toEqual([
      '7.54 kbps (7\u2009540 bps)',
      '75.4 kbps (75\u2009400 bps)',
      'at best 35 %',
    ]);
    // The full figure is still the one at the top.
    expect(block(sections, undefined).find((entry) => entry.label === 'Per call')?.value).toContain(
      '11.6 kbps',
    );
  });

  it('warns, in the engine’s sentence, when there are too few calls for the average to mean anything', () => {
    const few = voiceSections(g729OverMlppp).find(
      (section) => section.kind === 'note' && section.text === g729OverMlppp.vad?.caveat,
    );
    expect(few).toMatchObject({ tone: 'warn' });
    const enough = voiceSections(g711OverEthernet).find(
      (section) => section.kind === 'note' && section.text === g711OverEthernet.vad?.caveat,
    );
    expect(enough).toMatchObject({ tone: 'info' });
  });

  it('shows no estimate at all when none was asked for', () => {
    const sections = voiceSections(g726OverCustom);
    expect(JSON.stringify(sections)).not.toContain('voice-activity');
    expect(sections.filter((section) => section.kind === 'facts')).toHaveLength(1);
  });

  it('lists the packet part by part, in the order the engine adds them, with its sum', () => {
    const parts = voiceSections(g729OverMlppp).find((section) => section.kind === 'table');
    expect(parts?.kind === 'table' && parts.rows).toEqual([
      { name: 'MP header', bytes: 6 },
      { name: 'End-of-frame flag', bytes: 1 },
      { name: 'cRTP-compressed IP/UDP/RTP header', bytes: 2 },
      { name: 'Voice payload', bytes: 20 },
    ]);
    expect(parts?.kind === 'table' && parts.note).toBe('29 bytes in all.');
  });

  it('shows the arithmetic line by line, numbered, exactly as the engine wrote it', () => {
    const working = lines(voiceSections(g729OverMlppp), 'The arithmetic');
    expect(working?.numbered).toBe(true);
    expect(working?.lines).toEqual(g729OverMlppp.arithmetic);
    expect(working?.lines).toHaveLength(6);
    expect(working?.lines[3]).toBe('Per call: 29 bytes × 8 bits × 50 packets per second = 11600 bps');
  });

  it('passes on every note and names every source the engine cites', () => {
    const sections = voiceSections(g726OverCustom);
    expect(notes(sections)).toEqual(g726OverCustom.notes);
    const sources = lines(sections, 'Where the figures come from');
    expect(sources?.lines).toEqual(g726OverCustom.sources);
    expect(sources?.lines).toHaveLength(2);
    // Sources are a list, not steps: their order is not an argument.
    expect(sources?.numbered).toBeUndefined();
  });

  it('draws no empty heading when the engine cites nothing', () => {
    const uncited: VoiceBandwidth = { ...g726OverCustom, sources: [] };
    expect(lines(voiceSections(uncited), 'Where the figures come from')).toBeUndefined();
  });

  it('leaves no figure of the engine’s answer off the screen', () => {
    // Not a calculation: every field holds a value no other field has, so
    // that finding the value on screen proves that field is shown.
    const priced: VoiceBandwidth = {
      codec: {
        id: 'f-codec-id',
        name: 'F-CODEC-NAME',
        bit_rate_bps: 2101,
        sample_bytes: 2102,
        sample_ms: 2103,
        packet_ms: 2104,
        payload_bytes: 2105,
        mos: 2.106,
      },
      link: {
        id: 'f-link-id',
        name: 'F-LINK-NAME',
        overhead_bytes: 2201,
        parts: 'F-LINK-PARTS',
        crtp: true,
        source: 'F-LINK-SOURCE',
        note: 'F-LINK-NOTE',
      },
      calls: 2301,
      crtp: true,
      packet_ms: 2302,
      packets_per_second: 2303.5,
      payload_bytes: 2304,
      packet: [
        { name: 'F-PART-1', bytes: 2401 },
        { name: 'F-PART-2', bytes: 2402 },
      ],
      packet_bytes: 2403,
      per_call_bps: 2501,
      total_bps: 2502,
      per_call: 'F-PER-CALL',
      total: 'F-TOTAL',
      vad: {
        saving_pct: 2601,
        per_call_bps: 2602,
        total_bps: 2603,
        per_call: 'F-VAD-PER-CALL',
        total: 'F-VAD-TOTAL',
        enough_calls: false,
        caveat: 'F-VAD-CAVEAT',
      },
      arithmetic: ['F-STEP-1', 'F-STEP-2'],
      notes: ['F-NOTE-1', 'F-NOTE-2'],
      sources: ['F-SOURCE-1', 'F-SOURCE-2'],
    };
    const screen = onScreen(voiceSections(priced));
    const shown = [
      priced.codec.name,
      priced.codec.mos,
      priced.link.name,
      priced.link.overhead_bytes,
      priced.link.parts,
      priced.link.source,
      priced.calls,
      priced.packet_ms,
      priced.packets_per_second,
      priced.payload_bytes,
      ...priced.packet.flatMap((part) => [part.name, part.bytes]),
      priced.packet_bytes,
      priced.per_call_bps,
      priced.total_bps,
      priced.per_call,
      priced.total,
      ...Object.values(priced.vad ?? {}).filter((value) => typeof value !== 'boolean'),
      ...priced.arithmetic,
      ...priced.notes,
      ...priced.sources,
    ];
    for (const value of shown) expect(screen).toContain(String(value));
    // The two switches are said in words, and a warning is a warning.
    expect(screen).toContain('Counted');
    expect(JSON.stringify(voiceSections(priced))).toContain('"tone":"warn"');
    // What is deliberately not repeated here is the codec's and the link's
    // own row of the engine's table — its id, the codec's nominal bit rate,
    // sample and default packet, whether the link takes compressed RTP — which
    // is under the picker it was chosen from; and the link's note, which the
    // engine itself puts among `notes` when it has one.
  });
});

// ------------------------------------------------------------------ fixtures
// Shapes as the engine returns them (checked against a running
// `nocturne-server` while this screen was built).

const v4: SubnetInfo = {
  version: 4,
  address: '10.1.2.33',
  network: '10.1.2.0',
  prefix: 26,
  cidr: '10.1.2.0/26',
  netmask: '255.255.255.192',
  wildcard: '0.0.0.63',
  broadcast: '10.1.2.63',
  first_host: '10.1.2.1',
  last_host: '10.1.2.62',
  usable_hosts: '62',
  total_addresses: '64',
  class: 'A',
  scope: 'private (RFC 1918)',
  binary_address: '00001010.00000001.00000010.00100001',
  binary_netmask: '11111111.11111111.11111111.11000000',
  hex: '0x0A010221',
  integer: '167838241',
  reverse_dns: '33.2.1.10.in-addr.arpa',
  expanded: '10.1.2.33',
  compressed: '10.1.2.33',
  is_network_address: false,
  is_broadcast_address: false,
};

const v6: SubnetInfo = {
  ...v4,
  version: 6,
  address: '2001:db8::',
  network: '2001:db8::',
  prefix: 64,
  cidr: '2001:db8::/64',
  netmask: '',
  wildcard: '',
  broadcast: '',
  first_host: '2001:db8::',
  last_host: '2001:db8::ffff:ffff:ffff:ffff',
  class: '',
  expanded: '2001:0db8:0000:0000:0000:0000:0000:0000',
  compressed: '2001:db8::',
};

const ping: PingResult = {
  target: '10.0.0.2',
  address: '10.0.0.2',
  method: 'icmp_datagram',
  sent: 2,
  received: 1,
  loss_pct: 50,
  min_ms: 0.083,
  avg_ms: 0.083,
  max_ms: 0.083,
  stddev_ms: 0,
  echoes: [
    { seq: 1, rtt_ms: 0.083, ttl: 64, error: null },
    { seq: 2, rtt_ms: null, ttl: null, error: 'no reply' },
  ],
  from: 'this computer',
  run_id: 'r1',
};

const ports: PortsResult = {
  ports: [
    { port: 22, state: 'open', rtt_ms: 0.083, service: 'ssh', banner: 'SSH-2.0-Test' },
    { port: 23, state: 'closed', rtt_ms: 1.117, service: 'telnet', banner: null },
  ],
  open: [22],
  scanned: 2,
  host: '10.0.0.2',
  from: 'this computer',
  run_id: 'r2',
};

const cert = {
  subject: 'CN=router.example.net',
  issuer: 'CN=router.example.net',
  common_name: 'router.example.net',
  sans: ['router.example.net'],
  not_before: '2026-01-01T00:00:00Z',
  not_after: '2026-12-01T00:00:00Z',
  days_left: 12,
  serial: '01',
  signature_algorithm: 'ecdsa-with-SHA256',
  public_key: 'ECDSA P-256',
  is_ca: false,
  self_signed: true,
  sha256_fingerprint: 'AB:CD',
};

const tls: TlsResult = {
  host: 'router.example.net',
  address: '10.0.0.2',
  port: 443,
  protocol: 'TLSv1.3',
  cipher: 'TLS13_AES_128_GCM_SHA256',
  alpn: null,
  chain: [cert],
  trusted: false,
  trust_error: 'self-signed certificate',
  hostname_matches: true,
  handshake_ms: 4.2,
  expires_in_days: 12,
  from: 'this computer',
  run_id: 'r3',
};

const deviceProbe: DeviceProbeResult = {
  command: 'ping 10.0.0.1 repeat 3',
  output:
    'Type escape sequence to abort.\nSending 3, 100-byte ICMP Echos to 10.0.0.1:\n...\nSuccess rate is 0 percent (0/3)',
  summary: { sent: 3, received: 0, loss_pct: 100, min_ms: null, avg_ms: null, max_ms: null },
  family: 'ios-like (IOS, IOS-XE, EOS)',
  from: 'SW1 itself',
  device_id: 'd1',
  device_name: 'SW1',
  run_id: 'r4',
};

const dns: DnsResult = {
  query: 'core-sw1.example.net',
  kind: 'A',
  server: '10.0.0.53:53',
  elapsed_ms: 4.3,
  records: [],
  rcode: 'NOERROR',
  from: 'this computer',
  run_id: 'r5',
  devices: [],
};

const mac: MacInfo = {
  colon: '00:50:56:ab:cd:ef',
  hyphen: '00-50-56-AB-CD-EF',
  dotted: '0050.56ab.cdef',
  bare: '005056abcdef',
  oui: '00:50:56',
  vendor: 'VMware',
  is_multicast: false,
  is_broadcast: false,
  is_locally_administered: false,
  eui64: '0250:56ff:feab:cdef',
  link_local_ipv6: 'fe80::250:56ff:feab:cdef',
};

// The QoS answers below are the engine's own, copied whole from a running
// `nocturne-server` on 2026-11-22 (the call is above each one). Nothing in
// them was written or adjusted by hand; a test that needs a shape the engine
// did not return builds it from one of these and says so.

/** `tools.qos.marking` {"value": "ef", "as": "name"} */
const ef: Marking = {
  dscp: 46,
  name: 'EF',
  dscp_binary: '101110',
  dscp_hex: '0x2E',
  tos: 184,
  tos_hex: '0xB8',
  tos_binary: '10111000',
  precedence: 5,
  precedence_name: 'CRITIC/ECP',
  ecn: 0,
  ecn_binary: '00',
  ecn_name: 'Not-ECT',
  ecn_meaning: 'Not ECN-capable: a congested router can only drop this packet (RFC 3168)',
  phb: 'Expedited Forwarding (RFC 3246)',
  typical_use: 'Voice bearer (RTP): low loss, low delay, low jitter, strictly policed',
  notes: [
    'The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear.',
  ],
};

/** `tools.qos.marking` {"value": "af41", "as": "dscp"} */
const af41: Marking = {
  dscp: 34,
  name: 'AF41',
  dscp_binary: '100010',
  dscp_hex: '0x22',
  tos: 136,
  tos_hex: '0x88',
  tos_binary: '10001000',
  precedence: 4,
  precedence_name: 'Flash Override',
  ecn: 0,
  ecn_binary: '00',
  ecn_name: 'Not-ECT',
  ecn_meaning: 'Not ECN-capable: a congested router can only drop this packet (RFC 3168)',
  phb: 'Assured Forwarding class 4, low drop precedence (RFC 2597)',
  typical_use: 'Multimedia conferencing: interactive video',
  notes: [
    'The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear.',
  ],
};

/** `tools.qos.marking` {"value": "0xb9", "as": "tos"} */
const tosB9: Marking = {
  dscp: 46,
  name: 'EF',
  dscp_binary: '101110',
  dscp_hex: '0x2E',
  tos: 185,
  tos_hex: '0xB9',
  tos_binary: '10111001',
  precedence: 5,
  precedence_name: 'CRITIC/ECP',
  ecn: 1,
  ecn_binary: '01',
  ecn_name: 'ECT(1)',
  ecn_meaning: 'ECN-capable transport; in RFC 9331 this codepoint identifies L4S traffic',
  phb: 'Expedited Forwarding (RFC 3246)',
  typical_use: 'Voice bearer (RTP): low loss, low delay, low jitter, strictly policed',
  notes: [
    'The DSCP is the top six bits of the byte; with the ECN bits clear the byte is 184 (0xB8).',
  ],
};

/** `tools.qos.marking` {"value": "5", "as": "precedence"} */
const precedence5: Marking = {
  dscp: 40,
  name: 'CS5',
  dscp_binary: '101000',
  dscp_hex: '0x28',
  tos: 160,
  tos_hex: '0xA0',
  tos_binary: '10100000',
  precedence: 5,
  precedence_name: 'CRITIC/ECP',
  ecn: 0,
  ecn_binary: '00',
  ecn_name: 'Not-ECT',
  ecn_meaning: 'Not ECN-capable: a congested router can only drop this packet (RFC 3168)',
  phb: 'Class Selector 5 (RFC 2474)',
  typical_use: 'Signalling in RFC 4594 (SIP, H.323); broadcast video in Cisco designs',
  notes: [
    'IP precedence fixes only the top three bits, so precedence 5 covers DSCP 40 to 47. Shown is the class selector CS5: what a precedence-5 packet carries when the other bits are clear (RFC 2474).',
    'The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear.',
  ],
};

/** `tools.qos.marking` {"value": "47", "as": "dscp"} */
const dscp47: Marking = {
  dscp: 47,
  name: null,
  dscp_binary: '101111',
  dscp_hex: '0x2F',
  tos: 188,
  tos_hex: '0xBC',
  tos_binary: '10111100',
  precedence: 5,
  precedence_name: 'CRITIC/ECP',
  ecn: 0,
  ecn_binary: '00',
  ecn_name: 'Not-ECT',
  ecn_meaning: 'Not ECN-capable: a congested router can only drop this packet (RFC 3168)',
  phb: 'No standard per-hop behaviour: pool 2, reserved for experimental or local use',
  typical_use: null,
  notes: [
    'The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear.',
  ],
};

/** `tools.qos.voice` {"codec": "g729", "calls": 10, "link": "mlppp", "crtp": true, "vad": true} */
const g729OverMlppp: VoiceBandwidth = {
  codec: {
    id: 'g729',
    name: 'G.729',
    bit_rate_bps: 8000,
    sample_bytes: 10,
    sample_ms: 10,
    packet_ms: 20,
    payload_bytes: 20,
    mos: 3.92,
  },
  link: {
    id: 'mlppp',
    name: 'Multilink PPP',
    overhead_bytes: 7,
    parts: 'MP header 6 + End-of-frame flag 1',
    crtp: true,
    source: 'Cisco 7934',
    note: 'The Cisco SRND table counts 10 bytes for MLPPP; use the `custom` link with 10 bytes for that figure.',
  },
  calls: 10,
  crtp: true,
  packet_ms: 20,
  packets_per_second: 50,
  payload_bytes: 20,
  packet: [
    { name: 'MP header', bytes: 6 },
    { name: 'End-of-frame flag', bytes: 1 },
    { name: 'cRTP-compressed IP/UDP/RTP header', bytes: 2 },
    { name: 'Voice payload', bytes: 20 },
  ],
  packet_bytes: 29,
  per_call_bps: 11600,
  total_bps: 116000,
  per_call: '11.6 kbps',
  total: '116 kbps',
  vad: {
    saving_pct: 35,
    per_call_bps: 7540,
    total_bps: 75400,
    per_call: '7.54 kbps',
    total: '75.4 kbps',
    enough_calls: false,
    caveat: 'Up to 35 % is an average across more than 24 calls; with 10 it must not be counted on. Provision the full figure.',
  },
  arithmetic: [
    'Voice payload: 20 ms ÷ 10 ms per sample × 10 bytes = 20 bytes',
    'Packet: 6 (MP header) + 1 (End-of-frame flag) + 2 (cRTP-compressed IP/UDP/RTP header) + 20 (Voice payload) = 29 bytes',
    'Packet rate: 1000 ms ÷ 20 ms = 50 packets per second',
    'Per call: 29 bytes × 8 bits × 50 packets per second = 11600 bps',
    'Total: 10 × 11600 bps = 116000 bps',
    'With voice-activity detection, at best 35 % less: 116000 bps × 65 % = 75400 bps',
  ],
  notes: [
    'One direction of each call; the other direction costs the same again.',
    'Voice packets only: call signalling and RTCP are not included.',
    'The Cisco SRND table counts 10 bytes for MLPPP; use the `custom` link with 10 bytes for that figure.',
    'cRTP is counted at 2 bytes, as in Cisco\'s table. With UDP checksums it is 4 bytes: 800 bps more per call. Both ends of the link have to run it.',
  ],
  sources: [
    'Cisco, "Modify Bandwidth Consumption Calculation for Voice Calls", Document ID 7934, revision 3.0 of 11 December 2023 (https://www.cisco.com/c/en/us/support/docs/voice/voice-quality/7934-bwidth-consume.html)',
  ],
};

/** `tools.qos.voice` {"codec": "g711", "calls": 30, "link": "ethernet", "crtp": false, "vad": true} */
const g711OverEthernet: VoiceBandwidth = {
  codec: {
    id: 'g711',
    name: 'G.711',
    bit_rate_bps: 64000,
    sample_bytes: 80,
    sample_ms: 10,
    packet_ms: 20,
    payload_bytes: 160,
    mos: 4.1,
  },
  link: {
    id: 'ethernet',
    name: 'Ethernet',
    overhead_bytes: 18,
    parts: 'Ethernet header 14 + Ethernet FCS 4',
    crtp: false,
    source: 'Cisco 7934',
    note: 'The frame as Cisco counts it: without the preamble and the inter-frame gap. The Cisco SRND table counts the 14-byte header alone.',
  },
  calls: 30,
  crtp: false,
  packet_ms: 20,
  packets_per_second: 50,
  payload_bytes: 160,
  packet: [
    { name: 'Ethernet header', bytes: 14 },
    { name: 'Ethernet FCS', bytes: 4 },
    { name: 'IPv4 header', bytes: 20 },
    { name: 'UDP header', bytes: 8 },
    { name: 'RTP header', bytes: 12 },
    { name: 'Voice payload', bytes: 160 },
  ],
  packet_bytes: 218,
  per_call_bps: 87200,
  total_bps: 2616000,
  per_call: '87.2 kbps',
  total: '2.62 Mbps',
  vad: {
    saving_pct: 35,
    per_call_bps: 56680,
    total_bps: 1700400,
    per_call: '56.68 kbps',
    total: '1.7 Mbps',
    enough_calls: true,
    caveat: 'Up to 35 % is an average over time across more than 24 calls, never a promise for one call, and music on hold and fax defeat it. Cisco\'s advice is to provision the full figure and let data use what VAD frees.',
  },
  arithmetic: [
    'Voice payload: 20 ms ÷ 10 ms per sample × 80 bytes = 160 bytes',
    'Packet: 14 (Ethernet header) + 4 (Ethernet FCS) + 20 (IPv4 header) + 8 (UDP header) + 12 (RTP header) + 160 (Voice payload) = 218 bytes',
    'Packet rate: 1000 ms ÷ 20 ms = 50 packets per second',
    'Per call: 218 bytes × 8 bits × 50 packets per second = 87200 bps',
    'Total: 30 × 87200 bps = 2616000 bps',
    'With voice-activity detection, at best 35 % less: 2616000 bps × 65 % = 1700400 bps',
  ],
  notes: [
    'One direction of each call; the other direction costs the same again.',
    'Voice packets only: call signalling and RTCP are not included.',
    'The frame as Cisco counts it: without the preamble and the inter-frame gap. The Cisco SRND table counts the 14-byte header alone.',
  ],
  sources: [
    'Cisco, "Modify Bandwidth Consumption Calculation for Voice Calls", Document ID 7934, revision 3.0 of 11 December 2023 (https://www.cisco.com/c/en/us/support/docs/voice/voice-quality/7934-bwidth-consume.html)',
  ],
};

/** `tools.qos.voice` {"codec": "g726-24", "calls": 2, "link": "custom", "l2_bytes": 26, "packet_ms": 40, "crtp": false, "vad": false} */
const g726OverCustom: VoiceBandwidth = {
  codec: {
    id: 'g726-24',
    name: 'G.726 (24 kbps)',
    bit_rate_bps: 24000,
    sample_bytes: 15,
    sample_ms: 5,
    packet_ms: 20,
    payload_bytes: 60,
    mos: null,
  },
  link: {
    id: 'custom',
    name: 'Custom layer 2',
    overhead_bytes: 26,
    parts: 'Layer 2, as given 26',
    crtp: true,
    source: 'given with the request',
    note: '',
  },
  calls: 2,
  crtp: false,
  packet_ms: 40,
  packets_per_second: 25,
  payload_bytes: 120,
  packet: [
    { name: 'Layer 2, as given', bytes: 26 },
    { name: 'IPv4 header', bytes: 20 },
    { name: 'UDP header', bytes: 8 },
    { name: 'RTP header', bytes: 12 },
    { name: 'Voice payload', bytes: 120 },
  ],
  packet_bytes: 186,
  per_call_bps: 37200,
  total_bps: 74400,
  per_call: '37.2 kbps',
  total: '74.4 kbps',
  vad: null,
  arithmetic: [
    'Voice payload: 40 ms ÷ 5 ms per sample × 15 bytes = 120 bytes',
    'Packet: 26 (Layer 2, as given) + 20 (IPv4 header) + 8 (UDP header) + 12 (RTP header) + 120 (Voice payload) = 186 bytes',
    'Packet rate: 1000 ms ÷ 40 ms = 25 packets per second',
    'Per call: 186 bytes × 8 bits × 25 packets per second = 37200 bps',
    'Total: 2 × 37200 bps = 74400 bps',
  ],
  notes: [
    'One direction of each call; the other direction costs the same again.',
    'Voice packets only: call signalling and RTCP are not included.',
    'Cisco\'s SRND says a sampling rate above 30 ms usually results in very poor voice quality: 40 ms of voice in a packet is that much delay before it is sent.',
  ],
  sources: [
    'Cisco, "Modify Bandwidth Consumption Calculation for Voice Calls", Document ID 7934, revision 3.0 of 11 December 2023 (https://www.cisco.com/c/en/us/support/docs/voice/voice-quality/7934-bwidth-consume.html)',
    'Cisco Collaboration System 12.x SRND, chapter 3 "Network Infrastructure", revised 1 March 2018, tables 3-7 and 3-8 (https://www.cisco.com/c/en/us/td/docs/voice_ip_comm/cucm/srnd/collab12/collab12/netstruc.pdf)',
  ],
};


/** Every `phb` of `tools.reference {"table": "dscp"}`, in the engine's order. */
const PHB_SENTENCES = [
  'Class Selector 7 (RFC 2474)',
  'Class Selector 6 (RFC 2474)',
  'Expedited Forwarding (RFC 3246)',
  'Non-Queue-Building (RFC 9956)',
  'Voice-Admit (RFC 5865)',
  'Class Selector 5 (RFC 2474)',
  'Assured Forwarding class 4, high drop precedence (RFC 2597)',
  'Assured Forwarding class 4, medium drop precedence (RFC 2597)',
  'Assured Forwarding class 4, low drop precedence (RFC 2597)',
  'Class Selector 4 (RFC 2474)',
  'Assured Forwarding class 3, high drop precedence (RFC 2597)',
  'Assured Forwarding class 3, medium drop precedence (RFC 2597)',
  'Assured Forwarding class 3, low drop precedence (RFC 2597)',
  'Class Selector 3 (RFC 2474)',
  'Assured Forwarding class 2, high drop precedence (RFC 2597)',
  'Assured Forwarding class 2, medium drop precedence (RFC 2597)',
  'Assured Forwarding class 2, low drop precedence (RFC 2597)',
  'Class Selector 2 (RFC 2474)',
  'Assured Forwarding class 1, high drop precedence (RFC 2597)',
  'Assured Forwarding class 1, medium drop precedence (RFC 2597)',
  'Assured Forwarding class 1, low drop precedence (RFC 2597)',
  'Class Selector 1 (RFC 2474)',
  'Lower Effort (RFC 8622)',
  'Default Forwarding, best effort; the same code point as CS0 (RFC 2474)',
];
