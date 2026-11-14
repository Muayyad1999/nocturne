import { describe, expect, it } from 'vitest';
import type {
  Base64Result,
  DeviceProbeResult,
  DnsResult,
  MacInfo,
  MaskInfo,
  MtuResult,
  OidResult,
  PingResult,
  PortsResult,
  SubnetInfo,
  TlsResult,
} from '@/api/types/toolbox';
import { formatCount } from '@/lib/format';
import {
  base64Sections,
  deviceProbeSections,
  dnsSections,
  fact,
  lossFact,
  macSections,
  maskSections,
  ms,
  mtuSections,
  oidSections,
  pingSections,
  portSections,
  sectionsFor,
  subnetSections,
  tlsSections,
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
