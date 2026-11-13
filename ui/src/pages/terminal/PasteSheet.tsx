/**
 * The seat belt: what is about to be pasted, line by line, with what the
 * device's own policy thinks of each line.
 *
 * It is not a lock. *Paste anyway* has the focus the moment the sheet opens,
 * so Enter sends and Escape does not — one keystroke either way.
 */

import { ClipboardPaste } from 'lucide-react';
import type { TerminalLineClass } from '@/api/types/terminal';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Kbd } from '@/design/Kbd';
import styles from './PasteSheet.module.css';
import { pasteRisk, pasteSummary, splitLines } from './paste';

/** Lines shown in full; a paste longer than this is summarised at the end. */
const SHOWN = 200;

export interface PasteRequest {
  sessionId: string;
  /** The session's title, for the question. */
  title: string;
  text: string;
  /** What `terminal.classify` said, or null when it could not be asked. */
  classes: TerminalLineClass[] | null;
  /** Where the text came from, so the sheet can say it. */
  source: 'paste' | 'snippet';
  /** How many sessions it is going to, when a broadcast is on. */
  targets: number;
}

export interface PasteSheetProps {
  request: PasteRequest | null;
  onCancel: () => void;
  onConfirm: (request: PasteRequest) => void;
}

const CLASS_LABEL: Record<TerminalLineClass['class'], string> = {
  read_only: 'reads',
  change: 'changes',
  dangerous: 'disruptive',
};

export function PasteSheet({ request, onCancel, onConfirm }: PasteSheetProps) {
  if (!request) return null;

  const lines = splitLines(request.text.replace(/\n$/, ''));
  const classes = request.classes;
  const risk = classes ? pasteRisk(classes) : null;
  const shown = lines.slice(0, SHOWN);
  const verb = request.source === 'snippet' ? 'Send this snippet' : 'Paste';
  const where =
    request.targets > 1 ? `${request.targets} sessions` : request.title;

  return (
    <Dialog
      open
      onClose={onCancel}
      size="lg"
      title={`${verb} into ${where}?`}
      description={
        <>
          {pasteSummary(request.text, risk)}{' '}
          {risk?.headline ? <span className={styles.headline}>{risk.headline}.</span> : null}
          {classes === null && ' The device policy could not be asked about these lines.'}
        </>
      }
      footer={
        <>
          <Button onClick={onCancel}>Cancel</Button>
          <Button
            data-autofocus
            variant={risk && risk.dangerous > 0 ? 'danger' : 'primary'}
            icon={ClipboardPaste}
            onClick={() => onConfirm(request)}
          >
            Paste anyway <Kbd shortcut="enter" />
          </Button>
        </>
      }
    >
      <ol className={styles.lines}>
        {shown.map((line, index) => {
          const classified = classes?.[index];
          return (
            <li key={index} className={styles.line} data-class={classified?.class ?? 'unknown'}>
              <span className={styles.number}>{index + 1}</span>
              <code className={styles.text}>{line === '' ? ' ' : line}</code>
              {classified && classified.class !== 'read_only' && (
                <span className={styles.tag} data-class={classified.class} title={classified.reason}>
                  {CLASS_LABEL[classified.class]}
                </span>
              )}
            </li>
          );
        })}
        {lines.length > SHOWN && (
          <li className={styles.more}>and {lines.length - SHOWN} more lines</li>
        )}
      </ol>
    </Dialog>
  );
}
