/**
 * Every answer this screen shows, worked out as data instead of markup.
 *
 * A calculator's result is a handful of facts, a table, a piece of text, or a
 * sentence — never a bespoke layout — so each result shape is turned into
 * {@link Section}s here, in a pure function, and one component renders them.
 * That is what makes it possible to cover forty-odd tools without forty-odd
 * layouts drifting apart, and it is what lets the interesting parts be tested
 * without a browser.
 *
 * The rules that live in this file, because they are easy to get wrong in
 * markup and impossible to get wrong in a tested function:
 *
 * * **An absent value is left out, never printed as nothing.** The backend
 *   writes `""` for the netmask of an IPv6 prefix and for the broadcast
 *   address of a /31, because those do not exist — so {@link fact} drops an
 *   empty value rather than showing a label with a dash after it, and `null`
 *   values say what they mean in words ("not in the bundled registry").
 * * **A measured zero is shown and named.** `0 % loss` and `100 % loss` are
 *   both readings and both get a tone; neither is ever produced for a probe
 *   that could not run — that path never reaches this file (see `probes.ts`).
 * * **A truncated list says so.** A capped set of subnets, reference rows or
 *   OUI assignments carries the note, and the number shown is never presented
 *   as the total.
 * * **A figure is the engine's.** The two QoS calculators come back with
 *   their arithmetic, their notes and their sources already written; what is
 *   done here is layout — which line goes where, digits grouped — and never a
 *   sum. A bandwidth worked out a second time in TypeScript would be a second
 *   answer, and the day the two differ nobody could say which is shown.
 */

import type {
  AclResult,
  Hop,
  Base64Result,
  CertInfo,
  Conversion,
  ContainsResult,
  ConvertResult,
  DeviceProbeResult,
  Digests,
  DnsResult,
  History,
  HttpResult,
  InterfaceResult,
  LocalReport,
  MacInfo,
  Marking,
  MaskInfo,
  MtuResult,
  NtpResult,
  OidResult,
  OuiResult,
  OverlapResult,
  PingResult,
  PortsResult,
  RangeResult,
  SplitResult,
  SubnetInfo,
  SummaryResult,
  SweepResult,
  SweptHost,
  TlsResult,
  TraceResult,
  TransferEstimate,
  VlsmPlan,
  VoiceBandwidth,
  WhoisResult,
  WolResult,
} from '@/api/types/toolbox';
import { formatCount, formatDuration } from '@/lib/format';

export type FactTone = 'ok' | 'warn' | 'crit' | 'info' | 'idle';

export interface Fact {
  label: string;
  value: string;
  /** Monospace: anything a device said or will be told. */
  mono?: boolean;
  tone?: FactTone;
  /** Offers a copy button: an address, a key, a command. */
  copy?: boolean;
}

export interface Column {
  id: string;
  header: string;
  align?: 'left' | 'right';
  mono?: boolean;
  /**
   * Width in pixels. Columns without one share what is left, so a column of
   * two-digit numbers beside a column of prose is given its width here —
   * otherwise the table spreads them evenly and a sequence number sits a
   * hundred pixels from the time beside it.
   */
  width?: number;
}

export type Row = Record<string, string | number | boolean | null>;

export type Section =
  | { kind: 'facts'; title?: string; facts: Fact[] }
  | { kind: 'table'; title?: string; columns: Column[]; rows: Row[]; empty: string; note?: string }
  | { kind: 'code'; title: string; text: string; wrap?: boolean }
  /** Sentences the engine wrote, one per line: the steps of a sum, the sources of a figure. */
  | { kind: 'lines'; title: string; lines: string[]; numbered?: boolean; mono?: boolean }
  | { kind: 'note'; text: string; tone?: FactTone };

// ----------------------------------------------------------------- helpers

/** A fact, unless there is nothing to say: an empty string is not a value. */
export function fact(
  label: string,
  value: string | number | null | undefined,
  extra: Omit<Fact, 'label' | 'value'> = {},
): Fact[] {
  if (value === null || value === undefined) return [];
  const text = typeof value === 'number' ? String(value) : value;
  if (text.trim() === '') return [];
  return [{ label, value: text, ...extra }];
}

const yesNo = (value: boolean, yes: string, no: string): string => (value ? yes : no);

/** A round-trip time as a person reads one. `null` is the caller's to handle. */
export function ms(value: number | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const digits = value < 10 ? 2 : value < 100 ? 1 : 0;
  return `${value.toFixed(digits)} ms`;
}

/** Packet loss. A percentage that is a *reading*, with the tone it deserves. */
export function lossFact(lossPct: number, received: number, sent: number): Fact {
  return {
    label: 'Loss',
    value: `${lossPct}% — ${received} of ${sent} answered`,
    tone: lossPct === 0 ? 'ok' : lossPct >= 100 ? 'crit' : 'warn',
  };
}

const facts = (items: Fact[], title?: string): Section => ({ kind: 'facts', title, facts: items });

function table(
  columns: Column[],
  rows: Row[],
  empty: string,
  title?: string,
  note?: string,
): Section {
  return { kind: 'table', columns, rows, empty, title, note };
}

// ------------------------------------------------------------- addressing

export function subnetSections(info: SubnetInfo): Section[] {
  const range =
    info.first_host && info.last_host ? `${info.first_host} – ${info.last_host}` : undefined;
  return [
    facts([
      ...fact('Network', info.cidr, { mono: true, copy: true }),
      ...fact('Netmask', info.netmask, { mono: true, copy: true }),
      ...fact('Wildcard', info.wildcard, { mono: true, copy: true }),
      ...fact('Broadcast', info.broadcast, { mono: true }),
      ...fact('Host range', range, { mono: true }),
      ...fact('Usable hosts', info.usable_hosts, { mono: true }),
      ...fact('Addresses', info.total_addresses, { mono: true }),
      ...fact('Class', info.class),
      ...fact('Scope', info.scope),
      {
        label: 'The address given',
        value: info.is_network_address
          ? `${info.address} is the network address`
          : info.is_broadcast_address
            ? `${info.address} is the broadcast address`
            : `${info.address} is a host address`,
        mono: false,
      },
    ]),
    facts(
      [
        ...fact('Binary', info.binary_address, { mono: true }),
        ...fact('Binary netmask', info.binary_netmask, { mono: true }),
        ...fact('Hex', info.hex, { mono: true, copy: true }),
        ...fact('Integer', info.integer, { mono: true, copy: true }),
        ...fact('Reverse DNS', info.reverse_dns, { mono: true, copy: true }),
        ...(info.version === 6
          ? [
              ...fact('Expanded', info.expanded, { mono: true, copy: true }),
              ...fact('Compressed', info.compressed, { mono: true, copy: true }),
            ]
          : []),
      ],
      'In other forms',
    ),
  ];
}

