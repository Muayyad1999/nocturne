// @vitest-environment happy-dom
/**
 * "Keep as evidence…", as far as a DOM without an engine can show.
 *
 * Three things are held here: a build without `evidence.*` is offered
 * nothing; what is sent is *what to read* and never what is on screen; and a
 * refusal is shown in the engine's own words, with the dialog still open and
 * nothing claimed to be kept. `tests/ops.spec.ts` keeps a real chart and a
 * real drawing on a real engine.
 *
 * The refusal messages and the stored item are real answers recorded from
 * `nocturne-ops`' own test run and from a running `nocturne-server`.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RpcError } from '@/api/errors';
import type { EvidenceWithBody } from '@/api/types/ops';
import { useToasts } from '@/design/Toast';
import { useKeepAsEvidence, type ChartEvidenceRequest, type KeepAsEvidence } from './KeepAsEvidence';
import { captureCall, describeRequest, refusal } from './keep';

/** What the stand-in engine has, says and was asked. */
const engine = vi.hoisted(() => ({
  methods: undefined as string[] | undefined,
  calls: [] as { method: string; params: unknown }[],
  answer: (_method: string, _params: unknown): unknown => undefined,
}));

vi.mock('@/api/hooks', () => ({
  useRpc: (method: string) => {
    if (method === 'app.methods') return { data: engine.methods, isError: false, error: null };
    if (method === 'incidents.list') {
      return {
        data: {
          incidents: [{ id: 'i74e62eqcai2xnh5', title: 'SW1 uplink flapping', state: 'open' }],
          counts: {},
        },
        isError: false,
        error: null,
      };
    }
    return { data: undefined, isError: false, error: null };
  },
}));

vi.mock('@/api/client', () => ({
  rpc: async (method: string, params: unknown) => {
    engine.calls.push({ method, params });
    const result = engine.answer(method, params);
    if (result instanceof Error) throw result;
    return result;
  },
}));

const EVERYTHING = [
  'evidence.capture_chart',
  'evidence.capture_topology',
  'evidence.attach',
  'evidence.list',
  'incidents.list',
];

/** A real `evidence.capture_chart` answer, its points included. */
const KEPT: EvidenceWithBody = {
  id: 'vxyqjilvi777plur',
  kind: 'chart',
  title: 'cpu.pct, if.oper on Ops/SW1',
  captured_at: 1795386621677,
  source: {
    from: 'metrics.query_many',
    device_id: '7u5ygdhiejg4mfx3',
    device: 'Ops/SW1',
    lab: true,
    params: { series: [{ device_id: '7u5ygdhiejg4mfx3', metric: 'cpu.pct' }], since: 1795386021633, until: 1795386621633, step_s: 1 },
  },
  redacted: true,
  bytes: 746,
  sha256: '9726cb34525cd6efbde0f56e72846ae72940cf593e2d33021e5d8689d3c832bd',
  incident_ids: [],
  body: {
    type: 'chart',
    series: [{ device_id: '7u5ygdhiejg4mfx3', metric: 'cpu.pct', instance: '', points: [[1795386599000, 2.0]], step_s: 1, rollup: 'raw' }],
    coverage: { since: 1795386021633, until: 1795386621633, points: 1, series: [] },
  },
};

const CHART: ChartEvidenceRequest = {
  kind: 'chart',
  series: [
    { device_id: '7u5ygdhiejg4mfx3', metric: 'cpu.pct', instance: '' },
    { device_id: '7u5ygdhiejg4mfx3', metric: 'if.oper', instance: 'FastEthernet0/1' },
  ],
  since: 1795386021633,
  until: 1795386621633,
  step_s: 1,
};

let root: Root | null = null;
let host: HTMLElement | null = null;
let keep: KeepAsEvidence | null = null;

function Screen() {
  keep = useKeepAsEvidence();
  return <>{keep.dialog}</>;
}

function mount(): KeepAsEvidence {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => root?.render(<Screen />));
  if (!keep) throw new Error('the screen did not render');
  return keep;
}

function dialog(): HTMLElement | null {
  return document.querySelector('[role="dialog"]');
}

function button(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find((candidate) => candidate.textContent?.trim() === name);
  if (!found) throw new Error(`no button called “${name}”`);
  return found;
}

