/**
 * Reconciliation: the plan and the network, compared — and nothing decided.
 *
 * It leads with **coverage**, because every list below it is only as good as
 * what was looked at, and because the two sentences a reconciliation can say
 * about a missing address — "nobody looked" and "it is not there" — are
 * different claims that must never be allowed to look alike.
 *
 * Then it shows `refused`: the decisions the engine will not make. Those lines
 * are not a disclaimer, they are the contract, so nothing on this screen
 * offers a button that would make one of them for the user. The one write
 * offered here is `ipam.learn`, which previews first and creates records a
 * person then owns — the explicit act the refusals describe as a decision.
 */

import { Ban, CircleCheck, Eye, FileQuestion, RotateCw, ScanSearch } from 'lucide-react';
import type { Reconciliation } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, plural } from '@/lib/format';
import { Conflicts } from './Conflicts';
import styles from './Reconcile.module.css';
import {
  coverageReading,
  groupMismatches,
  sections,
  truncatedNote,
  unobservedReading,
  type Section,
} from './reconciliation';
import { siteLabel } from './tree';

export interface ReconcileProps {
  report: Reconciliation;
  labNames: ReadonlyMap<string, string>;
  /** Names the device ids the backend puts on both sides of a `device` row. */
  deviceName: (id: string) => string | undefined;
  refreshing: boolean;
  learning: boolean;
  onRefresh: () => void;
  onLearn: () => void;
  onOpenPrefix: (id: string) => void;
  onWriteRecord: (ip: string, prefixId: string | null) => void;
}

