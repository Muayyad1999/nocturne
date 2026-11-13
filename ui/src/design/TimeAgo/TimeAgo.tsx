import type { ComponentProps } from 'react';
import { EMPTY } from '@/lib/format';
import { formatRelative, formatTimestamp, relativeRefreshInterval } from '@/lib/time';
import { useNow } from '@/lib/useNow';
import { Tooltip } from '../Tooltip';

export interface TimeAgoProps extends Omit<ComponentProps<'time'>, 'children' | 'dateTime'> {
  /** The instant, in milliseconds since the epoch. Missing values show a dash. */
  at: number | null | undefined;
}

/**
 * A relative time that keeps itself current ("5m ago"), with the exact local
 * and UTC time in a tooltip for comparing against device logs.
 */
export function TimeAgo({ at, ...rest }: TimeAgoProps) {
  const known = at != null && Number.isFinite(at);
  // Refresh as often as the label can change: every second at first, then less.
  const now = useNow(known ? relativeRefreshInterval(at, Date.now()) : 60_000);

  if (!known) return <span {...rest}>{EMPTY}</span>;
  return (
    <Tooltip content={formatTimestamp(at)} describe={false}>
      <time {...rest} dateTime={new Date(at).toISOString()}>
        {formatRelative(at, now)}
      </time>
    </Tooltip>
  );
}
