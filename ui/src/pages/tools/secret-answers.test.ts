import { describe, expect, it } from 'vitest';
import type { Identified } from '@/api/types/toolbox';
import { decoderOf, explainSecret } from './secret-answers';

/** The engine's own answers, from `tools.secret.identify` against a real build. */
const TYPE7: Identified = {
  kind: 'cisco_type7',
  name: 'Cisco type 7 (Vigenère obfuscation)',
  reversible: true,
  strength: 'weak',
  advice: 'Use `secret` (type 8 or 9) for logins.',
  decodable: true,
  decoder: 'cisco7',
};

const TYPE6: Identified = {
  kind: 'cisco_type6',
  name: 'Cisco type 6 (AES, reversible with the master key)',
  reversible: true,
  strength: 'moderate',
  advice:
    "AES encryption under the device's master key, meant for secrets the device has to read back.",
  decodable: false,
  decoder: null,
};

const TYPE5: Identified = {
  kind: 'md5crypt',
  name: 'Cisco type 5 (MD5-crypt)',
  reversible: false,
  strength: 'weak',
  advice: 'Prefer type 8 or 9.',
  decodable: false,
  decoder: null,
};

const TYPE9: Identified = {
  kind: 'scrypt',
  name: 'Cisco type 9 (scrypt)',
  reversible: false,
  strength: 'strong',
  advice: 'This is the one to use.',
  decodable: false,
  decoder: null,
};

const NOTHING: Identified = {
  kind: 'unknown',
  name: 'unknown',
  reversible: false,
  strength: 'unknown',
  advice: '',
  decodable: false,
  decoder: null,
};

describe('reversible and decodable are two different questions', () => {
  it('says of a type 6 secret that the format is reversible and that Nocturne still cannot read it', () => {
    const explained = explainSecret(TYPE6);
    expect(explained.format.headline).toBe('Reversible by design');
    expect(explained.recovery.headline).toBe('Nocturne cannot recover it');
    expect(explained.recovery.detail).toContain('key belongs to the device');
    // The reason must not read as "the format is strong".
    expect(explained.recovery.detail).toContain('Not because the format is strong');
  });

  it('does not give a type 6 and a hash the same two statements', () => {
    const six = explainSecret(TYPE6);
    const hash = explainSecret(TYPE5);
    expect(six.format.headline).not.toBe(hash.format.headline);
    expect(six.recovery.headline).not.toBe(hash.recovery.headline);
    expect(hash.recovery.headline).toBe('Nothing can recover it');
  });

  it('says of a type 7 that Nocturne can read it, and why that is the format’s fault', () => {
    const explained = explainSecret(TYPE7);
    expect(explained.recovery.headline).toBe('Nocturne can recover it');
    expect(explained.recovery.tone).toBe('crit');
    expect(explained.recovery.detail).toContain('needs no key');
    expect(decoderOf(TYPE7)).toBe('cisco7');
  });

  it('offers no decoder for anything this toolbox cannot undo', () => {
    expect(decoderOf(TYPE6)).toBeNull();
    expect(decoderOf(TYPE5)).toBeNull();
    expect(decoderOf(NOTHING)).toBeNull();
  });

  it('treats a one-way hash as good news about the format', () => {
    expect(explainSecret(TYPE9).format.tone).toBe('ok');
    expect(explainSecret(TYPE9).format.headline).toBe('One way: a hash');
    expect(explainSecret(TYPE6).format.tone).toBe('warn');
  });

  it('says plainly when it recognises nothing, rather than calling it a hash', () => {
    const explained = explainSecret(NOTHING);
    expect(explained.format.headline).toBe('Not a format Nocturne knows');
    expect(explained.recovery.detail).toContain('nothing here this toolbox can decode');
  });
});

describe('what the format is worth', () => {
  it('uses the engine’s own word and gives it a tone', () => {
    expect(explainSecret(TYPE7).strength).toMatchObject({ headline: 'weak', tone: 'crit' });
    expect(explainSecret(TYPE6).strength).toMatchObject({ headline: 'moderate', tone: 'warn' });
    expect(explainSecret(TYPE9).strength).toMatchObject({ headline: 'strong', tone: 'ok' });
    expect(explainSecret(NOTHING).strength).toMatchObject({ headline: 'unknown', tone: 'idle' });
  });

  it('quotes the engine’s advice rather than paraphrasing it', () => {
    expect(explainSecret(TYPE6).advice).toBe(TYPE6.advice);
  });

  it('has a sentence for a strength word it has never seen, instead of nothing', () => {
    const odd = explainSecret({ ...TYPE9, strength: 'excellent' });
    expect(odd.strength.detail).toContain('no judgement');
    expect(odd.strength.tone).toBe('idle');
  });
});
