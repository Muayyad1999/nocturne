import { describe, expect, it } from 'vitest';
import { DESTINATIONS, destination, visibleDestinations } from './destinations';

describe('the destinations', () => {
  it('has a unique id and a path for each', () => {
    const ids = DESTINATIONS.map((entry) => entry.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const entry of DESTINATIONS) {
      expect(entry.path.startsWith('/'), entry.id).toBe(true);
      expect(entry.label.length, entry.id).toBeGreaterThan(2);
    }
    expect(destination('devices').path).toBe('/devices');
    expect(() => destination('nothing')).toThrow();
  });
});

describe('a destination a build cannot reach', () => {
  it('is left out when the build does not answer its method', () => {
    const free = visibleDestinations(['devices.list', 'app.methods']);
    expect(free.map((entry) => entry.id)).not.toContain('isp');
    // Everything that needs nothing is still there.
    expect(free.map((entry) => entry.id)).toContain('devices');
    expect(free).toHaveLength(DESTINATIONS.filter((entry) => entry.requires === undefined).length);
  });

  it('is there once the build answers it', () => {
    const paid = visibleDestinations(['isp.customers']);
    expect(paid.map((entry) => entry.id)).toContain('isp');
  });

  it('is left out while `app.methods` has not answered yet', () => {
    // A paid feature must not appear for a moment in the free edition's rail,
    // so `undefined` hides it rather than assuming it is there.
    expect(visibleDestinations(undefined).map((entry) => entry.id)).not.toContain('isp');
  });
});
