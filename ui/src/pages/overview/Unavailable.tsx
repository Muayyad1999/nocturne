/**
 * How this screen says "there is no number here", everywhere it has to.
 *
 * One component, so a missing alerting feature reads the same in a tile, a
 * panel and a list — and so it can never be mistaken for a zero. The reason
 * gets a headline of its own ("Not in this build" is not "Could not be read")
 * and the backend's own sentence underneath, quoted rather than paraphrased:
 * only the feature that failed knows why it failed.
 */

import { CircleDashed, PlugZap, TriangleAlert } from 'lucide-react';
import type { UnavailableReason } from '@/api/types/ops';
import { Icon, type LucideIcon } from '@/design/Icon';
import { REASON_HEADLINE, REASON_TONE } from './overview';
import styles from './Unavailable.module.css';

const ICON: Record<UnavailableReason, LucideIcon> = {
  not_installed: PlugZap,
  failed: TriangleAlert,
  timeout: TriangleAlert,
  malformed: TriangleAlert,
  no_data: CircleDashed,
};

export interface UnavailableNoteProps {
  reason: UnavailableReason;
  /** The backend's sentence for the person at the keyboard. */
  message: string;
  /** `tile` is terse, for the top row; `block` fills a panel. */
  size?: 'tile' | 'block';
}

export function UnavailableNote({ reason, message, size = 'tile' }: UnavailableNoteProps) {
  return (
    <div className={styles.note} data-tone={REASON_TONE[reason]} data-size={size}>
      <span className={styles.headline}>
        <Icon icon={ICON[reason]} size={size === 'tile' ? 13 : 15} />
        {REASON_HEADLINE[reason]}
      </span>
      <p className={styles.message}>{message}</p>
    </div>
  );
}
