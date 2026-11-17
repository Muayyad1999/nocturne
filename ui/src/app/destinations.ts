/**
 * Every place the nav rail and the command palette can take you. The route
 * table (routes.tsx) maps the same paths to pages.
 */

import {
  Activity,
  ClipboardList,
  FileCode,
  FlaskConical,
  GitPullRequestArrow,
  LayoutDashboard,
  Network,
  Receipt,
  Server,
  Settings,
  Sparkles,
  SquareTerminal,
  Waypoints,
  Workflow,
  Wrench,
} from 'lucide-react';
import type { LucideIcon } from '@/design/Icon';

export type DestinationGroup = 'operate' | 'change' | 'plan' | 'record' | 'system';

export interface Destination {
  /** Stable id: the key for nav badges and commands (`nav.devices`). */
  id: string;
  path: string;
  label: string;
  icon: LucideIcon;
  group: DestinationGroup;
  /** Extra words that find it in the command palette. */
  keywords: string[];
  /**
   * An RPC method this destination needs. A build that does not answer it
   * does not show the item at all — not greyed out, not "coming soon". It is
   * how an edition that is assembled without a feature crate (the ISP
   * feature, which is not in the free edition) shows no sign of it anywhere.
   */
  requires?: string;
}

/** The groups shown in the rail, in order. `system` items sit at the bottom. */
export const NAV_GROUPS: readonly { id: DestinationGroup; label: string }[] = [
  { id: 'operate', label: 'Operate' },
  { id: 'change', label: 'Change' },
  { id: 'plan', label: 'Plan' },
  { id: 'record', label: 'Record' },
];

export const DESTINATIONS: readonly Destination[] = [
  {
    id: 'overview',
    path: '/',
    label: 'Overview',
    icon: LayoutDashboard,
    group: 'operate',
    keywords: ['home', 'dashboard', 'summary'],
  },
  {
    id: 'devices',
    path: '/devices',
    label: 'Devices',
    icon: Server,
    group: 'operate',
    keywords: ['inventory', 'routers', 'switches', 'firewalls', 'hosts'],
  },
  {
    id: 'terminal',
    path: '/terminal',
    label: 'Terminal',
    icon: SquareTerminal,
    group: 'operate',
    keywords: ['ssh', 'telnet', 'console', 'cli', 'session'],
  },
  {
    id: 'topology',
    path: '/topology',
    label: 'Topology',
    icon: Waypoints,
    group: 'operate',
    keywords: ['map', 'links', 'neighbours', 'neighbors', 'lldp', 'cdp'],
  },
  {
    id: 'monitoring',
    path: '/monitoring',
    label: 'Monitoring',
    icon: Activity,
    group: 'operate',
    keywords: ['alerts', 'metrics', 'snmp', 'syslog', 'graphs'],
  },
  {
    id: 'config',
    path: '/config',
    label: 'Config',
    icon: FileCode,
    group: 'change',
    keywords: ['configuration', 'backups', 'diff', 'compliance', 'templates'],
  },
  {
    id: 'changes',
    path: '/changes',
    label: 'Changes',
    icon: GitPullRequestArrow,
    group: 'change',
    keywords: ['approvals', 'change requests', 'rollback', 'review'],
  },
  {
    id: 'automation',
    path: '/automation',
    label: 'Automation',
    icon: Workflow,
    group: 'change',
    keywords: ['jobs', 'playbooks', 'schedules', 'tasks'],
  },
  {
    id: 'lab',
    path: '/lab',
    label: 'Lab',
    icon: FlaskConical,
    group: 'plan',
    keywords: ['simulation', 'sandbox', 'digital twin', 'virtual'],
  },
  {
    id: 'ipam',
    path: '/ipam',
    label: 'IPAM',
    icon: Network,
    group: 'plan',
    keywords: ['addresses', 'subnets', 'prefixes', 'vlans', 'ip'],
  },
  {
    id: 'tools',
    path: '/tools',
    label: 'Tools',
    icon: Wrench,
    group: 'plan',
    keywords: ['ping', 'traceroute', 'dns', 'subnet calculator', 'port scan'],
  },
  {
    id: 'ops',
    path: '/ops',
    label: 'Operations',
    icon: ClipboardList,
    group: 'record',
    keywords: ['incidents', 'handover', 'shift', 'reports', 'audit log'],
  },
  {
    id: 'isp',
    path: '/isp',
    label: 'ISP',
    icon: Receipt,
    group: 'record',
    keywords: [
      'customers',
      'subscribers',
      'billing',
      'invoices',
      'payments',
      'arrears',
      'tariffs',
      'revenue',
      'margin',
      'oversubscription',
    ],
    requires: 'isp.customers',
  },
  {
    id: 'assistant',
    path: '/assistant',
    label: 'Assistant',
    icon: Sparkles,
    group: 'system',
    keywords: ['ai', 'chat', 'history', 'conversations'],
  },
  {
    id: 'settings',
    path: '/settings',
    label: 'Settings',
    icon: Settings,
    group: 'system',
    keywords: ['preferences', 'credentials', 'options'],
  },
];

export function destination(id: string): Destination {
  const found = DESTINATIONS.find((entry) => entry.id === id);
  if (!found) throw new Error(`Unknown destination "${id}"`);
  return found;
}

/**
 * The destinations this build can actually reach, given the methods it
 * answers (`app.methods`).
 *
 * `methods` is `undefined` while that call is still in flight, and a
 * destination that declares `requires` is left out until the answer is in:
 * an edition without the feature and an edition whose method list has not
 * arrived yet look identical, and a paid feature appearing for a moment in
 * the free edition's rail is the worse of the two mistakes.
 */
export function visibleDestinations(
  methods: readonly string[] | undefined,
): readonly Destination[] {
  return DESTINATIONS.filter(
    (entry) => entry.requires === undefined || (methods?.includes(entry.requires) ?? false),
  );
}