function type(input: HTMLInputElement, text: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
  act(() => {
    setter?.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

function choose(select: HTMLSelectElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
  act(() => {
    setter?.call(select, value);
    select.dispatchEvent(new Event('change', { bubbles: true }));
  });
}

/** Lets the click's promises settle: the capture, then the attachment. */
async function settled(): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < 6; turn += 1) await Promise.resolve();
  });
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  engine.methods = [...EVERYTHING];
  engine.calls = [];
  engine.answer = (method) => (method === 'evidence.attach' ? { ...KEPT, incident_ids: ['i74e62eqcai2xnh5'] } : KEPT);
  useToasts.setState({ toasts: [], history: [], unread: 0 });
  window.localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  keep = null;
});

describe('a build without evidence', () => {
  it('offers no entry for a chart and opens nothing when asked anyway', () => {
    engine.methods = ['incidents.list', 'metrics.query_many', 'topology.export'];
    const screen = mount();
    expect(screen.can('chart')).toBe(false);
    expect(screen.can('topology')).toBe(false);
    expect(screen.chartEntries(CHART)).toEqual([]);
    act(() => screen.keep(CHART));
    expect(dialog()).toBeNull();
    expect(engine.calls).toEqual([]);
  });

  it('offers nothing while the method list has not arrived, rather than guessing', () => {
    engine.methods = undefined;
    const screen = mount();
    expect(screen.can('chart')).toBe(false);
    expect(screen.chartEntries(CHART)).toEqual([]);
  });

  it('offers each kind only where that kind can be kept', () => {
    engine.methods = ['evidence.capture_topology'];
    const screen = mount();
    expect(screen.can('topology')).toBe(true);
    expect(screen.can('chart')).toBe(false);
  });
});

describe('keeping a chart', () => {
  it('asks the engine to read the same series and period, and sends none of the numbers', async () => {
    const screen = mount();
    const [entry] = screen.chartEntries(CHART);
    expect(entry?.label).toBe('Keep as evidence…');
    act(() => entry?.onSelect());
    expect(dialog()?.textContent).toContain('The engine reads these samples again');

    act(() => button('Keep').click());
    await settled();

    expect(engine.calls).toEqual([
      {
        method: 'evidence.capture_chart',
        params: {
          // An empty instance is a device-level metric: it is not sent as one.
          series: [
            { device_id: '7u5ygdhiejg4mfx3', metric: 'cpu.pct' },
            { device_id: '7u5ygdhiejg4mfx3', metric: 'if.oper', instance: 'FastEthernet0/1' },
          ],
          since: 1795386021633,
          until: 1795386621633,
          step_s: 1,
        },
      },
    ]);
    const sent = JSON.stringify(engine.calls);
    for (const never of ['points', 'values', 'timestamps', 'body', 'png', 'text', 'redact']) {
      expect(sent, `"${never}" was sent`).not.toContain(`"${never}"`);
    }
    // The dialog closed, and the confirmation names the item and its hash.
    expect(dialog()).toBeNull();
    const [said] = useToasts.getState().toasts;
    expect(said?.title).toBe('Kept as evidence');
    expect(said?.description).toContain('cpu.pct, if.oper on Ops/SW1');
    expect(said?.description).toContain('9726cb34525c');
    expect(said?.action?.label).toBe('Open');
  });

  it('asks for the range at the moment the entry is chosen, not when the menu was built', () => {
    const screen = mount();
    let until = 1_000;
    const [entry] = screen.chartEntries(() => ({ ...CHART, until }));
    until = 2_000;
    act(() => entry?.onSelect());
    expect(dialog()).not.toBeNull();
    expect(captureCall({ ...CHART, until }, '').params).toMatchObject({ until: 2_000 });
  });

  it('sends a title only when one was typed, so the engine names the item otherwise', async () => {
    const screen = mount();
    act(() => screen.keep(CHART));
    const title = dialog()?.querySelector<HTMLInputElement>('input');
    if (!title) throw new Error('no title field');
    type(title, '  CPU while the uplink flapped  ');
    act(() => button('Keep').click());
    await settled();
    expect(engine.calls[0]?.params).toMatchObject({ title: 'CPU while the uplink flapped' });
    expect(captureCall(CHART, '   ').params).not.toHaveProperty('title');
  });

  it('attaches to the incident that was chosen, after the item is stored', async () => {
    window.localStorage.setItem('nocturne.ops.author', JSON.stringify('mo'));
    const screen = mount();
    act(() => screen.keep(CHART));
    const select = dialog()?.querySelector<HTMLSelectElement>('select');
    if (!select) throw new Error('no incident choice');
    expect([...select.options].map((option) => option.textContent)).toEqual(['Do not attach', 'SW1 uplink flapping']);
    choose(select, 'i74e62eqcai2xnh5');
    act(() => button('Keep and attach').click());
    await settled();

    expect(engine.calls.map((call) => call.method)).toEqual(['evidence.capture_chart', 'evidence.attach']);
    expect(engine.calls[1]?.params).toEqual({ id: 'vxyqjilvi777plur', incident_id: 'i74e62eqcai2xnh5', author: 'mo' });
    expect(useToasts.getState().toasts[0]?.title).toBe('Kept as evidence and attached');
  });
});