export function Reconcile({
  report,
  labNames,
  deviceName,
  refreshing,
  learning,
  onRefresh,
  onLearn,
  onOpenPrefix,
  onWriteRecord,
}: ReconcileProps) {
  const coverage = coverageReading(report.coverage);
  const list = sections(report);
  const byId = (id: Section['id']) => list.find((section) => section.id === id);
  const truncated = truncatedNote(report);

  return (
    <div className={styles.wrap}>
      <header className={styles.head}>
        <div className={styles.headText}>
          <h2 className={styles.title}>What was looked at</h2>
          <p className={styles.headline} data-tone={coverage.tone}>
            {coverage.headline}
          </p>
          <p className={styles.detail}>{coverage.detail}</p>
          {coverage.oldest !== null && coverage.newest !== null && (
            <p className={styles.span}>
              The readings behind this span <TimeAgo at={coverage.oldest} /> to{' '}
              <TimeAgo at={coverage.newest} />. The comparison itself was made{' '}
              <TimeAgo at={report.at} />.
            </p>
          )}
        </div>
        <div className={styles.headActions}>
          <Button
            size="sm"
            variant="secondary"
            icon={RotateCw}
            loading={refreshing}
            onClick={onRefresh}
          >
            Compare again
          </Button>
          <Button
            size="sm"
            variant="primary"
            icon={ScanSearch}
            loading={learning}
            onClick={onLearn}
          >
            Document what the network shows…
          </Button>
        </div>
      </header>

      <Panel
        title="Per device"
        padding="none"
        collapsible
        defaultCollapsed
        storageId="ipam.coverage"
        actions={
          <span className={styles.count}>
            {formatCount(coverage.devicesWithAddresses)} of{' '}
            {formatCount(coverage.devicesTotal)} reported addresses
          </span>
        }
      >
        <table className={styles.table}>
          <thead>
            <tr>
              <th scope="col">Device</th>
              <th scope="col">Site</th>
              <th scope="col">Interfaces read</th>
              <th scope="col">ARP read</th>
              <th scope="col" className={styles.number}>
                Addresses
              </th>
              <th scope="col" className={styles.number}>
                ARP entries
              </th>
              <th scope="col">Why it says nothing</th>
            </tr>
          </thead>
          <tbody>
            {report.coverage.devices.map((device) => (
              <tr key={device.device_id} data-silent={device.interfaces_at === null || undefined}>
                <td>
                  <span className={styles.device}>
                    {device.device}
                    {device.lab && <LabPill />}
                  </span>
                </td>
                <td>{siteLabel(device.site, labNames) ?? '—'}</td>
                <td>
                  {device.interfaces_at === null ? (
                    <span className={styles.never}>never</span>
                  ) : (
                    <TimeAgo at={device.interfaces_at} />
                  )}
                </td>
                <td>
                  {device.arp_at === null ? (
                    <span className={styles.never}>never</span>
                  ) : (
                    <TimeAgo at={device.arp_at} />
                  )}
                </td>
                <td className={styles.number}>{formatCount(device.addresses_configured)}</td>
                <td className={styles.number}>{formatCount(device.arp_entries)}</td>
                <td className={styles.reason}>{device.reason ?? ''}</td>
              </tr>
            ))}
            {report.coverage.devices.length === 0 && (
              <tr>
                <td colSpan={7} className={styles.reason}>
                  There is no device in the inventory.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Panel>

      <section className={styles.refused} aria-label="What this report will not decide">
        <h3 className={styles.refusedTitle}>
          <Icon icon={Ban} size={13} />
          What this report will not decide for you
        </h3>
        {report.refused.length === 0 ? (
          <p className={styles.detail}>The engine listed no refusals for this report.</p>
        ) : (
          <ul className={styles.refusedList}>
            {report.refused.map((line) => (
              // The engine's own words. Each one is a decision it leaves to a
              // person, which is why no button here makes it.
              <li key={line}>{line}</li>
            ))}
          </ul>
        )}
      </section>

      {truncated && (
        <p className={styles.truncated} role="status">
          {truncated}
        </p>
      )}

      <SectionPanel section={byId('undocumented')} storageId="ipam.undocumented">
        <ul className={styles.rows}>
          {report.undocumented.map((row) => (
            <li key={`${row.scope}-${row.ip}`} className={styles.row}>
              <div className={styles.rowMain}>
                <span className={styles.mono}>{row.ip}</span>
                {row.prefix !== null && (
                  <Tooltip
                    content={
                      row.prefix_documented
                        ? 'It falls inside a documented block.'
                        : 'Even the block it falls in exists only because something was seen inside it.'
                    }
                  >
                    <Badge tone={row.prefix_documented ? 'info' : 'warn'} variant="outline">
                      {row.prefix}
                    </Badge>
                  </Tooltip>
                )}
                <span className={styles.summary}>{row.summary}</span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => onWriteRecord(row.ip, row.prefix_id)}
                >
                  Write a record
                </Button>
              </div>
              <ul className={styles.evidence}>
                {row.observations.slice(0, 3).map((observation, index) => (
                  <li key={`${observation.device_id}-${observation.kind}-${index}`}>
                    <Icon icon={Eye} size={11} />
                    {observation.kind === 'interface' ? 'configured on' : 'answered to'}{' '}
                    {observation.device}
                    {observation.interface ? ` (${observation.interface})` : ''} · read{' '}
                    <TimeAgo at={observation.collected_at} />
                  </li>
                ))}
                {row.observations.length > 3 && (
                  <li>{formatCount(row.observations.length - 3)} more sightings</li>
                )}
              </ul>
            </li>
          ))}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('unobserved')} storageId="ipam.unobserved">
        <ul className={styles.rows}>
          {report.unobserved.map((row) => {
            const reading = unobservedReading(row);
            return (
              <li key={`${row.scope}-${row.ip}`} className={styles.row}>
                <div className={styles.rowMain}>
                  <span className={styles.mono}>{row.ip}</span>
                  {row.hostname !== '' && <span className={styles.host}>{row.hostname}</span>}
                  <Badge tone={reading.tone} variant="soft">
                    {reading.headline}
                  </Badge>
                  {row.prefix !== null && (
                    <span className={styles.block}>in {row.prefix}</span>
                  )}
                </div>
                <p className={styles.rowDetail}>{reading.detail}</p>
                <p className={styles.rowDetail}>
                  Written <TimeAgo at={row.documented_at} /> as “{row.status}”.
                </p>
              </li>
            );
          })}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('mismatches')} storageId="ipam.mismatches">
        <ul className={styles.rows}>
          {/* One contradiction per address and field; the devices that show
              it are its evidence, not three separate faults. */}
          {groupMismatches(report.mismatches, deviceName).map((row) => (
            <li
              key={`${row.ip}-${row.field}-${row.documented}-${row.observed}`}
              className={styles.row}
            >
              <div className={styles.rowMain}>
                <span className={styles.mono}>{row.ip}</span>
                <Badge tone="crit" variant="soft">
                  {row.field}
                </Badge>
                <span className={styles.sides}>
                  <span className={styles.sideLabel}>written</span>
                  <span className={styles.mono}>{row.documented}</span>
                  <span className={styles.sideLabel}>on the network</span>
                  <span className={styles.mono}>{row.observed}</span>
                </span>
              </div>
              <ul className={styles.evidence}>
                {row.evidence.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('mask_mismatches')} storageId="ipam.masks">
        <ul className={styles.rows}>
          {report.mask_mismatches.map((row, index) => (
            <li key={`${row.ip}-${row.device_id}-${index}`} className={styles.row}>
              <div className={styles.rowMain}>
                <span className={styles.mono}>{row.ip}</span>
                <Badge tone="warn" variant="soft">
                  /{row.configured_len} configured, block is /{row.prefix_len}
                </Badge>
                <span className={styles.summary}>
                  {row.device}
                  {row.interface ? ` (${row.interface})` : ''} in {row.prefix}
                </span>
                <Button size="sm" variant="ghost" onClick={() => onOpenPrefix(row.prefix_id)}>
                  Open the block
                </Button>
              </div>
              <p className={styles.rowDetail}>
                read <TimeAgo at={row.collected_at} />
              </p>
            </li>
          ))}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('prefixes_not_documented')} storageId="ipam.undocumented-blocks">
        <ul className={styles.rows}>
          {report.prefixes_not_documented.map((row) => (
            <li key={row.id} className={styles.row}>
              <div className={styles.rowMain}>
                <span className={styles.mono}>{row.cidr}</span>
                {row.site !== null && (
                  <span className={styles.block}>{siteLabel(row.site, labNames)}</span>
                )}
                <span className={styles.summary}>{row.detail}</span>
                <Button size="sm" variant="ghost" onClick={() => onOpenPrefix(row.id)}>
                  Open it
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('prefixes_without_evidence')} storageId="ipam.empty-blocks">
        <ul className={styles.rows}>
          {report.prefixes_without_evidence.map((row) => (
            <li key={row.id} className={styles.row}>
              <div className={styles.rowMain}>
                <span className={styles.mono}>{row.cidr}</span>
                {row.site !== null && (
                  <span className={styles.block}>{siteLabel(row.site, labNames)}</span>
                )}
                <span className={styles.summary}>{row.detail}</span>
                <Button size="sm" variant="ghost" onClick={() => onOpenPrefix(row.id)}>
                  Open it
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </SectionPanel>

      <SectionPanel section={byId('conflicts')} storageId="ipam.reconcile-conflicts">
        <Conflicts conflicts={report.conflicts} onOpenSubject={() => undefined} />
      </SectionPanel>
    </div>
  );
}

interface SectionPanelProps {
  section: Section | undefined;
  storageId: string;
  children: React.ReactNode;
}

/**
 * One section of the report. A section that found nothing is still here, with
 * what finding nothing means — an absent section would read as an absent
 * question.
 */
function SectionPanel({ section, storageId, children }: SectionPanelProps) {
  if (!section) return null;
  const empty = section.count === 0;
  return (
    <Panel
      title={section.title}
      padding="md"
      collapsible
      defaultCollapsed={empty}
      storageId={storageId}
      actions={
        <span className={styles.count}>
          {empty ? (
            <span className={styles.clear}>
              <Icon icon={CircleCheck} size={12} />
              none
            </span>
          ) : (
            <Badge tone={section.tone} variant="soft">
              {section.truncated ? `at least ${formatCount(section.count)}` : formatCount(section.count)}
            </Badge>
          )}
        </span>
      }
    >
      <p className={styles.about}>{section.about}</p>
      {empty ? (
        <p className={styles.emptySentence}>
          <Icon icon={FileQuestion} size={12} />
          {section.empty}
        </p>
      ) : (
        children
      )}
    </Panel>
  );
}

/** The one-line summary the toolbar shows while this tab is open. */
export function reconcileSummary(report: Reconciliation): string {
  const list = sections(report);
  const found = list.filter((section) => section.count > 0);
  if (found.length === 0) return 'The plan and the network agree, as far as anything was read';
  return found.map((section) => `${plural(section.count, 'row')} ${section.title.toLowerCase()}`).join(' · ');
}
