/**
 * What is selected on the canvas, in words: a device and its ports, or a
 * cable and its lights.
 *
 * It reports the simulator's own state and nothing else. A port with no
 * address has no address here; a lab with no nodes says it is empty rather
 * than offering an imaginary one.
 */

import { Cable, MousePointerClick, Pencil, Power, Table2, Terminal, Trash, Unplug } from 'lucide-react';
import type { LabView, NodeView } from '@/api/types/lab';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { StatusDot } from '@/design/StatusDot';
import { formatRate } from '@/lib/format';
import type { Selection } from './Canvas';
import styles from './Inspector.module.css';
import {
  freeInterfaces,
  interfaceState,
  kindIcon,
  kindLabel,
  lightColor,
  lightLabel,
  nodeAddress,
  nodeIndex,
  portLabel,
  withLights,
  type LinkLights,
} from './model';

export interface InspectorProps {
  lab: LabView;
  lights: LinkLights;
  selection: Selection;
  onConsole: (node: NodeView) => void;
  onPower: (node: NodeView) => void;
  onRename: (node: NodeView) => void;
  onDelete: (node: NodeView) => void;
  onTables: (node: NodeView) => void;
  onCableFrom: (node: NodeView) => void;
  onRemoveLink: (id: number) => void;
  busy: boolean;
  /** The node whose console is being opened, if any: opening one starts a stopped lab. */
  opening: string | null;
}

export function Inspector(props: InspectorProps) {
  const { lab, selection } = props;

  if (selection?.kind === 'node') {
    const node = lab.snapshot.nodes.find((candidate) => candidate.name === selection.name);
    if (node) return <NodePanel {...props} node={node} />;
  }
  if (selection?.kind === 'link') {
    const link = lab.snapshot.links.find((candidate) => candidate.id === selection.id);
    if (link) return <LinkPanel {...props} link={withLights(link, props.lights)} />;
  }

  return (
    <div className={styles.inspector}>
      <EmptyState
        icon={MousePointerClick}
        size="compact"
        title="Nothing selected"
        description={
          lab.snapshot.nodes.length === 0
            ? 'This lab is empty. Add a device from the palette on the left, then cable two of them together.'
            : 'Click a device or a cable on the canvas to see what the simulator says about it.'
        }
      />
    </div>
  );
}

