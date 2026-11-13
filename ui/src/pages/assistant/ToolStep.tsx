import {
  ChevronRight,
  CircleCheck,
  CircleSlash,
  Database,
  OctagonExclamationPoint,
  Plug,
} from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { AiLink, AiToolInfo, ToolStatus, UiBlock } from '@/api/types/ai';
import { Icon, type LucideIcon } from '@/design/Icon';
import { Spinner } from '@/design/Spinner';
import { Tooltip } from '@/design/Tooltip';
import styles from './Transcript.module.css';

type ToolBlock = Extract<UiBlock, { type: 'tool' }>;

const STATUS: Record<ToolStatus, { icon: LucideIcon | null; tone: string; word: string }> = {
  running: { icon: null, tone: 'running', word: 'Running' },
  ok: { icon: CircleCheck, tone: 'ok', word: 'Done' },
  error: { icon: OctagonExclamationPoint, tone: 'crit', word: 'Failed' },
  denied: { icon: CircleSlash, tone: 'warn', word: 'Not allowed' },
};

const LINK_LABEL: Record<AiLink['type'], string> = {
  device: 'Open the device',
  change: 'Review the change',
  lab: 'Open the lab',
  alert: 'Open the alert',
  job: 'Open the job',
};

function routeFor(link: AiLink): string | null {
  switch (link.type) {
    case 'device':
      return `/devices/${link.id}`;
    case 'change':
      return `/changes/${link.id}`;
    case 'lab':
      return `/lab/${link.id}`;
    case 'alert':
      return '/monitoring/alerts';
    case 'job':
      return null;
  }
}

/**
 * One tool call, as a step the user can open.
 *
 * This is the difference between an answer and an auditable answer: the step
 * says which tool ran, with which arguments, what came back, and which RPC
 * methods that tool is allowed to reach at all. What came back is framed as
 * **data the assistant read** — never as something it said — because a
 * device's own output is untrusted text and the distinction has to survive
 * all the way to the screen.
 */
export function ToolStep({ block }: { block: ToolBlock }) {
  const [open, setOpen] = useState(false);
  const tools = useRpc<AiToolInfo[]>('ai.tools');
  const info = tools.data?.find((tool) => tool.name === block.name);
  const status = STATUS[block.status];
  const link = block.link;
  const route = link ? routeFor(link) : null;

  return (
    <div className={styles.step} data-status={block.status} data-open={open || undefined}>
      <button
        type="button"
        className={styles.stepHeader}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon icon={ChevronRight} size={13} className={styles.stepChevron} />
        <span className={styles.stepTitle}>{block.title}</span>
        <span className={styles.stepArgs}>{block.input_summary}</span>
        <span className={styles.stepStatus} data-tone={status.tone}>
          {status.icon ? (
            <Icon icon={status.icon} size={12} />
          ) : (
            <Spinner size={12} label={null} />
          )}
          {block.status === 'running' ? status.word : block.result_summary || status.word}
        </span>
      </button>
      {open && (
        <div className={styles.stepBody}>
          <dl className={styles.stepFacts}>
            <dt>Tool</dt>
            <dd className="mono">{block.name}</dd>
            <dt>Arguments</dt>
            <dd className="mono">{block.input_summary || 'none'}</dd>
            {info && (
              <>
                <dt>Can reach</dt>
                <dd className="mono">{info.methods.join(', ')}</dd>
              </>
            )}
          </dl>
          <div className={styles.data} role="group" aria-label="Data the assistant read">
            <div className={styles.dataHeader}>
              <Icon icon={Database} size={12} />
              <span>
                Data the assistant read{info?.touches_device ? ' from a device' : ''} — not its own
                words
              </span>
              {info?.touches_device && (
                <Tooltip content="This tool opens a session to a device. Nothing it reads is treated as an instruction.">
                  <span className={styles.touches}>
                    <Icon icon={Plug} size={11} />
                    device
                  </span>
                </Tooltip>
              )}
            </div>
            <p className={styles.dataBody}>
              {block.status === 'running'
                ? 'Still running.'
                : block.result_summary || 'Nothing came back.'}
            </p>
          </div>
          {link && route && (
            <Link className={styles.stepLink} to={route}>
              {LINK_LABEL[link.type]}
            </Link>
          )}
          {link && !route && (
            <span className={styles.stepLink}>
              {LINK_LABEL[link.type]} ({link.id})
            </span>
          )}
        </div>
      )}
    </div>
  );
}
