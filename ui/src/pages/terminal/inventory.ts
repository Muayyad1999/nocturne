/**
 * The little of the inventory this screen reads.
 *
 * `devices.list` belongs to the core `inventory` module, which has no
 * `api/types` file yet (its author writes it). Only the fields the device
 * list and the session headers use are declared here, so nothing is invented
 * and nothing breaks when the rest arrives.
 */

export type DeviceTransport = 'ssh' | 'telnet' | 'serial' | 'none' | (string & {});

export interface InventoryDevice {
  id: string;
  name: string;
  address: string;
  transport: DeviceTransport;
  /** The device-type profile, `''` while it has not been detected. */
  profile: string;
  site: string;
  role: string;
  tags: string[];
  /** Set when the device is a node in a simulated lab. */
  lab: { lab_id: string; node: string } | null;
  state?: {
    reachable: boolean | null;
    last_seen: number | null;
    last_error: string | null;
  } | null;
}

/** The status dot a device gets in the list: what is known, never a guess. */
export function deviceTone(device: InventoryDevice): 'ok' | 'crit' | 'idle' | 'maint' {
  if (device.lab) return 'maint';
  if (device.state?.reachable === true) return 'ok';
  if (device.state?.reachable === false) return 'crit';
  return 'idle';
}

export function deviceStateLabel(device: InventoryDevice): string {
  if (device.state?.reachable === true) return 'Reachable';
  if (device.state?.reachable === false) return device.state.last_error || 'Not reachable';
  return 'Not checked';
}
