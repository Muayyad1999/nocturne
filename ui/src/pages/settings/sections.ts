/**
 * The sections of Settings, and whether this build has a backend for each.
 *
 * Nocturne is assembled from feature crates, so an installation may not have
 * the module behind a section. A section whose methods are not in
 * `app.methods` is left out of the list entirely — never shown broken, and
 * never shown as "coming soon".
 */

import {
  BookOpen,
  Bot,
  Bell,
  Cable,
  Cpu,
  Info,
  KeyRound,
  ScrollText,
  ShieldCheck,
  SlidersHorizontal,
} from 'lucide-react';
import type { LucideIcon } from '@/design/Icon';

export interface SettingsSection {
  /** The `:section` in the route. */
  id: string;
  label: string;
  /** One line under the title: what this section is for. */
  description: string;
  icon: LucideIcon;
  /** The section is shown only when the build answers all of these. */
  requires: readonly string[];
}

export const SECTIONS: readonly SettingsSection[] = [
  {
    id: 'general',
    label: 'General',
    description: 'How Nocturne looks and how it shows times.',
    icon: SlidersHorizontal,
    requires: ['settings.get', 'settings.set'],
  },
  {
    id: 'security',
    label: 'Security',
    description: 'The credential vault and the SSH host keys Nocturne trusts.',
    icon: ShieldCheck,
    requires: ['vault.state'],
  },
  {
    id: 'credentials',
    label: 'Credentials',
    description: 'The logins devices are reached with. Secrets are never shown again.',
    icon: KeyRound,
    requires: ['credentials.list'],
  },
  {
    id: 'device-types',
    label: 'Device types',
    description: 'What Nocturne knows about each platform: how to log in, read and configure it.',
    icon: Cpu,
    requires: ['profiles.list'],
  },
  {
    id: 'connections',
    label: 'Connections',
    description: 'Timeouts, keepalives and how unknown SSH host keys are treated.',
    icon: Cable,
    requires: ['settings.get', 'devices.test'],
  },
  // Sections whose feature crates are not in every build. They appear by
  // themselves once the methods are there; nothing here has to change.
  {
    id: 'monitoring',
    label: 'Monitoring',
    description: 'How often devices are polled and what is kept.',
    icon: Bell,
    requires: ['monitor.settings'],
  },
  {
    id: 'logs',
    label: 'Logs & traps',
    description: 'The syslog and SNMP trap receivers, and what to tell devices to send.',
    icon: ScrollText,
    requires: ['logs.receivers'],
  },
  {
    id: 'assistant',
    label: 'Assistant',
    description: 'Model providers, how much the assistant may do by itself, and extra instructions.',
    icon: Bot,
    requires: ['ai.providers'],
  },
  {
    id: 'about',
    label: 'About',
    description: 'Version, where the data lives, and the licences.',
    icon: Info,
    requires: ['app.info'],
  },
  {
    id: 'audit',
    label: 'Audit trail',
    description: 'Everything Nocturne has done, and who asked for it.',
    icon: BookOpen,
    requires: ['audit.list'],
  },
];

/** The sections this build can actually show. */
export function availableSections(methods: ReadonlySet<string>): SettingsSection[] {
  return SECTIONS.filter((section) => section.requires.every((method) => methods.has(method)));
}