const SUBNET_COLUMNS: Column[] = [
  { id: 'cidr', header: 'Network', mono: true },
  { id: 'netmask', header: 'Netmask', mono: true },
  { id: 'first_host', header: 'First host', mono: true },
  { id: 'last_host', header: 'Last host', mono: true },
  { id: 'broadcast', header: 'Broadcast', mono: true },
  { id: 'usable_hosts', header: 'Hosts', align: 'right', mono: true, width: 90 },
];

export function splitSections(result: SplitResult): Section[] {
  return [
    facts([
      { label: 'Subnets', value: result.total, mono: true },
      ...(result.truncated
        ? [
            {
              label: 'Shown',
              value: `the first ${result.subnets.length} of them`,
              tone: 'warn' as FactTone,
            },
          ]
        : []),
    ]),
    table(
      SUBNET_COLUMNS,
      result.subnets.map((subnet) => ({ ...subnet })),
      'Nothing to split.',
      undefined,
      result.truncated
        ? `${result.total} subnets in all; the table holds the first ${result.subnets.length}.`
        : undefined,
    ),
  ];
}

export function vlsmSections(plan: VlsmPlan): Section[] {
  return [
    facts([
      { label: 'Addresses used', value: formatCount(plan.used_addresses), mono: true },
      { label: 'Addresses in all', value: formatCount(plan.total_addresses), mono: true },
      {
        label: 'Left over',
        value: `${formatCount(plan.total_addresses - plan.used_addresses)} in ${plan.unallocated.length} ${
          plan.unallocated.length === 1 ? 'block' : 'blocks'
        }`,
        mono: false,
      },
    ]),
    table(
      [
        { id: 'name', header: 'Name' },
        { id: 'hosts_requested', header: 'Asked for', align: 'right', mono: true, width: 112 },
        { id: 'cidr', header: 'Network', mono: true },
        { id: 'first_host', header: 'First host', mono: true },
        { id: 'last_host', header: 'Last host', mono: true },
        { id: 'usable_hosts', header: 'Hosts', align: 'right', mono: true, width: 90 },
        { id: 'spare', header: 'Spare', align: 'right', mono: true, width: 90 },
      ],
      plan.allocations.map((allocation) => ({ ...allocation })),
      'Nothing was asked for.',
      'Allocations, largest first',
    ),
    table(
      [{ id: 'cidr', header: 'Still free', mono: true }],
      plan.unallocated.map((cidr) => ({ cidr })),
      'The base network is fully allocated.',
      'What is left',
    ),
  ];
}

export function supernetSections(result: SummaryResult): Section[] {
  return [
    facts([{ label: 'Single supernet', value: result.supernet, mono: true, copy: true }]),
    table(
      [{ id: 'cidr', header: 'Summarised blocks', mono: true }],
      result.summarized.map((cidr) => ({ cidr })),
      'Nothing to summarise.',
      undefined,
      'These cover exactly the input. The single supernet above may cover more.',
    ),
  ];
}

export function containsSections(result: ContainsResult): Section[] {
  return [
    facts([
      {
        label: 'Contains it',
        value: yesNo(result.contains, 'Yes', 'No'),
        tone: result.contains ? 'ok' : 'idle',
      },
      {
        label: 'Overlap at all',
        value: yesNo(result.overlaps, 'Yes', 'No'),
        tone: result.overlaps ? 'ok' : 'idle',
      },
    ]),
  ];
}

export function overlapSections(result: OverlapResult): Section[] {
  return [
    {
      kind: 'note',
      tone: result.any ? 'warn' : 'ok',
      text: result.any
        ? `${result.pairs.length} ${result.pairs.length === 1 ? 'pair overlaps' : 'pairs overlap'}.`
        : 'No two of those blocks overlap.',
    },
    table(
      [
        { id: 'a', header: 'This block', mono: true },
        { id: 'b', header: 'overlaps', mono: true },
      ],
      result.pairs.map((pair) => ({ ...pair })),
      'Nothing overlaps.',
    ),
  ];
}

export function rangeSections(result: RangeResult): Section[] {
  return [
    facts([{ label: 'Blocks', value: String(result.cidrs.length), mono: true }]),
    table(
      [{ id: 'cidr', header: 'CIDR', mono: true }],
      result.cidrs.map((cidr) => ({ cidr })),
      'That range covers nothing.',
      undefined,
      'The smallest set of blocks that covers the range exactly.',
    ),
  ];
}

export function convertAddressSections(result: ConvertResult): Section[] {
  const sections: Section[] = [];
  if (result.number) {
    sections.push(
      facts(
        [
          ...fact('Decimal', result.number.decimal, { mono: true, copy: true }),
          ...fact('Hex', result.number.hex, { mono: true, copy: true }),
          ...fact('Binary', result.number.binary, { mono: true }),
          ...fact('Octal', result.number.octal, { mono: true }),
          ...fact('As an IPv4 address', result.number.ipv4, { mono: true, copy: true }),
        ],
        'As a number',
      ),
    );
  }
  if (result.address) {
    const address = result.address;
    sections.push(
      facts(
        [
          ...fact('Address', address.address, { mono: true, copy: true }),
          ...fact('Version', `IPv${address.version}`),
          ...fact('Scope', address.scope),
          ...fact('Integer', address.integer, { mono: true, copy: true }),
          ...fact('Hex', address.hex, { mono: true, copy: true }),
          ...fact('Binary', address.binary, { mono: true }),
          ...fact('Expanded', address.expanded, { mono: true, copy: true }),
          ...fact('Compressed', address.compressed, { mono: true, copy: true }),
          ...fact('Reverse DNS', address.reverse_dns, { mono: true, copy: true }),
        ],
        'As an address',
      ),
    );
  }
  if (sections.length === 0) {
    sections.push({
      kind: 'note',
      tone: 'idle',
      text: 'That is neither a number nor an address, so there is nothing to convert.',
    });
  }
  return sections;
}