function NodePanel({
  lab,
  node,
  opening,
  onConsole,
  onPower,
  onRename,
  onDelete,
  onTables,
  onCableFrom,
  busy,
}: InspectorProps & { node: NodeView }) {
  const index = nodeIndex(lab.snapshot.nodes);
  const address = nodeAddress(node);
  const free = freeInterfaces(node);
  const unsaved = lab.unsaved_nodes.includes(node.name);
  const hasConsole = node.cli !== 'none';
  const deviceId = lab.devices[node.name];

  return (
    <div className={styles.inspector}>
      <header className={styles.header}>
        <span className={styles.avatar}>
          <Icon icon={kindIcon(node.kind)} size={18} />
        </span>
        <div className={styles.heading}>
          <h2 className={styles.title}>{node.name}</h2>
          <p className={styles.subtitle}>
            {kindLabel(node.kind)} · <span className={styles.mono}>{node.model}</span>
          </p>
        </div>
      </header>

      <div className={styles.badges}>
        <Badge tone={node.powered ? 'ok' : 'idle'}>{node.powered ? 'Powered on' : 'Powered off'}</Badge>
        {unsaved && (
          <Badge tone="warn" title="A stop would lose this device's running configuration">
            Unsaved
          </Badge>
        )}
        {!hasConsole && <Badge tone="idle">No command line</Badge>}
      </div>

      <dl className={styles.facts}>
        <dt>Hostname</dt>
        <dd className={styles.mono}>{node.hostname}</dd>
        <dt>Address</dt>
        <dd className={styles.mono}>{address ?? 'none'}</dd>
        <dt>Free ports</dt>
        <dd>
          {free.length} of {node.interfaces.length}
        </dd>
        {deviceId && (
          <>
            <dt>Inventory</dt>
            <dd className={styles.mono} title="Every lab node is an inventory device">
              {lab.name}/{node.name}
            </dd>
          </>
        )}
      </dl>

      <div className={styles.actions}>
        <Button
          size="sm"
          icon={Terminal}
          loading={opening === node.name}
          disabled={!hasConsole || busy}
          title={hasConsole ? undefined : 'This model has no command line to open a console on'}
          onClick={() => onConsole(node)}
        >
          Console
        </Button>
        <Button size="sm" icon={Cable} disabled={busy || free.length === 0} onClick={() => onCableFrom(node)}>
          Cable…
        </Button>
        <Button size="sm" icon={Table2} disabled={!lab.running || busy} onClick={() => onTables(node)}>
          Tables
        </Button>
        <Button size="sm" icon={Power} disabled={busy} onClick={() => onPower(node)}>
          {node.powered ? 'Power off' : 'Power on'}
        </Button>
        <Button size="sm" icon={Pencil} disabled={busy} onClick={() => onRename(node)}>
          Rename
        </Button>
        <Button size="sm" variant="danger" icon={Trash} disabled={busy} onClick={() => onDelete(node)}>
          Delete
        </Button>
      </div>

      <h3 className={styles.sectionTitle}>Interfaces</h3>
      <ul className={styles.ports}>
        {node.interfaces.map((port) => {
          const link = port.link == null ? null : lab.snapshot.links.find((item) => item.id === port.link);
          const peerId = link ? (link.a === node.id ? link.b : link.a) : null;
          const peer = peerId == null ? null : index.get(peerId);
          const peerIf = link ? (link.a === node.id ? link.b_if : link.a_if) : '';
          return (
            <li key={port.name} className={styles.port}>
              <StatusDot tone={port.oper_up ? 'ok' : port.admin_up ? 'crit' : 'idle'} />
              <span className={styles.portName}>{port.short}</span>
              <span className={styles.portState}>{interfaceState(port)}</span>
              <span className={styles.portDetail}>
                {port.ip ?? (port.mode || '—')}
                {peer && (
                  <>
                    {' · '}
                    <span className={styles.mono}>
                      {peer.name} {portLabel(peer, peerIf)}
                    </span>
                  </>
                )}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function LinkPanel({
  lab,
  link,
  onRemoveLink,
  busy,
}: InspectorProps & { link: ReturnType<typeof withLights> }) {
  const index = nodeIndex(lab.snapshot.nodes);
  const a = index.get(link.a);
  const b = index.get(link.b);

  return (
    <div className={styles.inspector}>
      <header className={styles.header}>
        <span className={styles.avatar}>
          <Icon icon={Cable} size={18} />
        </span>
        <div className={styles.heading}>
          <h2 className={styles.title}>
            {a?.name ?? '?'} ↔ {b?.name ?? '?'}
          </h2>
          <p className={styles.subtitle}>{link.cable === 'auto' ? 'Cable' : `${link.cable} cable`}</p>
        </div>
      </header>

      <div className={styles.badges}>
        <Badge tone={link.up ? 'ok' : 'crit'}>{link.up ? 'Up' : 'Down'}</Badge>
      </div>

      <dl className={styles.facts}>
        <dt>{a?.name ?? 'End A'}</dt>
        <dd className={styles.light}>
          <span className={styles.lamp} style={{ background: lightColor(link.a_light) }} aria-hidden="true" />
          <span className={styles.mono}>{portLabel(a, link.a_if)}</span> — {lightLabel(link.a_light)}
        </dd>
        <dt>{b?.name ?? 'End B'}</dt>
        <dd className={styles.light}>
          <span className={styles.lamp} style={{ background: lightColor(link.b_light) }} aria-hidden="true" />
          <span className={styles.mono}>{portLabel(b, link.b_if)}</span> — {lightLabel(link.b_light)}
        </dd>
        <dt>Traffic</dt>
        <dd className={styles.mono}>
          {formatRate(link.bps_ab)} → · ← {formatRate(link.bps_ba)}
        </dd>
      </dl>

      <div className={styles.actions}>
        <Button size="sm" variant="danger" icon={Unplug} disabled={busy} onClick={() => onRemoveLink(link.id)}>
          Remove cable
        </Button>
      </div>
    </div>
  );
}
