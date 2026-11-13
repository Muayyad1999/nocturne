/**
 * The lab canvas: the nodes the simulator reports and the cables between
 * them, drawn as inline SVG inside the design system's `Viewport`.
 *
 * Everything on it comes from `labs.get` — positions, port names, link
 * lights — with the last `labs.tick` laid over the links so the lights move
 * without a refetch. While a node is being dragged its position is this
 * component's, because the pointer is the truth until it is let go; the
 * moment it is, `labs.node.move` is told and the backend's position wins
 * again.
 */

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type Ref } from 'react';
import type { LabView, LinkView, NodeView } from '@/api/types/lab';
import { Icon } from '@/design/Icon';
import { Viewport, useViewport, type ViewportApi } from '@/design/Viewport';
import { cx } from '@/lib/cx';
import { formatRate } from '@/lib/format';
import type { CableTool } from './cabling';
import styles from './Canvas.module.css';
import {
  NODE_H,
  NODE_LABEL_H,
  NODE_W,
  cableGeometry,
  kindIcon,
  kindLabel,
  lightColor,
  nodeIndex,
  portLabel,
  sceneBounds,
  withLights,
  type LinkLights,
  type Point,
} from './model';

export type Selection = { kind: 'node'; name: string } | { kind: 'link'; id: number } | null;

export interface CanvasProps {
  lab: LabView;
  lights: LinkLights;
  selection: Selection;
  onSelect: (selection: Selection) => void;
  /** A node was clicked while the cable tool is on. */
  onCableClick: (node: NodeView) => void;
  /** A click in open space. */
  onBackground: () => void;
  cable: CableTool;
  /** Called when a drag ends, with the node's new position in world units. */
  onMove: (node: string, position: Point) => void;
  /** Opens the console for a node (double click, or Enter on a focused node). */
  onConsole: (node: NodeView) => void;
  viewportRef?: Ref<ViewportApi>;
}

/** A drag only begins once the pointer has really moved, so a click still selects. */
const DRAG_THRESHOLD = 3;

export function Canvas({
  lab,
  lights,
  selection,
  onSelect,
  onCableClick,
  onBackground,
  cable,
  onMove,
  onConsole,
  viewportRef,
}: CanvasProps) {
  const { nodes, notes } = lab.snapshot;
  const bounds = sceneBounds(nodes, notes);

  return (
    <Viewport
      ref={viewportRef}
      aria-label={`${lab.name} topology`}
      className={cx(styles.viewport, cable.mode !== 'off' && styles.cabling)}
      grid
      fitOnMount
      minZoom={0.15}
      // Devices are drawn at a fixed size, so there is nothing to see past
      // this; without the cap, fitting a one-device lab zooms to 800 %.
      maxZoom={2.5}
      {...(bounds ? { contentBounds: bounds } : {})}
    >
      <Scene
        lab={lab}
        lights={lights}
        selection={selection}
        onSelect={onSelect}
        onCableClick={onCableClick}
        onBackground={onBackground}
        cable={cable}
        onMove={onMove}
        onConsole={onConsole}
      />
    </Viewport>
  );
}

type SceneProps = Omit<CanvasProps, 'viewportRef'>;

