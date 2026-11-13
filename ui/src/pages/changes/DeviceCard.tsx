/**
 * One device of a change: everything a person needs before they sign for it,
 * and everything the engine recorded afterwards.
 *
 * Collapsed it is a summary. Open it shows, in this order: the exact commands,
 * the predicted configuration, the risk and why, the protected resources it
 * touches, the dead-man switch that will be armed, and what would undo it.
 * Opening it also tells the approve bar that this plan has been read.
 */

import { ChevronRight, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import type { Change, Target } from '@/api/types/changes';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Tooltip } from '@/design/Tooltip';
import { cx } from '@/lib/cx';
import { formatDateTime } from '@/lib/time';
import { ApplyStepper, Checks } from './ApplyStepper';
import { CommandList } from './CommandList';
import { plural } from '@/lib/format';
import { hitKindLabel, RISK_LABEL, riskTone, TARGET_STATE_LABEL, targetStateTone } from './change';
import { PlanDiff } from './PlanDiff';
import styles from './DeviceCard.module.css';

export interface DeviceCardProps {
  change: Change;
  target: Target;
  /** The device's name as the inventory has it. */
  name: string;
  /** Simulated device: everything about it carries the LAB pill. */
  lab: boolean;
  /** The device is marked protected: no change to it is ever unattended. */
  protectedDevice: boolean;
  /** Called with the plan's hash the first time the card is opened. */
  onSeen?: (planHash: string) => void;
  defaultOpen?: boolean;
}

