/**
 * The ISP edition's end-to-end state, built through the RPC surface the app
 * itself uses — no fixtures and no database writing, so everything these
 * screens are asserted against is something a person could have entered.
 *
 * The shape of it is chosen so that every rule the screens exist to keep is
 * actually exercised:
 *
 * * **two currencies that must never be added together**: a customer billed
 *   in `IQD` (no minor unit — 150 000 dinars is `minor: 150000`) and one
 *   billed in `USD` (two minor digits — 250 dollars is `minor: 25000`);
 * * **a customer with no usage figure at all**, with a serving device and
 *   interface that monitoring has never collected
 *   (`interface_never_collected`: nobody has looked), and one with no serving
 *   device at all (`no_serving_device`) — two different nothings;
 * * **an invoice that is overdue**, issued and due in the past, and one that
 *   is owed but not due yet, so the ageing has something in two bands;
 * * **a payment in a currency the invoice is not in**, recorded at the rate
 *   it was taken at (1 USD = 1320 IQD, as a person quotes it);
 * * **a part period**, from a subscription that started mid-month, so an
 *   invoice line carries its proration;
 * * **a draft that is left a draft**, and a suspended account.
 */

export interface Rpc {
  <T = unknown>(method: string, params?: unknown): Promise<T>;
}

export interface Seeded {
  deviceId: string;
  /** The customer billed in dinars, with no serving device. */
  zagros: string;
  /** The customer billed in dollars, whose interface has never been collected. */
  tigris: string;
  /** The suspended account, whose plan started mid-month. */
  dohuk: string;
  homeTariff: string;
  businessTariff: string;
  linkId: string;
  /** The issued, overdue invoice in IQD. */
  overdueInvoice: string;
  /** The issued invoice in USD that is not due yet. */
  openInvoice: string;
  /** The draft that is deliberately left a draft, with a pro-rated line. */
  draftInvoice: string;
  /** The October window every invoice here is for. */
  october: { start: number; end: number };
}

const UTC = (year: number, month: number, day = 1) => Date.UTC(year, month - 1, day);

/** The whole of October 2026, as `[start, end)` — one whole monthly period. */
const OCTOBER = { start: UTC(2026, 10), end: UTC(2026, 11) };

