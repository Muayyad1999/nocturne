/**
 * One device's configuration: what it says now, what it said before, and
 * what changed in between.
 *
 * Left: every stored version, newest first — when it was taken, how it came
 * to be taken, how big it was, whether it is pinned. Right: the text, or the
 * engine's diff between the two versions that are ticked for comparison.
 *
 * The one thing this view must never do is make a device that has never been
 * captured look like a device with an empty configuration. The first has no
 * version, no timestamp and no checksum, and the backend says so in words
 * (`not_found`: "has no running-config backup yet"); the second is a stored
 * version with a time, a checksum and nothing in it. They get different
 * screens, and the words come from the engine.
 */

import { Camera, FileClock, GitCompare, Pin, PinOff, RotateCcw, Search } from 'lucide-react';
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import type { ConfigKind, ConfigVersion, Coverage } from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Splitter } from '@/design/Splitter';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Tooltip } from '@/design/Tooltip';
import { cx } from '@/lib/cx';
import { EMPTY, formatBytes, formatCount, plural } from '@/lib/format';
import { useConfigText, useDiff, usePin, useRestore, useUnpin, useVersions } from './data';
import { DevicePicker } from './DevicePicker';
import { findInText } from './find';
import styles from './History.module.css';
import { RedactedNote } from './Redacted';
import { askToReveal } from './reveal';
import { VersionDiff } from './VersionDiff';
import { isEmptyCapture, KIND_LABEL, originLabel, originSentence } from './vocab';

const ConfigView = lazy(() => import('./ConfigView'));

export interface HistoryTabProps {
  coverage: Coverage[] | undefined;
  loading: boolean;
  /** The device on screen, and how to change it (both live in the URL). */
  deviceId: string | null;
  onDevice: (deviceId: string | null) => void;
  kind: ConfigKind;
  onKind: (kind: ConfigKind) => void;
  /** Starts a backup of one device: the way out of "never captured". */
  onBackUp: (deviceId: string) => void;
  backingUp: boolean;
}

export function HistoryTab({
  coverage,
  loading,
  deviceId,
  onDevice,
  kind,
  onKind,
  onBackUp,
  backingUp,
}: HistoryTabProps) {
  const row = coverage?.find((entry) => entry.device_id === deviceId);
  const versions = useVersions(deviceId, kind);

  if (!deviceId) {
    return (
      <div className={styles.history}>
        <Toolbar coverage={coverage} deviceId={deviceId} onDevice={onDevice} kind={kind} onKind={onKind} />
        <EmptyState
          icon={FileClock}
          title="Pick a device"
          description="Every capture Nocturne has taken of a device is kept, so you can read what it says now and see what changed since last week."
        />
      </div>
    );
  }

  return (
    <div className={styles.history}>
      <Toolbar coverage={coverage} deviceId={deviceId} onDevice={onDevice} kind={kind} onKind={onKind} />
      {loading || versions.isPending ? (
        <div className={styles.loading}>
          <Skeleton shape="block" height={28} />
          <Skeleton shape="block" height={320} />
        </div>
      ) : versions.isError ? (
        <ErrorState
          title="The versions could not be listed"
          error={versions.error}
          onRetry={() => void versions.refetch()}
        />
      ) : (versions.data ?? []).length === 0 ? (
        <NeverCaptured
          row={row}
          kind={kind}
          deviceId={deviceId}
          onBackUp={onBackUp}
          backingUp={backingUp}
        />
      ) : (
        <Loaded key={`${deviceId}:${kind}`} versions={versions.data ?? []} kind={kind} />
      )}
    </div>
  );
}

function Toolbar({
  coverage,
  deviceId,
  onDevice,
  kind,
  onKind,
}: Pick<HistoryTabProps, 'coverage' | 'deviceId' | 'onDevice' | 'kind' | 'onKind'>) {
  return (
    <div className={styles.toolbar}>
      <DevicePicker coverage={coverage} value={deviceId} onChange={onDevice} />
      <Select
        aria-label="Which configuration"
        className={styles.kind}
        value={kind}
        onChange={(event) => onKind(event.target.value as ConfigKind)}
        size="sm"
        options={[
          { value: 'running', label: KIND_LABEL.running },
          { value: 'startup', label: KIND_LABEL.startup },
        ]}
      />
    </div>
  );
}

