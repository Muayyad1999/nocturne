/**
 * Which devices something runs on — as a filter, not a list.
 *
 * The backend resolves a selector when the run starts, so "every access
 * switch in HQ" keeps meaning that as switches are racked and
 * decommissioned. The screen therefore edits a *filter* and asks
 * `tasks.devices` what it means right now; it never quietly turns one into a
 * list of ids behind the user's back.
 */

import type { Filter, Selector } from '@/api/types/automation';
import { formatCount } from '@/lib/format';
import { given } from './given';

export type LabChoice = 'any' | 'only' | 'none';
export type StatusChoice = '' | 'up' | 'down' | 'unknown';

/** The filter as a form holds it: strings, because that is what inputs give. */
export interface SelectionDraft {
  /** Devices named one by one, by id. */
  deviceIds: string[];
  query: string;
  site: string;
  role: string;
  tag: string;
  profile: string;
  status: StatusChoice;
  lab: LabChoice;
  /** A cap, as typed; empty means no cap. */
  limit: string;
  excludeIds: string[];
}

export const EMPTY_SELECTION: SelectionDraft = {
  deviceIds: [],
  query: '',
  site: '',
  role: '',
  tag: '',
  profile: '',
  status: '',
  lab: 'any',
  limit: '',
  excludeIds: [],
};

/** True when the draft asks for nothing in particular: every device matches. */
export function filterIsOpen(draft: SelectionDraft): boolean {
  return (
    draft.query.trim() === '' &&
    draft.site === '' &&
    draft.role === '' &&
    draft.tag === '' &&
    draft.profile === '' &&
    draft.status === '' &&
    draft.lab === 'any' &&
    draft.limit.trim() === ''
  );
}

/**
 * The draft as the backend wants it. A filter is only sent when something is
 * set in it or no device was named by hand, because a filter with no criteria
 * means "all of them" — which is a real answer, not an empty one.
 */
export function toSelector(draft: SelectionDraft): Selector {
  const filter: Filter = {};
  if (draft.query.trim() !== '') filter.query = draft.query.trim();
  if (draft.site !== '') filter.site = draft.site;
  if (draft.role !== '') filter.role = draft.role;
  if (draft.tag !== '') filter.tag = draft.tag;
  if (draft.profile !== '') filter.profile = draft.profile;
  if (draft.status !== '') filter.status = draft.status;
  if (draft.lab !== 'any') filter.lab = draft.lab === 'only';
  const limit = Number.parseInt(draft.limit, 10);
  if (Number.isFinite(limit) && limit > 0) filter.limit = limit;

  const selector: Selector = {};
  if (draft.deviceIds.length > 0) selector.device_ids = [...draft.deviceIds];
  // With devices named by hand and nothing set in the filter, sending the
  // filter would add the whole inventory to them.
  if (Object.keys(filter).length > 0 || draft.deviceIds.length === 0) selector.filter = filter;
  if (draft.excludeIds.length > 0) selector.exclude_device_ids = [...draft.excludeIds];
  return selector;
}

/** A stored selector back into a form. */
export function fromSelector(selector: Selector | undefined): SelectionDraft {
  const filter = selector?.filter ?? {};
  const status = filter.status ?? '';
  return {
    deviceIds: [...(selector?.device_ids ?? [])],
    query: filter.query ?? '',
    site: filter.site ?? '',
    role: filter.role ?? '',
    tag: filter.tag ?? '',
    profile: filter.profile ?? '',
    status: status === 'up' || status === 'down' || status === 'unknown' ? status : '',
    lab: !given(filter.lab) ? 'any' : filter.lab ? 'only' : 'none',
    limit: given(filter.limit) ? String(filter.limit) : '',
    excludeIds: [...(selector?.exclude_device_ids ?? [])],
  };
}

/**
 * The selection in words, for a row in a list and for the sentence above the
 * device preview. It describes the *filter*, never the devices it happens to
 * match today.
 */
export function describeSelection(selector: Selector | undefined): string {
  const named = selector?.device_ids?.length ?? 0;
  const filter = selector?.filter;
  const criteria: string[] = [];
  if (filter?.query) criteria.push(`matching “${filter.query}”`);
  if (filter?.site) criteria.push(`in ${filter.site}`);
  if (filter?.role) criteria.push(`with the role ${filter.role}`);
  if (filter?.tag) criteria.push(`tagged ${filter.tag}`);
  if (filter?.tags?.length) criteria.push(`tagged ${filter.tags.join(' + ')}`);
  if (filter?.profile) criteria.push(`of type ${filter.profile}`);
  if (filter?.status) criteria.push(`last seen ${filter.status}`);
  if (filter?.protected === true) criteria.push('protected');
  if (filter?.protected === false) criteria.push('not protected');

  const parts: string[] = [];
  if (named > 0) parts.push(named === 1 ? '1 named device' : `${formatCount(named)} named devices`);
  if (given(filter)) {
    if (filter.lab === true) parts.push(criteria.length > 0 ? `lab devices ${criteria.join(', ')}` : 'every lab device');
    else if (filter.lab === false)
      parts.push(criteria.length > 0 ? `real devices ${criteria.join(', ')}` : 'every real device');
    else parts.push(criteria.length > 0 ? `every device ${criteria.join(', ')}` : 'every device');
  }
  const excluded = selector?.exclude_device_ids?.length ?? 0;
  let text = parts.join(', plus ') || 'nothing';
  if (filter?.limit) text += `, at most ${formatCount(filter.limit)}`;
  if (excluded > 0) text += `, less ${formatCount(excluded)} excluded`;
  return text.charAt(0).toUpperCase() + text.slice(1);
}