export async function seedIsp(rpc: Rpc): Promise<Seeded> {
  const device = await rpc<{ id: string }>('devices.create', {
    name: 'tower-a-rtr',
    address: '10.9.9.1',
    site: 'Erbil',
    role: 'edge',
    profile: 'mikrotik_routeros',
  });

  // Two tariffs in two currencies. `Home 10` is priced per megabit in dinars
  // (15 000 a megabit, so 150 000 a month); `Business 50` is a flat 250.00 in
  // dollars. Both are sent as whole minor units or an exact decimal string —
  // never a float.
  const home = await rpc<{ tariff: { id: string } }>('isp.tariff.save', {
    name: 'Home 10',
    description: 'Residential, 10 Mbit/s',
    mbit: 10,
    unit: 'per_mbit',
    price: { currency: 'IQD', minor: 15_000 },
    period: 'monthly',
  });
  const business = await rpc<{ tariff: { id: string } }>('isp.tariff.save', {
    name: 'Business 50',
    description: 'Business, 50 Mbit/s',
    mbit: 50,
    unit: 'flat',
    price: { currency: 'USD', text: '250.00' },
    period: 'monthly',
  });

  const zagros = await rpc<{ customer: { id: string } }>('isp.customer.save', {
    name: 'Zagros Net',
    code: 'C-001',
    currency: 'IQD',
    phone: '+964 750 000 0001',
    site: 'Erbil',
    tower: 'Tower A',
    location: 'Ainkawa, second floor',
    tags: ['wireless'],
  });
  const tigris = await rpc<{ customer: { id: string } }>('isp.customer.save', {
    name: 'Tigris Trading',
    code: 'C-002',
    currency: 'USD',
    email: 'accounts@example.invalid',
    site: 'Baghdad',
    tower: 'Tower A',
    device_id: device.id,
    interface: 'ether5',
    location: 'Karrada',
    tags: ['fibre', 'business'],
  });
  const dohuk = await rpc<{ customer: { id: string } }>('isp.customer.save', {
    name: 'Dohuk Dates',
    code: 'C-003',
    currency: 'IQD',
    site: 'Dohuk',
    tower: 'Tower A',
  });

  await rpc('isp.subscription.create', {
    customer_id: zagros.customer.id,
    tariff_id: home.tariff.id,
    started_at: UTC(2026, 8),
  });
  await rpc('isp.subscription.create', {
    customer_id: tigris.customer.id,
    tariff_id: business.tariff.id,
    started_at: UTC(2026, 8),
  });
  // Mid-October, so an October invoice for them is a part period.
  await rpc('isp.subscription.create', {
    customer_id: dohuk.customer.id,
    tariff_id: home.tariff.id,
    started_at: UTC(2026, 10, 16),
  });

  const link = await rpc<{ link: { id: string } }>('isp.link.save', {
    name: 'Uplink A',
    description: 'The feed to Tower A',
    tower: 'Tower A',
    capacity_mbit: 100,
    cost_per_mbit: { currency: 'USD', text: '3.00' },
    cost_period: 'monthly',
  });

  // Zagros: issued and overdue, with part of it paid in dinars.
  const zagrosDraft = await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
    customer_id: zagros.customer.id,
    period_start: OCTOBER.start,
    period_end: OCTOBER.end,
  });
  const overdue = await rpc<{ id: string }>('isp.invoice.issue', {
    id: zagrosDraft.invoice.id,
    issued_at: UTC(2026, 10, 2),
    due_at: UTC(2026, 10, 10),
  });
  await rpc('isp.payment.record', {
    customer_id: zagros.customer.id,
    invoice_id: overdue.id,
    amount: { currency: 'IQD', minor: 1_000 },
    method: 'cash',
    reference: 'R-1001',
  });

  // Tigris: issued, not due yet, and part-settled by a payment in dinars at
  // the rate it was taken at. 33 000 IQD ÷ 1 320 = 25.00 USD, exactly.
  const tigrisDraft = await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
    customer_id: tigris.customer.id,
    period_start: OCTOBER.start,
    period_end: OCTOBER.end,
  });
  const open = await rpc<{ id: string }>('isp.invoice.issue', {
    id: tigrisDraft.invoice.id,
    issued_at: UTC(2026, 11, 1),
    due_at: UTC(2026, 12, 1),
  });
  await rpc('isp.payment.record', {
    customer_id: tigris.customer.id,
    invoice_id: open.id,
    amount: { currency: 'IQD', minor: 33_000 },
    method: 'cash',
    reference: 'R-1002',
    conversion: { from: 'USD', to: 'IQD', rate: 1_320 },
  });

  // Dohuk: a draft with a pro-rated line, left as a draft, on a suspended
  // account — which is a decision about the account and nothing else.
  const dohukDraft = await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
    customer_id: dohuk.customer.id,
    period_start: OCTOBER.start,
    period_end: OCTOBER.end,
  });
  await rpc('isp.customer.state', {
    id: dohuk.customer.id,
    state: 'suspended',
    reason: 'Two months in arrears',
  });

  return {
    deviceId: device.id,
    zagros: zagros.customer.id,
    tigris: tigris.customer.id,
    dohuk: dohuk.customer.id,
    homeTariff: home.tariff.id,
    businessTariff: business.tariff.id,
    linkId: link.link.id,
    overdueInvoice: overdue.id,
    openInvoice: open.id,
    draftInvoice: dohukDraft.invoice.id,
    october: OCTOBER,
  };
}

/** Adds `count` more customers, so the list is asserted at a size that pages. */
export async function seedManyCustomers(rpc: Rpc, count: number): Promise<void> {
  const towns = ['Erbil', 'Baghdad', 'Basra', 'Mosul', 'Dohuk', 'Kirkuk'];
  for (let index = 0; index < count; index += 1) {
    await rpc('isp.customer.save', {
      name: `Subscriber ${String(index + 1).padStart(4, '0')}`,
      code: `S-${String(index + 1).padStart(4, '0')}`,
      currency: index % 3 === 0 ? 'USD' : 'IQD',
      site: towns[index % towns.length],
      tower: index % 2 === 0 ? 'Tower A' : 'Tower B',
    });
  }
}
