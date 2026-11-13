/**
 * Network value helpers: parsing and ordering addresses the way an engineer
 * expects (10.0.0.9 before 10.0.0.10, Gi1/0/2 before Gi1/0/10).
 */

export interface IpAddress {
  version: 4 | 6;
  /** The address as an unsigned integer (32 or 128 bits). */
  value: bigint;
}

export interface Cidr extends IpAddress {
  prefix: number;
  /** First address of the block. */
  network: bigint;
}

const IPV4_OCTET = /^(?:0|[1-9]\d{0,2})$/;

function parseIpv4Value(text: string): bigint | null {
  const octets = text.split('.');
  if (octets.length !== 4) return null;
  let value = 0n;
  for (const octet of octets) {
    if (!IPV4_OCTET.test(octet)) return null;
    const number = Number(octet);
    if (number > 255) return null;
    value = (value << 8n) | BigInt(number);
  }
  return value;
}

function parseIpv6Value(text: string): bigint | null {
  let source = text;
  // An embedded IPv4 tail (::ffff:192.0.2.1) is two hextets.
  const lastColon = source.lastIndexOf(':');
  if (source.includes('.')) {
    const tail = parseIpv4Value(source.slice(lastColon + 1));
    if (tail === null) return null;
    source = `${source.slice(0, lastColon + 1)}${(tail >> 16n).toString(16)}:${(tail & 0xffffn).toString(16)}`;
  }

  const halves = source.split('::');
  if (halves.length > 2) return null;
  const head = halves[0] ? halves[0].split(':') : [];
  const tail = halves.length === 2 && halves[1] ? halves[1].split(':') : [];
  const missing = 8 - head.length - tail.length;
  if (halves.length === 2 ? missing < 1 : missing !== 0) return null;

  const hextets = [...head, ...Array.from({ length: halves.length === 2 ? missing : 0 }, () => '0'), ...tail];
  let value = 0n;
  for (const hextet of hextets) {
    if (!/^[0-9a-fA-F]{1,4}$/.test(hextet)) return null;
    value = (value << 16n) | BigInt(Number.parseInt(hextet, 16));
  }
  return value;
}

/** Parses an IPv4 or IPv6 address. Returns null for anything else. */
export function parseIp(text: string): IpAddress | null {
  const trimmed = text.trim();
  if (trimmed.includes(':')) {
    const value = parseIpv6Value(trimmed);
    return value === null ? null : { version: 6, value };
  }
  const value = parseIpv4Value(trimmed);
  return value === null ? null : { version: 4, value };
}

/** Parses `address/prefix`. A bare address is a host route (/32 or /128). */
export function parseCidr(text: string): Cidr | null {
  const [addressText = '', prefixText, ...rest] = text.trim().split('/');
  if (rest.length > 0) return null;
  const address = parseIp(addressText);
  if (!address) return null;
  const bits = address.version === 4 ? 32 : 128;
  let prefix = bits;
  if (prefixText !== undefined) {
    if (!/^\d{1,3}$/.test(prefixText)) return null;
    prefix = Number(prefixText);
    if (prefix > bits) return null;
  }
  const hostBits = BigInt(bits - prefix);
  return { ...address, prefix, network: (address.value >> hostBits) << hostBits };
}

/** Whether `address` falls inside `cidr`. Mixed IP versions never match. */
export function cidrContains(cidr: Cidr, address: IpAddress): boolean {
  if (cidr.version !== address.version) return false;
  const hostBits = BigInt((cidr.version === 4 ? 32 : 128) - cidr.prefix);
  return address.value >> hostBits === cidr.network >> hostBits;
}

/**
 * Orders address strings numerically: IPv4 before IPv6, and anything that is
 * not an address last (alphabetically), so a column with a few hostnames in it
 * still sorts predictably. A prefix length after the address is a tie-breaker.
 */
export function compareIp(a: string, b: string): number {
  const left = parseCidr(a);
  const right = parseCidr(b);
  if (!left || !right) {
    if (left) return -1;
    if (right) return 1;
    return compareNatural(a, b);
  }
  if (left.version !== right.version) return left.version - right.version;
  if (left.value !== right.value) return left.value < right.value ? -1 : 1;
  return left.prefix - right.prefix;
}

const naturalCollator = new Intl.Collator('en', { numeric: true, sensitivity: 'base' });

/** Natural ordering: digit runs compare as numbers (`Gi1/0/2` before `Gi1/0/10`). */
export function compareNatural(a: string, b: string): number {
  return naturalCollator.compare(a, b);
}

/**
 * Normalises a MAC address written in any common notation
 * (`AA-BB-CC-DD-EE-FF`, `aabb.ccdd.eeff`, `aabbccddeeff`) to `aa:bb:cc:dd:ee:ff`.
 */
export function normalizeMac(text: string): string | null {
  const hex = text.trim().replace(/[:\-.]/g, '');
  if (!/^[0-9a-fA-F]{12}$/.test(hex)) return null;
  return (hex.toLowerCase().match(/.{2}/g) ?? []).join(':');
}
