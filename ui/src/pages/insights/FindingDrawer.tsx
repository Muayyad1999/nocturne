/**
 * One finding, whole: the evidence, the suggestion as configuration lines and
 * as read-only commands to confirm it with, the other end of the problem, its
 * history — and the entry from the catalogue, so the blind spots of the check
 * that raised it are on the same screen as the accusation.
 */

import { GitPullRequestArrow, Undo2 } from 'lucide-react';
import { Link } from 'react-router';
import type { Insight } from '@/api/types/insights';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Drawer } from '@/design/Drawer';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useChecks, useDraftChange, useInsight, useReopen } from './data';
import {
  categoryLabel,
  dismissal,
  severityLabel,
  severityTone,
  stateLabel,
  stateTone,
} from './insight';
import styles from './Findings.module.css';

export interface FindingDrawerProps {
  insight: Insight | null;
  site: string | undefined;
  canDraftChange: boolean;
  onClose: () => void;
  onDismiss: () => void;
}

export function FindingDrawer({
  insight: opened,
  site,
  canDraftChange,
  onClose,
  onDismiss,
}: FindingDrawerProps) {
  const checks = useChecks();
  const draft = useDraftChange();
  const reopen = useReopen();
  // Read the finding again by id: dismissing or reopening it while this is
  // open must change what it says, and the row it came from is a snapshot.
  const live = useInsight(opened?.id ?? null);
  const insight = live.data ?? opened;

  if (!insight) return null;

  const entry = checks.data?.find((check) => check.id === insight.check);
  const lines = insight.suggestion?.config_lines ?? [];
  const commands = insight.suggestion?.commands ?? [];
  const dismissed = dismissal(insight);

  const facts: KeyValueItem[] = [
    { label: 'Device', value: <Link to={`/devices/${insight.device_id}`}>{insight.device}</Link> },
    { label: 'Subject', value: insight.subject, mono: true },
    { label: 'Site', value: site === undefined || site === '' ? null : site },
    { label: 'Check', value: insight.check, mono: true },
    { label: 'Category', value: categoryLabel(insight.category) },
    { label: 'First seen', value: formatTimestamp(insight.first_seen) },
    { label: 'Last seen', value: formatTimestamp(insight.last_seen) },
    {
      label: 'Times found',
      value:
        insight.occurrences > 1
          ? `${formatCount(insight.occurrences)} separate times — it came back`
          : 'Once, and it has not gone away since',
    },
  ];
  if (insight.resolved_at != null) {
    facts.push({ label: 'Resolved', value: formatTimestamp(insight.resolved_at) });
  }
  if (insight.dismissed_at != null) {
    facts.push({ label: 'Dismissed', value: formatTimestamp(insight.dismissed_at) });
  }
  if (insight.state === 'dismissed') {
    facts.push({
      label: 'Dismissed until',
      value:
        insight.dismissed_until == null
          ? 'for good'
          : `${formatTimestamp(insight.dismissed_until)}${dismissed?.expired ? ' — has run out' : ''}`,
    });
  }
  if (insight.dismiss_note !== undefined && insight.dismiss_note !== '') {
    facts.push({ label: 'Reason given', value: insight.dismiss_note });
  }

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={insight.title}
      description={
        <span className={styles.drawerHead}>
          <StatusPill tone={severityTone(insight.severity)}>
            {severityLabel(insight.severity)}
          </StatusPill>
          <StatusPill tone={stateTone(insight.state)}>
            {dismissed ? dismissed.label : stateLabel(insight.state)}
          </StatusPill>
          {insight.lab && <LabPill />}
          <span className="mono">
            {insight.device} {insight.subject}
          </span>
        </span>
      }
      footer={
        <>
          {insight.state === 'open' ? (
            <Button variant="secondary" onClick={onDismiss}>
              Dismiss…
            </Button>
          ) : (
            <Button
              variant="secondary"
              icon={Undo2}
              loading={reopen.isPending}
              onClick={() => reopen.mutate({ id: insight.id })}
            >
              Reopen
            </Button>
          )}
          {lines.length > 0 && canDraftChange && (
            <Button
              variant="primary"
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
            >
              Draft a change
            </Button>
          )}
        </>
      }
    >
      <div className={styles.drawerBody}>
        <section>
          <h3 className={styles.sectionTitle}>Why it matters</h3>
          <p className={styles.detail}>{insight.detail}</p>
        </section>

        <section>
          <h3 className={styles.sectionTitle}>Evidence</h3>
          {insight.evidence.length === 0 ? (
            <p className={styles.noEvidence}>
              This finding carries no evidence. Treat it as a hint, not a verdict.
            </p>
          ) : (
            <KeyValue
              items={insight.evidence.map((item) => ({
                label: item.label,
                value: item.value,
                mono: true,
              }))}
            />
          )}
        </section>

        {insight.related.length > 0 && (
          <section>
            <h3 className={styles.sectionTitle}>The other end</h3>
            <KeyValue
              items={insight.related.map((related) => ({
                label: related.device,
                value: <Link to={`/devices/${related.device_id}`}>{related.subject}</Link>,
                mono: true,
              }))}
            />
            <p className={styles.hint}>
              One problem is one finding. This end owns it because that is where the fix belongs;
              filtering by either device shows it.
            </p>
          </section>
        )}

        {insight.suggestion && (
          <section>
            <h3 className={styles.sectionTitle}>Suggestion</h3>
            <p className={styles.detail}>{insight.suggestion.text}</p>
            {lines.length > 0 ? (
              <>
                <CodeBlock
                  title="Configuration this would draft as a change"
                  code={lines.join('\n')}
                  lineNumbers
                />
                <p className={styles.hint}>
                  <strong>Draft a change</strong> hands these lines to the change pipeline as a
                  draft. Nothing is sent to {insight.device}: the pipeline plans the change, shows
                  the commands exactly as they would be typed, and waits for a person to approve
                  them. {canDraftChange ? '' : 'This build has no change pipeline, so the lines are here to copy.'}
                </p>
              </>
            ) : (
              <p className={styles.hint}>
                There is no exact configuration fix for this one, so there is nothing to draft:
                what to do about it is the sentence above and the evidence.
              </p>
            )}
            {commands.length > 0 && (
              <CodeBlock
                title="Read-only commands that confirm it on the device"
                code={commands.join('\n')}
              />
            )}
          </section>
        )}

        <section>
          <h3 className={styles.sectionTitle}>The record</h3>
          <KeyValue items={facts} />
        </section>

        <section>
          <h3 className={styles.sectionTitle}>
            The check that found it
            {entry && (
              <Badge tone={entry.enabled ? 'neutral' : 'idle'} variant="outline">
                {entry.enabled ? 'On' : 'Off'}
              </Badge>
            )}
          </h3>
          {entry ? (
            <>
              <p className={styles.detail}>{entry.description}</p>
              <p className={styles.notEvaluated}>
                <span className={styles.suggestionLabel}>Not evaluated when:</span>{' '}
                {entry.not_evaluated}
              </p>
              <p className={styles.hint}>
                Reads {entry.needs.join(', ')} · default severity{' '}
                {severityLabel(entry.default_severity).toLowerCase()} ·{' '}
                {plural(entry.needs.length, 'source')}
              </p>
            </>
          ) : (
            <p className={styles.hint}>
              This build&rsquo;s catalogue does not describe <code>{insight.check}</code>.
            </p>
          )}
        </section>
      </div>
    </Drawer>
  );
}