/**
 * Nothing has ever been captured. This is not an empty configuration and it
 * does not pretend to be one: there is no version, no time and no checksum,
 * and the way out is one button.
 */
function NeverCaptured({
  row,
  kind,
  deviceId,
  onBackUp,
  backingUp,
}: {
  row: Coverage | undefined;
  kind: ConfigKind;
  deviceId: string;
  onBackUp: (deviceId: string) => void;
  backingUp: boolean;
}) {
  const skipped = row?.skipped;
  const error = row?.last_error;
  return (
    <EmptyState
      icon={Camera}
      title={`No ${kind === 'running' ? 'running' : 'startup'} configuration has ever been captured${
        row ? ` from ${row.name}` : ''
      }`}
      description={
        <>
          <p className={styles.emptyLine}>
            This device has no stored {kind} configuration at all — which is different from having an
            empty one. Nobody has looked yet, so Nocturne has nothing to show, diff or check.
          </p>
          {skipped && (
            <p className={styles.emptyLine} data-testid="skip-reason">
              Nocturne does not back this device up: {skipped}
            </p>
          )}
          {error && (
            <p className={styles.emptyLine} data-testid="last-error">
              The last attempt failed: {error}
            </p>
          )}
        </>
      }
      action={
        <Button variant="primary" icon={Camera} loading={backingUp} onClick={() => onBackUp(deviceId)}>
          Back up this device now
        </Button>
      }
    />
  );
}

function Loaded({ versions, kind }: { versions: ConfigVersion[]; kind: ConfigKind }) {
  const latest = versions[0];
  const [viewing, setViewing] = useState<number | null>(latest?.id ?? null);
  /** Versions ticked for comparison, oldest tick first. */
  const [compare, setCompare] = useState<number[]>([]);

  const pair = compare.length === 2 ? [...compare].sort((a, b) => a - b) : null;
  const diff = useDiff(pair?.[0] ?? null, pair?.[1] ?? null, 3);

  const toggleCompare = (id: number) => {
    setCompare((current) =>
      current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id].slice(-2),
    );
  };

  return (
    <Splitter
      sized="first"
      defaultSize={320}
      minSize={240}
      maxSize={520}
      storageId="config.history"
      handleLabel="Resize the version list"
      className={styles.split}
    >
      <VersionList
        versions={versions}
        viewing={viewing}
        onView={setViewing}
        compare={compare}
        onCompare={toggleCompare}
      />
      {pair ? (
        <div className={styles.pane}>
          <div className={styles.paneHead}>
            <Icon icon={GitCompare} size={14} />
            <span className={styles.paneTitle}>
              Version {pair[0]} compared with version {pair[1]}
            </span>
            <Button size="sm" variant="ghost" onClick={() => setCompare([])}>
              Stop comparing
            </Button>
          </div>
          {diff.isPending ? (
            <Skeleton shape="block" height={320} />
          ) : diff.isError ? (
            <ErrorState
              title="The two versions could not be compared"
              error={diff.error}
              onRetry={() => void diff.refetch()}
              size="compact"
            />
          ) : diff.data ? (
            <>
              <RedactedNote redacted={diff.data.redacted} text={rawOf(diff.data.diff.lines)} />
              <VersionDiff diff={diff.data} label={`Version ${pair[0]} compared with version ${pair[1]}`} />
            </>
          ) : null}
        </div>
      ) : (
        <Viewer versionId={viewing ?? latest?.id ?? null} versions={versions} kind={kind} />
      )}
    </Splitter>
  );
}

/** The text of a diff, for counting the redactions in what is on screen. */
function rawOf(lines: readonly { text: string }[]): string {
  return lines.map((line) => line.text).join('\n');
}

