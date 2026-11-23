/**
 * What each tool asks for, and how the form becomes the call's parameters.
 *
 * One table, no React: a tool is a list of fields and a pure function from
 * what is in them to the parameters of its method. That is the whole screen's
 * input half, which means it can be unit-tested rather than clicked through,
 * and that the parameters are read from the backend's own structs in one place
 * (`crates/nocturne-toolbox/src`, mirrored in `api/types/toolbox.ts`) instead
 * of being spelled out again in every panel.
 *
 * Three rules hold throughout:
 *
 * 1. **A field that is empty is not sent.** The backend has defaults for every
 *    optional parameter and clamps them as it reads them, so an empty "count"
 *    must mean "whatever the settings say", not `0` — which for a ping means
 *    "until cancelled". Sending an empty string instead of omitting the field
 *    is how a UI quietly overrides a user's settings.
 * 2. **`params` returns `null` when there is not enough to ask.** A calculator
 *    answers as you type, so the first keystroke must not produce a refusal
 *    from the engine: nothing is called until the required fields are filled.
 *    Once they are, a *wrong* value is the engine's to refuse, with its own
 *    message, and the screen shows that instead of guessing.
 * 3. **A probe aims at one thing.** `probe.*` refuses `host` and `device`
 *    together ("give either `host` or `device`, not both"), so the form has an
 *    explicit choice and sends exactly one.
 * 4. **Nothing is chosen for the reader where the engine refuses to choose.**
 *    `tools.qos.marking` has no default for what a number is and
 *    `tools.qos.voice` none for the number of calls, because a guess there is
 *    a wrong answer that looks like a right one. Their forms start empty at
 *    exactly those fields, and say why.
 */

import { formatCount } from '@/lib/format';
import type { Field, FieldOption, Values } from './field-kinds';

export type { Field, FieldOption, FieldSource, Values } from './field-kinds';

/** A row of a reference table, as `tools.reference` returns it. */
type TableRow = Record<string, string | number | boolean | null>;

/**
 * One way of recognising which field a refusal is about.
 *
 * The engine's refusals carry no field name — `details` is only `{reason}` —
 * but its sentences open with what the input was supposed to be, in a small
 * fixed vocabulary of its own: ``invalid link type `ethernet`: …``. That
 * opening phrase is what `when` matches, never the reason that follows it, so
 * a reason can be reworded without the sentence moving to another field.
 *
 * The refusals for an input that is not there at all are worded the other
 * way round — ``give the number of simultaneous calls in `calls` `` — and name
 * the parameter itself, which is the wire's name and so the steadiest thing
 * in the sentence. A form here does not ask until those inputs are filled, so
 * they are matched for the day one does.
 */
export interface Blame {
  when: RegExp;
  /** The name of the field the sentence belongs beside. */
  field: string;
}

export interface Spec {
  fields: readonly Field[];
  /** What the form starts with, and what "Clear" restores. */
  initial?: Values;
  /** The call's parameters, or `null` while the form is not answerable yet. */
  params: (values: Values) => Record<string, unknown> | null;
  /**
   * The inputs of this tool are a secret somebody typed, so they are never
   * written to `localStorage` — not even to be helpful next time.
   */
  secret?: boolean;
  /** The label of the button, for the tools that are asked rather than watched. */
  action?: string;
  /**
   * Which field each of the engine's refusals is about, so its sentence can be
   * shown beside that field. A refusal that matches none of these is shown
   * whole where the answer would have been, as for every other tool.
   */
  blame?: readonly Blame[];
}

// ------------------------------------------------------------------ reading

const str = (values: Values, name: string): string => (values[name] ?? '').trim();
const on = (values: Values, name: string): boolean => (values[name] ?? '') === 'on';

/** A finite number, or `undefined` for a field left empty or unreadable. */
function num(values: Values, name: string): number | undefined {
  const text = str(values, name);
  if (!text) return undefined;
  const value = Number(text);
  return Number.isFinite(value) ? value : undefined;
}

/** Splits a list the way a network engineer writes one: commas, or lines. */
export function listOf(text: string): string[] {
  return text
    .split(/[\n,;]/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

/** Drops the keys whose value is `undefined`, so they are not sent at all. */
function only(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(params).filter(([, value]) => value !== undefined));
}

// ----------------------------------------------------------------- the form

const INPUT: Field = { name: 'input', label: 'Input', kind: 'mono', span: 2 };

/** `probe.*` aims at one of the two, never both. */
const AIM: readonly Field[] = [
  {
    name: 'aim',
    label: 'Aim at',
    kind: 'select',
    options: [
      { value: 'host', label: 'An address or host name' },
      { value: 'device', label: 'A device in the inventory' },
    ],
  },
  {
    name: 'host',
    label: 'Host',
    kind: 'mono',
    placeholder: '10.0.0.1',
    when: { field: 'aim', is: 'host' },
  },
  {
    name: 'device',
    label: 'Device',
    kind: 'device',
    hint: 'Its address in the inventory is what gets probed.',
    when: { field: 'aim', is: 'device' },
  },
];

/** What the aim fields mean for the call. `null` until something is aimed at. */
function aimed(values: Values): Record<string, unknown> | null {
  if (str(values, 'aim') === 'device') {
    const device = str(values, 'device');
    return device ? { device } : null;
  }
  const host = str(values, 'host');
  return host ? { host } : null;
}

const DNS_KINDS = ['A', 'AAAA', 'CNAME', 'MX', 'NS', 'TXT', 'SOA', 'SRV', 'PTR', 'CAA', 'ANY'];

/**
 * The lines of a VLSM request: `Staff 200`, `200 Staff`, or just `200`.
 *
 * Exported because it is the one piece of parsing on this screen that can be
 * wrong in an interesting way, and it has its own tests.
 */
export function parseSizes(text: string): { name?: string; hosts: number }[] {
  const sizes: { name?: string; hosts: number }[] = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    // The number is whichever end of the line is a number; the rest is the name.
    const parts = trimmed.split(/[\s,:=]+/).filter(Boolean);
    const numberAt = parts.findIndex((part) => /^\d+$/.test(part));
    if (numberAt === -1) continue;
    const hosts = Number(parts[numberAt]);
    const name = parts.filter((_part, index) => index !== numberAt).join(' ');
    sizes.push(name ? { name, hosts } : { hosts });
  }
  return sizes;
}

