/**
 * What a search found: blocks, addresses and VLANs, in three lists.
 *
 * `ipam.search` answers an address with both its record **and** the block that
 * contains it, so the two are shown as two different kinds of hit rather than
 * merged into one row. A query that matches nothing says so for each kind: a
 * missing list would read as a list that was not looked at.
 *
 * Addresses need one more step. On top of what its text search finds, the
 * backend adds a row for a whole typed address in the **production**
 * namespace whether or not anything is known there, so an address documented
 * in a lab arrives twice: once with its record and sightings, once empty.
 * {@link addressHits} keys the hits by address *and* namespace and drops an
 * empty row for an address something knows elsewhere — while keeping the
 * empty row when nothing knows the address anywhere, because that is the
 * answer to that search. Each hit is labelled with its namespace, so a lab
 * and production never read as a duplicate of each other.
 */

import { SearchX } from 'lucide-react';
import type { SearchResult } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount } from '@/lib/format';
import { addressHits, addressScope, addressState, STATE_WORDS } from './addresses';
import styles from './SearchPanel.module.css';
import { recordState, scopeOfSite, siteLabel } from './tree';

export interface SearchPanelProps {
  query: string;
  result: SearchResult;
  labNames: ReadonlyMap<string, string>;
  /** The site of a block by id, so an address row knows its namespace. */
  siteOf: (prefixId: string) => string | null | undefined;
  onOpenPrefix: (id: string) => void;
  onOpenAddress: (ip: string, scope: string) => void;
  onClear: () => void;
}

export function SearchPanel({
  query,
  result,
  labNames,
  siteOf,
  onOpenPrefix,
  onOpenAddress,
  onClear,
}: SearchPanelProps) {
  const hits = addressHits(result.addresses, (address) => addressScope(address, siteOf));
  const nothing =
    result.prefixes.length === 0 && hits.length === 0 && result.vlans.length === 0;

  if (nothing) {
    return (
      <EmptyState
        icon={SearchX}
        title={`Nothing matches “${query}”`}
        description="No block, no address record, no sighting and no VLAN. A partial address, a MAC in any notation, a hostname, a tag or a VLAN name all work here."
        action={
          <Button variant="secondary" onClick={onClear}>
            Clear the search
          </Button>
        }
      />
    );
  }

  return (
    <div className={styles.wrap}>
      <section className={styles.section} aria-label="Blocks that match">
        <h3 className={styles.heading}>
          Blocks<span className={styles.count}>{formatCount(result.prefixes.length)}</span>
        </h3>
        {result.prefixes.length === 0 ? (
          <p className={styles.quiet}>No block matches, and no block contains a typed address.</p>
        ) : (
          <ul className={styles.list}>
            {result.prefixes.map((prefix) => {
              const record = recordState(prefix);
              return (
                <li key={prefix.id}>
                  <button type="button" className={styles.row} onClick={() => onOpenPrefix(prefix.id)}>
                    <span className={styles.mono}>{prefix.cidr}</span>
                    <Badge tone={record.tone} variant="outline">
                      {record.label}
                    </Badge>
                    {prefix.name !== '' && <span className={styles.name}>{prefix.name}</span>}
                    <span className={styles.meta}>
                      {siteLabel(prefix.site, labNames) ?? 'no site'}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className={styles.section} aria-label="Addresses that match">
        <h3 className={styles.heading}>
          Addresses<span className={styles.count}>{formatCount(hits.length)}</span>
        </h3>
        {hits.length === 0 ? (
          <p className={styles.quiet}>
            No address record and no sighting matches this. A record and a sighting are searched
            separately, and neither had a hit.
          </p>
        ) : (
          <ul className={styles.list}>
            {hits.map(({ address, scope }) => {
              const words = STATE_WORDS[addressState(address)];
              const namespace = scope === '' ? null : scopeOfSite(scope, labNames);
              return (
                <li key={`${scope}-${address.ip}`}>
                  <button
                    type="button"
                    className={styles.row}
                    onClick={() => onOpenAddress(address.ip, scope)}
                  >
                    <span className={styles.mono}>{address.ip}</span>
                    <Badge tone={words.tone} variant="outline">
                      {words.label}
                    </Badge>
                    {namespace && (
                      <>
                        <LabPill />
                        <span className={styles.name}>{namespace.label}</span>
                      </>
                    )}
                    {address.hostname && <span className={styles.name}>{address.hostname}</span>}
                    {address.mac && <span className={styles.mono}>{address.mac}</span>}
                    <span className={styles.meta}>
                      {address.last_seen === null ? (
                        'never seen'
                      ) : (
                        <>
                          seen <TimeAgo at={address.last_seen} />
                        </>
                      )}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className={styles.section} aria-label="VLANs that match">
        <h3 className={styles.heading}>
          VLANs<span className={styles.count}>{formatCount(result.vlans.length)}</span>
        </h3>
        {result.vlans.length === 0 ? (
          <p className={styles.quiet}>No VLAN record or collected VLAN matches.</p>
        ) : (
          <ul className={styles.list}>
            {result.vlans.map((vlan) => (
              <li key={`${vlan.site ?? ''}-${vlan.vid}`}>
                <span className={styles.row}>
                  <span className={styles.mono}>VLAN {vlan.vid}</span>
                  <span className={styles.name}>{vlan.name}</span>
                  <span className={styles.meta}>
                    {vlan.documented ? 'documented' : 'no record'} ·{' '}
                    {vlan.devices.length === 0
                      ? 'no device reports it'
                      : `${formatCount(vlan.devices.length)} devices`}
                  </span>
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
