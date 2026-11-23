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
import type { AiToolInfo, ToolStatus, UiBlock } from '@/api/types/ai';
import { Icon, type LucideIcon } from '@/design/Icon';
import { Spinner } from '@/design/Spinner';
import { Tooltip } from '@/design/Tooltip';
import { jobFact, linkTarget } from './links';
import { Shown } from './Shown';
import styles from './Transcript.module.css';

type ToolBlock = Extract<UiBlock, { type: 'tool' }>;

const STATUS: Record<ToolStatus, { icon: LucideIcon | null; tone: string; word: string }> = {
  running: { icon: null, tone: 'running', word: 'Running' },
  ok: { icon: CircleCheck, tone: 'ok', word: 'Done' },
  error: { icon: OctagonExclamationPoint, tone: 'crit', word: 'Failed' },
  denied: { icon: CircleSlash, tone: 'warn', word: 'Not allowed' },
};

/**
 * One tool call, as a step the user can open.
 *
 * This is the difference between an answer and an auditable answer: the step
 * says which tool ran, with which arguments, what came back, and which RPC
 * methods that tool is allowed to reach at all. What came back is framed as
 * **data the assistant read** — never as something it said — because a
 * device's own output is untrusted text and the distinction has to survive
 * all the way to the screen.
 *
 * Every tool is drawn the same way; nothing here knows one by name. A step
 * links to what the engine pointed at, and only when there is somewhere to
 * go (`links.ts`): a step the engine gave no link has none.
 */
export function ToolStep({ block }: { block: ToolBlock }) {
  const [open, setOpen] = useState(false);
  const tools = useRpc<AiToolInfo[]>('ai.tools');
  const methods = useRpc<string[]>('app.methods');
  const info = tools.data?.find((tool) => tool.name === block.name);
  const status = STATUS[block.status];
  const target = linkTarget(block.link, methods.data);
  const job = jobFact(block.link, methods.data);

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
        <span className={styles.stepArgs}>
          <Shown text={block.input_summary} />
        </span>
        <span className={styles.stepStatus} data-tone={status.tone}>
          {status.icon ? (
            <Icon icon={status.icon} size={12} />
          ) : (
            <Spinner size={12} label={null} />
          )}
          {block.status === 'running' || !block.result_summary ? (
            status.word
          ) : (
            <Shown text={block.result_summary} />
          )}
        </span>
      </button>
      {open && (
        <div className={styles.stepBody}>
          <dl className={styles.stepFacts}>
            <dt>Tool</dt>
            <dd className="mono">{block.name}</dd>
            <dt>Arguments</dt>
            <dd className="mono">
              {block.input_summary ? <Shown text={block.input_summary} /> : 'none'}
            </dd>
            {info && (
              <>
                <dt>Can reach</dt>
                <dd className="mono">{info.methods.join(', ')}</dd>
              </>
            )}
            {job && (
              <>
                <dt>Job</dt>
                <dd className="mono">{job}</dd>
              </>
            )}
          </dl>
          <div className={styles.data} role="group" aria-label="Data the assistant read">
            <div className={styles.dataHeader}>
              <Icon icon={Database} size={12} />
              <span>
                Data the assistant read{info?.touches_device ? ' from the network' : ''} — not its
                own words
              </span>
              {info?.touches_device && (
                <Tooltip content="This tool reaches devices on the network: a session, or the probes of a scan. Nothing it reads is treated as an instruction.">
                  <span className={styles.touches}>
                    <Icon icon={Plug} size={11} />
                    device
                  </span>
                </Tooltip>
              )}
            </div>
            <p className={styles.dataBody}>
              {block.status === 'running' ? (
                'Still running.'
              ) : block.result_summary ? (
                <Shown text={block.result_summary} />
              ) : (
                'Nothing came back.'
              )}
            </p>
          </div>
          {target && (
            <Link className={styles.stepLink} to={target.to}>
              {target.label}
            </Link>
          )}
        </div>
      )}
    </div>
  );
}
