/**
 * One lab: the canvas, the palette, the inspector and the consoles.
 *
 * Every piece of state on this screen that a person could care about lives in
 * the backend. The canvas is `labs.get`, refreshed by `labs.changed`; the
 * link lights are `labs.tick`; the open consoles are `terminal.list`. Close
 * the window and open it again and the screen comes back as the simulator
 * has it, not as this browser last remembered it — the only thing kept here
 * is which pane is how wide.
 */

import {
  CircleStop,
  FlaskConical,
  Pause,
  Play,
  RotateCcw,
  Route,
  Terminal as TerminalIcon,
  Trash,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { useEvent, useRpc, useRpcMutation } from '@/api/hooks';
import type {
  LabView,
  LabsChangedEvent,
  LabsTickEvent,
  LinkView,
  ModelInfo,
  NodeView,
} from '@/api/types/lab';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button, IconButton } from '@/design/Button';
import { confirm, Dialog } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { ResizeHandle } from '@/design/Splitter';
import type { ViewportApi } from '@/design/Viewport';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import { Canvas, type Selection } from './Canvas';
import { CableDialog } from './CableDialog';
import { ConsoleDock, useLabConsoles } from './ConsoleDock';
import { Inspector } from './Inspector';
import { Palette } from './Palette';
import { Reachability } from './Reachability';
import { TablesDrawer } from './TablesDrawer';
import { CABLE_OFF, cableHint, cancelEnd, clickNode, type CableTool } from './cabling';
import styles from './Editor.module.css';
import { NODE_W, freeSpot, simClock, type LinkLights, type Point } from './model';

const SPEEDS = [
  { value: '0', label: 'Paused' },
  { value: '0.25', label: '0.25×' },
  { value: '1', label: '1×' },
  { value: '2', label: '2×' },
  { value: '4', label: '4×' },
  { value: '8', label: '8×' },
  { value: '16', label: '16×' },
];

