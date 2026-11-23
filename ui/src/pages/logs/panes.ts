/**
 * What this build can do, decided once from `app.methods`.
 *
 * The Logs tab is six panes over five engine pieces, and a build is free to
 * have any subset of them: `flows.*` and `logs.forward.*` are registered by
 * the logs crate today and `detections.*` by another, but the screen does
 * not get to assume which crate a method lives in. So every pane, card and
 * control here asks one question — is the method it calls in the list — and
 * a control whose method is missing is not rendered at all. Nothing is left
 * on screen that would answer `unknown_method` when clicked.
 *
 * It is a pure function over the method list so that "no dead control in any
 * combination" can be tested as what it is: a statement about combinations.
 */

export type PaneId = 'lines' | 'receivers' | 'rules' | 'forwarding' | 'flows' | 'detections';

export interface Capabilities {
  // ---- the panes: each exists only when the method that fills it does
  /** `logs.query`: the search, the list and everything else of this tab. */
  lines: boolean;
  /** `logs.status`: the receiver cards. */
  receivers: boolean;
  /** `logs.rules.list`. */
  rules: boolean;
  /** `logs.forward.list`. */
  forwarding: boolean;
  /** `flows.top`. */
  flows: boolean;
  /** `detections.list`. */
  detections: boolean;

  // ---- controls inside a pane
  histogram: boolean;
  follow: boolean;
  sources: boolean;
  pull: boolean;
  deviceSetup: boolean;
  /** `settings.get`: the ports a receiver would bind while it is off. */
  settingsRead: boolean;
  /** `settings.set`: the switches and the ports. */
  settingsWrite: boolean;
  restart: boolean;
  ruleSave: boolean;
  ruleDelete: boolean;
  ruleTest: boolean;
  forwardSave: boolean;
  forwardDelete: boolean;
  forwardTest: boolean;
  exporters: boolean;
  detectionSwitch: boolean;
  detectionRecent: boolean;
  /** `devices.list`: names for ids, and the pickers. */
  devices: boolean;
}

const METHOD: Record<keyof Capabilities, string> = {
  lines: 'logs.query',
  receivers: 'logs.status',
  rules: 'logs.rules.list',
  forwarding: 'logs.forward.list',
  flows: 'flows.top',
  detections: 'detections.list',
  histogram: 'logs.histogram',
  follow: 'logs.follow',
  sources: 'logs.sources',
  pull: 'logs.pull',
  deviceSetup: 'logs.device_setup',
  settingsRead: 'settings.get',
  settingsWrite: 'settings.set',
  restart: 'logs.restart',
  ruleSave: 'logs.rules.save',
  ruleDelete: 'logs.rules.delete',
  ruleTest: 'logs.rules.test',
  forwardSave: 'logs.forward.save',
  forwardDelete: 'logs.forward.delete',
  forwardTest: 'logs.forward.test',
  exporters: 'flows.exporters',
  detectionSwitch: 'detections.set_enabled',
  detectionRecent: 'detections.recent',
  devices: 'devices.list',
};

/** The method each capability stands for, for a test and for a tooltip. */
export function methodOf(capability: keyof Capabilities): string {
  return METHOD[capability];
}

/** What a build with exactly these methods can do. */
export function capabilitiesOf(methods: Iterable<string>): Capabilities {
  const present = new Set(methods);
  const capabilities = {} as Capabilities;
  for (const key of Object.keys(METHOD) as (keyof Capabilities)[]) {
    capabilities[key] = present.has(METHOD[key]);
  }
  return capabilities;
}

export interface Pane {
  id: PaneId;
  label: string;
}

/**
 * The order is the order a line travels: it is received, read, judged by a
 * rule, sent on — and then the two things that are not lines at all.
 */
const PANES: readonly Pane[] = [
  { id: 'lines', label: 'Lines' },
  { id: 'receivers', label: 'Receivers' },
  { id: 'rules', label: 'Rules' },
  { id: 'forwarding', label: 'Forwarding' },
  { id: 'flows', label: 'Flows' },
  { id: 'detections', label: 'Detections' },
];

/** The panes this build has, in order. A pane whose method is absent is not offered. */
export function panesOf(capabilities: Capabilities): Pane[] {
  return PANES.filter((pane) => capabilities[pane.id]);
}

/**
 * The pane to show for what the address asks for: the one it names when this
 * build has it, otherwise the first one there is. `null` when the build has
 * none of them, which is "this feature is not in this build".
 */
export function paneFor(requested: string | null, capabilities: Capabilities): PaneId | null {
  const available = panesOf(capabilities);
  return (
    available.find((pane) => pane.id === requested)?.id ?? available[0]?.id ?? null
  );
}
