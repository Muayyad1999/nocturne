import { describe, expect, it } from 'vitest';
import { findRedactions, hasRedaction, redactionSentence, summarise } from './redaction';

/** As the backend really writes it (`nocturne_ai::redact`). */
const CONFIG = [
  'no service password-encryption',
  'username netops privilege 15 password <secret:1>',
  'snmp-server community <secret:2:weak> RO',
  'snmp-server community <secret:2:weak> RW',
  'ntp authentication-key 1 md5 <secret:1>',
].join('\n');

describe('findRedactions', () => {
  it('finds every placeholder with its number and whether the value was notorious', () => {
    const found = findRedactions(CONFIG);
    expect(found).toHaveLength(4);
    expect(found[0]).toMatchObject({ id: 1, weak: false });
    expect(found[1]).toMatchObject({ id: 2, weak: true });
    expect(CONFIG.slice(found[0]!.from, found[0]!.to)).toBe('<secret:1>');
  });

  it('counts the secrets, not the places, and says how many were defaults', () => {
    expect(summarise(CONFIG)).toEqual({ count: 4, distinct: 2, weak: 1 });
  });

  it('finds nothing in a configuration that has no secrets', () => {
    expect(summarise('hostname SW1\ninterface Vlan1')).toEqual({ count: 0, distinct: 0, weak: 0 });
  });

  it('is not fooled by the words around it', () => {
    expect(hasRedaction('no service password-encryption')).toBe(false);
    expect(hasRedaction('key <secret:9>')).toBe(true);
    expect(findRedactions('<secret:>')).toHaveLength(0);
  });
});

describe('redactionSentence', () => {
  it('says a marker is not a value', () => {
    expect(redactionSentence(true, summarise(CONFIG))).toContain('it is not the value');
  });

  it('distinguishes "nothing was hidden" from "nothing is here"', () => {
    const none = redactionSentence(true, summarise('hostname SW1'));
    expect(none).toContain('nothing here is held back');
    expect(none).not.toContain('taken out in');
  });

  it('says plainly when secrets are being shown, and that it was recorded', () => {
    const shown = redactionSentence(false, summarise('username x password hunter2'));
    expect(shown).toContain('audit');
  });

  it('mentions a well-known default when there is one', () => {
    expect(redactionSentence(true, summarise('snmp-server community <secret:3:weak> RO'))).toContain(
      'well-known default',
    );
    expect(redactionSentence(true, summarise('key <secret:3>'))).not.toContain('well-known default');
  });
});
