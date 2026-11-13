/**
 * The Ansible caveat, every time.
 *
 * The backend is explicit about this and says so in three places — the first
 * line of the job log, the run record, and `ansible.status` — because an
 * `ansible-playbook` run has none of the change pipeline's guarantees: no
 * backup, no drift check, no approved plan, no verification and **no
 * rollback**. A run that cannot promise what a change promises must not look
 * like one, so this banner is not dismissible, is shown wherever such a run
 * is started or read, and names the five missing guarantees as five words a
 * person can take in at a glance before reading the sentences.
 */

import { ShieldOff, TriangleAlert, X } from 'lucide-react';
import type { ReactNode } from 'react';
import { Icon } from '@/design/Icon';
import styles from './Automation.module.css';

/**
 * What a change gives and this does not. Taken from the backend's own first
 * caveat; the sentences underneath are the backend's words, verbatim.
 */
const MISSING = ['Backup', 'Drift check', 'Approved plan', 'Verification', 'Rollback'] as const;

export interface CaveatProps {
  /** The caveats as the backend worded them. The first is the important one. */
  caveats: readonly string[];
  /** A heading of your own; the default says what this is. */
  title?: string;
  /** Hides the sentences behind a disclosure, for a dense row. */
  brief?: boolean;
  children?: ReactNode;
}

export function Caveat({ caveats, title = 'Outside the change pipeline', brief = false, children }: CaveatProps) {
  const [lead, ...rest] = caveats;
  return (
    <section className={styles.caveat} role="note" aria-label={title}>
      <h3 className={styles.caveatTitle}>
        <Icon icon={TriangleAlert} size={14} />
        {title}
      </h3>
      <ul className={styles.caveatMissing} aria-label="What a change gives and this does not">
        {MISSING.map((item) => (
          <li key={item} className={styles.caveatChip}>
            <Icon icon={X} size={11} />
            {item}
          </li>
        ))}
      </ul>
      {lead !== undefined && <p className={styles.caveatLead}>{lead}</p>}
      {rest.length > 0 &&
        (brief ? (
          <details className={styles.caveatMore}>
            <summary>The rest of what this path does not promise ({rest.length})</summary>
            <ul>
              {rest.map((caveat) => (
                <li key={caveat}>{caveat}</li>
              ))}
            </ul>
          </details>
        ) : (
          <ul className={styles.caveatList}>
            {rest.map((caveat) => (
              <li key={caveat}>{caveat}</li>
            ))}
          </ul>
        ))}
      {children}
    </section>
  );
}

/**
 * The one-line version, for a run's header and a history row: enough to know
 * what you are looking at, with the whole thing one click away.
 */
export function CaveatLine({ caveat }: { caveat: string }) {
  return (
    <p className={styles.caveatLine} role="note">
      <Icon icon={ShieldOff} size={13} />
      <span>
        <strong>No backup, no drift check, no verification, no rollback.</strong> {caveat}
      </span>
    </p>
  );
}
