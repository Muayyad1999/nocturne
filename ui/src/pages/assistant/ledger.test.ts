import { describe, expect, it } from 'vitest';
import type { AiToolInfo } from '@/api/types/ai';
import { LEDGER_WORDS, ledgerSwitchAvailable, ledgerTools, offeredNow } from './ledger';

// The three ledger entries exactly as `ai.tools` answered in the ISP edition
// with `isp_access` on, and two of the tools beside them.
const LEDGER: AiToolInfo[] = [
  { name: 'isp_overview', title: 'ISP figures', touches_device: false, methods: ['isp.summary'] },
  {
    name: 'isp_customers',
    title: 'ISP customers',
    touches_device: false,
    methods: ['isp.customers', 'isp.subscriptions', 'isp.arrears'],
  },
  { name: 'isp_invoices', title: 'ISP invoices', touches_device: false, methods: ['isp.invoices'] },
];
const OTHERS: AiToolInfo[] = [
  { name: 'list_devices', title: 'List devices', touches_device: false, methods: ['devices.list'] },
  { name: 'qos_status', title: 'QoS classes', touches_device: false, methods: ['monitor.qos', 'devices.get'] },
];

describe('the customer ledger switch', () => {
  it('is offered only by a build with the ledger and the settings to change it with', () => {
    expect(ledgerSwitchAvailable(undefined)).toBeUndefined();
    expect(ledgerSwitchAvailable(['settings.get', 'settings.set', 'ai.tools'])).toBe(false);
    expect(ledgerSwitchAvailable(['isp.summary', 'settings.get'])).toBe(false);
    expect(ledgerSwitchAvailable(['isp.summary', 'settings.get', 'settings.set'])).toBe(true);
  });

  it('counts as the ledger’s only the tools that reach an isp method', () => {
    expect(ledgerTools([...OTHERS, ...LEDGER]).map((tool) => tool.name)).toEqual([
      'isp_overview',
      'isp_customers',
      'isp_invoices',
    ]);
    expect(ledgerTools(OTHERS)).toEqual([]);
  });

  it('says what the engine offers, not what the switch was set to', () => {
    expect(offeredNow([...OTHERS, ...LEDGER], true)).toBe(
      'Offered to the default model now: ISP figures, ISP customers, ISP invoices.',
    );
    expect(offeredNow(OTHERS, false)).toBe('None of the ledger’s tools is offered to the model now.');
    // On, but the default model is given the core set only.
    expect(offeredNow(OTHERS, true)).toMatch(/^None of the ledger’s tools is offered to the default model now/);
    expect(offeredNow(OTHERS, true)).toContain('16 000 tokens or fewer');
  });

  it('does not speak of a default model when there is none', () => {
    expect(offeredNow([...OTHERS, ...LEDGER], true, false)).toBe(
      'No model is set up yet; one would be offered: ISP figures, ISP customers, ISP invoices.',
    );
    expect(offeredNow([...OTHERS, ...LEDGER], true, false)).not.toContain('default model');
  });

  it('says that off sends nothing at all, and names what is never sent', () => {
    expect(LEDGER_WORDS.off).toContain('reads nothing from it');
    expect(LEDGER_WORDS.off).toContain('no total');
    for (const field of ['id', 'name', 'state', 'plan', 'balance']) {
      expect(LEDGER_WORDS.on).toContain(field);
    }
    for (const detail of ['phone numbers', 'e-mail', 'postal addresses', 'account codes', 'notes']) {
      expect(LEDGER_WORDS.never).toContain(detail);
    }
  });
});
