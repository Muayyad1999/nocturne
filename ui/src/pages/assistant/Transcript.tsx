import { Brain, ChevronRight, Info, OctagonExclamationPoint, Sparkles, User } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import type { TokenUsage, UiMessage } from '@/api/types/ai';
import { Icon } from '@/design/Icon';
import { Spinner } from '@/design/Spinner';
import { formatCount } from '@/lib/format';
import type { LiveBlock, LiveTurn } from './stream';
import { AssistantText } from './Text';
import { ToolStep } from './ToolStep';
import styles from './Transcript.module.css';

function Thinking({ summary }: { summary: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.thinking} data-open={open || undefined}>
      <button
        type="button"
        className={styles.thinkingHeader}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon icon={ChevronRight} size={13} className={styles.stepChevron} />
        <Icon icon={Brain} size={13} />
        Thinking
      </button>
      {open && <p className={styles.thinkingBody}>{summary}</p>}
    </div>
  );
}

function Notice({ children, tone = 'info' }: { children: ReactNode; tone?: 'info' | 'crit' }) {
  return (
    <p className={styles.notice} data-tone={tone}>
      <Icon icon={tone === 'crit' ? OctagonExclamationPoint : Info} size={13} />
      <span>{children}</span>
    </p>
  );
}

function Blocks({ blocks }: { blocks: readonly LiveBlock[] }) {
  return (
    <>
      {blocks.map((block, index) => {
        const key = `${index}-${block.type}`;
        switch (block.type) {
          case 'text':
            return <AssistantText key={key} text={block.text} />;
          case 'thinking':
            return <Thinking key={key} summary={block.summary} />;
          case 'tool':
            return <ToolStep key={block.call_id} block={block} />;
          case 'notice':
            return <Notice key={key}>{block.text}</Notice>;
          case 'error':
            return (
              <Notice key={key} tone="crit">
                {block.message} ({block.code})
              </Notice>
            );
        }
      })}
    </>
  );
}

function Usage({ usage }: { usage: TokenUsage }) {
  // A turn that has not reported any tokens yet has nothing to say here.
  if (usage.input_tokens + usage.output_tokens === 0) return null;
  return (
    <p className={styles.usage}>
      {formatCount(usage.input_tokens)} in · {formatCount(usage.output_tokens)} out
    </p>
  );
}

function Turn({
  role,
  children,
  usage,
}: {
  role: 'user' | 'assistant';
  children: ReactNode;
  usage?: TokenUsage | null;
}) {
  return (
    <article className={styles.turn} data-role={role}>
      <span className={styles.avatar} aria-hidden="true">
        <Icon icon={role === 'user' ? User : Sparkles} size={13} />
      </span>
      <div className={styles.body}>
        <h3 className="visually-hidden">{role === 'user' ? 'You' : 'The assistant'}</h3>
        {children}
        {usage && <Usage usage={usage} />}
      </div>
    </article>
  );
}

export interface TranscriptProps {
  messages: readonly UiMessage[];
  pendingUser: { text: string } | null;
  live: LiveTurn | null;
  /** Shown under the live turn: the approval cards, the stop button. */
  footer?: ReactNode;
}

/**
 * The conversation: what was said, what the assistant thought, and every tool
 * call as a step that opens.
 *
 * The stored messages come from `ai.chats.get`; the live turn is built from
 * the event stream and is thrown away the moment the stored copy of it
 * arrives, so nothing is ever shown twice.
 */
export function Transcript({ messages, pendingUser, live, footer }: TranscriptProps) {
  return (
    <div className={styles.transcript}>
      {messages.map((message) => (
        <Turn key={message.id} role={message.role} usage={message.usage}>
          {message.role === 'user' ? (
            <UserText blocks={message.blocks} />
          ) : (
            <Blocks blocks={message.blocks} />
          )}
        </Turn>
      ))}
      {pendingUser && (
        <Turn role="user">
          <p className={styles.userText}>{pendingUser.text}</p>
        </Turn>
      )}
      {live && (
        <Turn role="assistant" usage={live.usage}>
          {live.partial && (
            <Notice>
              This answer was already being written when this window attached. What came before is
              with the engine and appears here when the turn ends.
            </Notice>
          )}
          <Blocks blocks={live.blocks} />
          {live.running && live.approvals.length === 0 && (
            <p className={styles.working}>
              <Spinner size={13} label={null} />
              Working
            </p>
          )}
        </Turn>
      )}
      {footer}
    </div>
  );
}

function UserText({ blocks }: { blocks: UiMessage['blocks'] }) {
  return (
    <>
      {blocks.map((block, index) =>
        block.type === 'text' ? (
          <p key={`${index}-text`} className={styles.userText}>
            {block.text}
          </p>
        ) : block.type === 'notice' ? (
          <Notice key={`${index}-notice`}>{block.text}</Notice>
        ) : null,
      )}
    </>
  );
}