export default function LabEditor({ id }: { id: string }) {
  const navigate = useNavigate();
  const lab = useRpc<LabView>('labs.get', { id }, { invalidateOn: ['labs.changed'] });
  const catalog = useRpc<ModelInfo[]>('labs.catalog');

  const [selection, setSelection] = useState<Selection>(null);
  const [cable, setCable] = useState<CableTool>(CABLE_OFF);
  const [lights, setLights] = useState<LinkLights>(new Map());
  const [renaming, setRenaming] = useState<NodeView | null>(null);
  const [newName, setNewName] = useState('');
  const [cabling, setCabling] = useState<NodeView | null>(null);
  const [cableError, setCableError] = useState<string | null>(null);
  const [tablesFor, setTablesFor] = useState<string | null>(null);
  const [reachability, setReachability] = useState(false);
  const [adding, setAdding] = useState<string | null>(null);
  const [dockHeight, setDockHeight] = useState(readDockHeight);
  /** The canvas and its pan/zoom, so a new device lands where the user is looking. */
  const canvasRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<ViewportApi>(null);

  const consoles = useLabConsoles(lab.data);

  // Link lights and rates: four times a second at most, and only while
  // something changed. They are laid over the snapshot, never stored in it.
  useEvent<LabsTickEvent>('labs.tick', (payload) => {
    if (payload.lab_id !== id) return;
    setLights(new Map(payload.links.map((link) => [link.id, link])));
  });
  // A stopped lab has no lights at all; a restarted one has new ones.
  useEvent<LabsChangedEvent>('labs.changed', (payload) => {
    if (payload.lab_id === id && (payload.reason === 'stopped' || payload.reason === 'started')) {
      setLights(new Map());
    }
  });

  const start = useRpcMutation<LabView, { id: string }>('labs.start', {
    invalidates: ['labs.'],
    errorToast: 'The lab did not start',
  });
  const stop = useRpcMutation<unknown, { id: string }>('labs.stop', {
    invalidates: ['labs.'],
    errorToast: 'The lab did not stop',
  });
  const reset = useRpcMutation<LabView, { id: string }>('labs.reset', {
    invalidates: ['labs.'],
    errorToast: 'The lab was not reset',
  });
  const speed = useRpcMutation<unknown, { id: string; speed: number }>('labs.speed', {
    invalidates: ['labs.'],
    errorToast: 'The speed did not change',
  });
  const addNode = useRpcMutation<NodeView, { lab_id: string; model: string; x: number; y: number }>(
    'labs.node.add',
    { invalidates: ['labs.'], errorToast: 'The device was not added' },
  );
  const updateNode = useRpcMutation<
    NodeView,
    { lab_id: string; node: string; name?: string; powered?: boolean }
  >('labs.node.update', { invalidates: ['labs.'], errorToast: 'The device did not change' });
  const removeNode = useRpcMutation<unknown, { lab_id: string; node: string }>('labs.node.remove', {
    invalidates: ['labs.'],
    errorToast: 'The device was not removed',
  });
  const addLink = useRpcMutation<
    LinkView,
    { lab_id: string; a: { node: string; interface?: string }; b: { node: string; interface?: string }; cable?: string }
  >('labs.link.add', { invalidates: ['labs.'], errorToast: false });
  const removeLink = useRpcMutation<unknown, { lab_id: string; link: number }>('labs.link.remove', {
    invalidates: ['labs.'],
    errorToast: 'The cable was not removed',
  });
  const remove = useRpcMutation<unknown, { id: string }>('labs.delete', {
    invalidates: ['labs.'],
    errorToast: 'The lab was not deleted',
  });

  const data = lab.data;
  const running = data?.running ?? false;
  const nodeCount = data?.snapshot.nodes.length ?? 0;
  const busy =
    addNode.isPending ||
    updateNode.isPending ||
    removeNode.isPending ||
    addLink.isPending ||
    removeLink.isPending;

  usePageHeader({
    title: data?.name ?? 'Lab',
    breadcrumbs: [{ label: 'Lab', to: '/lab' }],
  });

  // A device that has just been added is shown: the canvas fits the scene
  // again so the new one is never off the edge of the view.
  const drawn = useRef(nodeCount);
  useEffect(() => {
    if (nodeCount > drawn.current) viewportRef.current?.fit();
    drawn.current = nodeCount;
  }, [nodeCount]);

  // The console dock takes a third of the height with it when it opens, and
  // gives it back when the last console closes; either way the scene is
  // fitted again rather than leaving devices under the dock. Dragging the
  // handle is deliberate and is left alone.
  const docked = consoles.tabs.length > 0;
  const wasDocked = useRef(docked);
  useEffect(() => {
    if (docked !== wasDocked.current) viewportRef.current?.fit();
    wasDocked.current = docked;
  }, [docked]);

  // Esc puts the cable tool away before anything else gets the key.
  useEffect(() => {
    if (cable.mode === 'off') return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      setCable(CABLE_OFF);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [cable.mode]);

  useEffect(
    () =>
      registerCommand({
        id: 'lab.reachability',
        title: 'Lab: check reachability',
        group: 'Lab',
        keywords: ['ping', 'connectivity', 'matrix', 'probe'],
        run: () => setReachability(true),
      }),
    [],
  );

  if (lab.isPending) {
    return (
      <Page flush>
        <div className={styles.loading} aria-busy="true" aria-label="Loading the lab">
          <Skeleton height={36} shape="block" />
          <Skeleton height={320} shape="block" />
        </div>
      </Page>
    );
  }

  if (lab.error || !data) {
    const missing = lab.error?.code === 'not_found';
    return (
      <Page>
        <ErrorState
          title={missing ? 'There is no such lab' : 'The lab could not be read'}
          error={lab.error}
          onRetry={missing ? undefined : () => void lab.refetch()}
          actions={
            <Button variant={missing ? 'primary' : 'secondary'} onClick={() => void navigate('/lab')}>
              Back to the labs
            </Button>
          }
        />
      </Page>
    );
  }

  const connect = (params: {
    a: { node: string; interface?: string };
    b: { node: string; interface?: string };
    cable?: string;
  }) => {
    setCableError(null);
    addLink.mutate(
      { lab_id: id, ...params },
      {
        onSuccess: (link) => {
          setCabling(null);
          setSelection({ kind: 'link', id: link.id });
          toast.success(`Cabled ${params.a.node} ${link.a_if} to ${params.b.node} ${link.b_if}`);
        },
        onError: (error) => {
          // The simulator's refusal is the explanation: no free port, a port
          // already cabled, a node that has no console. Never paraphrased.
          setCableError(error.message);
          if (!cabling) toast.error('That cable could not be made', { description: error.message });
        },
      },
    );
  };

  const onCableClick = (node: NodeView) => {
    const step = clickNode(cable, node);
    setCable(step.tool);
    if (step.refused) toast.error(step.refused);
    if (step.connect) connect({ a: { node: step.connect.a }, b: { node: step.connect.b } });
  };

  const onMove = (node: string, position: { x: number; y: number }) => {
    void rpc('labs.node.move', { lab_id: id, positions: { [node]: position } }).catch(
      (error: unknown) => {
        const message = error instanceof RpcError ? error.message : 'the position was not saved';
        toast.error('The device moved back', { description: message });
        void lab.refetch();
      },
    );
  };

  const deleteNode = async (node: NodeView) => {
    const ok = await confirm({
      title: `Delete ${node.name}?`,
      description: `${node.name} and every cable to it are removed from this lab, and its simulated device leaves the inventory with everything collected from it.`,
      confirmLabel: 'Delete',
      tone: 'danger',
    });
    if (!ok) return;
    setSelection(null);
    removeNode.mutate({ lab_id: id, node: node.name });
  };

  const deleteLab = async () => {
    const ok = await confirm({
      title: `Delete ${data.name}?`,
      description: `The lab, its ${data.snapshot.nodes.length} simulated devices and everything collected from them are removed. This cannot be undone.`,
      confirmLabel: 'Delete the lab',
      tone: 'danger',
      confirmText: data.name,
    });
    if (!ok) return;
    remove.mutate({ id }, { onSuccess: () => void navigate('/lab') });
  };

  const stopLab = async () => {
    if (data.unsaved_nodes.length > 0) {
      const ok = await confirm({
        title: 'Stop the lab and lose unsaved configuration?',
        description: `${data.unsaved_nodes.join(', ')} ${
          data.unsaved_nodes.length === 1 ? 'has' : 'have'
        } running configuration that was never written to startup-config. Stopping loses it, exactly as pulling the power would.`,
        confirmLabel: 'Stop anyway',
        tone: 'danger',
      });
      if (!ok) return;
    }
    stop.mutate({ id });
  };

  const hint = cableHint(cable);

  return (
    <Page flush>
      <div className={styles.main}>
      <Toolbar className={styles.toolbar}>
        <span className={styles.name}>{data.name}</span>
        <LabPill />
        <StatusPill tone={running ? 'ok' : 'idle'} icon={running ? Play : CircleStop}>
          {running ? 'Running' : 'Stopped'}
        </StatusPill>
        {running && (
          <span className={styles.clock} title="Simulated time since this lab was created">
            {simClock(data.sim_time)}
          </span>
        )}
        {data.unsaved_nodes.length > 0 && (
          <StatusPill
            tone="warn"
            title={`A stop would lose the running configuration of ${data.unsaved_nodes.join(', ')}`}
          >
            {data.unsaved_nodes.length} unsaved
          </StatusPill>
        )}

        <ToolbarSpacer />

        {running && (
          <Select
            size="sm"
            aria-label="Simulation speed"
            className={styles.speed}
            value={String(data.speed)}
            onChange={(event) => speed.mutate({ id, speed: Number(event.target.value) })}
            options={SPEEDS}
          />
        )}
        <Button
          size="sm"
          icon={Route}
          onClick={() => setReachability(true)}
          title="Can A reach B, and where does the packet die?"
        >
          <span className={styles.label}>Reachability</span>
        </Button>
        <Button
          size="sm"
          icon={RotateCcw}
          disabled={!running}
          loading={reset.isPending}
          onClick={() => reset.mutate({ id })}
          title="Reboot every device from its startup configuration"
        >
          <span className={styles.label}>Reset</span>
        </Button>
        <IconButton
          size="sm"
          variant="ghost"
          icon={Trash}
          aria-label={`Delete the lab ${data.name}`}
          onClick={() => void deleteLab()}
        />
        {running ? (
          <Button size="sm" icon={Pause} loading={stop.isPending} onClick={() => void stopLab()}>
            Stop
          </Button>
        ) : (
          <Button
            size="sm"
            variant="primary"
            icon={Play}
            loading={start.isPending}
            onClick={() => start.mutate({ id })}
          >
            Start
          </Button>
        )}
      </Toolbar>

      <div className={styles.frame}>
        <div className={styles.workbench}>
          <Palette
            models={catalog.data}
            loading={catalog.isPending}
            error={catalog.error}
            onRetry={() => void catalog.refetch()}
            adding={adding}
            disabled={busy}
            cable={cable}
            onCable={() => setCable(cable.mode === 'off' ? { mode: 'armed' } : CABLE_OFF)}
            onAdd={(model) => {
              setAdding(model.id);
              const where = dropArea(canvasRef.current, viewportRef.current);
              const at = freeSpot(data.snapshot.nodes, where?.start, where?.columns);
              addNode.mutate(
                { lab_id: id, model: model.id, x: at.x, y: at.y },
                {
                  onSuccess: (node) => setSelection({ kind: 'node', name: node.name }),
                  onSettled: () => setAdding(null),
                },
              );
            }}
          />

          <div className={styles.canvasArea} ref={canvasRef}>
            {data.snapshot.nodes.length === 0 ? (
              <EmptyState
                icon={FlaskConical}
                title="This lab is empty"
                description="Nothing has been put on the canvas yet. Add a device from the palette on the left, add a second one, then use Cable to join them."
              />
            ) : (
              <Canvas
                viewportRef={viewportRef}
                lab={data}
                lights={lights}
                selection={selection}
                onSelect={setSelection}
                onBackground={() => {
                  setCable(cancelEnd(cable));
                  if (cable.mode === 'off') setSelection(null);
                }}
                cable={cable}
                onCableClick={onCableClick}
                onMove={onMove}
                onConsole={(node) => consoles.open(node)}
              />
            )}
            {hint && (
              <p className={styles.hint} role="status">
                {hint}
              </p>
            )}
            {consoles.tabs.length === 0 && data.snapshot.nodes.length > 0 && (
              <p className={styles.tip}>
                <TerminalIcon size={12} aria-hidden="true" /> Double-click a device to open its console.
              </p>
            )}
          </div>

          <div className={styles.inspector}>
            <Inspector
              lab={data}
              lights={lights}
              selection={selection}
              busy={busy}
              opening={consoles.opening}
              onConsole={(node) => consoles.open(node)}
              onPower={(node) =>
                updateNode.mutate({ lab_id: id, node: node.name, powered: !node.powered })
              }
              onRename={(node) => {
                setRenaming(node);
                setNewName(node.name);
              }}
              onDelete={(node) => void deleteNode(node)}
              onTables={(node) => setTablesFor(node.name)}
              onCableFrom={(node) => {
                setCableError(null);
                setCabling(node);
              }}
              onRemoveLink={(link) => {
                setSelection(null);
                removeLink.mutate({ lab_id: id, link });
              }}
            />
          </div>
        </div>

        {/* The dock is a sibling of the canvas, never a wrapper around it: a
            console opening must not remount the canvas and lose its view. */}
        {consoles.tabs.length > 0 && (
          <>
            <ResizeHandle
              orientation="horizontal"
              reverse
              value={dockHeight}
              min={120}
              max={620}
              defaultValue={DOCK_DEFAULT}
              aria-label="Resize the console"
              onChange={setDockHeight}
              onChangeEnd={(size) => writeStored(DOCK_KEY, size)}
            />
            <div className={styles.dock} style={{ height: dockHeight }}>
              <ConsoleDock consoles={consoles} labName={data.name} />
            </div>
          </>
        )}
      </div>
      </div>

      <Reachability
        open={reachability}
        onClose={() => setReachability(false)}
        lab={data}
        onStart={() => start.mutate({ id })}
        starting={start.isPending}
      />
      <TablesDrawer
        open={tablesFor !== null}
        onClose={() => setTablesFor(null)}
        labId={id}
        labRunning={running}
        node={tablesFor}
      />
      <CableDialog
        open={cabling !== null}
        onClose={() => {
          setCabling(null);
          setCableError(null);
        }}
        lab={data}
        from={cabling}
        busy={addLink.isPending}
        error={cableError}
        onConnect={connect}
      />
      <Dialog
        open={renaming !== null}
        onClose={() => setRenaming(null)}
        title={`Rename ${renaming?.name ?? ''}`}
        description="The simulated device in the inventory is renamed with it."
        footer={
          <>
            <Button onClick={() => setRenaming(null)}>Cancel</Button>
            <Button
              variant="primary"
              loading={updateNode.isPending}
              disabled={newName.trim() === '' || newName === renaming?.name}
              onClick={() => {
                if (!renaming) return;
                updateNode.mutate(
                  { lab_id: id, node: renaming.name, name: newName.trim() },
                  {
                    onSuccess: () => {
                      setSelection({ kind: 'node', name: newName.trim() });
                      setRenaming(null);
                    },
                  },
                );
              }}
            >
              Rename
            </Button>
          </>
        }
      >
        <Field label="Name">
          <Input
            data-autofocus
            value={newName}
            onChange={(event) => setNewName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && renaming && newName.trim() !== '') {
                updateNode.mutate(
                  { lab_id: id, node: renaming.name, name: newName.trim() },
                  {
                    onSuccess: () => {
                      setSelection({ kind: 'node', name: newName.trim() });
                      setRenaming(null);
                    },
                  },
                );
              }
            }}
          />
        </Field>
      </Dialog>
    </Page>
  );

}

