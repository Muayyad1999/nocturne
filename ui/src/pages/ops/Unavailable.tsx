/**
 * How this screen says "there is nothing here", in its three different
 * meanings, so that they can never be mistaken for one another.
 *
 * {@link UnavailableNote} is a section that *was not read*: the feature is not
 * in this build, it failed, it timed out, or it has nothing recorded yet. It
 * shows a headline per reason and the backend's own sentence underneath,
 * quoted rather than paraphrased, because only the feature that failed knows
 * why it failed.
 *
 * {@link NothingHappened} is the opposite case and must not look like it: the
 * section *was* read, and what it found was nothing. It gets the section's own
 * sentence ("No backup failed."), which is good news and reads like it.
 *
 * The pattern is the Overview's `Unavailable.tsx`, and this is deliberately a
 * second copy rather than an import: a page folder owns its components, and
 * the two screens are being built in parallel. If a third screen needs it, it
 * belongs in `design/`.
 */

import { CircleCheck, CircleDashed, PlugZap, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import type { UnavailableReason } from '@/api/types/ops';
import { Icon, type LucideIcon } from '@/design/Icon';
import { REASON_HEADLINE, REASON_TONE } from './ops';
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
  /** `inline` fits a row or a panel header; `block` fills a panel body. */
  size?: 'inline' | 'block';
  /** What this would have been, when the heading does not already say. */
  about?: string;
}

export function UnavailableNote({ reason, message, size = 'block', about }: UnavailableNoteProps) {
  return (
    <div className={styles.note} data-tone={REASON_TONE[reason]} data-size={size} role="status">
      <span className={styles.headline}>
        <Icon icon={ICON[reason]} size={size === 'inline' ? 13 : 15} />
        {about ? `${about}: ${REASON_HEADLINE[reason].toLowerCase()}` : REASON_HEADLINE[reason]}
      </span>
      <p className={styles.message}>{message}</p>
    </div>
  );
}

export interface NothingHappenedProps {
  /** The sentence that says what was looked at and found to be empty. */
  sentence: ReactNode;
  /** The headline. "Nothing happened" unless the section can be more exact. */
  headline?: string;
  size?: 'inline' | 'block';
}

/** A section that was read and had nothing in it. Not a missing section. */
export function NothingHappened({
  sentence,
  headline = 'Nothing happened',
  size = 'block',
}: NothingHappenedProps) {
  return (
    <div className={styles.note} data-tone="ok" data-size={size}>
      <span className={styles.headline}>
        <Icon icon={CircleCheck} size={size === 'inline' ? 13 : 15} />
        {headline}
      </span>
      <p className={styles.message}>{sentence}</p>
    </div>
  );
}
