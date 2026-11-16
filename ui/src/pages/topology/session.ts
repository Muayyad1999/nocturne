/**
 * Whether a node on the map can be given a terminal, and why not when it
 * cannot.
 *
 * The Dude's most-used action is a right-click and a command line. The
 * session itself is `nocturne-terminal`'s — `terminal.open` and the xterm
 * registry the Terminal screen owns (`pages/terminal/sessions.ts`) — and
 * nothing here opens anything. What is here is the question the map has to
 * answer *before* it offers the action: a button that fails when pressed is
 * worse than a button that is not there, and a button that is simply missing
 * teaches nobody anything. So every refusal is a sentence that says what to
 * do about it.
 *
 * The refusals it can know about from the inventory alone: a node that is not
 * a device, a device whose device type has no command line, one with no
 * address to dial, and one with no credential to log in with. Lab devices are
 * the exception on credentials — the simulator needs none — and have two
 * refusals of their own, which need the lab feature's own answer: a lab that
 * is not running, and a node that is powered off.
 */

import type { Device } from '@/api/types/core';
import type { LabView } from '@/api/types/lab';
import type { SessionInfo } from '@/api/types/terminal';
import type { Node } from '@/api/types/topology';

export type SessionOffer =
  | { ok: true; deviceId: string }
  /**
   * `why` is the sentence shown where the button would be; `brief` is the
   * same refusal in three or four words, for the one line a menu item has.
   */
  | { ok: false; why: string; brief: string };

/** What the inventory says about a device, as far as a session is concerned. */
export type SessionDevice = Pick<Device, 'id' | 'transport' | 'address' | 'credential_id' | 'lab'>;

/**
 * Whether this node can be given a session. `device` is the inventory row for
 * `node.device_id`, `lab` the lab the device belongs to when the lab feature
 * answered about it. Both may be absent, and absence is never read as a
 * refusal: the backend has the last word, and a guess that hides the button
 * would be worse than letting `terminal.open` say so itself.
 */
export function sessionOffer(
  node: Pick<Node, 'id' | 'name' | 'device_id' | 'unmanaged'>,
  device: SessionDevice | undefined,
  lab: LabView | undefined,
): SessionOffer {
  if (!node.device_id) return { ok: false, why: notADevice(node), brief: 'not a device' };
  if (!device) {
    return {
      ok: false,
      why: 'The inventory no longer has this device, so there is nothing to log in to.',
      brief: 'no such device',
    };
  }

  if (device.transport === 'none') {
    return {
      ok: false,
      why: 'This device has no command line: its transport is set to “none”. Set SSH, Telnet or serial on the device to log in.',
      brief: 'no command line',
    };
  }

  if (device.lab) {
    // A lab device's console is the simulator's, so it needs no credential
    // and no address — but the lab has to be running and the node powered.
    if (lab && !lab.running) {
      return {
        ok: false,
        why: `The lab “${lab.name}” is not running, so this device has no console. Start the lab and it comes back.`,
        brief: 'the lab is not running',
      };
    }
    const member = lab?.snapshot.nodes.find((entry) => entry.name === device.lab?.node);
    if (member && !member.powered) {
      return {
        ok: false,
        why: `${member.name} is powered off in the lab, so there is nothing listening. Power it on and it comes back.`,
        brief: 'powered off',
      };
    }
    if (lab && !member) {
      return {
        ok: false,
        why: `The lab “${lab.name}” no longer has a node called ${device.lab.node}.`,
        brief: 'gone from the lab',
      };
    }
    if (lab && lab.devices[device.lab.node] === undefined) {
      return {
        ok: false,
        why: `${device.lab.node} is a lab node with no console of its own.`,
        brief: 'no console',
      };
    }
    return { ok: true, deviceId: device.id };
  }

  if (device.transport !== 'serial' && device.address.trim() === '') {
    return {
      ok: false,
      why: 'This device has no address to connect to. Give it one on the device page.',
      brief: 'no address',
    };
  }
  if (!device.credential_id) {
    return {
      ok: false,
      why: 'No credential is set on this device, so there is nothing to log in with. Give it one on the device page.',
      brief: 'no credential',
    };
  }
  return { ok: true, deviceId: device.id };
}

/** Why a node that is not a device cannot be logged in to. Its id says which kind it is. */
function notADevice(node: Pick<Node, 'id' | 'name'>): string {
  if (node.id.startsWith('h:')) {
    return 'This is a host learnt from a MAC table, not a device Nocturne manages. There is no session to open on it.';
  }
  if (node.id.startsWith('m:')) {
    return 'This end of a link is a name somebody typed, not a device. There is nothing to log in to.';
  }
  return `A neighbour protocol reported ${node.name}, but it is not in the inventory. Add it as a device and give it a credential, and a terminal becomes possible.`;
}

/**
 * The session this device already has, if any, so a second click lands in the
 * one that is open instead of dialling the device again. A closed session is
 * no use for typing into, but it is still the place this device's scrollback
 * is, so it counts.
 */
export function sessionOn(
  sessions: readonly SessionInfo[] | undefined,
  deviceId: string,
): SessionInfo | null {
  const mine = (sessions ?? []).filter((session) => session.device_id === deviceId);
  return mine.find((session) => session.state !== 'closed') ?? mine[0] ?? null;
}

/** What the button says: opening one, or going back to the one that is open. */
export function sessionAction(existing: SessionInfo | null): 'open' | 'show' {
  return existing && existing.state !== 'closed' ? 'show' : 'open';
}
