import { Check, Hand, X } from 'lucide-react';
import { useState } from 'react';
import { rpc } from '@/api/client';
import type { AiApproved, AiToolInfo } from '@/api/types/ai';
import { useRpc } from '@/api/hooks';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { toast } from '@/design/Toast';
import type { PendingApproval } from './stream';
import styles from './Transcript.module.css';

export interface ApprovalCardProps {
  chatId: string;
  approval: PendingApproval;
}

/**
 * The assistant is waiting for a person. The turn is stopped inside the tool
 * gate until `ai.approve` answers, so the card has to say exactly what is
 * being asked — the tool, its arguments as the backend summarised them, and
 * what it is allowed to reach — and then answer it in place.
 */
export function ApprovalCard({ chatId, approval }: ApprovalCardProps) {
  const [answering, setAnswering] = useState<'allow' | 'deny' | null>(null);
  const tools = useRpc<AiToolInfo[]>('ai.tools');
  const info = tools.data?.find((tool) => tool.name === approval.name);

  const answer = (allow: boolean) => {
    setAnswering(allow ? 'allow' : 'deny');
    void rpc<AiApproved>('ai.approve', { chat_id: chatId, call_id: approval.call_id, allow })
      .catch((error: Error) => {
        // `not_found` means the request timed out while the card was open.
        toast.error('That request could not be answered', { description: error.message });
      })
      .finally(() => setAnswering(null));
  };

  return (
    <div className={styles.approval} role="group" aria-label="Approval needed">
      <p className={styles.approvalTitle}>
        <Icon icon={Hand} size={13} />
        The assistant is waiting for you
      </p>
      <p className={styles.approvalWhat}>
        It wants to run <strong>{approval.title}</strong>
        {info?.touches_device ? ', which opens a session to a device' : ''}.
      </p>
      <dl className={styles.stepFacts}>
        <dt>Tool</dt>
        <dd className="mono">{approval.name}</dd>
        <dt>Arguments</dt>
        <dd className="mono">{approval.detail || 'none'}</dd>
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
          onClick={() => answer(true)}
        >
          Allow
        </Button>
        <Button size="sm" icon={X} loading={answering === 'deny'} onClick={() => answer(false)}>
          Deny
        </Button>
      </div>
    </div>
  );
}