/**
 * Where devices added from the palette go: a grid inside what is on screen
 * right now, however the canvas has been panned and zoomed, so one never
 * lands out of sight. Without a canvas yet (an empty lab shows its empty
 * state instead) the default start is used and the viewport fits to it as
 * soon as it is drawn.
 */
function dropArea(
  area: HTMLElement | null,
  viewport: ViewportApi | null,
): { start: Point; columns: number } | undefined {
  const rect = area?.getBoundingClientRect();
  if (!rect || !viewport || rect.width === 0) return undefined;
  const topLeft = viewport.clientToWorld(rect.left, rect.top);
  const bottomRight = viewport.clientToWorld(rect.right, rect.bottom);
  const width = bottomRight.x - topLeft.x;
  return {
    // A margin in from the corner, so a device never lands half under an edge.
    start: { x: Math.round(topLeft.x) + 90, y: Math.round(topLeft.y) + 80 },
    columns: Math.floor((width - 180) / (NODE_W + 60)),
  };
}

/** How tall the console dock is, remembered per machine — the one piece of this screen that is not the backend's. */
const DOCK_KEY = storageKey('lab.console');
const DOCK_DEFAULT = 260;

function readDockHeight(): number {
  const stored = readStored(DOCK_KEY, (value): value is number => typeof value === 'number' && Number.isFinite(value));
  return Math.min(620, Math.max(120, stored ?? DOCK_DEFAULT));
}