/** `Name: value` per line, as an HTTP request's headers. */
export function parseHeaders(text: string): [string, string][] {
  const headers: [string, string][] = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const at = trimmed.indexOf(':');
    if (at <= 0) continue;
    headers.push([trimmed.slice(0, at).trim(), trimmed.slice(at + 1).trim()]);
  }
  return headers;
}

// ---------------------------------------------------------------------- qos

const word = (row: TableRow, name: string): string => {
  const value = row[name];
  return typeof value === 'string' ? value.trim() : '';
};

const figure = (row: TableRow, name: string): number | undefined => {
  const value = row[name];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
};

/**
 * A row of the engine's `voice_codecs` table as a choice.
 *
 * The hint is the row's own figures, laid out: nothing here is worked out, and
 * a figure the row does not carry is left out of the sentence rather than
 * written as nothing. A row without an id cannot be asked for, so it is not
 * offered.
 */
export function codecOption(row: TableRow): FieldOption | null {
  const id = word(row, 'id');
  if (!id) return null;
  const rate = figure(row, 'bit_rate_bps');
  const sampleBytes = figure(row, 'sample_bytes');
  const sampleMs = figure(row, 'sample_ms');
  const packetMs = figure(row, 'packet_ms');
  const payload = figure(row, 'payload_bytes');
  const mos = figure(row, 'mos');
  const said = [
    rate === undefined ? '' : `${formatCount(rate)} bps`,
    sampleBytes === undefined || sampleMs === undefined ? '' : `${sampleBytes} bytes every ${sampleMs} ms`,
    packetMs === undefined
      ? ''
      : `${packetMs} ms${payload === undefined ? '' : ` (${payload} bytes)`} per packet by default`,
    mos === undefined ? '' : `MOS ${mos}`,
  ].filter(Boolean);
  return { value: id, label: word(row, 'name') || id, hint: said.length > 0 ? `${said.join(' · ')}.` : undefined };
}

/** A row of the engine's `voice_links` table as a choice. */
export function linkOption(row: TableRow): FieldOption | null {
  const id = word(row, 'id');
  if (!id) return null;
  const bytes = figure(row, 'overhead_bytes');
  const parts = word(row, 'parts');
  const source = word(row, 'source');
  const said = [
    bytes === undefined ? '' : `${bytes} ${bytes === 1 ? 'byte' : 'bytes'} on every packet${parts ? `: ${parts}` : ''}`,
    source ? `Figure from ${source}` : '',
    // Said before the switch is tried, in the table's own terms; the engine
    // still refuses it, and its sentence is the one that explains why.
    row['crtp'] === false ? 'Compressed RTP is not counted on it' : '',
  ].filter(Boolean);
  const name = word(row, 'name') || id;
  return {
    value: id,
    label: bytes === undefined ? name : `${name} — ${bytes} ${bytes === 1 ? 'byte' : 'bytes'}`,
    hint: said.length > 0 ? `${said.join('. ')}.` : undefined,
  };
}

/** The four things a marking can be given as: the engine's `as`. */
const MARKING_FORMS: readonly FieldOption[] = [
  { value: 'dscp', label: 'A DSCP', hint: '0 to 63, or a name such as ef, af41 or cs3.' },
  {
    value: 'tos',
    label: 'The whole ToS byte',
    hint: '0 to 255. Its bottom two bits come back as the ECN field.',
  },
  {
    value: 'precedence',
    label: 'An IP precedence',
    hint: '0 to 7, or its name. Answered as the class selector for it.',
  },
  {
    value: 'name',
    label: 'A code point name, and nothing else',
    hint: 'EF, AF41, CS3, DF… A number is refused, not read as one.',
  },
];

// ---------------------------------------------------------------- the table

