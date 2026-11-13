/**
 * The side panel: one node, or one link, with what the backend knows about it.
 *
 * A node's "connected to" list is `topology.neighbors`, which resolves both
 * ends of every link the device has and carries the warnings that mention it.
 * A link's panel leads with how the link is known, because that is the thing a
 * person is about to act on.
 */

import { ExternalLink, Pencil, Pin, PinOff, Plus, Route } from 'lucide-react';
import { useNavigate } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { Graph, Link, NeighborRow, Neighbors, Node } from '@/api/types/topology';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import { EMPTY, formatPercent, formatRate, plural } from '@/lib/format';
import { evidence, nodeKindLabel } from './graph';
import styles from './Inspector.module.css';

const STATUS_TONE = { up: 'ok', down: 'crit', unknown: 'idle' } as const;
const STATUS_WORD = { up: 'Up', down: 'Down', unknown: 'Not known' } as const;

export interface InspectorProps {
  graph: Graph;
  node: Node | null;
  link: Link | null;
  onClose: () => void;
  onTraceFrom: (node: Node) => void;
  onTraceBetween: (a: Node, b: Node) => void;
  onPin: (node: Node, pinned: boolean) => void;
  onAddToInventory: (node: Node) => void;
  onDeleteManual: (link: Link) => void;
  onSelectNode: (id: string) => void;
}

export function Inspector(props: InspectorProps) {
  const { node, link } = props;
  return (
    <aside className={styles.panel} aria-label="Selection">
      {node && <NodePanel {...props} node={node} />}
      {!node && link && <LinkPanel {...props} link={link} />}
      {!node && !link && (
        <div className={styles.nothing}>
          <p>Nothing selected.</p>
          <p className={styles.hint}>
            Click a node or a link to see what the devices said about it. Drag a node to arrange the
            drawing; where you put it is saved for this view.
          </p>
        </div>
      )}
    </aside>
  );
}

// -------------------------------------------------------------------- a node

function NodePanel({
  graph,
  node,
  onTraceFrom,
  onPin,
  onAddToInventory,
  onSelectNode,
}: InspectorProps & { node: Node }) {
  const navigate = useNavigate();
  const neighbors = useRpc<Neighbors>(
    'topology.neighbors',
    node.device_id ? { device_id: node.device_id, include_endpoints: true } : undefined,
    { enabled: Boolean(node.device_id), invalidateOn: ['topology.changed'] },
  );

  const facts: KeyValueItem[] = [
    { label: 'What it is', value: nodeKindLabel(node.kind) },
    { label: 'Address', value: node.address, mono: true },
    { label: 'Site', value: node.site },
    { label: 'Role', value: node.role },
    { label: 'Model', value: node.model, mono: true },
    { label: 'Device type', value: node.profile, mono: true },
    {
      label: 'Data collected',
      value: node.collected_at ? <TimeAgo at={node.collected_at} /> : 'never',
    },
  ];
  if (node.alerts !== undefined && node.alerts > 0) {
    facts.push({ label: 'Open alerts', value: node.alerts });
  }

  return (
    <>
      <header className={styles.head}>
        <h2>{node.name}</h2>
        <div className={styles.pills}>
          <StatusPill tone={STATUS_TONE[node.status]}>{STATUS_WORD[node.status]}</StatusPill>
          {node.lab && <LabPill />}
          {node.stale && (
            <StatusPill tone="warn">Data older than the staleness limit</StatusPill>
          )}
        </div>
      </header>

      {node.unmanaged && (
        <section className={styles.note}>
          <h3>Not in the inventory</h3>
          <p>
            {node.id.startsWith('m:')
              ? 'This end of a link was named by a person, not discovered.'
              : node.id.startsWith('h:')
                ? 'A host found in a MAC table on an access port. It is drawn where it was learnt.'
                : 'A neighbour protocol reported this device, but Nocturne does not manage it, so nothing was read from it.'}
          </p>
          {/*
            Only a neighbour another device reported. An endpoint learnt from a
            MAC table is a printer or a laptop, and an end of a drawn link is a
            name somebody typed: neither is something to try to log in to.
          */}
          {node.id.startsWith('u:') && (
            <Button size="sm" icon={Plus} onClick={() => onAddToInventory(node)}>
              Add as a device…
            </Button>
          )}
        </section>
      )}

      <KeyValue items={facts} layout="stacked" />

      <div className={styles.actions}>
        {node.device_id && (
          <Button
            size="sm"
            variant="secondary"
            icon={ExternalLink}
            onClick={() => navigate(`/devices/${node.device_id}`)}
          >
            Open device
          </Button>
        )}
        <Button size="sm" variant="secondary" icon={Route} onClick={() => onTraceFrom(node)}>
          Trace from here
        </Button>
        <Button
          size="sm"
          variant="secondary"
          icon={node.pinned ? PinOff : Pin}
          onClick={() => onPin(node, !node.pinned)}
        >
          {node.pinned ? 'Unpin' : 'Pin here'}
        </Button>
      </div>

      {node.device_id && (
        <section className={styles.section}>
          <h3>
            Connected to
            {neighbors.data && ` · ${neighbors.data.rows.length}`}
          </h3>
          {neighbors.isPending && (
            <div className={styles.loading} aria-busy="true">
              <Skeleton />
              <Skeleton width="80%" />
              <Skeleton width="60%" />
            </div>
          )}
          {neighbors.isError && <p className={styles.problem}>{neighbors.error.message}</p>}
          {neighbors.data?.rows.length === 0 && (
            <p className={styles.hint}>
              Nothing reported a link to this device, and it reported none itself.
            </p>
          )}
          <ul className={styles.neighbors}>
            {(neighbors.data?.rows ?? []).map((row) => (
              <NeighborEntry key={row.link_id} row={row} onSelectNode={onSelectNode} />
            ))}
          </ul>
          {(neighbors.data?.warnings.length ?? 0) > 0 && (
            <div className={styles.warnings}>
              <h4>{plural(neighbors.data?.warnings.length ?? 0, 'warning')} about this device</h4>
              <ul>
                {neighbors.data?.warnings.map((warning) => <li key={warning}>{warning}</li>)}
              </ul>
            </div>
          )}
        </section>
      )}

      {!node.device_id && (
        <p className={styles.hint}>
          There is nothing to read from this node: it is not a device Nocturne manages.
          {graph.stats.unmanaged > 1 &&
            ` ${graph.stats.unmanaged} nodes in this drawing are like that.`}
        </p>
      )}
    </>
  );
}

