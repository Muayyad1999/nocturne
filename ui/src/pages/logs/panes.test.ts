import { describe, expect, it } from 'vitest';
import { capabilitiesOf, methodOf, paneFor, panesOf, type Capabilities, type PaneId } from './panes';

/** Every method this folder calls, as a full build registers them. */
const ALL = (Object.keys(capabilitiesOf([])) as (keyof Capabilities)[]).map(methodOf);

const PANE_METHOD: Record<PaneId, string> = {
  lines: 'logs.query',
  receivers: 'logs.status',
  rules: 'logs.rules.list',
  forwarding: 'logs.forward.list',
  flows: 'flows.top',
  detections: 'detections.list',
};

function ids(methods: readonly string[]): PaneId[] {
  return panesOf(capabilitiesOf(methods)).map((pane) => pane.id);
}

describe('which panes a build has', () => {
  it('offers all six, in the order a line travels, when every method is there', () => {
    expect(ids(ALL)).toEqual(['lines', 'receivers', 'rules', 'forwarding', 'flows', 'detections']);
  });

  it('has no Flows pane in a build without flows.*', () => {
    const without = ALL.filter((method) => !method.startsWith('flows.'));
    expect(ids(without)).not.toContain('flows');
    expect(ids(without)).toHaveLength(5);
  });

  it('has no Detections pane in a build without detections.*', () => {
    expect(ids(ALL.filter((method) => !method.startsWith('detections.')))).not.toContain('detections');
  });

  it('has no Forwarding pane in a build without logs.forward.*', () => {
    expect(ids(ALL.filter((method) => !method.startsWith('logs.forward.')))).not.toContain('forwarding');
  });

  it('offers a pane exactly when the method that fills it is registered, in every combination', () => {
    const panes = Object.keys(PANE_METHOD) as PaneId[];
    // All 64 subsets of the six methods that fill a pane.
    for (let mask = 0; mask < 1 << panes.length; mask += 1) {
      const present = panes.filter((_pane, index) => (mask & (1 << index)) !== 0);
      const offered = ids(present.map((pane) => PANE_METHOD[pane]));
      expect(offered.sort()).toEqual([...present].sort());
    }
  });

  it('never offers a pane on the strength of a neighbouring method', () => {
    // `logs.forward.save` without `logs.forward.list` is a build that cannot
    // list forwarders: there is no pane to put the button in.
    expect(ids(['logs.forward.save', 'logs.forward.test', 'flows.exporters', 'detections.recent'])).toEqual([]);
  });
});

describe('which controls a pane has', () => {
  it('ties every control to one method and to nothing else', () => {
    for (const key of Object.keys(capabilitiesOf([])) as (keyof Capabilities)[]) {
      const only = capabilitiesOf([methodOf(key)]);
      // Exactly the capabilities that stand for this method are on.
      for (const other of Object.keys(only) as (keyof Capabilities)[]) {
        expect(only[other], `${other} with only ${methodOf(key)}`).toBe(methodOf(other) === methodOf(key));
      }
    }
  });

  it('leaves no control behind whose method is missing', () => {
    for (const missing of ALL) {
      const capabilities = capabilitiesOf(ALL.filter((method) => method !== missing));
      for (const key of Object.keys(capabilities) as (keyof Capabilities)[]) {
        if (methodOf(key) === missing) expect(capabilities[key], `${key} without ${missing}`).toBe(false);
        else expect(capabilities[key], `${key} without ${missing}`).toBe(true);
      }
    }
  });

  it('lets a list be read where it cannot be changed', () => {
    const readOnly = capabilitiesOf(['logs.rules.list', 'logs.forward.list', 'detections.list']);
    expect(readOnly.rules).toBe(true);
    expect(readOnly.ruleSave || readOnly.ruleDelete || readOnly.ruleTest).toBe(false);
    expect(readOnly.forwarding).toBe(true);
    expect(readOnly.forwardSave || readOnly.forwardDelete || readOnly.forwardTest).toBe(false);
    expect(readOnly.detections).toBe(true);
    expect(readOnly.detectionSwitch || readOnly.detectionRecent).toBe(false);
  });
});

describe('which pane the address opens', () => {
  it('opens the pane the address names when this build has it', () => {
    expect(paneFor('flows', capabilitiesOf(ALL))).toBe('flows');
  });

  it('falls back to the first pane there is for a pane this build lacks', () => {
    const noFlows = capabilitiesOf(ALL.filter((method) => !method.startsWith('flows.')));
    expect(paneFor('flows', noFlows)).toBe('lines');
    expect(paneFor(null, noFlows)).toBe('lines');
    expect(paneFor('nonsense', noFlows)).toBe('lines');
  });

  it('opens what there is in a build that has detections and no log store', () => {
    expect(paneFor(null, capabilitiesOf(['detections.list']))).toBe('detections');
  });

  it('says there is nothing to open in a build with none of it', () => {
    expect(paneFor('lines', capabilitiesOf(['devices.list', 'settings.get']))).toBeNull();
  });
});