export function maskSections(info: MaskInfo): Section[] {
  return [
    facts([
      ...(info.prefix === null
        ? [
            {
              label: 'Prefix length',
              value: 'none — the mask is not contiguous',
              tone: 'warn' as FactTone,
            },
          ]
        : fact('Prefix length', `/${info.prefix}`, { mono: true, copy: true })),
      ...fact('Netmask', info.netmask, { mono: true, copy: true }),
      ...fact('Wildcard', info.wildcard, { mono: true, copy: true }),
      ...fact('Addresses covered', info.addresses, { mono: true }),
      {
        label: 'Contiguous',
        value: yesNo(info.contiguous, 'Yes', 'No — it has gaps, which only an ACL can express'),
        tone: info.contiguous ? 'ok' : 'warn',
      },
    ]),
  ];
}

export function aclSections(result: AclResult): Section[] {
  return [
    { kind: 'note', text: result.explanation },
    facts([
      ...(result.prefix === null
        ? [{ label: 'Prefix length', value: 'none — not a contiguous mask', tone: 'warn' as FactTone }]
        : fact('Prefix length', `/${result.prefix}`, { mono: true })),
      ...fact('Netmask', result.netmask, { mono: true }),
      ...fact('Addresses matched', result.addresses, { mono: true }),
      ...(result.matches === null
        ? []
        : [
            {
              label: 'The candidate',
              value: result.matches ? 'is matched by the pair' : 'is not matched by the pair',
              tone: (result.matches ? 'ok' : 'idle') as FactTone,
            },
          ]),
    ]),
  ];
}

// ---------------------------------------------------------------- identity

export function macSections(info: MacInfo): Section[] {
  return [
    facts([
      { label: 'Colons', value: info.colon, mono: true, copy: true },
      { label: 'Hyphens', value: info.hyphen, mono: true, copy: true },
      { label: 'Cisco', value: info.dotted, mono: true, copy: true },
      { label: 'Bare', value: info.bare, mono: true, copy: true },
    ]),
    facts(
      [
        { label: 'OUI', value: info.oui, mono: true },
        {
          label: 'Vendor',
          value: info.vendor ?? 'not in the bundled IEEE registry',
          tone: info.vendor ? undefined : 'idle',
        },
        {
          label: 'Addressing',
          value: info.is_broadcast
            ? 'the broadcast address'
            : info.is_multicast
              ? 'multicast'
              : 'unicast',
        },
        {
          label: 'Assigned by',
          value: info.is_locally_administered
            ? 'an administrator or software (locally administered)'
            : 'a manufacturer (globally unique)',
        },
      ],
      'What it is',
    ),
    facts(
      [
        { label: 'EUI-64', value: info.eui64, mono: true, copy: true },
        { label: 'IPv6 link-local', value: info.link_local_ipv6, mono: true, copy: true },
      ],
      'What SLAAC makes of it',
    ),
  ];
}

export function ouiSections(result: OuiResult): Section[] {
  return [
    facts([
      {
        label: 'Read as',
        value: result.read_as === 'prefix' ? 'a MAC address or OUI prefix' : 'part of a vendor’s name',
      },
      { label: 'Matches', value: formatCount(result.total), mono: true },
      {
        label: 'Registry',
        value: `${formatCount(result.registry.assignments)} assignments, ${result.registry.date}`,
      },
    ]),
    table(
      [
        { id: 'prefix', header: 'Prefix', mono: true, width: 140 },
        { id: 'vendor', header: 'Vendor' },
      ],
      result.assignments.map((entry) => ({ ...entry })),
      'Nothing in the registry matches that.',
      undefined,
      result.note ??
        (result.total > result.assignments.length
          ? `${formatCount(result.total)} matches; the table holds the first ${result.assignments.length}.`
          : undefined),
    ),
  ];
}

export function oidSections(result: OidResult): Section[] {
  if (!result.known) {
    return [
      {
        kind: 'note',
        tone: 'idle',
        text: `Nothing in the built-in dictionary matches \`${result.input}\`. It may still be a valid OID from a MIB Nocturne does not carry.`,
      },
    ];
  }
  return [
    facts([
      ...fact('OID', result.oid, { mono: true, copy: true }),
      ...fact('Name', result.name, { mono: true, copy: true }),
      ...fact('Notification', result.notification, { mono: true }),
    ]),
  ];
}

export function interfaceSections(result: InterfaceResult): Section[] {
  return [
    facts([
      { label: 'Long', value: result.long, mono: true, copy: true },
      { label: 'Short', value: result.short, mono: true, copy: true },
    ]),
  ];
}

// -------------------------------------------------------------- converters

export function conversionSections(result: Conversion): Section[] {
  return [
    facts([
      { label: 'Read as', value: `${result.input} (${result.family})` },
      { label: 'As people write it', value: result.human, mono: true, copy: true },
      {
        label: `In ${result.canonical_unit}`,
        value: String(result.canonical),
        mono: true,
        copy: true,
      },
      ...(result.unit === null || result.value === null
        ? []
        : [
            {
              label: `In ${result.unit}`,
              value: String(result.value),
              mono: true,
              copy: true,
              tone: 'info' as FactTone,
            },
          ]),
    ]),
    table(
      [
        { id: 'unit', header: 'Unit', width: 120 },
        { id: 'value', header: 'Value', align: 'right', mono: true },
      ],
      result.forms.map((form) => ({ ...form })),
      'No other unit in this family.',
      'Every unit of the family',
    ),
  ];
}

