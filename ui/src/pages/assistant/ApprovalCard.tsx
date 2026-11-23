import { ArrowDown, Check, CircleCheck, Hand, X } from 'lucide-react';
import { useId, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from 'react';
import { rpc } from '@/api/client';
import type { AiApproved, AiToolInfo } from '@/api/types/ai';
import { useRpc } from '@/api/hooks';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { toast } from '@/design/Toast';
import { requestLines } from './request';
import { Shown } from './Shown';
import type { PendingApproval } from './stream';
import styles from './Transcript.module.css';

export interface ApprovalCardProps {
  chatId: string;
  approval: PendingApproval;
}

interface Overflow {
  /** It holds more than it shows. */
  scrolls: boolean;
  /** Its last line has been in view. */
  atEnd: boolean;
}

/**
 * Whether `element` holds more than it shows, and whether its end has been
 * reached. A box that scrolls has to be reachable by keyboard to be scrolled,
 * one that does not must not be an extra stop on the way to the buttons, and
 * a person has to be told that there is more of a request than is in view.
 */
function useOverflow<T extends HTMLElement>(): [RefObject<T | null>, Overflow] {
  const ref = useRef<T>(null);
  const [overflow, setOverflow] = useState<Overflow>({ scrolls: false, atEnd: true });
  // A layout effect, so the first measure lands before the first paint. The
  // transcript keeps to its end only while it is at the end, and a card that
  // grew a second time a moment after it appeared (the note below the frame)
  // could leave its own buttons under the fold.
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      const scrolls = element.scrollHeight > element.clientHeight + 1;
      const atEnd = element.scrollTop + element.clientHeight >= element.scrollHeight - 2;
      setOverflow((now) => (now.scrolls === scrolls && now.atEnd === atEnd ? now : { scrolls, atEnd }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const child of element.children) observer.observe(child);
    element.addEventListener('scroll', measure, { passive: true });
    return () => {
      observer.disconnect();
      element.removeEventListener('scroll', measure);
    };
  }, []);
  return [ref, overflow];
}

/**
 * The assistant is waiting for a person. The turn is stopped inside the tool
 * gate until `ai.approve` answers, so the card has to say exactly what is
 * being asked and then answer it in place.
 *
 * What is being asked is the engine's `detail`: one line of arguments when
 * `confirm_reads` asks about a read, and for a tool that writes a record the
 * whole question — the alert and the note, the prefix and the name, every
 * range of a scan. Parts of it were written by devices, by other people and
 * by the model, and any of them may be worded to look like part of this
 * card. So it is shown as text in a frame of its own, one numbered block per
 * line the engine wrote (`request.ts`), never parsed, and the card's own
 * words and its two buttons stay outside that frame. A long one scrolls
 * inside its frame, so the buttons never move off the card, and the card
 * says so until the last line has been in view.
 */
export function ApprovalCard({ chatId, approval }: ApprovalCardProps) {
  const [answering, setAnswering] = useState<'allow' | 'deny' | null>(null);
  const tools = useRpc<AiToolInfo[]>('ai.tools');
  const info = tools.data?.find((tool) => tool.name === approval.name);
  const [request, { scrolls, atEnd }] = useOverflow<HTMLElement>();
  const headingId = useId();
  const moreId = useId();
  const lines = requestLines(approval.detail);

  const answer = (allow: boolean) => {
    // One card, one answer. The engine takes the first and has forgotten the
    // question by the second, which would come back as a failure to answer
    // and read as though the first had not counted. An answer the engine
    // took holds until the card goes, with the step's end; only one that
    // did not reach it can be given again.
    if (answering) return;
    setAnswering(allow ? 'allow' : 'deny');
    void rpc<AiApproved>('ai.approve', { chat_id: chatId, call_id: approval.call_id, allow }).catch(
      (error: Error) => {
        // `not_found` means the request timed out while the card was open.
        toast.error('That request could not be answered', { description: error.message });
        setAnswering(null);
      },
    );
  };

  return (
    <div className={styles.approval} role="group" aria-label="Approval needed">
      <p className={styles.approvalTitle}>
        <Icon icon={Hand} size={13} />
        The assistant is waiting for you
      </p>
      <p className={styles.approvalWhat}>
        It wants to run <strong>{approval.title}</strong>
        {info?.touches_device ? ', which reaches devices on the network' : ''}.
      </p>
      <section
        ref={request}
        className={styles.request}
        aria-labelledby={headingId}
        aria-describedby={scrolls ? moreId : undefined}
        // Only a frame that scrolls is a stop on the way to the buttons.
        tabIndex={scrolls ? 0 : undefined}
      >
        <header className={styles.requestHead}>
          <span id={headingId} className={styles.requestTitle}>
            The request, word for word
          </span>
          <span>Names, titles and notes in it were written by devices, people or the model.</span>
        </header>
        {lines.length > 0 ? (
          <pre className={styles.requestBody}>
            {lines.map((line, index) => (
              <span
                // The request never changes while its card is open.
                key={index}
                className={styles.requestLine}
                style={{ '--lead': line.lead } as CSSProperties}
              >
                <Shown text={line.text} runs={line.runs} />
              </span>
            ))}
          </pre>
        ) : (
          <p className={styles.requestEmpty}>The engine gave no details.</p>
        )}
      </section>
      {scrolls && (
        <p id={moreId} className={styles.requestMore} data-end={atEnd || undefined}>
          <Icon icon={atEnd ? CircleCheck : ArrowDown} size={12} />
          {atEnd
            ? 'That is the whole request.'
            : 'More of the request is below: scroll it to read every line before you answer.'}
        </p>
      )}
      <dl className={styles.stepFacts}>
        <dt>Tool</dt>
        <dd className="mono">{approval.name}</dd>
        {info && (
          <>
            <dt>Can reach</dt>
            <dd className="mono">{info.methods.join(', ')}</dd>
          </>
        )}
      </dl>
      <div className={styles.approvalActions}>
        <Button
          size="sm"
          variant="ai"
          icon={Check}
          loading={answering === 'allow'}
          disabled={answering === 'deny'}
          onClick={() => answer(true)}
        >
          Allow
        </Button>
        <Button
          size="sm"
          icon={X}
          loading={answering === 'deny'}
          disabled={answering === 'allow'}
          onClick={() => answer(false)}
        >
          Deny
        </Button>
      </div>
    </div>
  );
}