function VersionList({
  versions,
  viewing,
  onView,
  compare,
  onCompare,
}: {
  versions: ConfigVersion[];
  viewing: number | null;
  onView: (id: number) => void;
  compare: number[];
  onCompare: (id: number) => void;
}) {
  return (
    <div className={styles.versions}>
      <div className={styles.versionsHead}>
        <span>{plural(versions.length, 'version')}</span>
        <span className={styles.hint}>
          {compare.length === 1 ? 'Tick one more to compare' : 'Tick two to compare'}
        </span>
      </div>
      <ul className={styles.versionList} aria-label="Stored versions">
        {versions.map((version, index) => {
          const previous = versions[index + 1];
          const selected = viewing === version.id;
          return (
            <li key={version.id}>
              <div className={cx(styles.version, selected && styles.versionOn)} data-testid="version-row">
                <Checkbox
                  checked={compare.includes(version.id)}
                  onChange={() => onCompare(version.id)}
                  aria-label={`Compare version ${version.id}`}
                />
                <button
                  type="button"
                  className={styles.versionButton}
                  aria-current={selected || undefined}
                  onClick={() => onView(version.id)}
                >
                  <span className={styles.versionWhen}>
                    <TimeAgo at={version.last_seen} />
                    {index === 0 && <Badge tone="accent">Latest</Badge>}
                    {version.pinned && (
                      <Tooltip content={version.pin_reasons?.join('; ') ?? 'Pinned: never pruned'}>
                        <span className={styles.pin}>
                          <Icon icon={Pin} size={11} label="Pinned" />
                        </span>
                      </Tooltip>
                    )}
                  </span>
                  <span className={styles.versionMeta}>
                    <span>{originLabel(version.origin)}</span>
                    <span>·</span>
                    <span>{formatCount(version.lines)} lines</span>
                    {previous && (
                      <>
                        <span>·</span>
                        <span>{version.lines - previous.lines >= 0 ? '+' : '−'}
                          {Math.abs(version.lines - previous.lines)} vs the one before
                        </span>
                      </>
                    )}
                  </span>
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function Viewer({
  versionId,
  versions,
  kind,
}: {
  versionId: number | null;
  versions: ConfigVersion[];
  kind: ConfigKind;
}) {
  const [revealed, setRevealed] = useState(false);
  const [query, setQuery] = useState('');
  const [hit, setHit] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();

  const text = useConfigText(versionId, !revealed);
  const pin = usePin();
  const unpin = useUnpin();
  const restore = useRestore();
  const version = versions.find((entry) => entry.id === versionId);

  // A different version, or hidden secrets again: start the search over.
  useEffect(() => setHit(0), [versionId, revealed, query]);

  const found = useMemo(
    () => findInText(text.data?.text ?? '', query),
    [text.data?.text, query],
  );

  const reveal = async () => {
    if (revealed) {
      setRevealed(false);
      return;
    }
    if (await askToReveal()) setRevealed(true);
  };

  const onRestore = async () => {
    if (!version) return;
    const ok = await confirm({
      title: `Restore version ${version.id}?`,
      description:
        'Nocturne drafts a change that would make the running configuration equal this stored version. Nothing is sent to the device: the change is planned, read and approved like any other.',
      confirmLabel: 'Draft the change',
    });
    if (!ok) return;
    restore.mutate(
      { version_id: version.id },
      {
        onSuccess: (handover) => {
          toast.success('A change was drafted', {
            description: 'Read the commands it would send, then approve it.',
            action: { label: 'Open the change', onSelect: () => navigate(`/changes/${handover.change_id}`) },
          });
        },
      },
    );
  };

  if (versionId === null) {
    return (
      <div className={styles.pane}>
        <EmptyState icon={FileClock} title="Pick a version" size="compact" />
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <div className={styles.paneHead}>
        <span className={styles.paneTitle}>
          {KIND_LABEL[kind]}
          {version && <span className={styles.paneVersion}>version {version.id}</span>}
        </span>
        <div className={styles.find}>
          <SearchInput
            ref={searchRef}
            size="sm"
            aria-label="Find in this configuration"
            placeholder="Find in this configuration"
            value={query}
            onChange={setQuery}
          />
          {query !== '' && (
            <span className={styles.findCount} data-testid="find-count">
              {found.ranges.length === 0
                ? 'no match'
                : `${(hit % found.ranges.length) + 1} of ${found.ranges.length}`}
            </span>
          )}
          {found.ranges.length > 1 && (
            <>
              <Button size="sm" variant="ghost" onClick={() => setHit((at) => at - 1)}>
                Previous
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setHit((at) => at + 1)}>
                Next
              </Button>
            </>
          )}
        </div>
        {version && (
          <>
            <Button
              size="sm"
              variant="ghost"
              icon={version.pinned ? PinOff : Pin}
              loading={pin.isPending || unpin.isPending}
              onClick={() => {
                if (version.pinned) unpin.mutate({ version_id: version.id });
                else pin.mutate({ version_id: version.id, reason: 'Kept by hand from the Configuration screen' });
              }}
            >
              {version.pinned ? 'Unpin' : 'Pin'}
            </Button>
            <Button
              size="sm"
              variant="secondary"
              icon={RotateCcw}
              loading={restore.isPending}
              onClick={() => void onRestore()}
            >
              Restore this version…
            </Button>
          </>
        )}
      </div>

      {version && <VersionMeta version={version} />}

      {text.isPending ? (
        <Skeleton shape="block" height={360} />
      ) : text.isError ? (
        <ErrorState
          title="The configuration could not be read"
          error={text.error}
          onRetry={() => void text.refetch()}
          size="compact"
        />
      ) : text.data ? (
        <>
          <RedactedNote
            redacted={text.data.redacted}
            text={text.data.text}
            busy={text.isFetching}
            onToggle={() => void reveal()}
          />
          {isEmptyCapture(text.data.version) ? (
            <div className={styles.emptyCapture} data-testid="empty-capture">
              <Icon icon={Search} size={16} />
              <p>
                This capture is empty. The device answered and the answer had no configuration lines in
                it — that is what was stored, checksum{' '}
                <code>{text.data.version.sha256.slice(0, 12)}</code>, taken{' '}
                <TimeAgo at={text.data.version.first_seen} />. It is not a device that has never been
                captured.
              </p>
            </div>
          ) : (
            <Suspense fallback={<Skeleton shape="block" height={360} />}>
              <ConfigView
                text={text.data.text}
                label={`${KIND_LABEL[kind]}, version ${text.data.version.id}`}
                matches={found.ranges}
                current={found.ranges.length === 0 ? 0 : ((hit % found.ranges.length) + found.ranges.length) % found.ranges.length}
              />
            </Suspense>
          )}
        </>
      ) : null}
    </div>
  );
}

/** When it was taken, how, how big, and what it hashes to. */
function VersionMeta({ version }: { version: ConfigVersion }) {
  return (
    <dl className={styles.meta} data-testid="version-meta">
      <div>
        <dt>Captured</dt>
        <dd>
          <TimeAgo at={version.first_seen} />
        </dd>
      </div>
      <div>
        <dt>Last seen unchanged</dt>
        <dd>
          <TimeAgo at={version.last_seen} />
        </dd>
      </div>
      <div>
        <dt>How</dt>
        <dd>{originSentence(version.origin)}</dd>
      </div>
      <div>
        <dt>Size</dt>
        <dd>
          {formatCount(version.lines)} lines, {formatBytes(version.bytes)}
        </dd>
      </div>
      <div>
        <dt>Checksum</dt>
        <dd>
          <code>{version.sha256.slice(0, 12)}</code>
        </dd>
      </div>
      <div>
        <dt>Pinned</dt>
        <dd>{version.pinned ? (version.pin_reasons?.join('; ') ?? 'Yes') : EMPTY}</dd>
      </div>
    </dl>
  );
}
