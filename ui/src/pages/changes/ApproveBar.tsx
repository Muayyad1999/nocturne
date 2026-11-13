/**
 * The signature.
 *
 * Approving is not a click: it is a statement that these exact plans were
 * read. So the bar names the devices and the number of commands, refuses
 * until every plan has been opened (or the user says outright that they have
 * read them), and for every device whose plan touches a protected resource it
 * asks for the device's **name, typed out**. A tick box would not do: the
 * engine will not accept one either, and this screen must not pretend
 * otherwise.
 *
 * What is sent is the `plan_hash` of the plans on screen. If a plan changed
 * while it was being read, that is the hash the engine is given, and the
 * engine refuses it — which is the whole point of binding an approval to a
 * hash.
 */

import { ShieldAlert, TriangleAlert } from 'lucide-react';
import type { RpcError } from '@/api/errors';
import type { RiskLevel } from '@/api/types/changes';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { plural, RISK_LABEL } from './change';
import {
  needsConfirmation,
  readiness,
  typedConfirmationOk,
  type NamedTarget,
  type ReviewState,
} from './approval';
import styles from './ApproveBar.module.css';

export interface ApproveBarProps {
  targets: readonly NamedTarget[];
  risk: RiskLevel | undefined;
  typed: ReadonlyMap<string, string>;
  onTyped: (deviceId: string, value: string) => void;
  review: ReviewState;
  onDeclare: (declared: boolean) => void;
  note: string;
  onNote: (note: string) => void;
  onApprove: (apply: boolean) => void;
  pending: boolean;
  error: RpcError | null;
}

export function ApproveBar({
  targets,
  risk,
  typed,
  onTyped,
  review,
  onDeclare,
  note,
  onNote,
  onApprove,
  pending,
  error,
}: ApproveBarProps) {
  const gate = readiness(targets, typed, review);
  const confirmations = needsConfirmation(targets);
  const commands = targets.reduce((total, entry) => total + (entry.plan?.commands.length ?? 0), 0);
  const dangerous = risk === 'high' || risk === 'critical';
  const shown = gate.blockers.filter(
    (blocker) => !(blocker.kind === 'confirm' && confirmations.length > 0),
  );

  return (
    <section className={styles.bar} aria-label="Approval">
      <div className={styles.inner}>
        <p className={styles.lead}>
          Approving sends <strong>{plural(commands, 'command')}</strong> to{' '}
          <strong>{plural(targets.length, 'device')}</strong>
          {risk ? (
            <>
              {' '}
              at <strong>{RISK_LABEL[risk].toLowerCase()}</strong>
            </>
          ) : null}
          . The approval is bound to the plans above: if they are worked out again, it stops being
          valid.
        </p>

        {confirmations.length > 0 && (
          <div className={styles.confirms}>
            <p className={styles.confirmLead}>
              <ShieldAlert size={14} aria-hidden />
              {confirmations.length === 1
                ? 'This change touches a protected resource. Type the device’s name to confirm you mean it.'
                : 'This change touches protected resources on these devices. Type each name to confirm you mean it.'}
            </p>
            {confirmations.map((entry) => {
              const value = typed.get(entry.deviceId) ?? '';
              const ok = typedConfirmationOk(value, entry.name);
              return (
                <Field
                  key={entry.deviceId}
                  label={`Type ${entry.name} to confirm`}
                  hint={
                    ok
                      ? 'Confirmed.'
                      : `${plural(entry.plan?.protected_hits.length ?? 0, 'protected resource')} on this device.`
                  }
                  error={value !== '' && !ok ? `That is not ${entry.name}.` : undefined}
                >
                  <Input
                    mono
                    value={value}
                    placeholder={entry.name}
                    autoComplete="off"
                    spellCheck={false}
                    onChange={(event) => onTyped(entry.deviceId, event.target.value)}
                  />
                </Field>
              );
            })}
          </div>
        )}

        {/* The fields above already ask for each name, so that blocker is not
            repeated here; everything else is. */}
        {shown.length > 0 && (
          <ul className={styles.blockers} data-testid="approve-blockers">
            {shown.map((blocker) => (
              <li key={blocker.kind}>{blocker.text}</li>
            ))}
          </ul>
        )}

        {error && (
          <p className={styles.error} role="alert" data-testid="approve-error">
            <TriangleAlert size={14} aria-hidden /> {error.message}
          </p>
        )}

        <div className={styles.row}>
          <Checkbox
            checked={review.declared}
            onChange={(event) => onDeclare(event.target.checked)}
          >
            I have reviewed the plans
          </Checkbox>
          <Field label="Note for the record" className={styles.note}>
            <Input
              value={note}
              placeholder="Why this is being done, or who asked for it"
              onChange={(event) => onNote(event.target.value)}
            />
          </Field>
          <div className={styles.actions}>
            <Button
              variant="secondary"
              disabled={!gate.ready || pending}
              onClick={() => onApprove(false)}
            >
              Approve without applying
            </Button>
            <Button
              variant={dangerous ? 'danger' : 'primary'}
              disabled={!gate.ready}
              loading={pending}
              onClick={() => onApprove(true)}
            >
              Approve &amp; apply to {plural(targets.length, 'device')}
            </Button>
          </div>
        </div>
      </div>
    </section>
  );
}
