import { describe, expect, it } from 'vitest';
import type { CreditNote, InvoiceView, Money } from '@/api/types/isp';
import {
  allocationChoices,
  CREDIT_NOTE_MEANING,
  creditActions,
  creditEffect,
  creditRefusal,
  creditRoom,
  creditStateWords,
  standingText,
} from './credit-notes';

const usd = (minor: number, text: string): Money => ({ currency: 'USD', minor, digits: 2, text });
const say = (money: Money) => `${money.text} ${money.currency}`;

type Creditable = Pick<
  InvoiceView,
  'id' | 'state' | 'total' | 'credited' | 'outstanding' | 'customer_id'
>;

/** An invoice of 600.00 with 200.00 paid and 100.00 credited already. */
function invoice(over: Partial<Creditable> = {}): Creditable {
  return {
    id: 'i1',
    customer_id: 'c1',
    state: 'issued',
    total: usd(60000, '600.00'),
    credited: usd(10000, '100.00'),
    outstanding: usd(30000, '300.00'),
    ...over,
  };
}

function note(over: Partial<CreditNote> = {}): CreditNote {
  return {
    id: 'cn1',
    number: 'CN-2026-0001',
    customer_id: 'c1',
    invoice_id: null,
    amount: usd(15000, '150.00'),
    reason: 'Goodwill',
    note: '',
    at: 1,
    allocated_at: null,
    state: 'issued',
    void_reason: '',
    voided_at: null,
    recorded_at: 1,
    ...over,
  };
}

describe('how much can be credited on an invoice', () => {
  it('is its total less what has been credited, not what is left outstanding', () => {
    // 600.00 charged, 100.00 credited: 500.00 of room, though only 300.00
    // is still owed. Crediting an invoice that has been paid is a real case.
    expect(creditRoom(invoice())).toEqual(usd(50000, '500.00'));
    const paid = invoice({ credited: usd(0, '0.00'), outstanding: usd(0, '0.00') });
    expect(creditRoom(paid)).toEqual(usd(60000, '600.00'));
    // Credited to its total: no room, and not a negative one.
    expect(creditRoom(invoice({ credited: usd(60000, '600.00') }))).toEqual(usd(0, '0.00'));
  });

  it('is nothing at all on a draft or a void invoice', () => {
    expect(creditRoom(invoice({ state: 'draft' }))).toBeNull();
    expect(creditRoom(invoice({ state: 'void' }))).toBeNull();
  });

  it('refuses nothing, less than nothing, and more than was ever charged', () => {
    expect(creditRefusal(0, invoice(), say)).toContain('more than nothing');
    expect(creditRefusal(-500, null, say)).toContain('more than nothing');
    expect(creditRefusal(50000, invoice(), say)).toBeNull();
    const over = creditRefusal(50001, invoice(), say) ?? '';
    expect(over).toContain('600.00 USD');
    expect(over).toContain('100.00 USD of it has been credited already');
    expect(over).toContain('at most 500.00 USD more');
    expect(creditRefusal(100, invoice({ state: 'void' }), say)).toContain('Only an issued invoice');
    // On the account there is no invoice to cap it.
    expect(creditRefusal(9_000_000, null, say)).toBeNull();
  });

  it('says beforehand what the note would leave, including a customer left in credit', () => {
    expect(creditEffect(10000, invoice(), say)).toBe('200.00 USD would still be outstanding.');
    expect(creditEffect(30000, invoice(), say)).toBe('Nothing would be left outstanding.');
    // 50.00 beyond what is owed: it stands in their favour, and is not a debt.
    const beyond = creditEffect(35000, invoice(), say);
    expect(beyond).toContain('50.00 USD beyond its total');
    expect(beyond).toContain('in the customer’s favour');
    expect(beyond).toContain('not taken off anything else they owe');
  });
});

describe('where a credit note stands', () => {
  it('says a note on the account reduces nothing yet', () => {
    expect(standingText(note())).toBe(
      'on the account: it reduces nothing until it is set against an invoice',
    );
  });

  it('names the invoice when it has the number, and does not print an id when it has not', () => {
    const against = note({ invoice_id: '0b8e2c' });
    expect(standingText(against, '2026-0002')).toBe('against invoice 2026-0002');
    expect(standingText(against)).toBe('against an invoice');
    expect(standingText(against, null)).not.toContain('0b8e2c');
  });

  it('says a void note does nothing, wherever it stood', () => {
    expect(standingText(note({ state: 'void' }))).toBe('void: it credits nothing');
    expect(standingText(note({ state: 'void', invoice_id: 'i1' }), '2026-0002')).toBe(
      'void: what it wrote off is owed again',
    );
    expect(creditStateWords(note({ state: 'void' })).label).toBe('Void');
    expect(creditStateWords(note()).label).toBe('Issued');
  });

  it('offers to void an issued note, to set one on the account against an invoice, and nothing on a void one', () => {
    expect(creditActions(note())).toEqual({ void: true, allocate: true });
    // Already against an invoice: allocation happens once.
    expect(creditActions(note({ invoice_id: 'i1' }))).toEqual({ void: true, allocate: false });
    expect(creditActions(note({ state: 'void' }))).toEqual({ void: false, allocate: false });
  });

  it('offers only invoices that can take the whole note', () => {
    const invoices = [
      invoice({ id: 'room' }),
      // 100.00 left to credit: a 150.00 note is not split to fit.
      invoice({ id: 'tight', credited: usd(50000, '500.00') }),
      invoice({ id: 'draft', state: 'draft' }),
      invoice({ id: 'void', state: 'void' }),
      invoice({ id: 'theirs', customer_id: 'c2' }),
      invoice({
        id: 'dinars',
        total: { currency: 'IQD', minor: 600000, digits: 0, text: '600000' },
        credited: { currency: 'IQD', minor: 0, digits: 0, text: '0' },
      }),
    ];
    expect(allocationChoices(note(), invoices).map((row) => row.id)).toEqual(['room']);
    // A note of exactly the room that is left still fits.
    expect(
      allocationChoices(note({ amount: usd(10000, '100.00') }), invoices).map((row) => row.id),
    ).toEqual(['room', 'tight']);
  });

  it('never calls a credit note money, a payment or a refund', () => {
    expect(CREDIT_NOTE_MEANING).toContain('not money');
    expect(CREDIT_NOTE_MEANING).toContain('nothing is received');
    expect(CREDIT_NOTE_MEANING).not.toMatch(/refund(ed)? to|pay(s|ing)? back/i);
  });
});
