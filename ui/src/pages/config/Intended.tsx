/**
 * What the device should say, and what it says instead.
 *
 * The intended configuration is a document the engineer writes: either the
 * whole configuration the device must have (`exact`) or a fragment it must
 * contain. `config.drift` compares it with the latest stored running
 * configuration and — where the device type's grammar is modelled — works
 * out the lines that would close the gap.
 *
 * Those lines are never sent from here. *Fix the drift* hands them to the
 * change pipeline, which plans them, shows them, asks a human and only then
 * talks to a device. The button says so before it is pressed.
 */

import { Crosshair, GitPullRequestArrow, Save, Target, Trash } from 'lucide-react';
import { lazy, Suspense, useEffect, useState } from 'react';
import { useNavigate } from 'react-router';
import type { Coverage } from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { RadioGroup } from '@/design/RadioGroup';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { plural } from '@/lib/format';
import { useClearIntended, useDrift, useFixDrift, useIntended, useSetIntended } from './data';
import { DevicePicker } from './DevicePicker';
import styles from './Intended.module.css';
import { RedactedLine } from './Redacted';
import { VersionDiff } from './VersionDiff';

const IntendedEditor = lazy(() => import('./IntendedEditor'));

export interface IntendedTabProps {
  coverage: Coverage[] | undefined;
  deviceId: string | null;
  onDevice: (deviceId: string | null) => void;
  /** This build can hand drift to the change pipeline. */
  canFix: boolean;
}

export function IntendedTab({ coverage, deviceId, onDevice, canFix }: IntendedTabProps) {
  const intended = useIntended(deviceId);
  const [text, setText] = useState('');
  const [exact, setExact] = useState(false);
  const [dirty, setDirty] = useState(false);
  const save = useSetIntended();
  const clear = useClearIntended();
  const stored = intended.data ?? null;

  // The editor follows the stored document until the user types.
  useEffect(() => {
    setText(stored?.text ?? '');
    setExact(stored?.exact ?? false);
    setDirty(false);
  }, [stored?.text, stored?.exact, deviceId]);

  const drift = useDrift(deviceId, stored !== null && !dirty);
  const row = coverage?.find((entry) => entry.device_id === deviceId);

  if (!deviceId) {
    return (
      <div className={styles.intended}>
        <DevicePicker coverage={coverage} value={deviceId} onChange={onDevice} />
        <EmptyState
          icon={Target}
          title="Write down what a device should have"
          description="Nocturne compares the configuration you intend with the one it last captured, and can hand the difference to the change pipeline. Pick a device to start."
        />
      </div>
    );
  }

  const noBackup = row !== undefined && !row.running;

  return (
    <div className={styles.intended}>
      <div className={styles.toolbar}>
        <DevicePicker coverage={coverage} value={deviceId} onChange={onDevice} />
        <RadioGroup
          name="intended-meaning"
          value={exact ? 'exact' : 'fragment'}
          onChange={(value) => {
            setExact(value === 'exact');
            setDirty(true);
          }}
          aria-label="What the text means"
          orientation="horizontal"
          options={[
            { value: 'fragment', label: 'Must contain these lines' },
            { value: 'exact', label: 'Must equal this exactly' },
          ]}
        />
        <div className={styles.spacer} />
        {stored && (
          <Button
            variant="ghost"
            icon={Trash}
            loading={clear.isPending}
            onClick={async () => {
              const ok = await confirm({
                title: 'Forget the intended configuration?',
                description:
                  'The document is deleted and this device stops being checked for drift. Nothing on the device changes.',
                confirmLabel: 'Forget it',
                tone: 'danger',
              });
              if (ok) clear.mutate({ device_id: deviceId });
            }}
          >
            Forget it
          </Button>
        )}
        <Button
          variant={dirty ? 'primary' : 'secondary'}
          icon={Save}
          loading={save.isPending}
          disabled={!dirty}
          onClick={() => {
            save.mutate(
              { device_id: deviceId, text, exact },
              { onSuccess: () => setDirty(false) },
            );
          }}
        >
          Save the intended configuration
        </Button>
      </div>

      <div className={styles.columns}>
        <div className={styles.column}>
          <h3 className={styles.columnHead}>
            Intended
            {stored && (
              <span className={styles.quiet}>
                saved <TimeAgo at={stored.updated_at} /> · {plural(stored.lines, 'line')}
              </span>
            )}
            {dirty && <Badge tone="warn">Unsaved</Badge>}
          </h3>
          {intended.isPending ? (
            <Skeleton shape="block" height={320} />
          ) : (
            <Suspense fallback={<Skeleton shape="block" height={320} />}>
              <IntendedEditor
                value={text}
                onChange={(next) => {
                  setText(next);
                  setDirty(true);
                }}
                label="Intended configuration"
                placeholder={
                  'The configuration this device should have — a whole file, or just the lines that matter:\n\nservice password-encryption\nip ssh version 2\nntp server 10.0.0.1'
                }
              />
            </Suspense>
          )}
          <p className={styles.explain}>
            {exact
              ? 'Exact: anything on the device that is not in this text counts as drift, and would be removed.'
              : 'A fragment: every line here must be on the device. Anything else the device has is left alone.'}
          </p>
        </div>

        <div className={styles.column}>
          <h3 className={styles.columnHead}>Drift</h3>
          {!stored ? (
            <EmptyState
              icon={Crosshair}
              title="Nothing is intended yet"
              description="Write the configuration on the left and save it; the difference from the last capture appears here."
              size="compact"
            />
          ) : dirty ? (
            <div className={styles.waiting}>
              Save the text to compare it with the device’s last capture.
            </div>
          ) : noBackup ? (
            <EmptyState
              icon={Crosshair}
              title="There is nothing to compare against"
              description="This device has never been captured, so Nocturne cannot say whether it drifts. Back it up first — an unknown configuration is not a matching one."
              size="compact"
            />
          ) : drift.isPending ? (
            <Skeleton shape="block" height={320} />
          ) : drift.isError ? (
            <ErrorState
              title="The drift could not be worked out"
              error={drift.error}
              onRetry={() => void drift.refetch()}
              size="compact"
            />
          ) : drift.data ? (
            <Drift drift={drift.data} canFix={canFix} deviceId={deviceId} />
          ) : null}
        </div>
      </div>
    </div>
  );
}

