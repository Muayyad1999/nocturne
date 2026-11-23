/**
 * Detections: things worth telling somebody about that no single log line
 * and no single metric shows.
 *
 * The catalogue is short and fixed, and each entry is stated in full: what
 * it is decided from, what it cannot see, whether it is switched on, whether
 * this build can run it at all (and the reason when it cannot), the last
 * check that could not be made, and how often it fired in a day. A
 * detection only tells — it never changes a device — and a hit goes into
 * the one alert list; there is no second inbox here, only the record.
 *
 * The switch is `detections.set_enabled`. While detections are off as a
 * whole the engine refuses to switch one on, and its sentence is shown where
 * the switch is.
 *
 * Below the catalogue are the recent hits with their evidence: ids and
 * counts, as text. `detections.hit` puts a new one in front of the list as
 * it happens; `detections.changed` refreshes the catalogue.
 */

import { ShieldAlert, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import type { Device } from '@/api/types/core';
import type { Detection, DetectionHit } from '@/api/types/detect';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { KeyValue } from '@/design/KeyValue';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useDetections, useDevices, useRecentHits, useSetDetectionEnabled } from './data';
import { allOff, detectionName, evidenceFacts, hitLinks, hitsPhrase, severityTone, standingOf } from './detections';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';

export interface DetectionsPaneProps {
  capabilities: Capabilities;
}

export function DetectionsPane({ capabilities }: DetectionsPaneProps) {
  const catalogue = useDetections();
  const devices = useDevices(capabilities.devices);
  const [filter, setFilter] = useState('');
  const [openHit, setOpenHit] = useState<DetectionHit | null>(null);

  if (catalogue.isError) {
    return (
      <ErrorState
        title="The detections could not be read"
        error={catalogue.error}
        onRetry={() => void catalogue.refetch()}
      />
    );
  }
  if (!catalogue.data) {
    return (
      <div className={styles.paneBody}>
        <Skeleton shape="block" height={140} />
        <Skeleton shape="block" height={140} />
        <Skeleton shape="block" height={140} />
      </div>
    );
  }

  const entries = catalogue.data;
  const names = new Map((devices.data ?? []).map((device) => [device.id, device]));

  return (
    <div className={styles.paneBody}>
      <p className={styles.paneLead}>
        A short, fixed catalogue: each detection puts two sources side by side and says what it was
        decided from. It only tells — it never changes a device and never drafts a change — and a hit
        is raised in the one alert list, with the same notification channels as every other alert.
        Nocturne has one operator identity, not users, so “who” in a hit is whatever the device said,
        never an account.
      </p>

      {allOff(entries) && (
        <p className={styles.note} role="note">
          Every detection reads as switched off. If detections are off as a whole — the{' '}
          <code className={styles.inlineCode}>detect</code> section’s <code className={styles.inlineCode}>enabled</code>{' '}
          setting — none can be switched on here until that is switched back on.
        </p>
      )}

      <ul className={styles.catalogue} aria-label="Detection catalogue">
        {entries.map((entry) => (
          <li key={entry.id}>
            <DetectionCard entry={entry} capabilities={capabilities} onShowHits={() => setFilter(entry.id)} />
          </li>
        ))}
      </ul>

      {capabilities.detectionRecent && (
        <RecentHits
          catalogue={entries}
          devices={names}
          filter={filter}
          onFilter={setFilter}
          onOpen={setOpenHit}
        />
      )}

      {openHit !== null && (
        <HitDrawer hit={openHit} catalogue={entries} devices={names} onClose={() => setOpenHit(null)} />
      )}
    </div>
  );
}

