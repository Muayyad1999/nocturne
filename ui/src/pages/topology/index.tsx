/**
 * The Topology screen: the network as the devices themselves described it.
 *
 * Everything on it comes from `topology.get` — the nodes, the links, the
 * groups, the warnings and the positions — refetched when the backend says
 * `topology.changed`. Nothing is drawn that the backend did not answer with,
 * and the three kinds of claim a link can be (both ends reported it, one end
 * did, it was worked out from addresses or MAC tables) are drawn differently
 * and explained in the legend, because they are not the same statement about
 * reality.
 *
 * The screen's own jobs: pan, zoom, fit and drag (positions through
 * `topology.layout.save`, automatic arrangements through
 * `topology.autolayout`), named views, path tracing, finding a host, the five
 * exports, and the links a person drew. "Refresh data" is a `collect.run`
 * over the devices in the drawing — the topology feature never touches a
 * device itself.
 */

import {
  Activity,
  Boxes,
  Download,
  ExternalLink,
  Layers,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  Route,
  Save,
  Search,
  TerminalSquare,
  Trash2,
  Wand2,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { DeviceInput, DeviceRow, Job, JobStarted } from '@/api/types/core';
import type { LabView } from '@/api/types/lab';
import type { IfStatus, MonitorSettings } from '@/api/types/monitor';
import type { SessionInfo, TerminalOpenParams } from '@/api/types/terminal';
import type {
  Algorithm,
  AutoLayoutResult,
  Graph,
  Layer,
  LayoutResult,
  Located,
  Node,
  Position,
  TopologySettings,
  Trace,
  View,
} from '@/api/types/topology';
import { DEFAULT_VIEW } from '@/api/types/topology';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button, IconButton } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { DropdownMenu, type MenuEntry } from '@/design/DropdownMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import type { ViewportApi } from '@/design/Viewport';
import { plural } from '@/lib/format';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import { formatRelative } from '@/lib/time';
import { useNow } from '@/lib/useNow';
import { useTerminalLayout } from '@/pages/terminal/layout';
import { Canvas, type HostHighlight, type Selection } from './Canvas';
import { ExportDialog } from './ExportDialog';
import { HostPanel } from './HostPanel';
import { Inspector } from './Inspector';
import { Legend } from './Legend';
import { ManualLinks } from './ManualLinks';
import { PathPanel } from './PathPanel';
import { SessionDock } from './SessionDock';
import { Warnings } from './Warnings';
import { deviceIdsOf, facetsOf, overlapping, positionsOf, type Point } from './graph';
import {
  deviceIndex,
  layerState,
  linkLoad,
  rateIndex,
  type LinkLoad,
} from './load';
import { classify, matches, pathReference } from './search';
import { sessionAction, sessionOffer, sessionOn, type SessionOffer } from './session';
import { traceOverlay } from './trace';
import styles from './topology.module.css';

const VIEW_KEY = storageKey('topology.view');
const LABELS_KEY = storageKey('topology.labels');
const LOAD_KEY = storageKey('topology.load');

/**
 * How often the drawing re-reckons how old its rates are. Nothing is fetched
 * on this tick: it is the clock that turns a live reading into a stale one
 * when no new measurement arrives.
 */
const AGE_TICK_MS = 15_000;

/** The size a session from the map is opened at; the dock's fit corrects it. */
const OPEN_COLS = 120;
const OPEN_ROWS = 28;

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isBoolean(value: unknown): value is boolean {
  return typeof value === 'boolean';
}

type Panel = 'selection' | 'path' | 'host';

const ALGORITHMS: readonly { value: Algorithm; label: string; about: string }[] = [
  { value: 'layered', label: 'Layered', about: 'Ranks by role: core at the top, access below.' },
  { value: 'force', label: 'Force', about: 'A spring embedder, for meshes with no hierarchy.' },
  { value: 'radial', label: 'Radial', about: 'Rings around the selected node.' },
];

