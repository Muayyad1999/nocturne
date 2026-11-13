/**
 * Count badges on nav items: open alerts on Monitoring, pending approvals on
 * Changes. A slice sets its own count from real data; no count, no badge.
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { Tone } from '@/design/tone';

export interface NavBadge {
  count: number;
  /** Defaults to `accent`. Use a status tone when the count is a problem (`crit` for open critical alerts). */
  tone?: Tone;
  /** What is being counted, for the tooltip and screen readers: "open alerts". */
  label: string;
}

interface NavBadgesState {
  badges: Record<string, NavBadge>;
}

export const useNavBadges = create<NavBadgesState>(() => ({ badges: {} }));

/** Sets or clears (`null`, or a count of zero) the badge of a destination by id. */
export function setNavBadge(destinationId: string, badge: NavBadge | null): void {
  useNavBadges.setState((state) => {
    const { [destinationId]: _previous, ...rest } = state.badges;
    return { badges: badge && badge.count > 0 ? { ...rest, [destinationId]: badge } : rest };
  });
}

/** Shows a badge on a destination while the component is mounted. */
export function useNavBadge(destinationId: string, badge: NavBadge | null): void {
  const count = badge?.count ?? 0;
  const tone = badge?.tone;
  const label = badge?.label ?? '';
  useEffect(() => {
    setNavBadge(destinationId, count > 0 ? { count, tone, label } : null);
    return () => setNavBadge(destinationId, null);
  }, [destinationId, count, tone, label]);
}