export function DeviceCard({
  change,
  target,
  name,
  lab,
  protectedDevice,
  onSeen,
  defaultOpen = false,
}: DeviceCardProps) {
  const [open, setOpen] = useState(defaultOpen);
  const [transcript, setTranscript] = useState(false);
  const plan = target.plan ?? undefined;
  const result = target.result ?? undefined;
  const hits = plan?.protected_hits ?? [];

  /** The plan this card has reported as read. */
  const seen = useRef<string | null>(null);

  // Opening the card is what "the plan was read" means — and only opening it.
  // A plan that is replaced while the card happens to be open is *not* read:
  // nobody asked for the new commands to appear, so the approval bar hears
  // nothing and goes back to asking.
  const markSeen = () => {
    if (!plan || seen.current === plan.plan_hash) return;
    seen.current = plan.plan_hash;
    onSeen?.(plan.plan_hash);
  };

  // A card that should be open by default is opened when it *becomes* so —
  // a change lands on this page while it is still being planned, and the
  // plan worth reading arrives a moment later.
  const opened = useRef(false);
  useEffect(() => {
    if (!defaultOpen || opened.current) return;
    opened.current = true;
    setOpen(true);
    markSeen();
  }, [defaultOpen, plan?.plan_hash]);

  const applying = change.state === 'applying';

  return (
    <section className={styles.card} data-open={open || undefined} data-testid="device-card">
      <h3 className={styles.heading}>
        <button
          type="button"
          className={styles.summary}
          aria-expanded={open}
          onClick={() => {
            if (!open) markSeen();
            setOpen((was) => !was);
          }}
        >
          <ChevronRight className={styles.chevron} size={14} aria-hidden />
          <span className={styles.name}>{name}</span>
          {lab && <LabPill />}
          {protectedDevice && (
            <Tooltip content="A protected device: Nocturne never changes it unattended.">
              <Badge tone="crit">Protected device</Badge>
            </Tooltip>
          )}
          <span className={styles.spacer} />
          {plan && (
            <>
              {hits.length > 0 && (
                <Badge tone="crit" data-testid="hit-count">
                  {plural(hits.length, 'protected resource')}
                </Badge>
              )}
              {plan.maturity !== 'verified' && (
                <Tooltip
                  content={`The \`${plan.profile}\` device type is ${plan.maturity}: it has not been checked against real hardware.`}
                >
                  <Badge tone="warn">{plan.maturity}</Badge>
                </Tooltip>
              )}
              <Badge tone={riskTone(plan.risk.level)}>{RISK_LABEL[plan.risk.level]}</Badge>
              <span className={styles.count}>{plural(plan.commands.length, 'command')}</span>
            </>
          )}
          {!plan && <span className={styles.count}>Not planned yet</span>}
          {result && (
            <StatusPill tone={targetStateTone(result.state)}>
              {TARGET_STATE_LABEL[result.state]}
            </StatusPill>
          )}
        </button>
      </h3>

      {open && (
        <div className={styles.body}>
          {!plan ? (
            <p className={styles.note}>
              Nothing has been worked out for {name} yet. Planning reads its running configuration
              and turns this change into the exact commands it would be sent.
            </p>
          ) : (
            <>
              {(result || applying) && (
                <div className={styles.block}>
                  <h4 className={styles.blockTitle}>How it went</h4>
                  <ApplyStepper
                    result={result}
                    guard={plan.guard != null}
                    label={`Pipeline on ${name}`}
                  />
                  {result?.error && (
                    <p className={styles.callout} data-tone="crit" data-testid="target-error">
                      <TriangleAlert size={14} aria-hidden /> {result.error}
                    </p>
                  )}
                  {result?.state === 'rolled_back' && (
                    <p className={styles.callout} data-tone="warn">
                      <TriangleAlert size={14} aria-hidden /> The change was undone: the running
                      configuration of {name} is back to the version it had before
                      {result.before_version_id ? ` (version ${result.before_version_id})` : ''}.
                    </p>
                  )}
                  {result?.state === 'rollback_failed' && (
                    <p className={styles.callout} data-tone="crit">
                      <TriangleAlert size={14} aria-hidden /> The rollback did not finish:{' '}
                      {name} is still different from what it had before. Read the difference below
                      before touching it again.
                    </p>
                  )}
                  {result?.state === 'guard_fired' && (
                    <p className={styles.callout} data-tone="crit">
                      <TriangleAlert size={14} aria-hidden /> {name} went silent during the change.
                      Its dead-man switch is armed and will undo everything that was not saved.
                    </p>
                  )}
                  {result && result.verification.length > 0 && (
                    <>
                      <h5 className={styles.subTitle}>Checks after the change</h5>
                      <Checks checks={result.verification} label={`Checks on ${name}`} />
                    </>
                  )}
                </div>
              )}

              <div className={styles.block}>
                <h4 className={styles.blockTitle}>
                  {result ? 'What was sent' : 'What will be sent'}
                </h4>
                <p className={styles.caption}>
                  {result
                    ? 'Sent in configuration mode, in this order, one line at a time, with what the device answered.'
                    : 'In configuration mode, in this order, one line at a time. A line the device rejects stops it there and the change is rolled back.'}
                </p>
                <CommandList
                  commands={plan.commands}
                  sent={result?.sent}
                  failedLine={result?.failed_line}
                  label={`Commands for ${name}`}
                />
              </div>

              <div className={styles.block}>
                <h4 className={styles.blockTitle}>Predicted configuration</h4>
                <p className={styles.caption}>
                  What the running configuration becomes, against version {plan.base_version_id} of
                  it as it was read at {formatDateTime(plan.planned_at)}. This prediction is what
                  verification compares the device against afterwards.
                </p>
                <PlanDiff diff={plan.diff} title={`${name}: now → predicted`} />
              </div>

              <div className={styles.block}>
                <h4 className={styles.blockTitle}>Risk</h4>
                <div className={styles.riskRow}>
                  <StatusPill tone={riskTone(plan.risk.level)}>
                    {RISK_LABEL[plan.risk.level]}
                  </StatusPill>
                </div>
                <ul className={styles.reasons}>
                  {plan.risk.reasons.map((reason) => (
                    <li key={reason}>{reason}</li>
                  ))}
                </ul>
              </div>

              {hits.length > 0 && (
                <div className={styles.block}>
                  <h4 className={styles.blockTitle}>
                    Protected resources — {name} must be named to approve this
                  </h4>
                  <ul className={styles.hits}>
                    {hits.map((hit, index) => (
                      <li
                        key={`${hit.kind}-${hit.command}-${index}`}
                        className={styles.hit}
                        data-testid="protected-hit"
                      >
                        <span className={styles.hitHead}>
                          <ShieldAlert size={14} aria-hidden />
                          <strong>{hitKindLabel(hit.kind)}</strong>
                          <span className={styles.hitSubject}>{hit.subject}</span>
                        </span>
                        <code className={styles.hitCommand}>{hit.command}</code>
                        <span className={styles.hitWhy}>{hit.why}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {plan.warnings.length > 0 && (
                <div className={styles.block}>
                  <h4 className={styles.blockTitle}>Read before you approve</h4>
                  <ul className={styles.reasons} data-testid="plan-warnings">
                    {plan.warnings.map((warning) => (
                      <li key={warning}>{warning}</li>
                    ))}
                  </ul>
                </div>
              )}

              <div className={styles.block}>
                <h4 className={styles.blockTitle}>Dead-man switch</h4>
                {plan.guard ? (
                  <>
                    <p className={styles.caption}>
                      <code>{plan.guard.name}</code>, armed for {plan.guard.minutes} minutes before
                      the first command and cancelled once the change verifies. {plan.guard.effect}
                    </p>
                    <div className={styles.guardCommands}>
                      <CodeBlock
                        title="Arm"
                        code={plan.guard.arm.join('\n')}
                        copyable={false}
                        maxHeight={120}
                      />
                      <CodeBlock
                        title="Cancel"
                        code={plan.guard.disarm.join('\n')}
                        copyable={false}
                        maxHeight={120}
                      />
                    </div>
                  </>
                ) : (
                  <p className={styles.callout} data-tone="warn">
                    <TriangleAlert size={14} aria-hidden /> None will be armed for this change: if
                    it cuts Nocturne off, {name} keeps the new configuration until somebody reaches
                    it another way. Nocturne arms one when the risk is high or when the change asks
                    for it — and when a device type has none at all, the plan says so above.
                  </p>
                )}
              </div>

              <div className={styles.block}>
                <h4 className={styles.blockTitle}>If it has to be undone</h4>
                {plan.rollback.length > 0 ? (
                  <CommandList commands={plan.rollback} label={`Rollback commands for ${name}`} />
                ) : (
                  <p className={styles.callout} data-tone="crit">
                    <TriangleAlert size={14} aria-hidden /> Nocturne could not work out how to undo
                    this change automatically. It will still restore the configuration it backed up,
                    but there is no prepared command list.
                  </p>
                )}
              </div>

              {result && result.transcript !== '' && (
                <div className={styles.block}>
                  <h4 className={styles.blockTitle}>Session transcript</h4>
                  {transcript ? (
                    <CodeBlock
                      code={result.transcript}
                      maxHeight={360}
                      title={`${name}: what went over the wire (redacted)`}
                    />
                  ) : (
                    <Button size="sm" variant="secondary" onClick={() => setTranscript(true)}>
                      Show the transcript
                    </Button>
                  )}
                </div>
              )}

              <dl className={cx(styles.block, styles.facts)}>
                <div>
                  <dt>Plan</dt>
                  <dd className={styles.mono} data-testid="plan-hash">
                    {plan.plan_hash.slice(0, 12)}
                  </dd>
                </div>
                <div>
                  <dt>Read from</dt>
                  <dd className={styles.mono}>
                    version {plan.base_version_id} · {plan.base_sha256.slice(0, 12)}
                  </dd>
                </div>
                <div>
                  <dt>Predicted</dt>
                  <dd className={styles.mono}>
                    {plan.predicted_sha256 ? plan.predicted_sha256.slice(0, 12) : 'not predictable'}
                  </dd>
                </div>
                <div>
                  <dt>Device type</dt>
                  <dd className={styles.mono}>
                    {plan.profile} · {plan.maturity}
                  </dd>
                </div>
                {result?.after_version_id != null && (
                  <div>
                    <dt>Afterwards</dt>
                    <dd className={styles.mono}>version {result.after_version_id}</dd>
                  </div>
                )}
                <div>
                  <dt>Device</dt>
                  <dd>
                    <Link to={`/devices/${target.device_id}`}>Open {name}</Link>
                  </dd>
                </div>
              </dl>
            </>
          )}
        </div>
      )}
    </section>
  );
}