export const SPECS: Record<string, Spec> = {
  // ----------------------------------------------------------- addressing
  'tools.subnet': {
    fields: [{ ...INPUT, label: 'Address, prefix or mask', placeholder: '10.1.2.33/26' }],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },
  'tools.subnet.split': {
    fields: [
      { name: 'cidr', label: 'Network', kind: 'mono', placeholder: '10.0.0.0/22', span: 2 },
      {
        name: 'by',
        label: 'Split by',
        kind: 'select',
        options: [
          { value: 'new_len', label: 'New prefix length' },
          { value: 'count', label: 'How many subnets' },
        ],
      },
      { name: 'value', label: 'Value', kind: 'number', placeholder: '26' },
    ],
    initial: { by: 'new_len' },
    params: (values) => {
      const cidr = str(values, 'cidr');
      const value = num(values, 'value');
      if (!cidr || value === undefined) return null;
      return str(values, 'by') === 'count' ? { cidr, count: value } : { cidr, new_len: value };
    },
  },
  'tools.vlsm': {
    fields: [
      { name: 'base', label: 'Base network', kind: 'mono', placeholder: '10.0.0.0/22', span: 2 },
      {
        name: 'sizes',
        label: 'What has to fit',
        kind: 'textarea',
        rows: 5,
        span: 2,
        placeholder: 'Staff 200\nVoice 50\nUplink 2',
        hint: 'One per line: a name and the number of hosts it needs.',
      },
      {
        name: 'p2p31',
        label: 'Give a two-host network a /31 (RFC 3021)',
        kind: 'switch',
        span: 2,
      },
    ],
    params: (values) => {
      const base = str(values, 'base');
      const sizes = parseSizes(values['sizes'] ?? '');
      if (!base || sizes.length === 0) return null;
      return only({ base, sizes, p2p31: on(values, 'p2p31') || undefined });
    },
  },
  'tools.supernet': {
    fields: [
      {
        name: 'cidrs',
        label: 'Networks',
        kind: 'textarea',
        rows: 6,
        span: 2,
        placeholder: '10.0.0.0/24\n10.0.1.0/24',
        hint: 'One per line, or separated by commas.',
      },
    ],
    params: (values) => {
      const cidrs = listOf(values['cidrs'] ?? '');
      return cidrs.length > 0 ? { cidrs } : null;
    },
  },
  'tools.contains': {
    fields: [
      { name: 'a', label: 'Network', kind: 'mono', placeholder: '10.0.0.0/8' },
      { name: 'b', label: 'Address or network', kind: 'mono', placeholder: '10.1.2.3' },
    ],
    params: (values) =>
      str(values, 'a') && str(values, 'b') ? { a: str(values, 'a'), b: str(values, 'b') } : null,
  },
  'tools.overlaps': {
    fields: [
      {
        name: 'cidrs',
        label: 'Networks',
        kind: 'textarea',
        rows: 6,
        span: 2,
        placeholder: '10.0.0.0/23\n10.0.1.0/24\n192.168.1.0/24',
        hint: 'One per line. The first thing to check in an addressing plan.',
      },
    ],
    params: (values) => {
      const cidrs = listOf(values['cidrs'] ?? '');
      return cidrs.length > 1 ? { cidrs } : null;
    },
  },
  'tools.ip.range': {
    fields: [
      { name: 'first', label: 'First address', kind: 'mono', placeholder: '10.0.0.5' },
      { name: 'last', label: 'Last address', kind: 'mono', placeholder: '10.0.0.70' },
    ],
    params: (values) =>
      str(values, 'first') && str(values, 'last')
        ? { first: str(values, 'first'), last: str(values, 'last') }
        : null,
  },
  'tools.ip.convert': {
    fields: [
      {
        ...INPUT,
        label: 'Address or number',
        placeholder: '3232235777, 0xC0A80101, 2001:db8::1',
      },
    ],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },
  'tools.wildcard': {
    fields: [
      {
        ...INPUT,
        label: 'Prefix length, netmask or wildcard',
        placeholder: '255.255.255.192, /26, 0.0.0.63',
      },
    ],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },
  'tools.wildcard.acl': {
    fields: [
      { name: 'address', label: 'Address', kind: 'mono', placeholder: '10.1.1.0' },
      { name: 'wildcard', label: 'Wildcard', kind: 'mono', placeholder: '0.0.0.255' },
      {
        name: 'candidate',
        label: 'Does it match this address?',
        kind: 'mono',
        placeholder: '10.1.1.9',
        span: 2,
      },
    ],
    params: (values) => {
      const address = str(values, 'address');
      const wildcard = str(values, 'wildcard');
      if (!address || !wildcard) return null;
      return only({ address, wildcard, candidate: str(values, 'candidate') || undefined });
    },
  },

  // ------------------------------------------------------------- identity
  'tools.mac': {
    fields: [{ ...INPUT, label: 'MAC address', placeholder: '0050.56ab.cdef' }],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },
  'tools.oui': {
    fields: [
      {
        name: 'query',
        label: 'Prefix or vendor',
        kind: 'mono',
        span: 2,
        placeholder: '00:1b:21, or Cisco',
        hint: 'A MAC address or OUI prefix looks up the vendor; a name looks up its blocks.',
      },
    ],
    params: (values) => (str(values, 'query') ? { query: str(values, 'query'), limit: 200 } : null),
  },
  'tools.oid': {
    fields: [{ ...INPUT, label: 'OID or object name', placeholder: 'ifInOctets.3' }],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },
  'tools.interface': {
    fields: [{ ...INPUT, label: 'Interface name', placeholder: 'Gi1/0/1' }],
    params: (values) => (str(values, 'input') ? { input: str(values, 'input') } : null),
  },

  // -------------------------------------------------------------- secrets
  'tools.secret.identify': {
    secret: true,
    fields: [
      {
        name: 'text',
        label: 'The line from the configuration',
        kind: 'textarea',
        rows: 3,
        span: 2,
        placeholder: 'enable secret 5 $1$mERr$3GCGQMcGNJZPAJLtgvmgO0',
        hint: 'Nothing is stored and nothing is logged.',
      },
    ],
    params: (values) => (str(values, 'text') ? { text: str(values, 'text') } : null),
  },
  'tools.secret.decode': {
    secret: true,
    fields: [
      {
        name: 'text',
        label: 'The value, or the line it sits in',
        kind: 'textarea',
        rows: 3,
        span: 2,
        placeholder: 'username admin password 7 0822455D0A16',
      },
      {
        name: 'kind',
        label: 'Encoding',
        kind: 'select',
        options: [
          { value: '', label: 'Recognise it' },
          { value: 'cisco7', label: 'Cisco type 7' },
          { value: 'junos9', label: 'Junos $9$' },
        ],
      },
    ],
    params: (values) => {
      const text = str(values, 'text');
      if (!text) return null;
      return only({ text, kind: str(values, 'kind') || undefined });
    },
  },
  'tools.secret.encode': {
    secret: true,
    action: 'Encode',
    fields: [
      {
        name: 'kind',
        label: 'Encoding',
        kind: 'select',
        options: [
          { value: 'cisco7', label: 'Cisco type 7' },
          { value: 'junos9', label: 'Junos $9$' },
        ],
      },
      {
        name: 'salt',
        label: 'Salt (0–15)',
        kind: 'number',
        when: { field: 'kind', is: 'cisco7' },
        hint: 'Left empty, a random one is used.',
      },
      {
        name: 'text',
        label: 'The text to obfuscate',
        kind: 'text',
        span: 2,
        hint: 'For comparing with a configuration — this protects nothing.',
      },
    ],
    initial: { kind: 'cisco7' },
    params: (values) => {
      const text = values['text'] ?? '';
      if (!text) return null;
      return only({ kind: str(values, 'kind') || 'cisco7', text, salt: num(values, 'salt') });
    },
  },
  'tools.password': {
    secret: true,
    action: 'Generate',
    fields: [
      { name: 'length', label: 'Length', kind: 'number' },
      { name: 'count', label: 'How many', kind: 'number' },
      { name: 'upper', label: 'Capitals', kind: 'switch' },
      { name: 'lower', label: 'Lower case', kind: 'switch' },
      { name: 'digits', label: 'Digits', kind: 'switch' },
      { name: 'symbols', label: 'Symbols', kind: 'switch' },
      {
        name: 'avoid_ambiguous',
        label: 'Leave out characters that look alike (0 O o 1 l I . ,)',
        kind: 'switch',
        span: 2,
      },
    ],
    initial: {
      length: '20',
      count: '5',
      upper: 'on',
      lower: 'on',
      digits: 'on',
      symbols: 'on',
    },
    params: (values) =>
      only({
        length: num(values, 'length'),
        count: num(values, 'count'),
        upper: on(values, 'upper'),
        lower: on(values, 'lower'),
        digits: on(values, 'digits'),
        symbols: on(values, 'symbols'),
        avoid_ambiguous: on(values, 'avoid_ambiguous'),
      }),
  },
  'tools.keys.generate': {
    secret: true,
    action: 'Generate a key pair',
    fields: [
      {
        name: 'kind',
        label: 'Type',
        kind: 'select',
        options: [
          { value: 'ed25519', label: 'Ed25519 (recommended)' },
          { value: 'rsa3072', label: 'RSA 3072' },
          { value: 'rsa4096', label: 'RSA 4096' },
        ],
      },
      { name: 'comment', label: 'Comment', kind: 'text', placeholder: 'noc@example.net' },
    ],
    initial: { kind: 'ed25519' },
    params: (values) => only({ kind: str(values, 'kind') || 'ed25519', comment: str(values, 'comment') }),
  },

  // ----------------------------------------------------------- converters
  'tools.convert': {
    fields: [
      { name: 'value', label: 'Amount', kind: 'mono', placeholder: '700 MB, 1.25Gbps, -7 dBm' },
      { name: 'to', label: 'Into', kind: 'mono', placeholder: 'MiB', hint: 'Left empty, every unit of the family.' },
      {
        name: 'from',
        label: 'Unit of the amount',
        kind: 'mono',
        span: 2,
        hint: 'Only needed when the amount is a bare number.',
      },
    ],
    params: (values) => {
      const value = str(values, 'value');
      if (!value) return null;
      return only({ value, from: str(values, 'from') || undefined, to: str(values, 'to') || undefined });
    },
  },
  'tools.transfer_time': {
    fields: [
      { name: 'size', label: 'Size', kind: 'mono', placeholder: '4.7 GB' },
      { name: 'rate', label: 'Link rate', kind: 'mono', placeholder: '100 Mbps' },
      {
        name: 'overhead_pct',
        label: 'Protocol overhead (%)',
        kind: 'number',
        span: 2,
        hint: 'What the link loses to framing and the protocol. 0 to just under 100.',
      },
    ],
    params: (values) => {
      const size = str(values, 'size');
      const rate = str(values, 'rate');
      if (!size || !rate) return null;
      return only({ size, rate, overhead_pct: num(values, 'overhead_pct') });
    },
  },
  'tools.mtu': {
    fields: [
      { name: 'link_mtu', label: 'Link MTU', kind: 'number', placeholder: '1500' },
      {
        name: 'encapsulations',
        label: 'Encapsulations, outermost first',
        kind: 'mono',
        placeholder: 'gre, ipsec-esp-tunnel',
        hint: 'The Reference tool’s “encapsulations” table lists the names.',
      },
    ],
    params: (values) => {
      const link = num(values, 'link_mtu');
      if (link === undefined) return null;
      return { link_mtu: link, encapsulations: listOf(values['encapsulations'] ?? '') };
    },
  },

  // ------------------------------------------------------------------ qos
  'tools.qos.marking': {
    fields: [
      {
        name: 'value',
        label: 'Marking',
        kind: 'mono',
        placeholder: '46, ef, af41, cs3, 0xb8',
        hint: 'One value. A number is decimal unless prefixed: 0x2e, 0b101110.',
      },
      {
        name: 'as',
        label: 'What the value is',
        kind: 'select',
        // No form is chosen for the reader: 46 is EF as a DSCP and something
        // unrelated as a ToS byte, and the engine refuses to guess as well.
        placeholder: 'Choose what it is…',
        hint: 'Never guessed: one number is a different marking as each.',
        options: MARKING_FORMS,
      },
    ],
    params: (values) => {
      const value = str(values, 'value');
      const as = str(values, 'as');
      return value && as ? { value, as } : null;
    },
    blame: [
      { when: /^invalid (DSCP|DSCP name|ToS byte|IP precedence)( `|:)/, field: 'value' },
      { when: /^invalid kind of marking\b/, field: 'as' },
      { when: /^give the marking in `value`|^`value` is one marking\b/, field: 'value' },
      { when: /^say what the value is in `as`/, field: 'as' },
    ],
  },
  'tools.qos.voice': {
    fields: [
      {
        name: 'codec',
        label: 'Codec',
        kind: 'select',
        placeholder: 'Choose a codec…',
        source: { table: 'voice_codecs', option: codecOption },
      },
      {
        name: 'calls',
        label: 'Simultaneous calls',
        kind: 'number',
        placeholder: '10',
        hint: 'No default: one call would look like the answer for sixty.',
      },
      {
        name: 'link',
        label: 'Link type',
        kind: 'select',
        placeholder: 'Choose a link type…',
        source: { table: 'voice_links', option: linkOption },
        // Not a row of the engine's table, because it has no figure of its
        // own: the figure is the one typed beside it.
        options: [
          {
            value: 'custom',
            label: 'Custom — a layer-2 size of your own',
            hint: 'For a link the table has no row for, or a figure you would rather count yourself.',
          },
        ],
      },
      {
        name: 'packet_ms',
        label: 'Voice per packet (ms)',
        kind: 'number',
        hint: 'Left empty, the codec’s own default. It has to hold whole samples.',
      },
      {
        name: 'l2_bytes',
        label: 'Layer-2 bytes on every packet',
        kind: 'number',
        span: 2,
        when: { field: 'link', is: 'custom' },
        hint: '0 to 256: everything this link puts around the IP packet.',
      },
      { name: 'crtp', label: 'Compressed RTP (cRTP) on the link', kind: 'switch' },
      { name: 'vad', label: 'Also estimate voice-activity detection (VAD)', kind: 'switch' },
    ],
    params: (values) => {
      const codec = str(values, 'codec');
      const link = str(values, 'link');
      const calls = num(values, 'calls');
      if (!codec || !link || calls === undefined) return null;
      // `l2_bytes` belongs to the custom link and to nothing else: the engine
      // refuses it beside a named link, so a figure left over from an earlier
      // custom calculation must not ride along with one.
      const custom = link === 'custom';
      const l2 = num(values, 'l2_bytes');
      if (custom && l2 === undefined) return null;
      return only({
        codec,
        calls,
        link,
        crtp: on(values, 'crtp'),
        vad: on(values, 'vad'),
        packet_ms: num(values, 'packet_ms'),
        l2_bytes: custom ? l2 : undefined,
      });
    },
    blame: [
      { when: /^invalid codec\b/, field: 'codec' },
      { when: /^invalid link type\b/, field: 'link' },
      { when: /^invalid layer-2 overhead\b/, field: 'l2_bytes' },
      { when: /^invalid voice per packet\b/, field: 'packet_ms' },
      { when: /^invalid number of calls\b|^\d+ simultaneous calls is more than\b/, field: 'calls' },
      { when: /^give the number of simultaneous calls in `calls`/, field: 'calls' },
    ],
  },

  // ------------------------------------------------------------ reference
  // The table chooser is filled from `tools.reference.tables`, so its options
  // are not written here: see `Reference.tsx`.
  'tools.reference': {
    fields: [],
    params: () => null,
  },

  // ----------------------------------------------------------------- text
  'tools.regex': {
    fields: [
      {
        name: 'pattern',
        label: 'Pattern',
        kind: 'mono',
        span: 2,
        placeholder: '^(\\S+)\\s+is\\s+(up|down)',
        hint: 'Rust regex, the same engine a device profile’s patterns run in.',
      },
      { name: 'text', label: 'Against this text', kind: 'textarea', rows: 6, span: 2 },
      { name: 'case_insensitive', label: 'Ignore case', kind: 'switch' },
      { name: 'multiline', label: '^ and $ match every line', kind: 'switch' },
    ],
    params: (values) => {
      const pattern = values['pattern'] ?? '';
      if (!pattern) return null;
      return {
        pattern,
        text: values['text'] ?? '',
        case_insensitive: on(values, 'case_insensitive'),
        multiline: on(values, 'multiline'),
      };
    },
  },
  'tools.diff': {
    fields: [
      { name: 'a', label: 'Before', kind: 'textarea', rows: 8 },
      { name: 'b', label: 'After', kind: 'textarea', rows: 8 },
      {
        name: 'context',
        label: 'Unchanged lines to keep around a change',
        kind: 'number',
        span: 2,
        hint: 'Left empty, every line is kept.',
      },
    ],
    params: (values) => {
      const a = values['a'] ?? '';
      const b = values['b'] ?? '';
      if (!a && !b) return null;
      return only({ a, b, context: num(values, 'context') });
    },
  },
  'tools.hash': {
    fields: [
      {
        name: 'mode',
        label: 'The input is',
        kind: 'select',
        options: [
          { value: 'text', label: 'Text' },
          { value: 'base64', label: 'Bytes, as Base64' },
        ],
      },
      { name: 'text', label: 'Input', kind: 'textarea', rows: 5, span: 2 },
    ],
    initial: { mode: 'text' },
    params: (values) => {
      const text = values['text'] ?? '';
      if (!text) return null;
      return str(values, 'mode') === 'base64' ? { base64: text.trim() } : { text };
    },
  },
  'tools.base64': {
    fields: [
      {
        name: 'mode',
        label: 'Direction',
        kind: 'select',
        options: [
          { value: 'encode', label: 'Text to Base64' },
          { value: 'decode', label: 'Base64 to text' },
        ],
      },
      { name: 'text', label: 'Input', kind: 'textarea', rows: 5, span: 2 },
    ],
    initial: { mode: 'encode' },
    params: (values) => {
      const text = values['text'] ?? '';
      if (!text) return null;
      return { mode: str(values, 'mode') || 'encode', text };
    },
  },

  // --------------------------------------------------------------- probes
  'probe.ping': {
    action: 'Ping',
    fields: [
      ...AIM,
      {
        name: 'count',
        label: 'Echoes',
        kind: 'number',
        hint: '0 keeps going until you stop it.',
      },
      { name: 'interval_ms', label: 'Every (ms)', kind: 'number' },
      { name: 'timeout_ms', label: 'Wait (ms)', kind: 'number' },
      { name: 'payload', label: 'Payload bytes', kind: 'number' },
    ],
    initial: { aim: 'host', count: '5' },
    params: (values) => {
      const target = aimed(values);
      if (!target) return null;
      return only({
        ...target,
        count: num(values, 'count'),
        interval_ms: num(values, 'interval_ms'),
        timeout_ms: num(values, 'timeout_ms'),
        payload: num(values, 'payload'),
      });
    },
  },
  'probe.traceroute': {
    action: 'Trace',
    fields: [
      ...AIM,
      { name: 'max_hops', label: 'Hops at most', kind: 'number' },
      { name: 'probes', label: 'Probes per hop', kind: 'number' },
      { name: 'timeout_ms', label: 'Wait (ms)', kind: 'number' },
      { name: 'resolve_names', label: 'Look up the name of each hop', kind: 'switch', span: 2 },
    ],
    initial: { aim: 'host', resolve_names: 'on' },
    params: (values) => {
      const target = aimed(values);
      if (!target) return null;
      return only({
        ...target,
        max_hops: num(values, 'max_hops'),
        probes: num(values, 'probes'),
        timeout_ms: num(values, 'timeout_ms'),
        resolve_names: on(values, 'resolve_names'),
      });
    },
  },
  'probe.ports': {
    action: 'Check ports',
    fields: [
      ...AIM,
      {
        name: 'ports',
        label: 'Ports',
        kind: 'mono',
        placeholder: '22,80,443 · 1-1024 · top100 · common',
      },
      { name: 'timeout_ms', label: 'Wait (ms)', kind: 'number' },
      { name: 'banners', label: 'Read the banner a server volunteers', kind: 'switch', span: 2 },
    ],
    initial: { aim: 'host', banners: 'on' },
    params: (values) => {
      const target = aimed(values);
      if (!target) return null;
      return only({
        ...target,
        ports: str(values, 'ports') || undefined,
        timeout_ms: num(values, 'timeout_ms'),
        banners: on(values, 'banners'),
      });
    },
  },
  'probe.sweep': {
    action: 'Sweep',
    fields: [
      {
        name: 'targets',
        label: 'Addresses',
        kind: 'mono',
        span: 2,
        placeholder: '10.0.0.0/24, 10.0.1.5-10.0.1.40',
        hint: 'Private address space, unless the settings and the request both allow more.',
      },
      {
        name: 'ports',
        label: 'TCP ports to try on a silent address',
        kind: 'mono',
        placeholder: '22,443',
      },
      { name: 'concurrency', label: 'At a time', kind: 'number' },
      { name: 'resolve_names', label: 'Look up names', kind: 'switch' },
      { name: 'allow_public', label: 'Allow public address space', kind: 'switch' },
    ],
    initial: { resolve_names: 'on' },
    params: (values) => {
      const targets = str(values, 'targets');
      if (!targets) return null;
      return only({
        targets,
        ports: str(values, 'ports') || undefined,
        concurrency: num(values, 'concurrency'),
        resolve_names: on(values, 'resolve_names'),
        allow_public: on(values, 'allow_public') || undefined,
      });
    },
  },
  'probe.dns': {
    action: 'Look up',
    fields: [
      { name: 'name', label: 'Name', kind: 'mono', placeholder: 'core-sw1.example.net' },
      {
        name: 'kind',
        label: 'Record',
        kind: 'select',
        options: DNS_KINDS.map((kind) => ({ value: kind, label: kind })),
      },
      {
        name: 'server',
        label: 'Server',
        kind: 'mono',
        span: 2,
        placeholder: '10.0.0.53',
        hint: 'Left empty, this machine’s own resolvers.',
      },
    ],
    initial: { kind: 'A' },
    params: (values) => {
      const name = str(values, 'name');
      if (!name) return null;
      return only({
        name,
        kind: str(values, 'kind') || undefined,
        server: str(values, 'server') || undefined,
      });
    },
  },
  'probe.whois': {
    action: 'Ask WHOIS',
    fields: [
      {
        name: 'query',
        label: 'Address, block, AS number or domain',
        kind: 'mono',
        span: 2,
        placeholder: 'AS13335',
      },
      { name: 'server', label: 'Server', kind: 'mono', span: 2, hint: 'Left empty, it starts at IANA.' },
    ],
    params: (values) => {
      const query = str(values, 'query');
      if (!query) return null;
      return only({ query, server: str(values, 'server') || undefined });
    },
  },
  'probe.tls': {
    action: 'Read the certificate',
    fields: [
      ...AIM,
      { name: 'port', label: 'Port', kind: 'number', placeholder: '443' },
      {
        name: 'sni',
        label: 'Server name to send',
        kind: 'mono',
        hint: 'Instead of the host, when they differ.',
      },
    ],
    initial: { aim: 'host' },
    params: (values) => {
      const target = aimed(values);
      if (!target) return null;
      return only({ ...target, port: num(values, 'port'), sni: str(values, 'sni') || undefined });
    },
  },
  'probe.http': {
    action: 'Send the request',
    fields: [
      { name: 'url', label: 'URL', kind: 'mono', span: 2, placeholder: 'https://10.0.0.1/health' },
      {
        name: 'method',
        label: 'Method',
        kind: 'select',
        options: ['GET', 'HEAD', 'POST', 'PUT', 'DELETE', 'OPTIONS'].map((method) => ({
          value: method,
          label: method,
        })),
      },
      { name: 'follow_redirects', label: 'Follow redirects', kind: 'number', hint: '0 makes the redirect itself the answer.' },
      {
        name: 'headers',
        label: 'Request headers',
        kind: 'textarea',
        rows: 3,
        span: 2,
        placeholder: 'Authorization: Bearer …',
        hint: 'Sent and then forgotten: one of them may be a token, so no part of the request is recorded.',
      },
      { name: 'verify_tls', label: 'Verify the certificate', kind: 'switch', span: 2 },
    ],
    initial: { method: 'GET', verify_tls: 'on' },
    params: (values) => {
      const url = str(values, 'url');
      if (!url) return null;
      const headers = parseHeaders(values['headers'] ?? '');
      return only({
        url,
        method: str(values, 'method') || undefined,
        follow_redirects: num(values, 'follow_redirects'),
        verify_tls: on(values, 'verify_tls'),
        headers: headers.length > 0 ? headers : undefined,
      });
    },
  },
  'probe.ntp': {
    action: 'Ask the server',
    fields: [
      {
        name: 'server',
        label: 'NTP server',
        kind: 'mono',
        span: 2,
        placeholder: '10.0.0.1',
        hint: 'The offset is measured against this machine’s own clock.',
      },
    ],
    params: (values) => (str(values, 'server') ? { server: str(values, 'server') } : null),
  },
  'probe.local': {
    action: 'Read this machine',
    fields: [],
    params: () => ({}),
  },
  'probe.wol': {
    action: 'Send the magic packet',
    fields: [
      { name: 'mac', label: 'MAC address', kind: 'mono', placeholder: '0050.56ab.cdef' },
      {
        name: 'broadcast',
        label: 'Broadcast address',
        kind: 'mono',
        hint: 'Left empty, every broadcast address of this machine.',
      },
    ],
    params: (values) => {
      const mac = str(values, 'mac');
      if (!mac) return null;
      return only({ mac, broadcast: str(values, 'broadcast') || undefined });
    },
  },

  // --------------------------------------------------------------- device
  'probe.device.ping': {
    action: 'Ping from the device',
    fields: [
      { name: 'device_id', label: 'Device', kind: 'device', span: 2 },
      { name: 'target', label: 'Target', kind: 'mono', span: 2, placeholder: '10.0.0.1' },
      { name: 'count', label: 'Echoes', kind: 'number' },
      { name: 'source', label: 'Source interface or address', kind: 'mono' },
      { name: 'vrf', label: 'VRF or routing instance', kind: 'mono', span: 2 },
    ],
    params: (values) => {
      const device = str(values, 'device_id');
      const target = str(values, 'target');
      if (!device || !target) return null;
      return only({
        device_id: device,
        target,
        count: num(values, 'count'),
        source: str(values, 'source') || undefined,
        vrf: str(values, 'vrf') || undefined,
      });
    },
  },
  'probe.device.traceroute': {
    action: 'Trace from the device',
    fields: [
      { name: 'device_id', label: 'Device', kind: 'device', span: 2 },
      { name: 'target', label: 'Target', kind: 'mono', span: 2, placeholder: '10.0.0.1' },
      { name: 'source', label: 'Source interface or address', kind: 'mono' },
      { name: 'vrf', label: 'VRF or routing instance', kind: 'mono' },
    ],
    params: (values) => {
      const device = str(values, 'device_id');
      const target = str(values, 'target');
      if (!device || !target) return null;
      return only({
        device_id: device,
        target,
        source: str(values, 'source') || undefined,
        vrf: str(values, 'vrf') || undefined,
      });
    },
  },

  // -------------------------------------------------------------- servers
  'tftp.start': {
    action: 'Start serving',
    fields: [
      {
        name: 'root',
        label: 'Directory to serve',
        kind: 'mono',
        span: 2,
        hint: 'It has to exist already. Every transfer is confined to it.',
      },
      { name: 'port', label: 'Port', kind: 'number', hint: '0 takes any free port.' },
      { name: 'bind', label: 'Listen on', kind: 'mono', placeholder: '0.0.0.0' },
      { name: 'allow_write', label: 'Accept uploads', kind: 'switch' },
      {
        name: 'allow_overwrite',
        label: 'Let an upload replace a file',
        kind: 'switch',
        when: { field: 'allow_write', is: 'on' },
      },
    ],
    params: (values) =>
      only({
        root: str(values, 'root') || undefined,
        port: num(values, 'port'),
        bind: str(values, 'bind') || undefined,
        allow_write: on(values, 'allow_write'),
        allow_overwrite: on(values, 'allow_write') && on(values, 'allow_overwrite'),
      }),
  },
  'probe.syslog.start': {
    action: 'Start listening',
    fields: [
      { name: 'port', label: 'Port', kind: 'number', hint: '0 takes any free port.' },
      { name: 'bind', label: 'Listen on', kind: 'mono', placeholder: '0.0.0.0' },
      { name: 'tcp', label: 'Also listen on TCP (RFC 6587)', kind: 'switch', span: 2 },
    ],
    params: (values) =>
      only({
        port: num(values, 'port'),
        bind: str(values, 'bind') || undefined,
        tcp: on(values, 'tcp'),
      }),
  },

  // -------------------------------------------------------------- history
  'probe.history': {
    fields: [],
    params: () => null,
  },
};

/** The form's starting values: the spec's own, and every switch off. */
export function initialValues(spec: Spec | undefined): Values {
  const values: Values = {};
  for (const field of spec?.fields ?? []) values[field.name] = '';
  return { ...values, ...(spec?.initial ?? {}) };
}

/** Whether a field is shown, given what the rest of the form says. */
export function fieldShown(field: Field, values: Values): boolean {
  if (!field.when) return true;
  return (values[field.when.field] ?? '') === field.when.is;
}

/**
 * The choices of a `select`, in the order they are offered: "nothing chosen
 * yet" when the field has a placeholder, then the engine's rows, then the
 * choices written in the spec.
 *
 * `undefined` while a table the field depends on has not been read: an empty
 * list would say "the engine has no codecs", which is a different statement
 * from "not known yet".
 */
export function choicesOf(
  field: Field,
  rows?: readonly TableRow[],
): readonly FieldOption[] | undefined {
  if (field.source && rows === undefined) return undefined;
  const { source } = field;
  const fromEngine = source ? (rows ?? []).flatMap((row) => source.option(row) ?? []) : [];
  const nothingYet = field.placeholder ? [{ value: '', label: field.placeholder }] : [];
  return [...nothingYet, ...fromEngine, ...(field.options ?? [])];
}

/** What is known of one reference table a form's choices come from. */
export interface TableRead {
  /** `undefined` until the engine has answered: not known is not empty. */
  rows?: readonly TableRow[];
  /** The engine's own sentence, when the table could not be read. */
  error?: string;
  /** Said when the engine has more rows than were read. */
  partial?: string;
}

/** A `select` as it is drawn: its choices, and what is said under it. */
export interface Picker {
  /** The choices — or the one line that stands where they would be. */
  options: readonly FieldOption[];
  /** False while there is nothing to choose from: not read yet, or not readable. */
  ready: boolean;
  hint?: string;
  /** Why there is nothing to choose from, in the engine's words. */
  error?: string;
}

/**
 * A `select` in each state its table can be in.
 *
 * Four states that must not look alike: still being read (wait), could not be
 * read (the engine's sentence, and nothing offered in the table's place), read
 * and empty (said in words: an empty list alone would look like a broken
 * control), and read. What the chosen option means takes the hint's place —
 * the figures of the codec that is selected say more than "choose a codec"
 * does — and a picker that holds fewer choices than the engine has always
 * says so, whatever else is under it.
 *
 * In every state the list shows what the form holds, because that is what is
 * sent. A codec remembered from last time is still asked about while its
 * table is being read, or cannot be; a list that read "nothing to choose
 * from" over an answer for G.729 would be showing an answer to a question
 * that is nowhere on the screen. So a held value the list has no row for is
 * shown by the only name known for it, its id.
 */
export function pickerOf(field: Field, value: string, table?: TableRead): Picker {
  const read = choicesOf(field, table?.rows);
  if (read === undefined) {
    const unread = table?.error
      ? `The engine’s table of these could not be read, so there is nothing to choose from: ${table.error}`
      : undefined;
    const waiting = 'Reading the engine’s table…';
    return {
      options: [
        value === ''
          ? { value: '', label: unread ? 'Nothing to choose from' : waiting }
          : { value, label: value },
      ],
      ready: false,
      // With a value in the list, the list itself can no longer say it is waiting.
      hint: unread || value === '' ? undefined : waiting,
      error: unread,
    };
  }
  const chosen = read.find((option) => option.value === value);
  const nothing =
    field.source && table?.rows?.length === 0
      ? 'The engine’s table of these is empty: there is nothing of its own to choose.'
      : undefined;
  const hint = [chosen?.hint ?? nothing ?? field.hint, table?.partial].filter(Boolean).join(' ');
  return {
    options:
      chosen || value === '' ? read : [...read, { value, label: `${value} — not one of the choices` }],
    ready: true,
    hint: hint || undefined,
  };
}

/** The reference tables a form's choices are read from, each named once. */
export function sourcesOf(fields: readonly Field[]): string[] {
  return [...new Set(fields.flatMap((field) => (field.source ? [field.source.table] : [])))];
}

/**
 * The name of the field a refusal is about, or `undefined` when the engine's
 * sentence does not say — or names a field that is not on screen, where there
 * is nothing to put the sentence beside.
 */
export function blamed(spec: Spec, message: string, values: Values): string | undefined {
  const found = spec.blame?.find((entry) => entry.when.test(message));
  if (!found) return undefined;
  const field = spec.fields.find((entry) => entry.name === found.field);
  return field && fieldShown(field, values) ? field.name : undefined;
}

/**
 * Where a row of a reference table leads: the calculator that takes that row
 * as its input, and the form it opens with.
 *
 * The reference tables stay what they are — a list to look something up in —
 * and the working-out lives in one place. A DSCP row leads to the marking
 * calculator rather than growing hexadecimal, ECN and ToS columns of its own.
 */
const LEADS: Record<string, { tool: string; values: (row: TableRow) => Values | null }> = {
  dscp: {
    tool: 'tools.qos.marking',
    // By number, not by name: a value the registry has not named still has a
    // row when it is searched for, and its "name" there is not one.
    values: (row) => {
      const dscp = figure(row, 'dscp');
      return dscp === undefined ? null : { value: String(dscp), as: 'dscp' };
    },
  },
  voice_codecs: {
    tool: 'tools.qos.voice',
    values: (row) => (word(row, 'id') ? { codec: word(row, 'id') } : null),
  },
  voice_links: {
    tool: 'tools.qos.voice',
    values: (row) => (word(row, 'id') ? { link: word(row, 'id') } : null),
  },
};

/** The tool the rows of `table` lead to, when there is one. */
export function leadOf(table: string): string | undefined {
  return LEADS[table]?.tool;
}

/** What that tool's form is given for one row; `null` for a row that cannot be asked about. */
export function leadValues(table: string, row: TableRow): Values | null {
  return LEADS[table]?.values(row) ?? null;
}