function DetectionCard({
  entry,
  capabilities,
  onShowHits,
}: {
  entry: Detection;
  capabilities: Capabilities;
  onShowHits: () => void;
}) {
  const change = useSetDetectionEnabled();
  const standing = standingOf(entry);

  return (
    <Card className={styles.detection} data-detection={entry.id} data-standing={standing.kind}>
      <header className={styles.detectionHead}>
        <div className={styles.detectionTitle}>
          <h3 className={styles.cardTitle}>{entry.name}</h3>
          <StatusPill tone={severityTone(entry.severity)}>{entry.severity}</StatusPill>
          <StatusPill tone={standing.tone}>{standing.label}</StatusPill>
        </div>
        {capabilities.detectionSwitch ? (
          <Switch
            checked={entry.enabled}
            aria-busy={change.isPending || undefined}
            className={change.isPending ? styles.busySwitch : undefined}
            aria-label={`${entry.name}: switched ${entry.enabled ? 'on' : 'off'}`}
            onChange={(event) => {
              if (change.isPending) return;
              change.mutate({ id: entry.id, enabled: event.target.checked });
            }}
          />
        ) : null}
      </header>

      <p className={styles.detectionWhat}>{entry.description}</p>

      {/* The engine's refusal, where the switch is: while detections are off
          as a whole, one cannot be switched on. */}
      {change.error && (
        <p className={styles.fieldError} role="alert">
          {change.error.message}
        </p>
      )}

      {standing.reason !== null && (
        <p className={styles.receiverError}>
          <Icon icon={TriangleAlert} size={12} /> {standing.reason}
        </p>
      )}

      {entry.last_error && (
        <p className={styles.receiverError} data-testid="detection-last-error">
          <Icon icon={TriangleAlert} size={12} /> A check could not be made <TimeAgo at={entry.last_error.at} />:{' '}
          {entry.last_error.message}. That is not a check that found nothing.
        </p>
      )}

      <dl className={styles.detectionFacts}>
        <div>
          <dt>Decided from</dt>
          <dd>{entry.decided_from}</dd>
        </div>
        <div>
          <dt>Cannot see</dt>
          <dd>{entry.cannot_see}</dd>
        </div>
      </dl>

      <div className={styles.detectionFoot}>
        {/* The count narrows the list of hits below, which a build without
            `detections.recent` does not have: there it is only a number. */}
        {capabilities.detectionRecent ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={onShowHits}
            title={`Show only the hits of “${entry.name}” below`}
          >
            {hitsPhrase(entry.hits_24h)}
          </Button>
        ) : (
          <span>{hitsPhrase(entry.hits_24h)}</span>
        )}
        {entry.last_hit_at === undefined ? (
          <span className={styles.muted}>never fired</span>
        ) : (
          <span>
            last <TimeAgo at={entry.last_hit_at} />
          </span>
        )}
      </div>
    </Card>
  );
}

