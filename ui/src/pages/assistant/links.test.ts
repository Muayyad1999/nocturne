import { describe, expect, it } from 'vitest';
import { jobFact, linkTarget } from './links';

const WITH_JOBS = ['devices.list', 'incidents.list', 'jobs.list'];
const WITHOUT_OPS = ['devices.list', 'jobs.list'];

describe('linkTarget', () => {
  it('a step whose link the engine left out has nothing to follow', () => {
    expect(linkTarget(undefined, WITH_JOBS)).toBeNull();
    expect(jobFact(undefined, WITH_JOBS)).toBeNull();
  });

  it('opens the device, the change and the lab a step named', () => {
    expect(linkTarget({ type: 'device', id: '5uedsz2uxxh7moam' }, WITH_JOBS)).toEqual({
      to: '/devices/5uedsz2uxxh7moam',
      label: 'Open the device',
    });
    expect(linkTarget({ type: 'change', id: 'c1' }, WITH_JOBS)?.to).toBe('/changes/c1');
    expect(linkTarget({ type: 'lab', id: 'l1' }, WITH_JOBS)?.to).toBe('/lab/l1');
  });

  it('an alert goes to the alerts list and says so, since an alert has no page', () => {
    expect(linkTarget({ type: 'alert', id: 'yi3yo246t6q665jk' }, WITH_JOBS)).toEqual({
      to: '/monitoring/alerts',
      label: 'Open the alerts',
    });
  });

  it('an id that holds route syntax stays one id', () => {
    expect(linkTarget({ type: 'device', id: '../settings?tab=x#y' }, WITH_JOBS)?.to).toBe(
      '/devices/..%2Fsettings%3Ftab%3Dx%23y',
    );
  });

  it('a job opens the Jobs view where the build has one, and is a fact where it has not', () => {
    const job = { type: 'job', id: 'x4clm3otkqk45ykz' } as const;
    expect(linkTarget(job, WITH_JOBS)).toEqual({
      to: '/ops/jobs?job=x4clm3otkqk45ykz',
      label: 'Open the job',
    });
    expect(jobFact(job, WITH_JOBS)).toBeNull();

    expect(linkTarget(job, WITHOUT_OPS)).toBeNull();
    expect(jobFact(job, WITHOUT_OPS)).toBe('x4clm3otkqk45ykz');

    // Not known yet is not "known to be absent": nothing is drawn as a link.
    expect(linkTarget(job, undefined)).toBeNull();
  });

  it('a kind of link this interface does not know is not drawn as one', () => {
    const unknown = { type: 'incident', id: 'i1' } as unknown as Parameters<typeof linkTarget>[0];
    expect(linkTarget(unknown, WITH_JOBS)).toBeNull();
    expect(jobFact(unknown, WITH_JOBS)).toBeNull();
  });
});