export function transferSections(result: TransferEstimate): Section[] {
  return [
    facts([
      { label: 'Time', value: result.human, mono: true },
      { label: 'Seconds', value: String(result.seconds), mono: true },
      {
        label: 'Effective rate',
        value: `${formatCount(result.effective_bps)} bit/s`,
        mono: true,
      },
      { label: 'As a duration', value: formatDuration(result.seconds * 1000) },
    ]),
  ];
}

export function mtuSections(result: MtuResult): Section[] {
  return [
    facts([
      { label: 'Payload MTU', value: String(result.payload_mtu), mono: true, copy: true, tone: 'info' },
      { label: 'TCP MSS, IPv4', value: String(result.tcp_mss_v4), mono: true, copy: true },
      { label: 'TCP MSS, IPv6', value: String(result.tcp_mss_v6), mono: true, copy: true },
      { label: 'Overhead', value: `${result.overhead} bytes`, mono: true },
    ]),
    table(
      [
        { id: 'name', header: 'Encapsulation' },
        { id: 'bytes', header: 'Bytes', align: 'right', mono: true, width: 90 },
      ],
      result.breakdown.map(([name, bytes]) => ({ name, bytes })),
      'No encapsulation: the payload is the link MTU.',
      'Where the bytes go',
    ),
  ];
}

// --------------------------------------------------------------------- qos

/**
 * The engine's per-hop-behaviour sentence, laid out for reading.
 *
 * The engine says three things in one sentence — `Assured Forwarding class 4,
 * low drop precedence (RFC 2597)` — and a reader looking for the drop
 * precedence should not have to parse it. This only cuts the sentence where
 * the engine itself put the comma and the bracket; every word shown is still
 * the engine's. A sentence that is not in that shape (an unassigned value:
 * `No standard per-hop behaviour: pool 2, …`) is kept whole, with no drop
 * precedence and no source invented for it.
 */
export function phbParts(phb: string): { behaviour: string; drop?: string; source?: string } {
  // Only a bare RFC in the closing bracket is a citation: `(standards action)`
  // at the end of a pool's description is part of what it says.
  const cited = /^(.*\S)\s+\((RFC \d+)\)$/.exec(phb.trim());
  const sentence = cited?.[1] ?? phb.trim();
  const dropped = /^(.*\S), (\w+) drop precedence$/.exec(sentence);
  return {
    behaviour: dropped?.[1] ?? sentence,
    drop: dropped?.[2],
    source: cited?.[2],
  };
}

export function markingSections(marking: Marking): Section[] {
  const phb = phbParts(marking.phb);
  return [
    facts([
      marking.name === null
        ? {
            label: 'Name',
            value: 'none — the registry has not named this value',
            tone: 'idle',
          }
        : { label: 'Name', value: marking.name, mono: true, copy: true, tone: 'info' },
      { label: 'DSCP', value: String(marking.dscp), mono: true, copy: true },
      { label: 'DSCP, binary', value: marking.dscp_binary, mono: true, copy: true },
      { label: 'DSCP, hex', value: marking.dscp_hex, mono: true, copy: true },
    ]),
    facts(
      [
        { label: 'ToS byte', value: String(marking.tos), mono: true, copy: true },
        { label: 'ToS byte, binary', value: marking.tos_binary, mono: true, copy: true },
        { label: 'ToS byte, hex', value: marking.tos_hex, mono: true, copy: true },
        {
          label: 'IP precedence',
          value: `${marking.precedence} — ${marking.precedence_name}`,
          mono: true,
        },
        // The number as well as the bits: ECT(1) is 1 and ECT(0) is 2, so the
        // name alone invites reading the field the wrong way round.
        {
          label: 'ECN bits',
          value: `${marking.ecn_binary} (${marking.ecn}) — ${marking.ecn_name}`,
          mono: true,
        },
        ...fact('What ECN says', marking.ecn_meaning),
      ],
      'The whole byte',
    ),
    facts(
      [
        { label: 'Per-hop behaviour', value: phb.behaviour },
        ...fact('Drop precedence', phb.drop),
        ...fact('Defined in', phb.source),
        marking.typical_use === null
          ? { label: 'Typically for', value: 'nothing: this value has no standard use', tone: 'idle' }
          : { label: 'Typically for', value: marking.typical_use },
      ],
      'How a router treats it',
    ),
    ...marking.notes.map((text): Section => ({ kind: 'note', tone: 'idle', text })),
  ];
}

/** A rate the engine wrote for reading, with its exact figure beside it. */
const bps = (written: string, exact: number): string => `${written} (${formatCount(exact)} bps)`;