function RecentHits({
  catalogue,
  devices,
  filter,
  onFilter,
  onOpen,
}: {
  catalogue: readonly Detection[];
  devices: ReadonlyMap<string, Device>;
  filter: string;
  onFilter: (id: string) => void;
  onOpen: (hit: DetectionHit) => void;
}) {
  const hits = useRecentHits(filter);

  const columns: readonly TableColumn<DetectionHit>[] = [
    {
      id: 'at',
      header: 'When',
      width: 110,
      required: true,
      cell: (hit) => <TimeAgo at={hit.at} />,
      sortValue: (hit) => hit.at,
    },
    {
      id: 'severity',
      header: 'Severity',
      width: 110,
      cell: (hit) => <StatusPill tone={severityTone(hit.severity)}>{hit.severity}</StatusPill>,
      sortValue: (hit) => hit.severity,
    },
    {
      id: 'detection',
      header: 'Detection',
      width: 240,
      cell: (hit) => detectionName(hit.detection, catalogue),
      sortValue: (hit) => detectionName(hit.detection, catalogue),
    },
    {
      id: 'title',
      header: 'What it found',
      required: true,
      // A title can hold what a device said (`by`, `from`): text, as given.
      cell: (hit) => <span title={hit.title}>{hit.title}</span>,
    },
    {
      id: 'device',
      header: 'Device',
      width: 190,
      cell: (hit) => {
        const links = hitLinks(hit);
        if (links.device === null || hit.device_id === undefined) {
          return <span className={styles.muted}>no one device</span>;
        }
        return (
          <span className={styles.hitDevice}>
            <Link to={links.device}>{devices.get(hit.device_id)?.name ?? hit.device_id}</Link>
            {hit.lab && <LabPill />}
          </span>
        );
      },
    },
    {
      id: 'alert',
      header: 'Alert',
      width: 130,
      cell: (hit) => {
        const links = hitLinks(hit);
        return links.alert === null ? (
          <span className={styles.muted} title="No alert was raised: alerts are not in this build, or the monitor refused it.">
            none raised
          </span>
        ) : (
          <Link to={links.alert} title={`Alert ${hit.alert_id}, in the alert list`}>
            In the alert list
          </Link>
        );
      },
    },
  ];

  return (
    <section aria-label="Recent hits" className={styles.hits}>
      <div className={styles.paneHead}>
        <h3 className={styles.cardTitle}>Recent hits</h3>
        <Select
          className={styles.filterControlWide}
          size="sm"
          aria-label="Detection"
          value={filter}
          onChange={(event) => onFilter(event.target.value)}
        >
          <option value="">Every detection</option>
          {catalogue.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </Select>
      </div>
      {hits.isError ? (
        <ErrorState
          size="compact"
          title="The recent hits could not be read"
          error={hits.error}
          onRetry={() => void hits.refetch()}
        />
      ) : (
        <Table
          id="logs.detections.hits"
          columns={columns}
          rows={hits.data ?? []}
          rowKey={(hit) => hit.id}
          loading={hits.isPending}
          defaultSort={{ columnId: 'at', direction: 'desc' }}
          density="comfortable"
          maxHeight={420}
          onRowActivate={onOpen}
          rowActions={(hit) => [{ label: 'Show the evidence', onSelect: () => onOpen(hit) }]}
          empty={
            <EmptyState
              size="compact"
              icon={ShieldAlert}
              title={filter === '' ? 'No detection has fired' : 'This detection has not fired'}
              description="Hits are kept in the event stream and age out with it. A detection that is switched off, or that cannot run in this build, is silent — which is not the same as finding nothing."
            />
          }
          aria-label="Recent detection hits"
        />
      )}
      {hits.data && hits.data.length > 0 && (
        <p className={styles.muted}>
          {plural(hits.data.length, 'hit')}, newest first. A new one appears here as it happens.
        </p>
      )}
    </section>
  );
}

function HitDrawer({
  hit,
  catalogue,
  devices,
  onClose,
}: {
  hit: DetectionHit;
  catalogue: readonly Detection[];
  devices: ReadonlyMap<string, Device>;
  onClose: () => void;
}) {
  const links = hitLinks(hit);
  const facts = evidenceFacts(hit.evidence);
  const entry = catalogue.find((candidate) => candidate.id === hit.detection);

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={detectionName(hit.detection, catalogue)}
      description={formatTimestamp(hit.at)}
    >
      <div className={styles.form}>
        {/* What a device said can be in here; it is text and stays text. */}
        <p className={styles.hitTitle}>{hit.title}</p>

        <div className={styles.badges}>
          <StatusPill tone={severityTone(hit.severity)}>{hit.severity}</StatusPill>
          {hit.lab && <LabPill />}
        </div>
        {/* Who is told is the monitor's decision, in one place for every
            source of alerts: this feature passes no channels of its own. */}
        {hit.lab && (
          <p className={styles.muted} data-testid="hit-lab-note">
            About a simulated device. Its alert is listed like any other and sent to no notification
            channel, unless Monitoring is set to notify about lab devices.
          </p>
        )}

        <KeyValue
          items={[
            {
              label: 'Happened',
              value: formatTimestamp(hit.at),
              mono: true,
            },
            {
              label: 'Device',
              value:
                links.device === null || hit.device_id === undefined ? (
                  <span className={styles.muted}>the hit is about no one device</span>
                ) : (
                  <Link to={links.device}>{devices.get(hit.device_id)?.name ?? hit.device_id}</Link>
                ),
            },
            {
              label: 'Alert',
              value:
                links.alert === null ? (
                  <span className={styles.muted}>
                    none was raised — alerts are not in this build, or the monitor refused it
                  </span>
                ) : (
                  <Link to={links.alert}>
                    Raised in the alert list <span className={styles.monoText}>({hit.alert_id})</span>
                  </Link>
                ),
            },
          ]}
        />

        <section aria-label="Evidence" className={styles.group}>
          <h4 className={styles.groupLegend}>Evidence — what it was decided from</h4>
          {facts.length === 0 ? (
            <p className={styles.muted}>This hit carries no evidence.</p>
          ) : (
            <KeyValue
              items={facts.map((fact) => ({ label: fact.label, value: fact.value, mono: fact.mono }))}
            />
          )}
          <p className={styles.muted}>
            Evidence is ids and counts, never the text of a log line or a configuration — apart from the
            two words a device says about a configuration change, “by” and “from”.
            {facts.some((fact) => fact.path === 'log_entry_id')
              ? ' The log entry is in the Lines pane, under its id.'
              : ''}
          </p>
        </section>

        {entry && (
          <section aria-label="What this detection cannot see" className={styles.group}>
            <h4 className={styles.groupLegend}>What this detection cannot see</h4>
            <p className={styles.hintDetail}>{entry.cannot_see}</p>
          </section>
        )}

        <p className={styles.muted}>
          One edit can be seen more than once — in the device’s log, in the next
          backup — and each sighting is a hit; they share one alert, with a count.
        </p>
      </div>
    </Drawer>
  );
}