function Scene({
  lab,
  lights,
  selection,
  onSelect,
  onCableClick,
  onBackground,
  cable,
  onMove,
  onConsole,
}: SceneProps) {
  const viewport = useViewport();
  const { nodes, links, notes } = lab.snapshot;
  const index = nodeIndex(nodes);

  /** Where a node is while the pointer holds it; cleared once the backend agrees. */
  const [dragged, setDragged] = useState<{ name: string; at: Point } | null>(null);
  const gesture = useRef<{ name: string; grab: Point; origin: Point; moved: boolean } | null>(null);

  // The backend has caught up with the drag: let its position take over again.
  useEffect(() => {
    if (!dragged) return;
    const node = nodes.find((candidate) => candidate.name === dragged.name);
    if (!node) {
      setDragged(null);
      return;
    }
    if (Math.abs(node.x - dragged.at.x) < 0.5 && Math.abs(node.y - dragged.at.y) < 0.5) setDragged(null);
  }, [nodes, dragged]);

  const positionOf = (node: NodeView): Point =>
    dragged && dragged.name === node.name ? dragged.at : { x: node.x, y: node.y };

  const onPointerDown = (event: ReactPointerEvent<SVGGElement>, node: NodeView) => {
    if (event.button !== 0) return;
    // The viewport pans on a drag of its own; a node's drag is its business.
    event.stopPropagation();
    if (cable.mode !== 'off') return;
    const at = viewport.clientToWorld(event.clientX, event.clientY);
    gesture.current = {
      name: node.name,
      grab: { x: at.x - node.x, y: at.y - node.y },
      origin: { x: node.x, y: node.y },
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
    setDragged({ name: active.name, at: { x: Math.round(next.x), y: Math.round(next.y) } });
  };

  const onPointerUp = (event: ReactPointerEvent<SVGGElement>, node: NodeView) => {
    const active = gesture.current;
    gesture.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (!active) return;
    if (!active.moved) {
      if (cable.mode === 'off') onSelect({ kind: 'node', name: node.name });
      return;
    }
    const at = dragged?.name === active.name ? dragged.at : active.origin;
    if (at.x !== active.origin.x || at.y !== active.origin.y) onMove(active.name, at);
  };

  return (
    <>
      {/* The background takes clicks that miss everything: it clears the selection. */}
      <rect
        className={styles.background}
        x={-100_000}
        y={-100_000}
        width={200_000}
        height={200_000}
        onPointerDown={() => onBackground()}
      />

      {notes.map((note, position) => (
        <g key={`note-${position}`} className={styles.note} aria-hidden="true">
          {/* The note keeps its own colour as its edge; its text is drawn in
              the theme's, because a pale yellow sticky note is unreadable on
              a light canvas. */}
          <rect x={note.x} y={note.y} width={note.w} height={note.h} rx={10} style={{ stroke: note.color }} />
          <text x={note.x + 12} y={note.y + 20}>
            {note.text}
          </text>
        </g>
      ))}

      <g className={styles.cables}>
        {links.map((link) => (
          <Cable
            key={link.id}
            link={withLights(link, lights)}
            a={index.get(link.a)}
            b={index.get(link.b)}
            aAt={positionAt(index.get(link.a), dragged)}
            bAt={positionAt(index.get(link.b), dragged)}
            selected={selection?.kind === 'link' && selection.id === link.id}
            onSelect={() => onSelect({ kind: 'link', id: link.id })}
          />
        ))}
      </g>

      {nodes.map((node) => (
        <NodeGlyph
          key={node.id}
          node={node}
          at={positionOf(node)}
          selected={selection?.kind === 'node' && selection.name === node.name}
          cabling={cable.mode !== 'off'}
          cableFrom={cable.mode === 'from' && cable.node === node.name}
          onPointerDown={(event) => onPointerDown(event, node)}
          onPointerMove={onPointerMove}
          onPointerUp={(event) => onPointerUp(event, node)}
          onClick={() => {
            if (cable.mode !== 'off') onCableClick(node);
          }}
          onDoubleClick={() => onConsole(node)}
          onSelect={() => onSelect({ kind: 'node', name: node.name })}
        />
      ))}
    </>
  );
}

function positionAt(node: NodeView | undefined, dragged: { name: string; at: Point } | null): Point | undefined {
  if (!node) return undefined;
  return dragged && dragged.name === node.name ? dragged.at : { x: node.x, y: node.y };
}

interface CableProps {
  link: LinkView;
  a: NodeView | undefined;
  b: NodeView | undefined;
  aAt: Point | undefined;
  bAt: Point | undefined;
  selected: boolean;
  onSelect: () => void;
}

function Cable({ link, a, b, aAt, bAt, selected, onSelect }: CableProps) {
  if (!aAt || !bAt) return null;
  const geometry = cableGeometry(aAt, bAt);
  const traffic = link.bps_ab + link.bps_ba;
  const aName = portLabel(a, link.a_if);
  const bName = portLabel(b, link.b_if);
  const title = `${a?.name ?? link.a} ${aName} to ${b?.name ?? link.b} ${bName}: ${
    link.up ? 'up' : 'down'
  }${traffic > 0 ? `, ${formatRate(traffic)}` : ''}`;

  return (
    <g className={styles.cable} data-selected={selected || undefined} data-up={link.up || undefined}>
      <title>{title}</title>
      {/* A wide, invisible line: a 1px cable is impossible to hit. */}
      <line
        className={styles.hit}
        x1={geometry.a.x}
        y1={geometry.a.y}
        x2={geometry.b.x}
        y2={geometry.b.y}
        onPointerDown={(event) => {
          event.stopPropagation();
          onSelect();
        }}
      />
      <line
        className={styles.wire}
        x1={geometry.a.x}
        y1={geometry.a.y}
        x2={geometry.b.x}
        y2={geometry.b.y}
        data-cable={link.cable}
      />
      {traffic > 0 && (
        <line
          className={styles.traffic}
          x1={geometry.a.x}
          y1={geometry.a.y}
          x2={geometry.b.x}
          y2={geometry.b.y}
        />
      )}
      <circle cx={geometry.aLight.x} cy={geometry.aLight.y} r={4} style={{ fill: lightColor(link.a_light) }} />
      <circle cx={geometry.bLight.x} cy={geometry.bLight.y} r={4} style={{ fill: lightColor(link.b_light) }} />
      <text className={styles.port} x={geometry.aLabel.x} y={geometry.aLabel.y}>
        {aName}
      </text>
      <text className={styles.port} x={geometry.bLabel.x} y={geometry.bLabel.y}>
        {bName}
      </text>
    </g>
  );
}

interface NodeGlyphProps {
  node: NodeView;
  at: Point;
  selected: boolean;
  cabling: boolean;
  cableFrom: boolean;
  onPointerDown: (event: ReactPointerEvent<SVGGElement>) => void;
  onPointerMove: (event: ReactPointerEvent<SVGGElement>) => void;
  onPointerUp: (event: ReactPointerEvent<SVGGElement>) => void;
  onClick: () => void;
  onDoubleClick: () => void;
  onSelect: () => void;
}

function NodeGlyph({
  node,
  at,
  selected,
  cabling,
  cableFrom,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onClick,
  onDoubleClick,
  onSelect,
}: NodeGlyphProps) {
  const left = at.x - NODE_W / 2;
  const top = at.y - NODE_H / 2;
  const description = `${node.name}, ${kindLabel(node.kind)}${node.powered ? '' : ', powered off'}`;

  return (
    <g
      className={styles.node}
      role="button"
      tabIndex={0}
      aria-label={description}
      aria-pressed={selected}
      data-selected={selected || undefined}
      data-cabling={cabling || undefined}
      data-from={cableFrom || undefined}
      data-off={!node.powered || undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          if (cabling) onClick();
          else onSelect();
        }
      }}
      onFocus={() => {
        if (!cabling) onSelect();
      }}
    >
      <rect className={styles.body} x={left} y={top} width={NODE_W} height={NODE_H} rx={10} />
      <foreignObject x={left} y={top} width={NODE_W} height={NODE_H} aria-hidden="true">
        <div className={styles.glyph}>
          <Icon icon={kindIcon(node.kind)} size={24} />
        </div>
      </foreignObject>
      <text className={styles.name} x={at.x} y={top + NODE_H + 14}>
        {node.name}
      </text>
      {/* The icon says what kind of device it is; only a dark one needs words. */}
      {!node.powered && (
        <text className={styles.kind} x={at.x} y={top + NODE_H + NODE_LABEL_H - 4}>
          powered off
        </text>
      )}
    </g>
  );
}