export default function TopologyPage() {
  const navigate = useNavigate();
  const viewportRef = useRef<ViewportApi>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // What is being looked at. The view supplies the defaults; a control that
  // has been touched overrides it until the view is saved again.
  const [viewId, setViewId] = useState<string>(
    () => readStored(VIEW_KEY, isString) ?? DEFAULT_VIEW,
  );
  const [layer, setLayer] = useState<Layer | null>(null);
  const [endpoints, setEndpoints] = useState<boolean | null>(null);
  const [site, setSite] = useState<string>('');
  const [role, setRole] = useState<string>('');
  const [labels, setLabels] = useState<boolean>(() => readStored(LABELS_KEY, isBoolean) ?? true);
  const [showLoad, setShowLoad] = useState<boolean>(() => readStored(LOAD_KEY, isBoolean) ?? true);
  /** The session opened from the map, while its dock is up. */
  const [session, setSession] = useState<{ id: string; name: string; lab: boolean } | null>(null);

  const [panel, setPanel] = useState<Panel>('selection');
  const [selection, setSelection] = useState<Selection | null>(null);
  const [search, setSearch] = useState('');
  const [hostQuery, setHostQuery] = useState('');
  const [pathFrom, setPathFrom] = useState('');
  const [pathTo, setPathTo] = useState('');
  const [pathQuery, setPathQuery] = useState<{ from: string; to: string } | null>(null);
  const [exporting, setExporting] = useState(false);
  const [drawing, setDrawing] = useState(false);
  const [collectJob, setCollectJob] = useState<string | null>(null);

  // ------------------------------------------------------------------- reads

  const views = useRpc<View[]>('topology.views', undefined, {
    invalidateOn: ['topology.changed'],
  });
  const settings = useRpc<TopologySettings>('settings.get', { section: 'topology' });

  const filter = useMemo(
    () =>
      site || role
        ? { site: site || null, role: role || null, tag: null, lab_id: null, device_ids: [] }
        : undefined,
    [site, role],
  );

  const params = useMemo(
    () => ({
      view: viewId,
      ...(layer ? { layer } : {}),
      ...(endpoints === null ? {} : { include_endpoints: endpoints }),
      ...(filter ? { filter } : {}),
    }),
    [viewId, layer, endpoints, filter],
  );

  const graph = useRpc<Graph>('topology.get', params, { invalidateOn: ['topology.changed'] });

  const path = useRpc<Trace>('topology.path', pathQuery ?? undefined, {
    enabled: Boolean(pathQuery),
  });
  const located = useRpc<Located>('locate.host', hostQuery ? { query: hostQuery } : undefined, {
    enabled: hostQuery !== '',
  });
  const job = useRpc<Job>('jobs.get', collectJob ? { id: collectJob } : undefined, {
    enabled: Boolean(collectJob),
    invalidateOn: ['jobs.changed'],
  });

  /*
   * The live part. `monitor.interfaces` is the interface table monitoring
   * keeps — one row per port, with the rates it last worked out from the
   * device's own counters — and it is refetched when a poll says it has
   * changed, which is what makes the lines on this map move by themselves.
   * The whole table is asked for in one call rather than per link: a campus
   * has more links than ports worth asking about one at a time, and the
   * backend pages this query in SQL.
   *
   * A build without the monitor feature answers `unknown_method`; the layer
   * then says so once instead of drawing every link as "no rate".
   */
  const interfaces = useRpc<IfStatus[]>(
    'monitor.interfaces',
    { limit: 5000, sort: 'device' },
    { invalidateOn: ['monitor.interfaces', 'monitor.polled'] },
  );
  /**
   * The inventory, for two questions the drawing cannot answer itself: how
   * often a device's counters are actually measured (which decides when a
   * rate stops being current), and whether it could be logged in to at all.
   */
  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state'],
  });
  const monitorSettings = useRpc<MonitorSettings>('settings.get', { section: 'monitoring' });
  const sessions = useRpc<SessionInfo[]>('terminal.list', undefined, {
    invalidateOn: ['terminal.opened', 'terminal.closed', 'terminal.removed'],
  });

  // ------------------------------------------------------------------ writes

  const saveLayout = useRpcMutation<
    LayoutResult,
    { view: string; positions: Record<string, Position> }
  >('topology.layout.save', { errorToast: 'The new position could not be saved' });
  const autolayout = useRpcMutation<AutoLayoutResult, Record<string, unknown>>(
    'topology.autolayout',
    { invalidates: ['topology.get'] },
  );
  const saveView = useRpcMutation<View, Record<string, unknown>>('topology.view_save', {
    invalidates: ['topology.'],
  });
  const deleteView = useRpcMutation<{ deleted: boolean }, { id: string }>('topology.view_delete', {
    invalidates: ['topology.'],
  });
  const collect = useRpcMutation<JobStarted, { device_ids: string[]; origin: string }>(
    'collect.run',
  );
  const createDevice = useRpcMutation<DeviceRow, DeviceInput>('devices.create', {
    invalidates: ['devices.', 'topology.'],
  });
  // The terminal feature's own method, with the terminal feature's own
  // semantics: one session per call, held in the backend, listed by
  // `terminal.list`. The map opens one; it does not keep one.
  const openSession = useRpcMutation<SessionInfo, TerminalOpenParams>('terminal.open', {
    invalidates: ['terminal.list'],
    errorToast: 'Could not open a terminal',
  });
  const closeSession = useRpcMutation<unknown, { session_id: string }>('terminal.close', {
    invalidates: ['terminal.list'],
    errorToast: false,
  });

  // --------------------------------------------------------------- the state

  const data = graph.data;
  const nodes = data?.nodes ?? [];
  const current = views.data?.find((entry) => entry.id === viewId);
  const facets = useMemo(() => facetsOf(data), [data]);

  const selectedNode =
    selection?.kind === 'node' ? (nodes.find((node) => node.id === selection.id) ?? null) : null;
  const selectedLink =
    selection?.kind === 'link'
      ? (data?.links.find((link) => link.id === selection.id) ?? null)
      : null;

  const overlay = useMemo(
    () => (panel === 'path' ? traceOverlay(path.data, nodes, data?.links ?? []) : null),
    [panel, path.data, nodes, data?.links],
  );

  // ----------------------------------------------------------- the link load

  // The clock that ages a reading. Nothing is fetched on this tick: a rate
  // goes stale because time passed, and no event says so.
  //
  // `useNow` floors to its own interval so that it is stable between ticks,
  // which puts it up to fifteen seconds *behind* — and a reading taken two
  // seconds ago then read "measured in 11s", a measurement from the future.
  // The tick is the signal to re-reckon; the instant it is reckoned against
  // is the real one, taken once per tick.
  const tick = useNow(AGE_TICK_MS);
  const now = useMemo(() => Math.max(tick, Date.now()), [tick]);

  const deviceById = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device])),
    [devices.data],
  );

  const loadLayer = layerState({
    rows: interfaces.data,
    pending: interfaces.isPending,
    failed: interfaces.isError,
  });

  /** Every link's load, by link id, or `null` when there is no layer to draw. */
  const loads = useMemo<ReadonlyMap<string, LinkLoad> | null>(() => {
    if (loadLayer !== 'ready' || !data) return null;
    const context = {
      deviceOf: deviceIndex(data.nodes),
      rates: rateIndex(interfaces.data),
      devices: deviceById,
      monitoring: monitorSettings.data,
      now,
    };
    const byLink = new Map<string, LinkLoad>();
    for (const link of data.links) byLink.set(link.id, linkLoad(link, context));
    return byLink;
  }, [loadLayer, data, interfaces.data, deviceById, monitorSettings.data, now]);

  /** The newest measurement anywhere in the drawing, for the "as of" line. */
  const loadAt = useMemo(() => {
    let newest: number | null = null;
    for (const load of loads?.values() ?? []) {
      if (load.at != null && (newest == null || load.at > newest)) newest = load.at;
    }
    return newest;
  }, [loads]);

  // -------------------------------------------------------- a session from here

  const selectedDevice = selectedNode?.device_id
    ? deviceById.get(selectedNode.device_id)
    : undefined;
  /**
   * The lab the selected device belongs to, when it belongs to one. A lab
   * device's console needs no credential, but it does need the lab running
   * and the node powered on — and only the lab feature knows that. Asked for
   * one lab at a time, because that is the one whose button is on screen.
   */
  const labId = selectedDevice?.lab?.lab_id;
  const lab = useRpc<LabView>('labs.get', labId ? { id: labId } : undefined, {
    enabled: Boolean(labId),
    // Not `labs.tick`, which fires four times a second while a lab runs:
    // powering a node on or off is a `labs.changed`, and that is the only
    // part of a lab this screen's button depends on.
    invalidateOn: ['labs.changed'],
  });

  /** Why this build has no terminal at all, when it has none. */
  const terminalMissing =
    sessions.isError && sessions.error.code === 'unknown_method'
      ? 'This build has no terminal feature, so there is no session to open from here.'
      : sessions.isError
        ? sessions.error.message
        : null;

  const offerFor = (node: Node): SessionOffer => {
    if (terminalMissing) return { ok: false, why: terminalMissing, brief: 'no terminal here' };
    const device = node.device_id ? deviceById.get(node.device_id) : undefined;
    // Only the selected node's lab has been asked about; for any other node
    // the lab is unknown, and an unknown lab is not a refusal.
    const labOf = device?.lab && device.lab.lab_id === labId ? lab.data : undefined;
    return sessionOffer(node, device, labOf);
  };

  const highlight: HostHighlight | null = useMemo(() => {
    if (panel !== 'host') return null;
    const answer = located.data;
    const best = answer?.best === undefined ? undefined : answer.locations[answer.best];
    if (!answer || !best) return null;
    return {
      deviceId: best.device_id,
      interface: best.interface,
      alsoSeenOn: answer.locations
        .filter((_location, index) => index !== answer.best)
        .map((location) => location.device_id),
      label: `${answer.ip ?? answer.mac ?? 'this host'} on ${best.interface}`,
    };
  }, [panel, located.data]);

  // Finding a host is a question about *where*, so the drawing goes there
  // once per answer. Keyed on the answer, so panning away afterwards sticks.
  const foundAt = highlight?.deviceId;
  useEffect(() => {
    if (!foundAt) return;
    const node = nodes.find((candidate) => candidate.device_id === foundAt);
    if (!node) return;
    viewportRef.current?.fit({ x: node.x - 420, y: node.y - 260, width: 840, height: 520 });
  }, [foundAt, nodes]);

  // A view remembered in this browser can have been deleted somewhere else,
  // and `topology.get` then answers `not_found` for the rest of time. Fall
  // back to the default rather than leaving the screen stuck on an error.
  useEffect(() => {
    if (!views.data || views.data.length === 0) return;
    if (views.data.some((entry) => entry.id === viewId)) return;
    setViewId(DEFAULT_VIEW);
    writeStored(VIEW_KEY, DEFAULT_VIEW);
  }, [views.data, viewId]);

  // The backend promises the cards it positions never overlap. If one ever
  // does, the drawing is lying about what is next to what, so say so in
  // development rather than shipping a quiet mess.
  useEffect(() => {
    if (!import.meta.env.DEV || nodes.length === 0) return;
    const clash = overlapping(nodes);
    if (clash) {
      // eslint-disable-next-line no-console
      console.warn(
        `topology: ${clash[0].name} and ${clash[1].name} are drawn on top of each other`,
      );
    }
  }, [nodes]);

  // A finished collection: the graph refetches on `topology.changed` by itself.
  const finished = job.data && job.data.state !== 'running' ? job.data : null;
  useEffect(() => {
    if (!finished) return;
    setCollectJob(null);
    if (finished.state === 'succeeded') {
      toast.success('Neighbour data collected', { description: finished.message });
    } else if (finished.state === 'failed') {
      toast.error('The collection failed', { description: finished.error ?? finished.message });
    }
  }, [finished]);

  usePageHeader({ title: 'Topology' });

  // ----------------------------------------------------------------- actions

  const refresh = () => {
    const ids = deviceIdsOf(data);
    if (ids.length === 0) {
      toast.error('There is no device in this drawing to collect from');
      return;
    }
    collect.mutate(
      { device_ids: ids, origin: 'user' },
      {
        onSuccess: (started) => {
          setCollectJob(started.job_id);
          toast.info(`Collecting from ${plural(ids.length, 'device')}`);
        },
      },
    );
  };

  const move = (id: string, at: Point) => {
    // Every position is sent, not just the one that moved: `topology.get`
    // places a node that has none next to its neighbours, so saving one alone
    // would let the backend rearrange everything else around it.
    saveLayout.mutate({ view: viewId, positions: positionsOf(nodes, { id, at }) });
  };

  const pin = (node: Node, pinned: boolean) => {
    const positions = positionsOf(nodes);
    positions[node.id] = { x: node.x, y: node.y, pinned };
    saveLayout.mutate({ view: viewId, positions });
  };

  const arrange = (algorithm: Algorithm) => {
    autolayout.mutate(
      {
        ...params,
        algorithm,
        save: true,
        ...(algorithm === 'radial' && selectedNode ? { root: selectedNode.id } : {}),
      },
      {
        onSuccess: (result) => {
          const places = Object.values(result.positions);
          const spread =
            places.length > 0 ? Math.max(...places.map((position) => position.x)) : 0;
          toast.success(`Arranged ${plural(places.length, 'node')}`, {
            description:
              // A node with no links of its own is pushed away by everything
              // and pulled back by nothing, so a force layout can fling it a
              // long way. Say so instead of leaving the user looking at dust.
              algorithm === 'force' && spread > 20_000
                ? 'Nodes with no links of their own end up far away; press Fit to see them all.'
                : ALGORITHMS.find((entry) => entry.value === algorithm)?.about,
          });
          viewportRef.current?.fit();
        },
      },
    );
  };

  /**
   * A terminal on a node. The session is the backend's and the Terminal
   * screen's registry owns the xterm, exactly as the Lab screen's consoles
   * do; this only decides which device and lands the result in the dock. A
   * device that already has a session gets that one back rather than a second
   * login — a switch with two sessions open is how a person ends up typing
   * into the wrong one.
   */
  const terminal = (node: Node) => {
    const offer = offerFor(node);
    if (!offer.ok) {
      toast.error(`No terminal on ${node.name}`, { description: offer.why });
      return;
    }
    const existing = sessionOn(sessions.data, offer.deviceId);
    if (existing && existing.state !== 'closed') {
      setSession({ id: existing.id, name: node.name, lab: existing.lab });
      return;
    }
    openSession.mutate(
      { device_id: offer.deviceId, cols: OPEN_COLS, rows: OPEN_ROWS },
      {
        onSuccess: (info) => setSession({ id: info.id, name: node.name, lab: info.lab }),
      },
    );
  };

  /**
   * The same session, on the Terminal screen: its layout is told to open that
   * id and the screen attaches to it. Nothing is reopened and nothing is
   * dialled twice — there is one session, in the backend, and both screens
   * are looking at it.
   */
  const handOverSession = () => {
    if (!session) return;
    useTerminalLayout.getState().open(session.id);
    navigate('/terminal');
  };

  const endSession = async () => {
    if (!session) return;
    const info = sessions.data?.find((entry) => entry.id === session.id);
    if (info && info.state !== 'closed') {
      const sure = await confirm({
        title: `Close the session on ${session.name}?`,
        description: `The session to ${info.info.peer} ends now. Anything unsaved on the device stays unsaved.`,
        confirmLabel: 'Close session',
        tone: 'danger',
      });
      if (!sure) return;
    }
    setSession(null);
    closeSession.mutate({ session_id: session.id });
  };

  const traceFrom = (node: Node) => {
    setPathFrom(pathReference(node));
    setPanel('path');
  };

  const traceBetween = (a: Node, b: Node) => {
    setPathFrom(pathReference(a));
    setPathTo(pathReference(b));
    setPathQuery({ from: pathReference(a), to: pathReference(b) });
    setPanel('path');
  };

  const centreOn = (node: Node) => {
    setSelection({ kind: 'node', id: node.id });
    viewportRef.current?.fit({ x: node.x - 320, y: node.y - 200, width: 640, height: 400 });
  };

  const runSearch = () => {
    const query = classify(search);
    if (query.kind === 'empty') return;
    if (query.kind === 'address') {
      setHostQuery(query.text);
      setPanel('host');
      return;
    }
    const first = matches(nodes, query.text)[0];
    if (!first) {
      toast.error(`Nothing in this drawing is called “${query.text}”`, {
        description: 'An IP or a MAC address is looked up with locate.host instead.',
      });
      return;
    }
    setPanel('selection');
    centreOn(first);
  };

  const addToInventory = async (node: Node) => {
    const sure = await confirm({
      title: `Add ${node.name} to the inventory?`,
      description: `A device called ${node.name}${
        node.address ? ` at ${node.address}` : ''
      } is created. Nothing is connected to and nothing is read from it until it is given credentials.`,
      confirmLabel: 'Add device',
    });
    if (!sure) return;
    createDevice.mutate(
      {
        name: node.name,
        ...(node.address ? { address: node.address } : {}),
        ...(node.site ? { site: node.site } : {}),
      },
      {
        onSuccess: (device) => {
          toast.success(`${device.name} is in the inventory`, {
            description: 'Give it credentials and collect from it to see what it is connected to.',
          });
        },
      },
    );
  };

  const saveCurrentView = (asNew: boolean) => {
    const name = asNew ? window.prompt('Name for this view')?.trim() : (current?.name ?? 'Default');
    if (asNew && !name) return;
    saveView.mutate(
      {
        ...(asNew ? {} : { id: viewId }),
        name: name ?? 'Default',
        layer: layer ?? data?.layer ?? 'physical',
        ...(filter ? { filter } : {}),
        include_endpoints: endpoints ?? current?.include_endpoints ?? false,
      },
      {
        onSuccess: (view) => {
          setViewId(view.id);
          writeStored(VIEW_KEY, view.id);
          setLayer(null);
          setEndpoints(null);
          toast.success(`Saved the view “${view.name}”`);
        },
      },
    );
  };

  // --------------------------------------------------------------- shortcuts

  useEffect(
    () =>
      registerCommand({
        id: 'topology.search',
        title: 'Search the topology',
        group: 'Topology',
        keywords: ['find', 'device', 'host', 'mac', 'ip'],
        shortcut: '/',
        icon: Search,
        run: () => searchRef.current?.focus(),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'topology.refresh',
        title: 'Collect neighbour data for this drawing',
        group: 'Topology',
        keywords: ['refresh', 'collect', 'lldp', 'cdp'],
        icon: RefreshCw,
        run: refresh,
      }),
    // `refresh` closes over the drawing, so it is registered again with it.
    [data],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'topology.path',
        title: 'Trace a path',
        group: 'Topology',
        keywords: ['route', 'hops', 'traceroute'],
        icon: Route,
        run: () => setPanel('path'),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'topology.export',
        title: 'Export the topology',
        group: 'Topology',
        keywords: ['svg', 'drawio', 'mermaid', 'dot', 'json'],
        icon: Download,
        run: () => setExporting(true),
      }),
    [],
  );

  // ------------------------------------------------------------------ render

  const viewMenu: MenuEntry[] = [
    { type: 'label', label: 'Views' },
    ...(views.data ?? []).map(
      (entry): MenuEntry => ({
        label: entry.name,
        checked: entry.id === viewId,
        onSelect: () => {
          setViewId(entry.id);
          writeStored(VIEW_KEY, entry.id);
          setLayer(null);
          setEndpoints(null);
          setSite('');
          setRole('');
          setSelection(null);
        },
      }),
    ),
    { type: 'separator' },
    { label: 'Save as a new view…', icon: Save, onSelect: () => saveCurrentView(true) },
    {
      label: `Save changes to “${current?.name ?? 'this view'}”`,
      icon: Save,
      disabled: !current,
      onSelect: () => saveCurrentView(false),
    },
    {
      label: 'Delete this view…',
      icon: Trash2,
      tone: 'danger',
      disabled: viewId === DEFAULT_VIEW,
      onSelect: async () => {
        const sure = await confirm({
          title: `Delete the view “${current?.name ?? viewId}”?`,
          description: 'Its saved positions go with it. No device is touched.',
          confirmLabel: 'Delete view',
          tone: 'danger',
        });
        if (!sure) return;
        deleteView.mutate(
          { id: viewId },
          {
            onSuccess: () => {
              setViewId(DEFAULT_VIEW);
              writeStored(VIEW_KEY, DEFAULT_VIEW);
            },
          },
        );
      },
    },
  ];

  /**
   * What a right-click on a card offers. The same actions are in the side
   * panel — a context menu is a shortcut, not the only way to anything — and
   * each one that cannot be done says why instead of failing when pressed.
   */
  const nodeMenu = (node: Node): MenuEntry[] => {
    const offer = offerFor(node);
    const existing = offer.ok ? sessionOn(sessions.data, offer.deviceId) : null;
    return [
      { type: 'label', label: node.name },
      {
        // A menu item has one line, so a refusal is three or four words here
        // and the whole sentence in the side panel. Disabled and unexplained
        // teaches nobody anything; this at least says which thing is missing.
        label: offer.ok
          ? sessionAction(existing) === 'show'
            ? 'Show the session'
            : 'Terminal'
          : `Terminal — ${offer.brief}`,
        icon: TerminalSquare,
        disabled: !offer.ok,
        onSelect: () => terminal(node),
      },
      {
        label: node.device_id ? 'Open device' : 'Open device — not in the inventory',
        icon: ExternalLink,
        disabled: !node.device_id,
        onSelect: () => {
          if (node.device_id) navigate(`/devices/${node.device_id}`);
        },
      },
      { label: 'Trace from here', icon: Route, onSelect: () => traceFrom(node) },
      {
        label: node.pinned ? 'Unpin' : 'Pin here',
        icon: node.pinned ? PinOff : Pin,
        onSelect: () => pin(node, !node.pinned),
      },
      ...(node.unmanaged && node.id.startsWith('u:')
        ? [
            { type: 'separator' as const },
            {
              label: 'Add as a device…',
              icon: Plus,
              onSelect: () => void addToInventory(node),
            },
          ]
        : []),
    ];
  };

  const layoutMenu: MenuEntry[] = [
    { type: 'label', label: 'Arrange the nodes' },
    ...ALGORITHMS.map(
      (entry): MenuEntry => ({
        label: entry.value === 'radial' && selectedNode ? `Radial around ${selectedNode.name}` : entry.label,
        onSelect: () => arrange(entry.value),
      }),
    ),
    { type: 'separator' },
    {
      label: 'Interface names on the links',
      checked: labels,
      onSelect: () => {
        setLabels((was) => {
          writeStored(LABELS_KEY, !was);
          return !was;
        });
      },
    },
    { label: 'Fit to the window', onSelect: () => viewportRef.current?.fit() },
  ];

  const endpointsOn = endpoints ?? current?.include_endpoints ?? false;

  const toolbar = (
    <Toolbar className={styles.toolbar}>
      <DropdownMenu
        aria-label="Views"
        items={viewMenu}
        trigger={
          <Button size="sm" variant="secondary" icon={Boxes}>
            {current?.name ?? 'Default'}
          </Button>
        }
      />

      <Select
        size="sm"
        className={styles.filter}
        aria-label="Layer"
        value={layer ?? data?.layer ?? 'physical'}
        options={[
          { value: 'physical', label: 'Physical' },
          { value: 'l3', label: 'Layer 3' },
        ]}
        onChange={(event) => {
          setLayer(event.target.value as Layer);
          setSelection(null);
        }}
      />

      {facets.sites.length > 1 && (
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Site"
          value={site}
          options={[
            { value: '', label: 'Every site' },
            ...facets.sites.map((name) => ({ value: name, label: name })),
          ]}
          onChange={(event) => setSite(event.target.value)}
        />
      )}

      {facets.roles.length > 1 && (
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Role"
          value={role}
          options={[
            { value: '', label: 'Every role' },
            ...facets.roles.map((name) => ({ value: name, label: name })),
          ]}
          onChange={(event) => setRole(event.target.value)}
        />
      )}

      <Button
        size="sm"
        variant={endpointsOn ? 'secondary' : 'ghost'}
        aria-pressed={endpointsOn}
        icon={Layers}
        onClick={() => setEndpoints(!endpointsOn)}
        title="Hosts learnt from MAC tables on access ports"
      >
        Endpoints
      </Button>

      <Button
        size="sm"
        variant={showLoad ? 'secondary' : 'ghost'}
        aria-pressed={showLoad}
        icon={Activity}
        onClick={() => {
          setShowLoad(!showLoad);
          writeStored(LOAD_KEY, !showLoad);
        }}
        title="How much traffic each link is carrying, in both directions, from monitoring's interface counters"
      >
        Load
      </Button>

      <SearchInput
        ref={searchRef}
        size="sm"
        className={styles.search}
        aria-label="Search for a device, an IP or a MAC address"
        placeholder="Name, IP or MAC…"
        value={search}
        onChange={setSearch}
        onKeyDown={(event) => {
          if (event.key === 'Enter') runSearch();
        }}
      />

      <ToolbarSpacer />

      <DropdownMenu
        aria-label="Arrange"
        items={layoutMenu}
        trigger={
          <IconButton
            size="sm"
            variant="ghost"
            icon={Wand2}
            aria-label="Arrange the nodes"
            loading={autolayout.isPending}
          />
        }
      />
      <IconButton
        size="sm"
        variant="ghost"
        icon={Pencil}
        aria-label="Links drawn by hand"
        onClick={() => setDrawing(true)}
      />
      <IconButton
        size="sm"
        variant="ghost"
        icon={Download}
        aria-label="Export this drawing"
        onClick={() => setExporting(true)}
      />
      <Button
        size="sm"
        variant="primary"
        icon={RefreshCw}
        loading={collect.isPending || Boolean(collectJob)}
        onClick={refresh}
      >
        Refresh data
      </Button>
    </Toolbar>
  );

  if (graph.isError) {
    return (
      <Page flush>
        {toolbar}
        <ErrorState
          title="The topology could not be drawn"
          error={graph.error}
          onRetry={() => void graph.refetch()}
        />
      </Page>
    );
  }

  if (graph.isPending) {
    return (
      <Page flush>
        {toolbar}
        <div className={styles.loading} aria-busy="true">
          <Skeleton shape="block" height="100%" />
        </div>
      </Page>
    );
  }

  const empty = nodes.length === 0;

  return (
    <Page flush>
      {toolbar}
      {data && (
        <Warnings
          graph={data}
          staleAfterHours={settings.data?.stale_after_hours}
          onRefresh={refresh}
          refreshing={collect.isPending || Boolean(collectJob)}
        />
      )}

      <div className={styles.body}>
        <div className={styles.canvasWrap}>
          {empty && (
            <EmptyState
              icon={Boxes}
              title="No topology yet"
              description="Nocturne draws the network from what the devices themselves report: LLDP, CDP and MNDP neighbours, and the subnets their interfaces share. Collect that from the devices and the drawing appears."
              action={
                <Button variant="primary" icon={RefreshCw} onClick={refresh}>
                  Collect neighbour data
                </Button>
              }
            />
          )}
          {!empty && data && (
            <>
              <Canvas
                graph={data}
                viewportRef={viewportRef}
                // A new picture is fitted; the same picture redrawn is left
                // where the user put it. A trace counts as a new picture,
                // because its stub can be outside what is on screen — and so
                // does the terminal dock, which takes a third of the canvas:
                // without this, the cards at the bottom of the drawing end up
                // under the legend, where they can be seen and not clicked.
                fitKey={`${viewId}|${data.layer}|${endpointsOn}|${pathQuery?.from ?? ''}>${
                  pathQuery?.to ?? ''
                }|${session ? 'dock' : ''}`}
                selection={selection}
                onSelect={(next) => {
                  setSelection(next);
                  if (next) setPanel('selection');
                }}
                onMove={move}
                onOpen={(node) => {
                  if (node.device_id) navigate(`/devices/${node.device_id}`);
                }}
                trace={overlay}
                host={highlight}
                showLabels={labels}
                loads={loads}
                showLoad={showLoad}
                nodeMenu={nodeMenu}
              />
              <p className={styles.stats}>
                {plural(data.stats.devices, 'device')} · {plural(data.stats.links, 'link')}
                {data.stats.unmanaged > 0 && ` · ${data.stats.unmanaged} not in the inventory`}
                {data.stats.endpoints > 0 && ` · ${plural(data.stats.endpoints, 'endpoint')}`}
                {/*
                  When the load was measured, which is the one thing that
                  decides whether the numbers on the lines mean anything. A
                  drawing that says "live" is making a claim; this says when.
                */}
                {showLoad && (
                  <span className={styles.statsLoad} data-testid="load-as-of">
                    {' · '}
                    {loadLayer === 'ready' && loadAt != null
                      ? `load measured ${formatRelative(loadAt, now)}`
                      : loadLayer === 'ready' || loadLayer === 'nothing'
                        ? 'no link rates measured yet'
                        : loadLayer === 'loading'
                          ? 'reading the link rates…'
                          : 'no link rates: monitoring is not answering'}
                  </span>
                )}
              </p>
              {/* Under the drawing, never over it: a key that covers three
                  devices is not a key, which is what the first screenshots
                  showed. */}
              <Legend graph={data} loads={showLoad ? loads : null} loadLayer={loadLayer} />
            </>
          )}

          {/*
            The terminal opened from a card, under the drawing rather than
            floating over it: a session window on top of the map covers the
            thing the map is for.
          */}
          {session && (
            <SessionDock
              sessionId={session.id}
              name={session.name}
              lab={session.lab}
              onHide={() => setSession(null)}
              onClose={() => void endSession()}
              onHandOver={handOverSession}
            />
          )}
        </div>

        <div className={styles.side}>
          <div className={styles.tabs} role="tablist" aria-label="Side panel">
            {(
              [
                ['selection', 'Selection'],
                ['path', 'Path'],
                ['host', 'Find host'],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={panel === id}
                className={styles.tab}
                onClick={() => setPanel(id)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className={styles.sideBody}>
            {panel === 'selection' && data && (
              <Inspector
                graph={data}
                node={selectedNode}
                link={selectedLink}
                onClose={() => setSelection(null)}
                onTraceFrom={traceFrom}
                onTraceBetween={traceBetween}
                onPin={pin}
                onAddToInventory={(node) => void addToInventory(node)}
                onDeleteManual={() => setDrawing(true)}
                onSelectNode={(id) => setSelection({ kind: 'node', id })}
                {...(selectedNode ? { terminal: offerFor(selectedNode) } : {})}
                terminalAction={sessionAction(
                  selectedNode?.device_id
                    ? sessionOn(sessions.data, selectedNode.device_id)
                    : null,
                )}
                onTerminal={terminal}
                {...(selectedLink && loads?.get(selectedLink.id)
                  ? { load: loads.get(selectedLink.id) }
                  : {})}
                loadLayer={loadLayer}
              />
            )}

            {panel === 'path' && data && (
              <PathPanel
                graph={data}
                from={pathFrom}
                to={pathTo}
                onFrom={setPathFrom}
                onTo={setPathTo}
                asked={Boolean(pathQuery)}
                trace={path.data}
                pending={Boolean(pathQuery) && path.isFetching}
                error={path.error ?? null}
                onTrace={() => setPathQuery({ from: pathFrom.trim(), to: pathTo.trim() })}
                onClear={() => setPathQuery(null)}
              />
            )}

            {panel === 'host' && (
              <HostPanel
                query={hostQuery}
                located={located.data}
                pending={hostQuery !== '' && located.isFetching}
                error={located.error ?? null}
                onSelectDevice={(deviceId) => {
                  const node = nodes.find((candidate) => candidate.device_id === deviceId);
                  if (node) centreOn(node);
                }}
              />
            )}
          </div>
        </div>
      </div>

      <ExportDialog open={exporting} onClose={() => setExporting(false)} params={params} />
      <ManualLinks open={drawing} onClose={() => setDrawing(false)} graph={data} />
    </Page>
  );
}