export function voiceSections(result: VoiceBandwidth): Section[] {
  const { codec, link, vad } = result;
  const sections: Section[] = [
    facts([
      {
        label: 'Per call',
        value: bps(result.per_call, result.per_call_bps),
        mono: true,
        tone: 'info',
      },
      {
        label: 'Total',
        value: `${bps(result.total, result.total_bps)} for ${formatCount(result.calls)} ${
          result.calls === 1 ? 'call' : 'calls'
        }`,
        mono: true,
        tone: 'info',
      },
      {
        label: 'Each packet',
        value: `${result.packet_bytes} bytes, ${result.packets_per_second} of them a second`,
        mono: true,
      },
      { label: 'Compressed RTP', value: yesNo(result.crtp, 'Counted', 'Not counted') },
      {
        label: 'Codec',
        value: `${codec.name}: ${result.packet_ms} ms of voice, ${result.payload_bytes} bytes, in a packet`,
      },
      {
        label: 'Codec MOS',
        value: codec.mos === null ? 'none given in the source' : String(codec.mos),
        tone: codec.mos === null ? 'idle' : undefined,
      },
      { label: 'Link', value: `${link.name}: ${link.overhead_bytes} bytes (${link.parts})` },
      ...fact('Link figure from', link.source),
    ]),
  ];

  if (vad) {
    sections.push(
      facts(
        [
          { label: 'Per call', value: bps(vad.per_call, vad.per_call_bps), mono: true },
          { label: 'Total', value: bps(vad.total, vad.total_bps), mono: true },
          { label: 'Saving assumed', value: `at best ${vad.saving_pct} %` },
        ],
        // Beside the full figure, never in its place: the heading says which
        // of the two is the estimate.
        'With voice-activity detection — an estimate, not the figure to provision',
      ),
      { kind: 'note', tone: vad.enough_calls ? 'info' : 'warn', text: vad.caveat },
    );
  }

  sections.push(
    table(
      [
        { id: 'name', header: 'Part of the packet' },
        { id: 'bytes', header: 'Bytes', align: 'right', mono: true, width: 90 },
      ],
      result.packet.map((part) => ({ ...part })),
      'The engine listed no parts for this packet.',
      'Where the bytes go',
      `${result.packet_bytes} bytes in all.`,
    ),
    { kind: 'lines', title: 'The arithmetic', lines: result.arithmetic, numbered: true, mono: true },
    ...result.notes.map((text): Section => ({ kind: 'note', tone: 'idle', text })),
  );
  if (result.sources.length > 0) {
    sections.push({ kind: 'lines', title: 'Where the figures come from', lines: result.sources });
  }
  return sections;
}

// -------------------------------------------------------------------- text

export function digestSections(result: Digests): Section[] {
  return [
    facts([
      { label: 'Bytes', value: formatCount(result.bytes), mono: true },
      { label: 'MD5', value: result.md5, mono: true, copy: true },
      { label: 'SHA-1', value: result.sha1, mono: true, copy: true },
      { label: 'SHA-256', value: result.sha256, mono: true, copy: true },
      { label: 'SHA-512', value: result.sha512, mono: true, copy: true },
      { label: 'The same bytes as Base64', value: result.base64, mono: true, copy: true },
    ]),
  ];
}

export function base64Sections(result: Base64Result): Section[] {
  const sections: Section[] = [
    facts([
      { label: 'Base64', value: result.base64, mono: true, copy: true },
      { label: 'Bytes', value: formatCount(result.bytes), mono: true },
    ]),
  ];
  if (result.text !== null) {
    sections.push({ kind: 'code', title: 'As text', text: result.text, wrap: true });
  }
  if (result.note) sections.push({ kind: 'note', tone: 'idle', text: result.note });
  return sections;
}

// ------------------------------------------------------------------ probes

/** The one line that says where a probe ran, on every probe's answer. */
export function fromFact(from: string): Fact {
  return { label: 'Ran from', value: from };
}

export function pingSections(result: PingResult): Section[] {
  return [
    facts([
      { label: 'Target', value: result.target, mono: true },
      { label: 'Address', value: result.address, mono: true },
      lossFact(result.loss_pct, result.received, result.sent),
      ...fact('Shortest', ms(result.min_ms)),
      ...fact('Average', ms(result.avg_ms)),
      ...fact('Longest', ms(result.max_ms)),
      ...fact('Jitter (σ)', ms(result.stddev_ms)),
      { label: 'Sent as', value: METHOD_WORDS[result.method] ?? result.method },
      fromFact(result.from),
      ...(result.device ? [{ label: 'That address is', value: deviceWords(result.device) }] : []),
      ...(result.echoes_dropped
        ? fact('Echoes left out', `${formatCount(result.echoes_dropped)} older ones, to keep the newest`)
        : []),
    ]),
    table(
      [
        { id: 'seq', header: '#', align: 'right', mono: true, width: 56 },
        { id: 'rtt', header: 'Round trip', align: 'right', mono: true, width: 130 },
        { id: 'ttl', header: 'TTL', align: 'right', mono: true, width: 80 },
        { id: 'error', header: 'Why not' },
      ],
      result.echoes.map((echo) => ({
        seq: echo.seq,
        rtt: ms(echo.rtt_ms) ?? '—',
        ttl: echo.ttl,
        error: echo.error,
      })),
      'No echo was sent.',
      'Every echo',
    ),
  ];
}

/** How the operating system let the echoes be sent, in words. */
export const METHOD_WORDS: Record<string, string> = {
  icmp_datagram: 'unprivileged ICMP sockets',
  icmp_raw: 'raw ICMP sockets',
  system_command: 'the system’s own ping program',
};

export function deviceWords(device: { name: string; matched: string; lab: boolean }): string {
  return `${device.name}${device.lab ? ' (a lab device)' : ''} — matched by ${device.matched}`;
}

/**
 * The columns of a hop table, and one hop as a row.
 *
 * Exported because the live view and the finished report must be the same
 * table: a traceroute that is filling in and the same traceroute a minute
 * later should not have their columns move.
 */
export const HOP_COLUMNS: Column[] = [
  { id: 'ttl', header: '#', align: 'right', mono: true, width: 56 },
  { id: 'address', header: 'Address', mono: true, width: 160 },
  { id: 'hostname', header: 'Name' },
  { id: 'rtts', header: 'Round trips', mono: true, width: 200 },
  { id: 'note', header: 'Note' },
];

export function hopRow(hop: Hop): Row {
  return {
    ttl: hop.ttl,
    address: hop.address,
    hostname: hop.hostname,
    rtts: hop.rtts_ms.map((rtt) => ms(rtt) ?? '*').join('  '),
    note: hop.note,
  };
}

export const SWEEP_COLUMNS: Column[] = [
  { id: 'address', header: 'Address', mono: true, width: 150 },
  { id: 'via', header: 'Answered to', width: 130 },
  { id: 'rtt', header: 'Round trip', align: 'right', mono: true, width: 130 },
  { id: 'hostname', header: 'Name' },
  { id: 'mac', header: 'MAC', mono: true, width: 160 },
  { id: 'ports', header: 'Open ports', mono: true, width: 120 },
  { id: 'device', header: 'Device' },
];

