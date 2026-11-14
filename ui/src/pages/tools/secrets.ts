/**
 * Two questions about a secret in a configuration, kept apart.
 *
 * `tools.secret.identify` answers both, and they are not the same question:
 *
 * * **`reversible`** is about the *format*. Cisco type 7 and Junos `$9$` are
 *   obfuscation and come back with arithmetic. Cisco type 6 is AES: reversible
 *   *by design*, because the device has to read the secret back. A type 5, 8
 *   or 9 hash is not reversible by anything.
 * * **`decodable`** is about *this toolbox*. It can undo the two obfuscations
 *   and nothing else. Type 6 is `reversible: true, decodable: false`, and the
 *   reason is not weakness in the format — it is that the key is the device's
 *   master key, which Nocturne does not have.
 *
 * A screen that printed one tick for "can decode" would turn those into the
 * same thing and would teach the reader that type 6 is as safe as a hash, or
 * that a hash is as weak as type 7. So this module produces two separate
 * statements, each with its own words, and the panel shows both.
 *
 * Nothing here keeps, logs or stores a secret: the values pass through the
 * component and are gone on the next keystroke (`specs.ts` marks these tools
 * `secret`, which is what keeps their inputs out of `localStorage`).
 */

import type { Decoded, Encoded, Identified } from '@/api/types/toolbox';
import type { FactTone } from './facts';

export interface Statement {
  headline: string;
  detail: string;
  tone: FactTone;
}

export interface SecretExplanation {
  /** What the storage format is, and whether anything can undo it. */
  format: Statement;
  /** Whether Nocturne can recover the secret, and why or why not. */
  recovery: Statement;
  /** What the format is worth, in the engine's own vocabulary. */
  strength: Statement;
  /** The engine's advice, quoted rather than paraphrased. */
  advice: string;
}

const STRENGTH_TONE: Record<string, FactTone> = {
  none: 'crit',
  weak: 'crit',
  moderate: 'warn',
  strong: 'ok',
  unknown: 'idle',
};

const STRENGTH_WORDS: Record<string, string> = {
  none: 'No protection at all: the secret is in the configuration in the clear.',
  weak: 'Weak: this is undone with published arithmetic, not broken.',
  moderate: 'Moderate: it depends on a key or a salt staying out of reach.',
  strong: 'Strong: a modern password hash, meant to be slow to attack.',
  unknown: 'Unknown: this is not a format Nocturne recognises.',
};

export function explainSecret(found: Identified): SecretExplanation {
  const unknown = found.kind === 'unknown';
  const format: Statement = unknown
    ? {
        headline: 'Not a format Nocturne knows',
        detail:
          'Nothing in the line matches a secret format this toolbox recognises. If it is a password as typed, it is already plain text.',
        tone: 'idle',
      }
    : found.reversible
      ? {
          headline: 'Reversible by design',
          detail:
            'The format is meant to be undone: the device, or anybody with the published algorithm or the right key, can read the secret back. It is storage, not protection.',
          tone: 'warn',
        }
      : {
          headline: 'One way: a hash',
          detail:
            'There is no undoing this. A password can only be checked against it, or guessed at with a cracker.',
          tone: 'ok',
        };

  const recovery: Statement = found.decodable
    ? {
        headline: 'Nocturne can recover it',
        detail:
          'The algorithm is published and needs no key, which is exactly why this format protects nothing. Open “Decode a vendor secret” with this line.',
        tone: 'crit',
      }
    : found.reversible
      ? {
          headline: 'Nocturne cannot recover it',
          detail:
            'Not because the format is strong, but because the key belongs to the device. Anyone holding that key can read the secret, so keep it out of backups and tickets.',
          tone: 'warn',
        }
      : {
          headline: 'Nothing can recover it',
          detail: unknown
            ? 'There is nothing here this toolbox can decode.'
            : 'A hash cannot be decoded, by this toolbox or anything else.',
          tone: 'idle',
        };

  return {
    format,
    recovery,
    strength: {
      headline: found.strength,
      detail: STRENGTH_WORDS[found.strength] ?? 'Nocturne has no judgement about this format.',
      tone: STRENGTH_TONE[found.strength] ?? 'idle',
    },
    advice: found.advice,
  };
}

/** The decoder to pass to `tools.secret.decode`, and whether there is one. */
export function decoderOf(found: Identified): Identified['decoder'] {
  return found.decodable ? found.decoder : null;
}

/**
 * What must appear beside a value that has just been decoded or made.
 *
 * The engine writes this sentence ("…obfuscation with a published algorithm,
 * not encryption…") and it travels with the value: a decoded password pasted
 * into a ticket without it is how people come to believe type 7 is encryption.
 */
export function obfuscationNote(result: Decoded | Encoded): string {
  return result.note;
}

/**
 * The warning above a value nobody should keep.
 *
 * The screen never writes a generated or decoded secret to storage, to a URL,
 * or to an event — so it says so, once, where the value appears.
 */
export const KEPT_NOWHERE =
  'Shown here and kept nowhere: not in the probe history, not in the audit trail, not in this screen’s remembered inputs, and not in the assistant’s context.';
