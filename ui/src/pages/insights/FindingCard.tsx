/**
 * One finding, with the values it was decided from.
 *
 * The evidence is on the card, not behind a click: it is the difference
 * between a report and an accusation. The sentence that says what is wrong,
 * the device and interface it is about, why it matters and the suggestion are
 * all here too; the drawer behind "Details" adds the whole record, the check's
 * own blind spots and the history.
 *
 * "Draft a change" is deliberately not called "Fix". It creates a draft in the
 * change pipeline, which then does what it always does: plan, show the
 * commands, wait for a human, back up, apply, verify, roll back.
 */

import { GitPullRequestArrow, History, Undo2 } from 'lucide-react';
import { Link } from 'react-router';
import type { Insight } from '@/api/types/insights';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Tooltip } from '@/design/Tooltip';
import { formatCount } from '@/lib/format';
import { useDraftChange, useReopen } from './data';
import {
  categoryLabel,
  dismissal,
  isRepeat,
  severityLabel,
  severityTone,
  stateLabel,
  stateTone,
} from './insight';
import styles from './Findings.module.css';

export interface FindingCardProps {
  insight: Insight;
  /** The site of the finding's device, from the inventory. */
  site: string | undefined;
  canDraftChange: boolean;
  onOpen: () => void;
  onDismiss: () => void;
  /** Narrows the list to one check. */
  onCheck: (check: string) => void;
}

export function FindingCard({
  insight,
  site,
  canDraftChange,
  onOpen,
  onDismiss,
  onCheck,
}: FindingCardProps) {
  const draft = useDraftChange();
  const reopen = useReopen();
  const fixable = (insight.suggestion?.config_lines?.length ?? 0) > 0;
  const dismissed = dismissal(insight);

  return (
    <article className={styles.card} data-tone={severityTone(insight.severity)}>
      <header className={styles.cardHead}>
        <StatusPill tone={severityTone(insight.severity)}>
          {severityLabel(insight.severity)}
        </StatusPill>
        <h4 className={styles.cardTitle}>
          <button type="button" className={styles.cardTitleButton} onClick={onOpen}>
            {insight.title}
          </button>
        </h4>
        {insight.lab && <LabPill />}
        {insight.state !== 'open' && (
          <StatusPill tone={stateTone(insight.state)}>
            {dismissed ? dismissed.label : stateLabel(insight.state)}
          </StatusPill>
        )}
        {isRepeat(insight) && (
          <Tooltip
            content={`This problem has been found ${formatCount(insight.occurrences)} separate times: it went away and came back. It is not a new discovery.`}
          >
            <Badge tone="warn" variant="outline" className={styles.repeat}>
              <Icon icon={History} size={11} /> Repeat · {formatCount(insight.occurrences)}
            </Badge>
          </Tooltip>
        )}
      </header>

      <div className={styles.cardMeta}>
        <Link to={`/devices/${insight.device_id}`} className={styles.device}>
          {insight.device}
        </Link>
        <span className={styles.subject}>{insight.subject}</span>
        {site !== undefined && site !== '' && <span className={styles.site}>{site}</span>}
        <button
          type="button"
          className={styles.checkChip}
          onClick={() => onCheck(insight.check)}
          title={`Show only findings from the ${insight.check} check`}
        >
          {insight.check}
        </button>
        <Badge tone="neutral" variant="outline">
          {categoryLabel(insight.category)}
        </Badge>
        <span className={styles.seen}>
          last seen <TimeAgo at={insight.last_seen} />
        </span>
      </div>

      <div className={styles.cardBody}>
        <div className={styles.cardWhy}>
          <p className={styles.detail}>{insight.detail}</p>
          {insight.suggestion && (
            <p className={styles.suggestion}>
              <span className={styles.suggestionLabel}>Suggested:</span> {insight.suggestion.text}
            </p>
          )}
          {insight.related.length > 0 && (
            <p className={styles.related}>
              <span className={styles.suggestionLabel}>Also at:</span>{' '}
              {insight.related.map((related, at) => (
                <span key={`${related.device_id}/${related.subject}`}>
                  {at > 0 && ', '}
                  <Link to={`/devices/${related.device_id}`}>{related.device}</Link>{' '}
                  <span className={styles.subject}>{related.subject}</span>
                </span>
              ))}
            </p>
          )}
        </div>

        <div className={styles.evidence}>
          <h5 className={styles.evidenceTitle}>Evidence</h5>
          {insight.evidence.length === 0 ? (
            <p className={styles.noEvidence}>
              This finding carries no evidence. Treat it as a hint, not a verdict.
            </p>
          ) : (
            <dl className={styles.evidenceList}>
              {insight.evidence.map((item) => (
                <div key={`${item.label}/${item.value}`} className={styles.evidenceRow}>
                  <dt>{item.label}</dt>
                  <dd className="mono">{item.value}</dd>
                </div>
              ))}
            </dl>
          )}
        </div>
      </div>

      <footer className={styles.cardActions}>
        <Button size="sm" variant="secondary" onClick={onOpen}>
          Details
        </Button>
        {fixable && canDraftChange && (
          <Button
            size="sm"
            variant="ghost"
            icon={GitPullRequestArrow}
            loading={draft.isPending}
            onClick={() =>
              draft.mutate(
                { id: insight.id },
                {
                  onSuccess: () =>
                    toast.success('A change was drafted', {
                      description:
                        'Nothing has been sent to the device. The change pipeline will plan it and wait for a person to approve it.',
                    }),
                },
              )
            }
            title="Drafts a change from the suggested configuration. Nothing is sent to the device: the change pipeline plans it and waits for a person."
          >
            Draft a change
          </Button>
        )}
        {insight.state === 'open' ? (
          <Button size="sm" variant="ghost" onClick={onDismiss}>
            Dismiss…
          </Button>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            icon={Undo2}
            loading={reopen.isPending}
            onClick={() => reopen.mutate({ id: insight.id })}
          >
            Reopen
          </Button>
        )}
        {draft.data && (
          <Link to={`/changes/${draft.data.change_id}`} className={styles.draftLink}>
            Draft change created — review it
          </Link>
        )}
      </footer>
    </article>
  );
}
