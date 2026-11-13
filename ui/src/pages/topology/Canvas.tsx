/**
 * The drawing: the nodes, the links and the groups `topology.get` answered
 * with, as inline SVG inside the design system's `Viewport`.
 *
 * Everything on it comes from the backend. Positions are the backend's (its
 * layout guarantees the cards do not overlap); the only geometry worked out
 * here is where a line leaves a card and how far it bows to get round one.
 * While a card is being dragged its position is this component's, because the
 * pointer is the truth until it is let go — then `topology.layout.save` is
 * told and the backend's position wins again.
 *
 * The three confidences are drawn differently on purpose, and the same way the
 * backend's own SVG export draws them (solid, long dash, short dash), with a
 * colour for where the claim came from and a word on the line for anything
 * that is not a link both ends reported.
 */

import {
  useEffect,
  useId,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type Ref,
} from 'react';
import type { Graph, Group, Link, Node } from '@/api/types/topology';
import { Viewport, useViewport, type Bounds, type ViewportApi } from '@/design/Viewport';
import { cx } from '@/lib/cx';
import styles from './Canvas.module.css';
import {
  NODE_H,
  NODE_W,
  bows,
  centre,
  degrees,
  drawableGroups,
  evidence,
  fitName,
  groupBounds,
  labelPorts,
  linkGeometry,
  nodeIcon,
  nodeIndex,
  nodeKindLabel,
  sceneBounds,
  shortInterface,
  type Point,
} from './graph';
import type { TraceOverlay } from './trace';

export type Selection = { kind: 'node'; id: string } | { kind: 'link'; id: string };

/** What `locate.host` found, so the port it decided on can be pointed at. */
export interface HostHighlight {
  /** The device the host is probably plugged into. */
  deviceId: string;
  interface: string;
  /** The devices that only saw it pass through. */
  alsoSeenOn: string[];
  label: string;
}

export interface CanvasProps {
  graph: Graph;
  selection: Selection | null;
  onSelect: (selection: Selection | null) => void;
  /** A drag ended: the node's new place in world units. */
  onMove: (id: string, at: Point) => void;
  /** A double click, or Enter on a focused card. */
  onOpen: (node: Node) => void;
  trace?: TraceOverlay | null;
  host?: HostHighlight | null;
  /** Interface names on the links, when the zoom makes them readable. */
  showLabels: boolean;
  viewportRef?: Ref<ViewportApi>;
  /** Changing this fits the scene again (the panel opened, the layer changed). */
  fitKey?: string;
}

/** A drag only begins once the pointer has really moved, so a click still selects. */
const DRAG_THRESHOLD = 3;
/** Cards are drawn at a fixed size: past this there is nothing more to see. */
const MAX_ZOOM = 2;

export function Canvas({ graph, viewportRef, fitKey, ...scene }: CanvasProps) {
  // The stub that marks where a path stopped is part of the drawing, so the
  // fit has to include it: pointing off the top-right corner, it was clipped
  // by the edge of the canvas and the reason went with it.
  const bounds = withPoint(sceneBounds(graph.nodes), scene.trace?.gap?.to);
  return (
    <Viewport
      ref={viewportRef}
      aria-label="Network topology"
      className={styles.viewport}
      grid
      fitOnMount
      minZoom={0.04}
      maxZoom={MAX_ZOOM}
      {...(bounds ? { contentBounds: bounds } : {})}
    >
      <Scene graph={graph} fitKey={fitKey} {...scene} />
    </Viewport>
  );
}

/** Grows a box until it contains a point, with room for the point's label. */
function withPoint(bounds: Bounds | null, point: Point | undefined): Bounds | null {
  if (!bounds || !point) return bounds;
  const pad = 60;
  const minX = Math.min(bounds.x, point.x - pad);
  const minY = Math.min(bounds.y, point.y - pad);
  const maxX = Math.max(bounds.x + bounds.width, point.x + pad);
  const maxY = Math.max(bounds.y + bounds.height, point.y + pad);
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
}

type SceneProps = Omit<CanvasProps, 'viewportRef'>;