describe('an engine that says no', () => {
  it('shows its own reason for a capture it could not make, and claims nothing was kept', async () => {
    engine.answer = () =>
      new RpcError(
        'unavailable',
        'evidence cannot be captured from the topology: `topology.export` did not answer within 20000 ms',
      );
    const screen = mount();
    act(() => screen.keep({ kind: 'topology', view: 'default' }));
    act(() => button('Keep').click());
    await settled();

    const refused = dialog()?.querySelector('[role="alert"]');
    expect(refused?.textContent).toContain('The engine could not read it.');
    expect(refused?.textContent).toContain('`topology.export` did not answer within 20000 ms');
    // Still open, nothing announced as kept, nothing attached.
    expect(dialog()?.textContent).toContain('Keep as evidence');
    expect(useToasts.getState().toasts).toEqual([]);
    expect(engine.calls.map((call) => call.method)).toEqual(['evidence.capture_topology']);
  });

  it('says which limit was passed, in the engine’s sentence, which says what to do', () => {
    const answer = refusal(
      new RpcError(
        'invalid',
        'That is 20001 points and an evidence item holds at most 20000: ask for a larger `step_s`, a shorter period or fewer series. Nothing was stored.',
        { limit: 'points', max: 20000, actual: 20001 },
      ),
    );
    expect(answer.headline).toBe('Too many points for one item');
    expect(answer.message).toContain('ask for a larger `step_s`');
  });

  it('says the item is kept when only the attachment failed, and offers to try that again', async () => {
    engine.answer = (method) =>
      method === 'evidence.attach' ? new RpcError('not_found', 'incident `i74e62eqcai2xnh5` was not found') : KEPT;
    const screen = mount();
    act(() => screen.keep(CHART));
    const select = dialog()?.querySelector<HTMLSelectElement>('select');
    if (!select) throw new Error('no incident choice');
    choose(select, 'i74e62eqcai2xnh5');
    act(() => button('Keep and attach').click());
    await settled();

    expect(dialog()?.textContent).toContain('Kept, but not attached');
    expect(dialog()?.textContent).toContain('incident `i74e62eqcai2xnh5` was not found');
    expect(useToasts.getState().toasts).toEqual([]);

    // A second try does not capture a second item.
    engine.answer = () => ({ ...KEPT, incident_ids: ['i74e62eqcai2xnh5'] });
    act(() => button('Try attaching again').click());
    await settled();
    expect(engine.calls.map((call) => call.method)).toEqual([
      'evidence.capture_chart',
      'evidence.attach',
      'evidence.attach',
    ]);
    expect(dialog()).toBeNull();
  });
});

describe('what the dialog says will be read', () => {
  it('names the series, the devices and the method for a chart', () => {
    const facts = describeRequest(CHART);
    expect(facts.find((fact) => fact.label === 'Series')?.value).toBe(
      'cpu.pct, if.oper · FastEthernet0/1 (2 series on 1 device)',
    );
    expect(facts.find((fact) => fact.label === 'Read with')?.value).toBe('metrics.query_many');
  });

  it('sends a drawing’s graph as the screen has it, and nothing it did not set', () => {
    const call = captureCall({ kind: 'topology', view: 'default', filter: { site: 'Lab' }, include_endpoints: false }, '');
    expect(call).toEqual({
      method: 'evidence.capture_topology',
      params: { view: 'default', filter: { site: 'Lab' }, include_endpoints: false },
    });
    expect(captureCall({ kind: 'topology' }, '').params).toEqual({});
  });
});