function NeighborEntry({
  row,
  onSelectNode,
}: {
  row: NeighborRow;
  onSelectNode: (id: string) => void;
}) {
  const how = evidence(row);
  return (
    <li className={styles.neighbor} data-confidence={how.confidence} data-source={how.source}>
      <button type="button" className={styles.peer} onClick={() => onSelectNode(row.peer.id)}>
        <span className="mono">{row.local_interface ?? EMPTY}</span>
        <Icon icon={Route} size={11} />
        <span>{row.peer.name}</span>
        {row.peer_interface && <span className="mono">{row.peer_interface}</span>}
      </button>
      <span className={styles.how} title={how.claim}>
        {how.chip === '' ? `confirmed · ${how.basis}` : `${how.chip} · ${how.basis}`}
      </span>
    </li>
  );
}

// -------------------------------------------------------------------- a link

function LinkPanel({
  graph,
  link,
  onTraceBetween,
  onDeleteManual,
  onSelectNode,
}: InspectorProps & { link: Link }) {
  const how = evidence(link);
  const a = graph.nodes.find((node) => node.id === link.a.node);
  const b = graph.nodes.find((node) => node.id === link.b.node);

  const facts: KeyValueItem[] = [
    { label: 'State', value: STATUS_WORD[link.status] },
    { label: 'Speed', value: link.speed_bps ? formatRate(link.speed_bps) : null },
    {
      label: 'Utilisation',
      value:
        link.util_pct === undefined ? null : formatPercent(link.util_pct, { digits: 2 }),
    },
    { label: 'VLANs', value: link.vlans, mono: true },
    { label: 'Port-channel', value: link.aggregate, mono: true },
    { label: 'Label', value: link.label },
  ];

  return (
    <>
      <header className={styles.head}>
        <h2>
          {a?.name ?? link.a.node} ↔ {b?.name ?? link.b.node}
        </h2>
        <div className={styles.pills}>
          <StatusPill tone={STATUS_TONE[link.status]}>{STATUS_WORD[link.status]}</StatusPill>
        </div>
      </header>

      {/* How it is known comes first: it is what decides whether to trust the rest. */}
      <section className={styles.claim} data-confidence={how.confidence} data-source={how.source}>
        <h3>How this link is known</h3>
        <p>{how.claim}</p>
        <dl>
          <dt>Confidence</dt>
          <dd>
            {how.confidence.replace('_', '-')}
            {how.confidenceNote !== '' && (
              <span className={styles.note2}> — {how.confidenceNote}</span>
            )}
          </dd>
          <dt>Reported by</dt>
          <dd>{how.basis}</dd>
        </dl>
      </section>

      <section className={styles.section}>
        <h3>Ends</h3>
        <ul className={styles.ends}>
          {[
            { end: link.a, node: a },
            { end: link.b, node: b },
          ].map(({ end, node }) => (
            <li key={`${end.node}-${end.interface ?? ''}`}>
              <button
                type="button"
                className={styles.peer}
                onClick={() => onSelectNode(end.node)}
                disabled={!node}
              >
                <span>{node?.name ?? end.node}</span>
                {end.interface && <span className="mono">{end.interface}</span>}
                {end.ip && <span className="mono">{end.ip}</span>}
              </button>
            </li>
          ))}
        </ul>
      </section>

      <KeyValue items={facts} layout="stacked" />

      {link.members && link.members.length > 0 && (
        <section className={styles.section}>
          <h3>{plural(link.members.length, 'member interface pair')}</h3>
          <ul className={styles.members}>
            {link.members.map((member) => (
              <li key={member} className="mono">
                {member}
              </li>
            ))}
          </ul>
        </section>
      )}

      <div className={styles.actions}>
        {a && b && (
          <Button size="sm" variant="secondary" icon={Route} onClick={() => onTraceBetween(a, b)}>
            Trace between these
          </Button>
        )}
        {/*
          A drawn link's own id is not the id the graph gives it, so it can
          only be edited or removed where it was made: the list of drawn links.
        */}
        {link.kind === 'manual' && (
          <Button size="sm" variant="secondary" icon={Pencil} onClick={() => onDeleteManual(link)}>
            Edit it in the list of drawn links…
          </Button>
        )}
      </div>
    </>
  );
}