function Scene({
  graph,
  fitKey,
  selection,
  onSelect,
  onMove,
  onOpen,
  trace,
  host,
  showLabels,
}: SceneProps) {
  const viewport = useViewport();
  const index = nodeIndex(graph.nodes);

  /** Where a card is while the pointer holds it; cleared once the backend agrees. */
  const [dragged, setDragged] = useState<{ id: string; at: Point } | null>(null);
  const gesture = useRef<{ id: string; grab: Point; origin: Point; moved: boolean } | null>(null);

  // The backend has caught up with the drag: its position takes over again.
  useEffect(() => {
    if (!dragged) return;
    const node = index.get(dragged.id);
    if (!node) {
      setDragged(null);
      return;
    }
    if (Math.abs(node.x - dragged.at.x) < 0.5 && Math.abs(node.y - dragged.at.y) < 0.5) {
      setDragged(null);
    }
  }, [index, dragged]);

  // `Viewport` fits once on mount, which is before the first graph has
  // arrived, so the scene is fitted again when it first has one and whenever
  // the picture it should show changes (the layer, the view, the side panel).
  // A redraw of the same picture is left exactly where the user put it.
  const fit = viewport.fit;
  const key = `${fitKey ?? ''}|${graph.nodes.length > 0}`;
  const fitted = useRef<string>('');
  useEffect(() => {
    if (graph.nodes.length === 0 || fitted.current === key) return;
    fitted.current = key;
    fit();
  }, [key, graph.nodes.length, fit]);

  const positionOf = (node: Node): Point =>
    dragged && dragged.id === node.id ? dragged.at : centre(node);

  const positions = new Map<string, Point>();
  for (const node of graph.nodes) positions.set(node.id, positionOf(node));

  const bowOf = bows(graph.links, positions);
  // The words that say how a link is known are drawn after the cards, because
  // a short link between two cards put its chip half under one of them.
  const chipAt: { id: string; at: Point; how: ReturnType<typeof evidence> }[] = [];
  const degree = degrees(graph.links);
  const selfSeen = new Map<string, number>();

  const onPointerDown = (event: ReactPointerEvent<SVGGElement>, node: Node) => {
    if (event.button !== 0) return;
    // The viewport pans on a drag of its own; a card's drag is its business.
    event.stopPropagation();
    const at = viewport.clientToWorld(event.clientX, event.clientY);
    gesture.current = {
      id: node.id,
      grab: { x: at.x - node.x, y: at.y - node.y },
      origin: centre(node),
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent<SVGGElement>) => {
    const active = gesture.current;
    if (!active) return;
    const at = viewport.clientToWorld(event.clientX, event.clientY);
    const next = { x: at.x - active.grab.x, y: at.y - active.grab.y };
    if (!active.moved) {
      const far = Math.hypot(next.x - active.origin.x, next.y - active.origin.y) * viewport.transform.k;
      if (far < DRAG_THRESHOLD) return;
      active.moved = true;
    }
    setDragged({ id: active.id, at: { x: Math.round(next.x), y: Math.round(next.y) } });
  };

  const onPointerUp = (event: ReactPointerEvent<SVGGElement>, node: Node) => {
    const active = gesture.current;
    gesture.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (!active) return;
    if (!active.moved) {
      onSelect({ kind: 'node', id: node.id });
      return;
    }
    const at = dragged?.id === active.id ? dragged.at : active.origin;
    if (at.x !== active.origin.x || at.y !== active.origin.y) onMove(active.id, at);
  };

  const dimmed = Boolean(trace);
  // Zoomed out, every label and every word on a line lands on another one.
  // The words that say how a link is known survive longer than port names,
  // because they are the part a reader must not miss.
  const portNames = showLabels && viewport.transform.k >= 0.5;
  const chips = viewport.transform.k >= 0.28;

  return (
    <>
      {/* Takes the clicks that miss everything: they clear the selection. */}
      <rect
        className={styles.background}
        x={-200_000}
        y={-200_000}
        width={400_000}
        height={400_000}
        onPointerDown={() => onSelect(null)}
      />

      {drawableGroups(graph.groups).map((group, depth) => (
        <GroupOutline
          key={group.id}
          group={group}
          depth={depth}
          nodes={graph.nodes}
          positions={positions}
        />
      ))}

      <g className={styles.links} data-dimmed={dimmed || undefined}>
        {graph.links.map((link) => {
          const a = positions.get(link.a.node);
          const b = positions.get(link.b.node);
          if (!a || !b) return null;
          const selfIndex = selfSeen.get(link.a.node) ?? 0;
          if (link.a.node === link.b.node) selfSeen.set(link.a.node, selfIndex + 1);
          const how = evidence(link);
          if (chips && how.chip !== '') {
            chipAt.push({
              id: link.id,
              at: linkGeometry(a, b, bowOf.get(link.id) ?? 0, selfIndex).mid,
              how,
            });
          }
          return (
            <LinkLine
              key={link.id}
              link={link}
              a={a}
              b={b}
              bow={bowOf.get(link.id) ?? 0}
              selfIndex={selfIndex}
              aName={index.get(link.a.node)?.name ?? link.a.node}
              bName={index.get(link.b.node)?.name ?? link.b.node}
              selected={selection?.kind === 'link' && selection.id === link.id}
              onPath={trace?.usedLinks.has(link.id) ?? false}
              labels={
                portNames &&
                labelPorts(degree.get(link.a.node)) &&
                labelPorts(degree.get(link.b.node))
              }
              onSelect={() => onSelect({ kind: 'link', id: link.id })}
            />
          );
        })}
      </g>

      {trace && <TraceLayer trace={trace} />}

      {graph.nodes.map((node) => (
        <NodeCard
          key={node.id}
          node={node}
          at={positionOf(node)}
          selected={selection?.kind === 'node' && selection.id === node.id}
          dimmed={dimmed && !trace?.onPath.has(node.id)}
          host={
            host && node.device_id === host.deviceId
              ? { label: host.label, kind: 'access' as const }
              : host && node.device_id && host.alsoSeenOn.includes(node.device_id)
                ? { label: 'seen in passing', kind: 'passing' as const }
                : null
          }
          onPointerDown={(event) => onPointerDown(event, node)}
          onPointerMove={onPointerMove}
          onPointerUp={(event) => onPointerUp(event, node)}
          onDoubleClick={() => onOpen(node)}
          onSelect={() => onSelect({ kind: 'node', id: node.id })}
        />
      ))}

      <g className={styles.chips} data-dimmed={dimmed || undefined} aria-hidden="true">
        {chipAt.map((chip) => (
          <text
            key={chip.id}
            x={chip.at.x}
            y={chip.at.y}
            data-confidence={chip.how.confidence}
            data-source={chip.how.source}
          >
            {chip.how.chip}
          </text>
        ))}
      </g>
    </>
  );
}

// ------------------------------------------------------------------ the groups

interface GroupOutlineProps {
  group: Pick<Group, 'id' | 'kind' | 'label' | 'nodes'>;
  /** How many groups enclose this one: each is inset, so none hides another. */
  depth: number;
  nodes: readonly Node[];
  positions: ReadonlyMap<string, Point>;
}

function GroupOutline({ group, depth, nodes, positions }: GroupOutlineProps) {
  const members = nodes
    .filter((node) => group.nodes.includes(node.id))
    .map((node) => {
      const at = positions.get(node.id);
      return at ? { ...node, x: at.x, y: at.y } : node;
    });
  // A group is drawn around every one of its nodes or not at all: an outline
  // that covers some of them says something that is not true.
  if (members.length < 2 || members.length !== group.nodes.length) return null;
  const box = groupBounds(members);
  if (!box) return null;
  const inset = depth * 12;
  return (
    <g className={styles.group} data-kind={group.kind} aria-hidden="true">
      <rect
        x={box.x + inset}
        y={box.y + inset}
        width={box.width - inset * 2}
        height={box.height - inset * 2}
        rx={16}
      />
      <text x={box.x + inset + 14} y={box.y + inset + 18}>
        {group.label}
      </text>
    </g>
  );
}

// ------------------------------------------------------------------- the links

interface LinkLineProps {
  link: Link;
  a: Point;
  b: Point;
  bow: number;
  selfIndex: number;
  aName: string;
  bName: string;
  selected: boolean;
  onPath: boolean;
  labels: boolean;
  onSelect: () => void;
}

function LinkLine({
  link,
  a,
  b,
  bow,
  selfIndex,
  aName,
  bName,
  selected,
  onPath,
  labels,
  onSelect,
}: LinkLineProps) {
  const geometry = linkGeometry(a, b, bow, selfIndex);
  const how = evidence(link);
  const ends = `${aName}${link.a.interface ? ` ${link.a.interface}` : ''} to ${bName}${
    link.b.interface ? ` ${link.b.interface}` : ''
  }`;
  const status =
    link.status === 'down' ? 'down' : link.status === 'up' ? 'up' : 'state not known';

  return (
    <g
      className={styles.link}
      data-source={how.source}
      data-confidence={how.confidence}
      data-status={link.status}
      data-selected={selected || undefined}
      data-path={onPath || undefined}
      data-aggregate={link.aggregate ? '' : undefined}
    >
      <title>{`${ends} — ${status}. ${how.claim}`}</title>
      {/* A wide invisible line: a 2px one is impossible to hit. */}
      <path className={styles.hit} d={geometry.path} onPointerDown={(event) => {
        event.stopPropagation();
        onSelect();
      }} />
      <path className={styles.wire} d={geometry.path} />
      {labels && (
        <>
          {link.a.interface && (
            <text className={styles.port} {...portLabelAt(geometry.a, geometry.b)}>
              {shortInterface(link.a.interface)}
            </text>
          )}
          {link.b.interface && (
            <text className={styles.port} {...portLabelAt(geometry.b, geometry.a)}>
              {shortInterface(link.b.interface)}
            </text>
          )}
        </>
      )}
    </g>
  );
}

/**
 * Where a port's name goes: a fifth of the way along the line from its own
 * end, and set off to one side of it. On the line it would sit under the
 * line; at the card's edge two cables into the same card put their labels in
 * the same place.
 */
function portLabelAt(from: Point, to: Point): { x: number; y: number } {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  const along = Math.min(0.2, 46 / length);
  return {
    x: from.x + dx * along - (dy / length) * 9,
    y: from.y + dy * along + (dx / length) * 9,
  };
}

// -------------------------------------------------------------------- the path

function TraceLayer({ trace }: { trace: TraceOverlay }) {
  return (
    <g className={styles.trace} aria-hidden="true">
      {trace.segments.map((segment, position) => (
        <line
          key={`${segment.from.x},${segment.from.y}-${position}`}
          className={styles.traceLine}
          x1={segment.from.x}
          y1={segment.from.y}
          x2={segment.to.x}
          y2={segment.to.y}
        />
      ))}

      {trace.gap && (
        <g className={styles.gap}>
          <line
            x1={trace.gap.from.x}
            y1={trace.gap.from.y}
            x2={trace.gap.to.x}
            y2={trace.gap.to.y}
          />
          <circle cx={trace.gap.to.x} cy={trace.gap.to.y} r={7} />
          <text x={trace.gap.to.x} y={trace.gap.to.y + 26}>
            the path stops here
          </text>
        </g>
      )}

      {trace.steps.map((step) =>
        step.node ? (
          <g key={`${step.index}-${step.node.id}`} className={styles.hop}>
            <circle cx={step.node.x - NODE_W / 2} cy={step.node.y - NODE_H / 2} r={11} />
            <text x={step.node.x - NODE_W / 2} y={step.node.y - NODE_H / 2}>
              {step.index}
            </text>
          </g>
        ) : null,
      )}
    </g>
  );
}

// ------------------------------------------------------------------- the nodes

interface NodeCardProps {
  node: Node;
  at: Point;
  selected: boolean;
  dimmed: boolean;
  host: { label: string; kind: 'access' | 'passing' } | null;
  onPointerDown: (event: ReactPointerEvent<SVGGElement>) => void;
  onPointerMove: (event: ReactPointerEvent<SVGGElement>) => void;
  onPointerUp: (event: ReactPointerEvent<SVGGElement>) => void;
  onDoubleClick: () => void;
  onSelect: () => void;
}

function NodeCard({
  node,
  at,
  selected,
  dimmed,
  host,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onDoubleClick,
  onSelect,
}: NodeCardProps) {
  // Node ids carry colons (`h:024e43d10901`), which have no place in a
  // fragment reference, so the clip path is named by React instead.
  const clipId = useId();
  const left = at.x - NODE_W / 2;
  const top = at.y - NODE_H / 2;
  const Glyph = nodeIcon(node.kind);
  // The icon and the legend already say what kind of thing this is, so the
  // second line is for what only this node can say — and nothing when it has
  // nothing to say. A card reading "switch" under a switch icon is noise.
  // A host found in a MAC table is named after its address, and a card that
  // says the same thing twice says nothing the second time.
  const detail = node.address ?? node.model ?? '';
  const second = detail === node.name ? '' : detail;
  const description = [
    node.name,
    nodeKindLabel(node.kind),
    node.lab ? 'lab device' : null,
    node.unmanaged ? 'not in the inventory' : null,
    node.status === 'up' ? 'up' : node.status === 'down' ? 'down' : 'state not known',
    node.stale ? 'data is stale' : null,
    node.alerts ? `${node.alerts} open alerts` : null,
  ]
    .filter(Boolean)
    .join(', ');

  return (
    <g
      className={cx(styles.node)}
      role="button"
      tabIndex={0}
      aria-label={description}
      aria-pressed={selected}
      data-selected={selected || undefined}
      data-dimmed={dimmed || undefined}
      data-status={node.status}
      data-kind={node.kind}
      data-lab={node.lab || undefined}
      data-unmanaged={node.unmanaged || undefined}
      data-stale={node.stale || undefined}
      data-host={host?.kind}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={onDoubleClick}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onSelect();
        }
      }}
      onFocus={onSelect}
    >
      <rect className={styles.body} x={left} y={top} width={NODE_W} height={NODE_H} rx={10} />
      {/* The status is a bar down the left edge: colour with a position, not colour alone. */}
      <rect className={styles.statusBar} x={left} y={top + 8} width={3} height={NODE_H - 16} rx={2} />

      <g className={styles.glyph} transform={`translate(${left + 14} ${at.y - 9})`}>
        <Glyph size={18} strokeWidth={1.75} aria-hidden="true" />
      </g>

      {/* Clipped, not shrunk: a long name is cut off, never spilled over a link. */}
      <clipPath id={clipId}>
        <rect x={left + 38} y={top} width={NODE_W - 48} height={NODE_H} />
      </clipPath>
      <title>{description}</title>
      <g clipPath={`url(#${clipId})`}>
        <text className={styles.name} x={left + 38} y={second === '' ? at.y + 2 : at.y - 4}>
          {fitName(node.name)}
        </text>
        {second !== '' && (
          <text className={styles.second} x={left + 38} y={at.y + 11}>
            {second}
          </text>
        )}
      </g>

      {node.lab && (
        <g className={styles.pill} data-tone="lab">
          <rect x={left + NODE_W - 36} y={top + 5} width={31} height={13} rx={4} />
          <text x={left + NODE_W - 20.5} y={top + 12}>
            LAB
          </text>
        </g>
      )}
      {node.unmanaged && !node.lab && (
        <g className={styles.pill} data-tone="unmanaged">
          <title>Seen by a neighbour protocol but not in the inventory</title>
          <rect x={left + NODE_W - 20} y={top + 5} width={15} height={13} rx={4} />
          <text x={left + NODE_W - 12.5} y={top + 12}>
            +
          </text>
        </g>
      )}
      {node.stale && (
        <g className={styles.pill} data-tone="stale">
          <rect x={left + NODE_W - 48} y={top + NODE_H - 18} width={43} height={13} rx={4} />
          <text x={left + NODE_W - 26.5} y={top + NODE_H - 11}>
            stale
          </text>
        </g>
      )}
      {node.alerts !== undefined && node.alerts > 0 && (
        <g className={styles.alerts}>
          <circle cx={left + NODE_W - 8} cy={top + NODE_H / 2} r={8} />
          <text x={left + NODE_W - 8} y={top + NODE_H / 2}>
            {node.alerts}
          </text>
        </g>
      )}

      {host && (
        <text className={styles.hostMark} x={at.x} y={top + NODE_H + 14}>
          {host.label}
        </text>
      )}
    </g>
  );
}
