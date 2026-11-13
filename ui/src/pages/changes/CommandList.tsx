/**
 * The commands, exactly as they will be sent, in the order they will be sent.
 *
 * Nothing is reformatted, abbreviated or re-ordered here: the engine's list is
 * the list, numbered so that the order is a fact on screen rather than
 * something to infer. Where the plan makes a trunk's allowed-VLAN list before
 * it breaks it (`add` before `remove`), that is called out, because the order
 * is the whole reason the change is safe.
 *
 * Once a device has answered, the same list carries its answers, and the line
 * it rejected is marked.
 */

import type { CommandOutput } from '@/api/types/changes';
import { cx } from '@/lib/cx';
import styles from './CommandList.module.css';

export interface CommandListProps {
  commands: readonly string[];
  /** The device's answer per line, once it has been sent. */
  sent?: readonly CommandOutput[];
  /** Index into `commands` of the line the device rejected. */
  failedLine?: number;
  label: string;
  className?: string;
}

/**
 * Lines whose position in the list is load-bearing. `add` widens the trunk
 * before `remove` narrows it, which is what keeps every other VLAN on the
 * link alive while the change is made.
 */
function ordering(command: string): string | undefined {
  const text = command.trim().toLowerCase();
  if (/^switchport trunk allowed vlan add\b/.test(text)) return 'made before break';
  if (/^switchport trunk allowed vlan remove\b/.test(text)) return 'broken after make';
  if (/^(no )?shutdown$/.test(text)) return text === 'shutdown' ? 'takes the port down' : 'brings the port up';
  return undefined;
}

export function CommandList({ commands, sent, failedLine, label, className }: CommandListProps) {
  return (
    <div className={cx(styles.wrap, className)}>
      <ol className={styles.list} aria-label={label}>
        {commands.map((command, index) => {
          const answer = sent?.[index];
          const failed = failedLine === index || (answer?.error ?? '') !== '';
          const note = ordering(command);
          return (
            <li
              key={`${index}-${command}`}
              className={styles.item}
              data-state={failed ? 'rejected' : answer ? 'sent' : 'planned'}
              data-testid="command-line"
            >
              <span className={styles.number} aria-hidden>
                {index + 1}
              </span>
              <span className={styles.command}>{command}</span>
              {note && (
                <span className={styles.note} data-kind="order">
                  {note}
                </span>
              )}
              {failed && <span className={styles.note} data-kind="bad">rejected</span>}
              {answer && (answer.output.trim() !== '' || answer.error) && (
                <pre className={styles.answer}>{answer.error ?? answer.output.trim()}</pre>
              )}
            </li>
          );
        })}
      </ol>
      {commands.length === 0 && (
        <p className={styles.empty}>
          This plan sends nothing: the device already has what the change asks for.
        </p>
      )}
    </div>
  );
}
