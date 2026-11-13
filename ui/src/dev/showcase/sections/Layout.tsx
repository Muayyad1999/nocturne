import { Sparkles } from 'lucide-react';
import { useRef, useState } from 'react';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { Icon } from '@/design/Icon';
import { KeyValue } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Splitter } from '@/design/Splitter';
import { Viewport, useViewport, type ViewportApi } from '@/design/Viewport';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

interface SceneNode {
  id: string;
  x: number;
  y: number;
}

const NODES: SceneNode[] = [
  { id: 'core-sw-01', x: 0, y: 0 },
  { id: 'core-sw-02', x: 240, y: 0 },
  { id: 'dist-sw-01', x: -140, y: 150 },
  { id: 'dist-sw-02', x: 120, y: 150 },
  { id: 'dist-sw-03', x: 380, y: 150 },
  { id: 'access-sw-11', x: -200, y: 300 },
  { id: 'access-sw-12', x: -60, y: 300 },
  { id: 'access-sw-21', x: 120, y: 300 },
  { id: 'edge-fw-01', x: 120, y: -140 },
];
const LINKS: [string, string][] = [
  ['core-sw-01', 'core-sw-02'],
  ['core-sw-01', 'dist-sw-01'],
  ['core-sw-01', 'dist-sw-02'],
  ['core-sw-02', 'dist-sw-02'],
  ['core-sw-02', 'dist-sw-03'],
  ['dist-sw-01', 'access-sw-11'],
  ['dist-sw-01', 'access-sw-12'],
  ['dist-sw-02', 'access-sw-21'],
  ['edge-fw-01', 'core-sw-01'],
  ['edge-fw-01', 'core-sw-02'],
];
const byId = new Map(NODES.map((node) => [node.id, node]));

/** A scene for the example: content is drawn in world coordinates and can read the viewport. */
function Scene({ selected, onSelect }: { selected: string | null; onSelect: (id: string) => void }) {
  const { transform } = useViewport();
  return (
    <>
      {LINKS.map(([from, to]) => {
        const a = byId.get(from);
        const b = byId.get(to);
        return a && b ? <line key={`${from}-${to}`} className={styles.link} x1={a.x} y1={a.y} x2={b.x} y2={b.y} /> : null;
      })}
      {NODES.map((node) => (
        <g key={node.id} transform={`translate(${node.x} ${node.y})`}>
          <rect
            className={styles.node}
            data-selected={node.id === selected || undefined}
            x={-46}
            y={-16}
            width={92}
            height={32}
            rx={6}
            onClick={() => onSelect(node.id)}
          />
          {/* Labels are dropped when zoomed far out, where they would be unreadable. */}
          {transform.k > 0.45 && (
            <text className={styles.nodeLabel} y={4}>
              {node.id}
            </text>
          )}
        </g>
      ))}
    </>
  );
}

export function Layout() {
  const viewport = useRef<ViewportApi>(null);
  const [selected, setSelected] = useState<string | null>('core-sw-01');

  return (
    <>
      <Section
        title="Viewport"
        description="The pan/zoom/fit container the topology and lab canvases are built on. Wheel zooms towards the cursor, drag pans, two fingers pinch; from the keyboard, arrows pan, + and − zoom, 0 fits and 1 is 100%."
      >
        <Example title={`An SVG scene in world coordinates · selected: ${selected ?? 'nothing'}`} span={3} layout="fill">
          <div className={styles.frameTall}>
            <Viewport ref={viewport} aria-label="Sample topology" grid>
              <Scene selected={selected} onSelect={setSelected} />
            </Viewport>
          </div>
          <div className={styles.inline} style={{ marginTop: 12 }}>
            <Button size="sm" onClick={() => viewport.current?.fit()}>
              Fit all
            </Button>
            <Button size="sm" onClick={() => viewport.current?.fit({ x: -260, y: 120, width: 320, height: 220 })}>
              Fit the access layer
            </Button>
            <span className={styles.note}>Buttons call the same API through a ref that content gets from useViewport().</span>
          </div>
        </Example>
      </Section>

      <Section
        title="Splitter"
        description="Two panes with a draggable separator. It is keyboard-operable (arrows, Home, End; Enter resets) and remembers its size."
      >
        <Example title="Horizontal, first pane sized" span={2} layout="fill">
          <div className={styles.frame}>
            <Splitter storageId="showcase.horizontal" defaultSize={200} minSize={120} maxSize={420}>
              <div className={styles.pane}>Sized pane. Drag the line, or focus it and use the arrow keys.</div>
              <div className={styles.pane}>The other pane takes the remaining space.</div>
            </Splitter>
          </div>
        </Example>
        <Example title="Vertical, second pane sized" layout="fill">
          <div className={styles.frame}>
            <Splitter orientation="vertical" sized="second" defaultSize={100} minSize={60} maxSize={200}>
              <div className={styles.pane}>Content</div>
              <div className={styles.pane}>Details</div>
            </Splitter>
          </div>
        </Example>
      </Section>

      <Section title="Card and Panel">
        <Example title="Card" layout="column">
          <Card>A plain surface that groups related content.</Card>
          <Card padding="sm" proposed>
            <span className={styles.inline} style={{ gap: 8, flexWrap: 'nowrap', alignItems: 'flex-start' }}>
              <Icon icon={Sparkles} size={14} />
              Proposed by the assistant: tinted until a human accepts it.
            </span>
          </Card>
        </Example>
        <Example title="Panel" layout="column">
          <Panel title="Interfaces" actions={<Badge>48</Badge>}>
            <KeyValue
              items={[
                { label: 'Up', value: '46' },
                { label: 'Down', value: '2' },
              ]}
            />
          </Panel>
        </Example>
        <Example title="Collapsible panel: remembers its state" layout="column">
          <Panel title="Environment" collapsible storageId="showcase.environment">
            <KeyValue
              items={[
                { label: 'Inlet', value: '24 °C' },
                { label: 'Fans', value: '4 of 4 running' },
              ]}
            />
          </Panel>
          <Panel title="Power" collapsible defaultCollapsed>
            Two supplies, both delivering.
          </Panel>
        </Example>
      </Section>
    </>
  );
}
