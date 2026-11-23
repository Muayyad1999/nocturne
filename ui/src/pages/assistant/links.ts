/**
 * Where a tool step's link goes, and what it is called.
 *
 * The engine leaves a link out when what a feature gave as an id is not one
 * (`AiLink`), and then the step shows no link at all. The same holds here
 * for a link that has nowhere to go in this build: a step never shows
 * something worded and coloured as a link that cannot be followed. A job the
 * build has no Jobs view for is named as a fact of the step instead
 * (`jobFact`), so its id is still there to be read.
 */

import type { AiLink } from '@/api/types/ai';

export interface LinkTarget {
  to: string;
  label: string;
}

/**
 * What `/ops/jobs?job=<id>` needs to open a job: the operations screen is
 * only there with its own feature, and its Jobs tab with the core's list.
 */
export const JOBS_VIEW: readonly string[] = ['incidents.list', 'jobs.list'];

/**
 * The route a link opens, or `null` when there is none: no link, a job in a
 * build without the Jobs view, or a kind of link this interface does not
 * know. Ids are encoded, so one that holds a `/`, `?` or `#` stays one id
 * and cannot reach another route.
 */
export function linkTarget(
  link: AiLink | undefined,
  methods: readonly string[] | undefined,
): LinkTarget | null {
  if (!link) return null;
  const id = encodeURIComponent(link.id);
  switch (link.type) {
    case 'device':
      return { to: `/devices/${id}`, label: 'Open the device' };
    case 'change':
      return { to: `/changes/${id}`, label: 'Review the change' };
    case 'lab':
      return { to: `/lab/${id}`, label: 'Open the lab' };
    case 'alert':
      // Alerts have no page of their own: the list is where this one is.
      return { to: '/monitoring/alerts', label: 'Open the alerts' };
    case 'job':
      return methods !== undefined && JOBS_VIEW.every((method) => methods.includes(method))
        ? { to: `/ops/jobs?job=${id}`, label: 'Open the job' }
        : null;
    default:
      return null;
  }
}

/** The job a step started, when it cannot be linked to: shown as its id. */
export function jobFact(
  link: AiLink | undefined,
  methods: readonly string[] | undefined,
): string | null {
  if (link?.type !== 'job') return null;
  return linkTarget(link, methods) ? null : link.id;
}
