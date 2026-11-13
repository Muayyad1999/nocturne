/**
 * Saying that something was taken out.
 *
 * A placeholder that looked like configuration text would be the worst
 * possible outcome: a reader would take `<secret:3>` for the value. So every
 * view of a configuration carries this line — what was removed, how many
 * places, and whether any of the values were well-known defaults — and the
 * markers in the text itself are drawn as markers.
 */

import { Eye, EyeOff, ShieldAlert } from 'lucide-react';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { summarise, redactionSentence } from './redaction';
import styles from './Redacted.module.css';

export interface RedactedNoteProps {
  /** The backend's own flag for the text on screen. */
  redacted: boolean;
  /** The text as it is being shown. */
  text: string;
  /** Offered only where an unredacted read makes sense (one stored version). */
  onToggle?: () => void;
  busy?: boolean;
}

export function RedactedNote({ redacted, text, onToggle, busy }: RedactedNoteProps) {
  const summary = summarise(text);
  const tone = redacted ? (summary.weak > 0 ? 'warn' : 'maint') : 'crit';

  return (
    <div className={styles.note} data-tone={tone} data-testid="redaction-note">
      <Icon icon={redacted ? ShieldAlert : Eye} size={14} className={styles.icon} />
      <p className={styles.text}>
        <strong className={styles.lead}>
          {redacted ? 'Secrets hidden' : 'Secrets shown'}
        </strong>
        {redactionSentence(redacted, summary)}
      </p>
      {onToggle && (
        <Button
          size="sm"
          variant="ghost"
          icon={redacted ? Eye : EyeOff}
          loading={busy}
          onClick={onToggle}
        >
          {redacted ? 'Show secrets…' : 'Hide secrets'}
        </Button>
      )}
    </div>
  );
}

/**
 * One line of configuration shown outside an editor — a search hit, a piece
 * of compliance evidence — with its placeholders marked. The text is split
 * rather than injected: nothing here renders HTML from a device.
 */
export function RedactedLine({ line }: { line: string }) {
  const parts = line.split(/(<secret:\d+(?::weak)?>)/g);
  return (
    <>
      {parts.map((part, index) =>
        /^<secret:\d+(?::weak)?>$/.test(part) ? (
          <span
            key={index}
            className={styles.marker}
            data-weak={part.endsWith(':weak>') || undefined}
            title={
              part.endsWith(':weak>')
                ? 'A secret was taken out here, and its value is a well-known default'
                : 'A secret was taken out here; this is not the value'
            }
          >
            {part}
          </span>
        ) : (
          part
        ),
      )}
    </>
  );
}