export function sweptRow(host: SweptHost): Row {
  return {
    address: host.address,
    via: host.via,
    rtt: ms(host.rtt_ms),
    hostname: host.hostname,
    mac: host.mac,
    ports: host.open_ports.join(', '),
    device: host.device ? `${host.device.name}${host.device.lab ? ' (lab)' : ''}` : null,
  };
}

export function traceSections(result: TraceResult): Section[] {
  return [
    facts([
      { label: 'Target', value: `${result.target} (${result.address})`, mono: true },
      {
        label: 'Reached',
        value: result.reached ? 'Yes' : 'No — the target itself never answered',
        tone: result.reached ? 'ok' : 'warn',
      },
      { label: 'Hops found', value: String(result.hops.length), mono: true },
      { label: 'Sent as', value: result.method },
      fromFact(result.from),
    ]),
    table(HOP_COLUMNS, result.hops.map(hopRow), 'No hop answered.', 'The path'),
    ...(result.hop_devices.length > 0
      ? [
          table(
            [
              { id: 'ttl', header: '#', align: 'right' as const, mono: true, width: 56 },
              { id: 'name', header: 'Device' },
              { id: 'address', header: 'Address', mono: true },
              { id: 'site', header: 'Site' },
              { id: 'role', header: 'Role' },
            ],
            result.hop_devices.map((hop) => ({
              ttl: hop.ttl,
              name: `${hop.name}${hop.lab ? ' (lab)' : ''}`,
              address: hop.address,
              site: hop.site,
              role: hop.role,
            })),
            'None of the hops is a device Nocturne manages.',
            'Hops Nocturne knows',
          ),
        ]
      : []),
  ];
}

export function portSections(result: PortsResult): Section[] {
  return [
    facts([
      { label: 'Host', value: result.host, mono: true },
      {
        label: 'Open',
        value:
          result.open.length === 0
            ? `none of the ${result.scanned} ports tried`
            : `${result.open.length} of ${result.scanned}: ${result.open.join(', ')}`,
        tone: result.open.length === 0 ? 'idle' : 'ok',
        mono: result.open.length > 0,
      },
      fromFact(result.from),
    ]),
    table(
      [
        { id: 'port', header: 'Port', align: 'right', mono: true, width: 88 },
        { id: 'state', header: 'State', width: 90 },
        { id: 'service', header: 'Usually', width: 140 },
        { id: 'rtt', header: 'Answered in', align: 'right', mono: true, width: 135 },
        { id: 'banner', header: 'Banner', mono: true },
      ],
      result.ports.map((port) => ({
        port: port.port,
        state: port.state,
        service: port.service,
        rtt: ms(port.rtt_ms),
        banner: port.banner,
      })),
      'No port was tried.',
      'Every port tried',
      'A service name says what usually listens on that number. It says nothing about this host.',
    ),
  ];
}

export function sweepSections(result: SweepResult): Section[] {
  return [
    facts([
      { label: 'Targets', value: result.targets, mono: true },
      { label: 'Addresses probed', value: formatCount(result.planned), mono: true },
      {
        label: 'Answered',
        value: formatCount(result.alive),
        tone: result.alive > 0 ? 'ok' : 'idle',
        mono: true,
      },
      fromFact(result.from),
    ]),
    table(
      SWEEP_COLUMNS,
      result.hosts.map(sweptRow),
      'Nothing in that range answered.',
      'What answered',
    ),
  ];
}

export function dnsSections(result: DnsResult): Section[] {
  return [
    facts([
      { label: 'Asked', value: `${result.kind} ${result.query}`, mono: true },
      { label: 'Server', value: result.server, mono: true },
      {
        label: 'Response code',
        value: result.rcode,
        tone: result.rcode === 'NOERROR' ? 'ok' : 'warn',
      },
      { label: 'Took', value: ms(result.elapsed_ms) ?? '—' },
      fromFact(result.from),
    ]),
    table(
      [
        { id: 'name', header: 'Name', mono: true, width: 220 },
        { id: 'kind', header: 'Type', width: 80 },
        { id: 'ttl', header: 'TTL', align: 'right', mono: true, width: 80 },
        { id: 'data', header: 'Answer', mono: true },
      ],
      result.records.map((record) => ({ ...record })),
      result.rcode === 'NOERROR'
        ? 'The server answered, with no record of that type.'
        : `The server answered ${result.rcode}: no record.`,
      'Records',
    ),
    ...(result.devices.length > 0
      ? [
          table(
            [
              { id: 'name', header: 'Device' },
              { id: 'address', header: 'Address', mono: true },
              { id: 'site', header: 'Site' },
            ],
            result.devices.map((device) => ({
              name: `${device.name}${device.lab ? ' (lab)' : ''}`,
              address: device.address,
              site: device.site,
            })),
            '',
            'Addresses Nocturne knows',
          ),
        ]
      : []),
  ];
}

export function whoisSections(result: WhoisResult): Section[] {
  return [
    facts([
      { label: 'Query', value: result.query, mono: true },
      { label: 'Servers asked', value: result.servers.join(' → '), mono: true },
    ]),
    facts(
      result.summary.map(([field, value]) => ({ label: field, value })),
      'What it says',
    ),
    { kind: 'code', title: 'The answer, unchanged', text: result.text, wrap: true },
  ];
}

export function certFacts(cert: CertInfo): Fact[] {
  return [
    { label: 'Subject', value: cert.subject },
    { label: 'Common name', value: cert.common_name, mono: true },
    { label: 'Issuer', value: cert.issuer },
    ...fact('Names it is for', cert.sans.join(', '), { mono: true }),
    { label: 'Valid from', value: cert.not_before },
    { label: 'Valid to', value: cert.not_after },
    {
      label: 'Days left',
      value: String(cert.days_left),
      tone: cert.days_left < 0 ? 'crit' : cert.days_left < 30 ? 'warn' : 'ok',
    },
    { label: 'Public key', value: cert.public_key },
    { label: 'Signature', value: cert.signature_algorithm },
    { label: 'Serial', value: cert.serial, mono: true },
    { label: 'SHA-256', value: cert.sha256_fingerprint, mono: true, copy: true },
    {
      label: 'What it is',
      value: [cert.is_ca ? 'a CA certificate' : 'an end-entity certificate', cert.self_signed ? 'self-signed' : null]
        .filter(Boolean)
        .join(', '),
    },
  ];
}

