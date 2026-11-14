/**
 * How this screen says "there is nothing here", everywhere it has to.
 *
 * Four different absences live on the Tools screen and they must not read
 * alike, because the reader's next move is different in each case:
 *
 * * the toolbox crate is not in this build — nothing to do here at all;
 * * the tool exists in the engine but this screen has no form for it — the
 *   assistant and the RPC surface can still reach it;
 * * the tool needs the desktop app (the two servers are refused when Nocturne
 *   runs as a server, because they bind a socket on the machine);
 * * a probe could not run — which is {@link ProbeFailure}, the one that
 *   matters most, because the alternative is an empty chart that looks like a
 *   result.
 *
 * `ProbeFailure` never draws a figure. There is no round-trip time, no loss
 * percentage and no "0" anywhere in it: a probe that could not run produced no
 * measurement, and the headline says whether it was this machine that refused
 * (`forbidden`) or the network that stayed silent (`timeout`). The engine's own
 * sentence is quoted underneath, because only the engine knows why.
 */

import { Ban, CircleDashed, Link2, Monitor, PlugZap, TriangleAlert } from 'lucide-react';
import { Icon, type LucideIcon } from '@/design/Icon';
import { failureWords, nothingWasSent, type Failure } from './probes';
import styles from './Tools.module.css';

export type AbsenceReason = 'not_in_build' | 'no_form' | 'needs_desktop' | 'nothing_yet' | 'attached';

const ABSENCE: Record<AbsenceReason, { icon: LucideIcon; headline: string; tone: string }> = {
  not_in_build: { icon: PlugZap, headline: 'Not in this build', tone: 'idle' },
  no_form: { icon: CircleDashed, headline: 'No form for this tool yet', tone: 'idle' },
  needs_desktop: { icon: Monitor, headline: 'Only in the desktop app', tone: 'warn' },
  nothing_yet: { icon: CircleDashed, headline: 'Nothing asked yet', tone: 'idle' },
  attached: { icon: Link2, headline: 'Picked up where it was', tone: 'idle' },
};

export interface AbsenceProps {
  reason: AbsenceReason;
  /** The sentence for the person at the keyboard. */
  message: string;
}

export function Absence({ reason, message }: AbsenceProps) {
  const { icon, headline, tone } = ABSENCE[reason];
  return (
    <div className={styles.absence} data-tone={tone}>
      <span className={styles.absenceHead}>
        <Icon icon={icon} size={14} />
        {headline}
      </span>
      <p className={styles.absenceText}>{message}</p>
    </div>
  );
}

/** The same phrase, at the start of a sentence. */
const sentenceCase = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

export interface ProbeFailureProps {
  failure: Failure;
  /** What was asked for, so the sentence is about something. */
  what: string;
}

/**
 * A probe that produced no result, and why. Never a number.
 */
export function ProbeFailure({ failure, what }: ProbeFailureProps) {
  const { headline, aboutThisMachine } = failureWords(failure.code);
  const sent = nothingWasSent(failure.code);
  return (
    <div className={styles.failure} data-about={aboutThisMachine ? 'machine' : 'target'} role="status">
      <span className={styles.failureHead}>
        <Icon icon={aboutThisMachine ? Ban : TriangleAlert} size={15} />
        {headline}
      </span>
      <p className={styles.failureWhy}>{failure.message}</p>
      <p className={styles.failureMeaning}>
        {sent
          ? `Nothing was sent, so there is no measurement of ${what} — not a slow one and not a lost one.`
          : `${sentenceCase(what)} produced no result. This is not 100 % loss: loss is something a probe that ran can report.`}
      </p>
      <p className={styles.failureCode}>
        The engine called this <code>{failure.code}</code>.
      </p>
    </div>
  );
}
