import { describe, expect, it } from 'vitest';
import type { LabView } from '@/api/types/lab';
import type { SessionInfo } from '@/api/types/terminal';
import type { Node } from '@/api/types/topology';
import { sessionAction, sessionOffer, sessionOn, type SessionDevice } from './session';

function node(id: string, extra: Partial<Node> = {}): Node {
  return {
    id,
    kind: 'switch',
    name: 'AS1',
    lab: false,
    status: 'up',
    unmanaged: false,
    x: 0,
    y: 0,
    pinned: false,
    ...extra,
  };
}

function device(extra: Partial<SessionDevice> = {}): SessionDevice {
  return {
    id: 'dev-1',
    transport: 'ssh',
    address: '10.0.0.1',
    credential_id: 'cred-1',
    lab: null,
    ...extra,
  };
}

function lab(extra: Partial<LabView> = {}): LabView {
  return {
    id: 'lab-1',
    name: 'Campus',
    description: '',
    running: true,
    speed: 1,
    sim_time: 0,
    snapshot: {
      name: 'Campus',
      description: '',
      now: 0,
      nodes: [
        {
          id: 1,
          name: 'AS1',
          hostname: 'AS1',
          model: 'c2960',
          kind: 'switch',
          x: 0,
          y: 0,
          powered: true,
          cli: 'ios',
          interfaces: [],
        },
      ],
      links: [],
      notes: [],
    },
    devices: { AS1: 'dev-1' },
    unsaved_nodes: [],
    capture: false,
    ...extra,
  };
}

function session(extra: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: 's1',
    title: 'AS1',
    kind: 'ssh',
    device_id: 'dev-1',
    lab: false,
    opened_at: 0,
    state: 'open',
    cols: 120,
    rows: 40,
    info: { peer: '10.0.0.1:22' } as SessionInfo['info'],
    bytes_in: 0,
    bytes_out: 0,
    ...extra,
  };
}

describe('a node that cannot be given a session says why', () => {
  it('tells an unmanaged neighbour apart from a host and from a name somebody typed', () => {
    const neighbour = sessionOffer(node('u:1', { name: 'SW-CUPBOARD', unmanaged: true }), undefined, undefined);
    expect(neighbour.ok).toBe(false);
    expect(neighbour.ok === false && neighbour.why).toContain('not in the inventory');
    expect(neighbour.ok === false && neighbour.why).toContain('SW-CUPBOARD');
    // A menu item has one line, so every refusal has a few words as well as
    // a sentence; the words are what the right-click menu shows.
    expect(neighbour.ok === false && neighbour.brief).toBe('not a device');

    const host = sessionOffer(node('h:024e43d10901', { kind: 'host' }), undefined, undefined);
    expect(host.ok === false && host.why).toContain('learnt from a MAC table');

    const drawn = sessionOffer(node('m:1', { kind: 'cloud', name: 'ISP uplink' }), undefined, undefined);
    expect(drawn.ok === false && drawn.why).toContain('somebody typed');
  });

  it('refuses a device whose device type has no command line', () => {
    const offer = sessionOffer(node('n:1', { device_id: 'dev-1' }), device({ transport: 'none' }), undefined);
    expect(offer.ok === false && offer.why).toContain('“none”');
  });

  it('refuses a device with no address and one with no credential, differently', () => {
    const nowhere = sessionOffer(node('n:1', { device_id: 'dev-1' }), device({ address: '  ' }), undefined);
    expect(nowhere.ok === false && nowhere.why).toContain('no address');

    const anonymous = sessionOffer(
      node('n:1', { device_id: 'dev-1' }),
      device({ credential_id: null }),
      undefined,
    );
    expect(anonymous.ok === false && anonymous.why).toContain('No credential');
    expect(anonymous.ok === false && anonymous.brief).toBe('no credential');
  });

  it('says the inventory has lost the device rather than offering a dead button', () => {
    const offer = sessionOffer(node('n:1', { device_id: 'dev-1' }), undefined, undefined);
    expect(offer.ok === false && offer.why).toContain('no longer has this device');
  });
});

describe('a lab device is a device, with the lab’s own two refusals', () => {
  const labNode = node('n:1', { device_id: 'dev-1', lab: true });
  const labDevice = device({ transport: 'sim', address: '', credential_id: null, lab: { lab_id: 'lab-1', node: 'AS1' } });

  it('needs no credential and no address, because the simulator is the far end', () => {
    expect(sessionOffer(labNode, labDevice, lab())).toEqual({ ok: true, deviceId: 'dev-1' });
  });

  it('refuses a node that is powered off, and names it', () => {
    const off = lab({
      snapshot: {
        ...lab().snapshot,
        nodes: [{ ...lab().snapshot.nodes[0]!, powered: false }],
      },
    });
    const offer = sessionOffer(labNode, labDevice, off);
    expect(offer.ok).toBe(false);
    expect(offer.ok === false && offer.why).toContain('powered off');
    expect(offer.ok === false && offer.why).toContain('AS1');
    expect(offer.ok === false && offer.brief).toBe('powered off');
  });

  it('refuses when the lab is not running, and says to start it', () => {
    const offer = sessionOffer(labNode, labDevice, lab({ running: false }));
    expect(offer.ok === false && offer.why).toContain('not running');
    expect(offer.ok === false && offer.why).toContain('Start the lab');
  });

  it('does not refuse merely because the lab feature did not answer', () => {
    // The lab's state is unknown here, and a guess that hid the button would
    // be worse than letting `terminal.open` have the last word.
    expect(sessionOffer(labNode, labDevice, undefined).ok).toBe(true);
  });
});

describe('the session a device already has', () => {
  it('lands in the open one rather than dialling the device twice', () => {
    const sessions = [session({ id: 'old', state: 'closed' }), session({ id: 'live' })];
    expect(sessionOn(sessions, 'dev-1')?.id).toBe('live');
    expect(sessionAction(sessionOn(sessions, 'dev-1'))).toBe('show');
  });

  it('still finds a closed session, because the scrollback is there', () => {
    const closed = [session({ state: 'closed' })];
    expect(sessionOn(closed, 'dev-1')?.id).toBe('s1');
    // …but the button offers a new one, since a closed session takes no keys.
    expect(sessionAction(sessionOn(closed, 'dev-1'))).toBe('open');
  });

  it('is nothing for a device with no session, and for no answer at all', () => {
    expect(sessionOn([session()], 'dev-2')).toBeNull();
    expect(sessionOn(undefined, 'dev-1')).toBeNull();
    expect(sessionAction(null)).toBe('open');
  });
});

describe('a press while the first login is still connecting', () => {
  it('waits for the login that is on its way instead of dialling the device again', () => {
    // `terminal.open` answers only once the device has been dialled, and
    // until then `terminal.list` knows of no session: going by the list
    // alone, a second press three seconds into an SSH login is a first one.
    expect(sessionAction(null, true)).toBe('wait');
    // The same for a device whose only session is a closed one.
    expect(sessionAction(session({ state: 'closed' }), true)).toBe('wait');
  });

  it('goes to the session as soon as there is one, whatever is still in flight', () => {
    expect(sessionAction(session(), true)).toBe('show');
  });

  it('dials when nothing is on its way', () => {
    expect(sessionAction(null, false)).toBe('open');
  });
});