export function tlsSections(result: TlsResult): Section[] {
  return [
    facts([
      { label: 'Host', value: `${result.host} (${result.address}:${result.port})`, mono: true },
      { label: 'Protocol', value: `${result.protocol}, ${result.cipher}` },
      ...fact('ALPN', result.alpn),
      {
        label: 'Trusted',
        value: result.trusted ? 'Yes' : `No — ${result.trust_error ?? 'the chain did not verify'}`,
        tone: result.trusted ? 'ok' : 'crit',
      },
      {
        label: 'Name matches',
        value: result.hostname_matches ? 'Yes' : 'No — the certificate is for another name',
        tone: result.hostname_matches ? 'ok' : 'crit',
      },
      {
        label: 'Expires in',
        value: `${result.expires_in_days} days`,
        tone: result.expires_in_days < 0 ? 'crit' : result.expires_in_days < 30 ? 'warn' : 'ok',
      },
      { label: 'Handshake', value: ms(result.handshake_ms) ?? '—' },
      fromFact(result.from),
    ]),
    ...result.chain.map((cert, index) =>
      facts(certFacts(cert), index === 0 ? 'The certificate' : `Issuer ${index}`),
    ),
  ];
}

export function httpSections(result: HttpResult): Section[] {
  const sections: Section[] = [
    facts([
      {
        label: 'Status',
        value: `${result.status} ${result.reason}`,
        tone: result.status < 400 ? 'ok' : result.status < 500 ? 'warn' : 'crit',
      },
      { label: 'Version', value: result.version },
      { label: 'URL', value: result.url, mono: true },
      ...(result.final_url === result.url ? [] : fact('Ended at', result.final_url, { mono: true })),
      { label: 'Body', value: `${formatCount(result.body_bytes)} bytes`, mono: true },
      fromFact(result.from),
    ]),
    facts(
      [
        { label: 'DNS', value: ms(result.dns_ms) ?? '—' },
        { label: 'Connect', value: ms(result.connect_ms) ?? '—' },
        ...fact('TLS handshake', ms(result.tls_ms)),
        { label: 'First byte', value: ms(result.first_byte_ms) ?? '—' },
        { label: 'In all', value: ms(result.total_ms) ?? '—' },
      ],
      'Where the time went',
    ),
  ];
  if (result.redirects.length > 0) {
    sections.push(
      table(
        [
          { id: 'status', header: 'Status', align: 'right', mono: true, width: 95 },
          { id: 'location', header: 'Went to', mono: true },
        ],
        result.redirects.map(([status, location]) => ({ status, location })),
        '',
        'Redirects followed',
      ),
    );
  }
  sections.push(
    table(
      [
        { id: 'name', header: 'Header', mono: true },
        { id: 'value', header: 'Value', mono: true },
      ],
      result.headers.map(([name, value]) => ({ name, value })),
      'The response carried no headers.',
      'Response headers',
    ),
  );
  if (result.body_preview) {
    sections.push({ kind: 'code', title: 'The start of the body', text: result.body_preview, wrap: true });
  }
  if (result.tls) {
    sections.push(
      facts(
        [
          { label: 'Protocol', value: `${result.tls.protocol}, ${result.tls.cipher}` },
          {
            label: 'Trusted',
            value: result.tls.trusted ? 'Yes' : `No — ${result.tls.trust_error ?? 'did not verify'}`,
            tone: result.tls.trusted ? 'ok' : 'crit',
          },
          { label: 'Expires in', value: `${result.tls.expires_in_days} days` },
        ],
        'The certificate it was served with',
      ),
    );
  }
  return sections;
}

export function ntpSections(result: NtpResult): Section[] {
  return [
    facts([
      { label: 'Server', value: `${result.server} (${result.address})`, mono: true },
      { label: 'Stratum', value: String(result.stratum), mono: true },
      { label: 'Reference', value: result.reference, mono: true },
      {
        label: 'Leap',
        value: result.leap,
        tone: result.leap === 'none' ? 'ok' : result.leap === 'not_synchronised' ? 'crit' : 'warn',
      },
      { label: 'Version', value: String(result.version) },
      {
        label: 'Offset from this clock',
        value: ms(result.offset_ms) ?? '—',
        tone: Math.abs(result.offset_ms) > 1000 ? 'warn' : 'ok',
      },
      { label: 'Delay', value: ms(result.delay_ms) ?? '—' },
      { label: 'Root delay', value: ms(result.root_delay_ms) ?? '—' },
      { label: 'Root dispersion', value: ms(result.root_dispersion_ms) ?? '—' },
      fromFact(result.from),
    ]),
  ];
}

export function localSections(report: LocalReport): Section[] {
  return [
    facts([fromFact(report.from)]),
    table(
      [
        { id: 'name', header: 'Interface', mono: true },
        { id: 'addresses', header: 'Addresses', mono: true },
        { id: 'mac', header: 'MAC', mono: true },
        { id: 'state', header: 'State' },
      ],
      report.interfaces.map((entry) => ({
        name: entry.name,
        addresses: entry.addresses.join(', '),
        mac: entry.mac,
        state: [entry.is_up ? 'up' : 'down', entry.is_loopback ? 'loopback' : null]
          .filter(Boolean)
          .join(', '),
      })),
      'This machine reports no interfaces.',
      'Interfaces',
    ),
    table(
      [
        { id: 'destination', header: 'Destination', mono: true },
        { id: 'gateway', header: 'Gateway', mono: true },
        { id: 'interface', header: 'Through', mono: true },
      ],
      report.gateways.map((route) => ({ ...route })),
      'No default gateway.',
      'Default routes',
    ),
    table(
      [
        { id: 'address', header: 'Address', mono: true },
        { id: 'mac', header: 'MAC', mono: true },
        { id: 'vendor', header: 'Vendor' },
        { id: 'interface', header: 'On', mono: true },
        { id: 'state', header: 'State' },
        { id: 'device', header: 'Device' },
      ],
      report.neighbours.map((neighbour) => ({
        address: neighbour.address,
        mac: neighbour.mac,
        vendor: neighbour.vendor ?? null,
        interface: neighbour.interface,
        state: neighbour.state,
        device: neighbour.device
          ? `${neighbour.device.name}${neighbour.device.lab ? ' (lab)' : ''}`
          : null,
      })),
      'The neighbour table is empty.',
      'Neighbours',
    ),
  ];
}

