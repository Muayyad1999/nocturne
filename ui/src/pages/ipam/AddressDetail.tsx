/**
 * One address, with the record and the evidence side by side and nothing
 * merged — the same shape the backend sends.
 *
 * The left column is what a person wrote down; the right is what devices said,
 * each sighting carrying the device, the interface, when its table was read and
 * whether the address is still in it. Where the two contradict each other the
 * disagreement is shown as a disagreement: both sides, and the evidence. The
 * screen never picks a winner — the backend refuses to, and so does this.
 */

import { Pencil, Trash2 } from 'lucide-react';
import type { Address } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount } from '@/lib/format';
import styles from './AddressDetail.module.css';
import {
  addressState,
  documentedText,
  groupDisagreements,
  OBSERVATION_KIND_LABEL,
  sightingsNewestFirst,
  STATE_WORDS,
  stateMeaning,
  STATUS_LABEL,
} from './addresses';

export interface AddressDetailProps {
  open: boolean;
  address: Address | null;
  /** The block it was opened from, for the heading. */
  cidr: string | null;
  /** True when the containing block belongs to a lab. */
  lab: boolean;
  deviceName: (id: string) => string | undefined;
  onClose: () => void;
  onEdit: (address: Address) => void;
  onDelete: (address: Address) => void;
  deleting: boolean;
}

export function AddressDetail({
  open,
  address,
  cidr,
  lab,
  deviceName,
  onClose,
  onEdit,
  onDelete,
  deleting,
}: AddressDetailProps) {
  const state = address ? addressState(address) : null;
  const words = state ? STATE_WORDS[state] : null;
  const sightings = address ? sightingsNewestFirst(address) : [];
  // One contradiction per written-versus-seen pair, with every sighting that
  // shows it underneath: three devices reporting one wrong MAC is one fact.
  const disagreements = address ? groupDisagreements(address.disagreements, deviceName) : [];

  const record: KeyValueItem[] = address
    ? [
        { label: 'Status', value: address.documented ? STATUS_LABEL[address.status] : null },
        { label: 'Hostname', value: address.hostname },
        { label: 'Description', value: address.description },
        {
          label: 'Device',
          value: address.documented && address.device_id ? (deviceName(address.device_id) ?? address.device_id) : null,
        },
        { label: 'Interface', value: address.documented ? address.interface : null, mono: true },
        { label: 'MAC', value: address.documented ? address.mac : null, mono: true },
        { label: 'Tags', value: address.tags.length > 0 ? address.tags.join(', ') : null },
        {
          label: 'Written',
          value: documentedText(address) ?? null,
        },
      ]
    : [];

  return (
    <Drawer
      open={open}
      onClose={onClose}
      title={address?.ip ?? 'Address'}
      description={cidr ? `in ${cidr}` : undefined}
      footer={
        address && (
          <>
            {address.documented && (
              <Button
                variant="danger"
                icon={Trash2}
                loading={deleting}
                onClick={() => onDelete(address)}
              >
                Delete the record
              </Button>
            )}
            <Button variant="primary" icon={Pencil} onClick={() => onEdit(address)}>
              {address.documented ? 'Edit the record' : 'Write a record'}
            </Button>
          </>
        )
      }
    >
      {address && words && (
        <div className={styles.body}>
          <div className={styles.state}>
            <Badge tone={words.tone} variant="soft">
              {words.label}
            </Badge>
            {lab && <LabPill />}
            <p className={styles.meaning}>{stateMeaning(address)}</p>
          </div>

          {address.disagreements.length > 0 && (
            <section className={styles.section} aria-label="Disagreements">
              <h3 className={styles.heading} data-tone="crit">
                The record and the network disagree
              </h3>
              <ul className={styles.disagreements}>
                {disagreements.map((text) => (
                  <li key={`${text.field}-${text.documented}-${text.observed}`}>
                    <span className={styles.field}>{text.field}</span>
                    <span className={styles.sides}>
                      <span className={styles.documented}>
                        <span className={styles.sideLabel}>written</span>
                        <span className={styles.mono}>{text.documented}</span>
                      </span>
                      <span className={styles.observed}>
                        <span className={styles.sideLabel}>on the network</span>
                        <span className={styles.mono}>{text.observed}</span>
                      </span>
                    </span>
                    <ul className={styles.evidence}>
                      {text.evidence.map((line) => (
                        <li key={line}>{line}</li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
              <p className={styles.hint}>
                Nothing here is resolved for you: a record is left exactly as it was written.
              </p>
            </section>
          )}

          <section className={styles.section} aria-label="The record">
            <h3 className={styles.heading}>What a person wrote down</h3>
            {address.documented ? (
              <KeyValue items={record} layout="rows" />
            ) : (
              <p className={styles.quiet}>
                Nothing. No record exists for this address — only what the network reported, below.
              </p>
            )}
          </section>

          <section className={styles.section} aria-label="The evidence">
            <h3 className={styles.heading}>
              What the devices said
              <span className={styles.count}>{formatCount(sightings.length)}</span>
            </h3>
            {sightings.length === 0 ? (
              <p className={styles.quiet}>
                Nothing has ever reported this address. Whether that means it is unused depends on
                what has been collected — the block’s coverage is on the left.
              </p>
            ) : (
              <ul className={styles.sightings}>
                {sightings.map((sighting, index) => (
                  <li key={`${sighting.device_id}-${sighting.interface}-${sighting.kind}-${index}`}>
                    <div className={styles.sightingHead}>
                      <Badge
                        tone={sighting.kind === 'interface' ? 'ok' : 'info'}
                        variant="outline"
                      >
                        {sighting.kind === 'interface' ? 'interface' : 'ARP'}
                      </Badge>
                      <span className={styles.device}>{sighting.device}</span>
                      {sighting.interface !== '' && (
                        <span className={styles.mono}>{sighting.interface}</span>
                      )}
                      {!sighting.current && (
                        <Badge tone="idle" variant="outline">
                          no longer in that table
                        </Badge>
                      )}
                    </div>
                    <p className={styles.sightingDetail}>
                      {OBSERVATION_KIND_LABEL[sighting.kind]}
                      {sighting.cidr && ` as ${sighting.cidr}`}
                      {sighting.mac && ` · ${sighting.mac}`}
                      {sighting.vendor && ` (${sighting.vendor})`}
                    </p>
                    <p className={styles.sightingDetail}>
                      table read <TimeAgo at={sighting.collected_at} /> · first seen{' '}
                      <TimeAgo at={sighting.first_seen} /> · last seen{' '}
                      <TimeAgo at={sighting.last_seen} /> · collected by {sighting.collector}
                    </p>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </div>
      )}
    </Drawer>
  );
}