function Drift({
  drift,
  canFix,
  deviceId,
}: {
  drift: NonNullable<ReturnType<typeof useDrift>['data']>;
  canFix: boolean;
  deviceId: string;
}) {
  const fix = useFixDrift();
  const navigate = useNavigate();

  return (
    <div className={styles.drift}>
      <div className={styles.driftHead}>
        <StatusPill tone={drift.in_sync ? 'ok' : 'warn'}>
          {drift.in_sync ? 'In step' : `${plural(drift.commands.length, 'line')} out of step`}
        </StatusPill>
        <span className={styles.quiet}>
          judged from version {drift.version_id}, captured <TimeAgo at={drift.fetched_at} />
        </span>
        {!drift.supported && (
          <Badge tone="warn">
            The grammar of this device type is not modelled: the difference is shown, the commands cannot
            be derived
          </Badge>
        )}
      </div>

      {drift.warnings.length > 0 && (
        <ul className={styles.warnings} data-testid="drift-warnings">
          {drift.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}

      {drift.in_sync ? (
        <div className={styles.inSync}>
          <Icon icon={Target} size={16} />
          <p>
            The last capture of this device contains everything the intended configuration asks for, and
            nothing is left unexplained.
          </p>
        </div>
      ) : (
        <>
          <div className={styles.driftDiff}>
            <VersionDiff diff={driftAsDiff(drift)} label="Drift from the intended configuration" />
          </div>
          {drift.commands.length > 0 && (
            <div className={styles.commands}>
              <h4>
                What would be sent
                <span className={styles.quiet}>in this order</span>
              </h4>
              <ol className={styles.commandList} aria-label="Lines that would close the drift">
                {drift.commands.map((command, index) => (
                  <li key={`${index}-${command}`} data-testid="drift-command">
                    <span className={styles.commandNo}>{index + 1}</span>
                    <code>
                      <RedactedLine line={command} />
                    </code>
                  </li>
                ))}
              </ol>
              {canFix ? (
                <Button
                  variant="primary"
                  icon={GitPullRequestArrow}
                  loading={fix.isPending}
                  onClick={async () => {
                    const ok = await confirm({
                      title: `Draft a change for ${drift.name}?`,
                      description: `Nocturne creates a change with these ${plural(
                        drift.commands.length,
                        'line',
                      )}. It is planned and shown to you; nothing is sent to ${drift.name} until somebody approves that plan.`,
                      confirmLabel: 'Draft the change',
                    });
                    if (!ok) return;
                    fix.mutate(
                      { device_id: deviceId },
                      {
                        onSuccess: (handover) =>
                          toast.success('A change was drafted', {
                            description: 'Read what it would send, then approve it.',
                            action: {
                              label: 'Open the change',
                              onSelect: () => navigate(`/changes/${handover.change_id}`),
                            },
                          }),
                      },
                    );
                  }}
                >
                  Fix the drift…
                </Button>
              ) : (
                <p className={styles.quiet}>
                  This build has no change pipeline, so the drift can be read here but not handed on.
                </p>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** `config.drift` returns the same `TextDiff` shape, without the version records. */
function driftAsDiff(drift: NonNullable<ReturnType<typeof useDrift>['data']>) {
  return {
    a: emptyVersion(drift.device_id, drift.version_id),
    b: emptyVersion(drift.device_id, drift.version_id),
    diff: drift.diff,
    sections: [],
    redacted: drift.redacted,
  };
}

function emptyVersion(deviceId: string, id: number) {
  return {
    id,
    device_id: deviceId,
    kind: 'running' as const,
    sha256: '',
    lines: 0,
    bytes: 0,
    first_seen: 0,
    last_seen: 0,
    origin: '',
    pinned: false,
  };
}