export function wolSections(result: WolResult): Section[] {
  return [
    facts([
      { label: 'MAC', value: result.mac, mono: true },
      { label: 'Sent to', value: result.sent_to.join(', ') || 'nowhere: this machine has no broadcast address', mono: true },
      fromFact(result.from),
    ]),
    {
      kind: 'note',
      tone: 'idle',
      text: 'A magic packet is one-way: nothing answers it. Whether the machine woke up is for a ping to say.',
    },
  ];
}

export function deviceProbeSections(result: DeviceProbeResult): Section[] {
  const summary = result.summary;
  return [
    facts([
      { label: 'Ran on', value: result.from },
      { label: 'Platform family', value: result.family },
      { label: 'Command', value: result.command, mono: true, copy: true },
      ...(summary
        ? [
            lossFact(summary.loss_pct, summary.received, summary.sent),
            ...fact('Shortest', ms(summary.min_ms)),
            ...fact('Average', ms(summary.avg_ms)),
            ...fact('Longest', ms(summary.max_ms)),
          ]
        : []),
    ]),
    ...(summary
      ? []
      : [
          {
            kind: 'note' as const,
            tone: 'idle' as FactTone,
            text: 'This output is not in a format Nocturne reads, so there are no numbers for it — only the device’s own words below.',
          },
        ]),
    ...(result.error
      ? [{ kind: 'note' as const, tone: 'crit' as FactTone, text: `The device refused: ${result.error}` }]
      : []),
    { kind: 'code', title: `What ${result.device_name} printed`, text: result.output },
  ];
}

// ----------------------------------------------------------------- history

export function historyRows(history: History): Row[] {
  return history.runs.map((run) => ({
    id: run.id,
    kind: run.kind,
    title: run.title,
    from: run.from,
    started_at: run.started_at,
    summary: run.summary,
    ok: run.ok,
    error_code: run.error_code,
  }));
}

// ----------------------------------------------------------- the dispatcher

/**
 * The sections of one tool's answer.
 *
 * The cast on each branch is the one place where an `unknown` from the RPC
 * client becomes a typed result: the method decides the shape, and
 * `api/types/toolbox.ts` is where that correspondence is written down.
 */
export function sectionsFor(id: string, result: unknown): Section[] {
  switch (id) {
    case 'tools.subnet':
      return subnetSections(result as SubnetInfo);
    case 'tools.subnet.split':
      return splitSections(result as SplitResult);
    case 'tools.vlsm':
      return vlsmSections(result as VlsmPlan);
    case 'tools.supernet':
      return supernetSections(result as SummaryResult);
    case 'tools.contains':
      return containsSections(result as ContainsResult);
    case 'tools.overlaps':
      return overlapSections(result as OverlapResult);
    case 'tools.ip.range':
      return rangeSections(result as RangeResult);
    case 'tools.ip.convert':
      return convertAddressSections(result as ConvertResult);
    case 'tools.wildcard':
      return maskSections(result as MaskInfo);
    case 'tools.wildcard.acl':
      return aclSections(result as AclResult);
    case 'tools.mac':
      return macSections(result as MacInfo);
    case 'tools.oui':
      return ouiSections(result as OuiResult);
    case 'tools.oid':
      return oidSections(result as OidResult);
    case 'tools.interface':
      return interfaceSections(result as InterfaceResult);
    case 'tools.convert':
      return conversionSections(result as Conversion);
    case 'tools.transfer_time':
      return transferSections(result as TransferEstimate);
    case 'tools.mtu':
      return mtuSections(result as MtuResult);
    case 'tools.qos.marking':
      return markingSections(result as Marking);
    case 'tools.qos.voice':
      return voiceSections(result as VoiceBandwidth);
    case 'tools.hash':
      return digestSections(result as Digests);
    case 'tools.base64':
      return base64Sections(result as Base64Result);
    case 'probe.ping':
      return pingSections(result as PingResult);
    case 'probe.traceroute':
      return traceSections(result as TraceResult);
    case 'probe.ports':
      return portSections(result as PortsResult);
    case 'probe.sweep':
      return sweepSections(result as SweepResult);
    case 'probe.dns':
      return dnsSections(result as DnsResult);
    case 'probe.whois':
      return whoisSections(result as WhoisResult);
    case 'probe.tls':
      return tlsSections(result as TlsResult);
    case 'probe.http':
      return httpSections(result as HttpResult);
    case 'probe.ntp':
      return ntpSections(result as NtpResult);
    case 'probe.local':
      return localSections(result as LocalReport);
    case 'probe.wol':
      return wolSections(result as WolResult);
    case 'probe.device.ping':
    case 'probe.device.traceroute':
      return deviceProbeSections(result as DeviceProbeResult);
    default:
      return [];
  }
}

/** The tool whose sections describe a history record of this kind. */
export const KIND_TO_TOOL: Record<string, string> = {
  ping: 'probe.ping',
  traceroute: 'probe.traceroute',
  ports: 'probe.ports',
  sweep: 'probe.sweep',
  dns: 'probe.dns',
  whois: 'probe.whois',
  tls: 'probe.tls',
  http: 'probe.http',
  ntp: 'probe.ntp',
  wol: 'probe.wol',
  'device.ping': 'probe.device.ping',
  'device.traceroute': 'probe.device.traceroute',
};
