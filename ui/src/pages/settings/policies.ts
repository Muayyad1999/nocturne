/**
 * The host-key policy in words. Security and Connections both offer it, and
 * both must describe it the same way.
 */

export interface PolicyOption {
  value: string;
  label: string;
  /** What this policy does, and when it is the right one. */
  description: string;
}

export const HOST_KEY_POLICIES: readonly PolicyOption[] = [
  {
    value: 'tofu',
    label: 'Trust it the first time, then insist',
    description:
      'The key a host shows on the first connection is remembered; if it ever changes, Nocturne refuses to connect and asks you. This is the default and what SSH itself does.',
  },
  {
    value: 'strict',
    label: 'Refuse anything not already trusted',
    description:
      'Only hosts whose keys are already in the list are connected to. Every new device has to be trusted by hand first.',
  },
  {
    value: 'accept',
    label: 'Do not check at all',
    description:
      'Connect to whatever answers. Nothing warns you if a device is impersonated or a connection is intercepted. Lab use only.',
  },
];

export const LEGACY_CRYPTO_OPTIONS: readonly PolicyOption[] = [
  {
    value: 'auto',
    label: 'Fall back and tell me',
    description:
      'Try modern algorithms first, fall back to the old ones a decade-old switch needs, and mark that session so you know.',
  },
  {
    value: 'never',
    label: 'Modern algorithms only',
    description: 'Refuse anything that cannot negotiate current algorithms, whatever it costs.',
  },
  {
    value: 'always',
    label: 'Offer the old algorithms from the start',
    description:
      'Saves a round trip on a network of old equipment, at the cost of offering weak algorithms to everything.',
  },
];
