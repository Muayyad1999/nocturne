/**
 * "Where is this plugged in?" — `locate.host` by MAC, IP or the address a
 * person has in their hand.
 *
 * The backend resolves an address to a MAC through the collected ARP tables,
 * finds that MAC in the collected MAC tables, and prefers the port that no
 * neighbour protocol sees anything on: the edge. Every other sighting is the
 * path frames take to get there, not where the thing is, and the backend says
 * so in `how`. This panel prints `how` in full, marks the port it decided on,
 * and keeps the dismissed sightings visible underneath — a guess that hides
 * its workings is worse than no answer.
 */

import { MapPin, Search } from 'lucide-react';
import type { Located, Location } from '@/api/types/topology';
import type { RpcError } from '@/api/errors';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import { plural } from '@/lib/format';
import styles from './HostPanel.module.css';

export interface HostPanelProps {
  query: string;
  located: Located | undefined;
  pending: boolean;
  error: RpcError | null;
  onSelectDevice: (deviceId: string) => void;
}

const KIND_WORD: Record<Location['kind'], string> = {
  access: 'access port',
  trunk: 'trunk',
  uplink: 'uplink',
};

export function HostPanel({ query, located, pending, error, onSelectDevice }: HostPanelProps) {
  if (query.trim() === '') {
    return (
      <section className={styles.panel} aria-label="Find a host">
        <EmptyState
          icon={Search}
          title="Find where a host is plugged in"
          description="Type an IP or a MAC address in the search box. Nocturne reads the ARP and MAC tables it has already collected and says which switch port it decided on, and why."
        />
      </section>
    );
  }

  return (
    <section className={styles.panel} aria-label="Find a host">
      {pending && (
        <div className={styles.loading} aria-busy="true">
          <Skeleton />
          <Skeleton width="70%" />
          <Skeleton width="85%" />
        </div>
      )}

      {/* `not_found` is an answer, not a breakage: it is the engine's sentence. */}
      {error && (
        <div className={styles.nothing}>
          <h3>Nothing found for “{query}”</h3>
          <p>{error.message}</p>
        </div>
      )}

      {located && !pending && <Answer located={located} onSelectDevice={onSelectDevice} />}
    </section>
  );
}

function Answer({
  located,
  onSelectDevice,
}: {
  located: Located;
  onSelectDevice: (deviceId: string) => void;
}) {
  const best = located.best === undefined ? undefined : located.locations[located.best];
  const others = located.locations.filter((_location, index) => index !== located.best);

  return (
    <div className={styles.answer}>
      <header className={styles.identity}>
        {located.ip && (
          <span>
            <small>IP</small>
            <b className="mono">{located.ip}</b>
          </span>
        )}
        {located.mac && (
          <span>
            <small>MAC</small>
            <b className="mono">{located.mac}</b>
          </span>
        )}
        {located.vendor && (
          <span>
            <small>Vendor</small>
            <b>{located.vendor}</b>
          </span>
        )}
      </header>

      {best ? (
        <section className={styles.best}>
          <h3>
            <MapPin size={13} aria-hidden="true" /> The port it decided on
          </h3>
          <button type="button" className={styles.port} onClick={() => onSelectDevice(best.device_id)}>
            <b>{best.device}</b>
            <span className="mono">{best.interface}</span>
          </button>
          <div className={styles.meta}>
            <Badge tone="ok" variant="outline">
              {KIND_WORD[best.kind]}
            </Badge>
            {best.vlan !== undefined && <Badge variant="outline">VLAN {best.vlan}</Badge>}
            <Badge variant="outline">{best.entry_kind}</Badge>
            <Badge variant="outline">{plural(best.macs_on_port, 'MAC')} on the port</Badge>
            <span className={styles.seen}>
              from a table collected <TimeAgo at={best.seen_at} />
            </span>
          </div>
          <p className={styles.why}>{best.why}</p>
        </section>
      ) : (
        <section className={styles.best} data-unsure="">
          <h3>No port could be decided on</h3>
          <p className={styles.why}>
            The address was found, but none of the ports it was learnt on looks like the edge. The
            sightings are below; the reasoning is at the bottom.
          </p>
        </section>
      )}

      {others.length > 0 && (
        <section className={styles.others}>
          <h3>
            {plural(others.length, 'other sighting')} — the path frames take, not where it is
            plugged in
          </h3>
          <ul>
            {others.map((location) => (
              <li key={`${location.device_id}-${location.interface}`}>
                <button
                  type="button"
                  className={styles.port}
                  onClick={() => onSelectDevice(location.device_id)}
                >
                  <b>{location.device}</b>
                  <span className="mono">{location.interface}</span>
                </button>
                <span className={styles.kind}>{KIND_WORD[location.kind]}</span>
                <span className={styles.why}>{location.why}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* The reasoning, step by step, in the backend's own words. */}
      {located.how.length > 0 && (
        <section className={styles.how}>
          <h3>How it worked that out</h3>
          <ol>
            {located.how.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ol>
        </section>
      )}
    </div>
  );
}
