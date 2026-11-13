/**
 * What the shell itself contributes: navigation and view commands, and the
 * engine and version items of the status bar.
 */

import { Component, PanelLeft, Sparkles, SunMoon } from 'lucide-react';
import { useEffect } from 'react';
import { useNavigate } from 'react-router';
import { useConnection } from '@/api/connection';
import type { ConnectionState, TransportKind } from '@/api/transport';
import { useTheme } from '@/design/theme';
import { registerCommand } from './CommandPalette/commands';
import { DESTINATIONS } from './destinations';
import { useLayout } from './layout';
import { useStatusItem, type StatusItem } from './StatusBar/statusItems';

/** Registers "Go to …" for every destination and the view toggles. */
export function useBuiltinCommands(): void {
  const navigate = useNavigate();

  useEffect(() => {
    const unregister = [
      ...DESTINATIONS.map((item) =>
        registerCommand({
          id: `nav.${item.id}`,
          title: `Go to ${item.label}`,
          group: 'Navigate',
          keywords: item.keywords,
          icon: item.icon,
          run: () => void navigate(item.path),
        }),
      ),
      registerCommand({
        id: 'view.assistant',
        title: 'Toggle assistant',
        group: 'View',
        keywords: ['ai', 'chat', 'dock'],
        shortcut: 'mod+j',
        icon: Sparkles,
        run: () => useLayout.getState().toggleDock(),
      }),
      registerCommand({
        id: 'view.navigation',
        title: 'Toggle navigation',
        group: 'View',
        keywords: ['sidebar', 'rail', 'collapse', 'expand'],
        shortcut: 'mod+b',
        icon: PanelLeft,
        run: () => useLayout.getState().toggleNav(),
      }),
      registerCommand({
        id: 'view.theme',
        title: 'Toggle theme',
        group: 'View',
        keywords: ['night', 'day', 'dark', 'light', 'appearance'],
        icon: SunMoon,
        run: () => useTheme.getState().toggleTheme(),
      }),
    ];
    if (import.meta.env.DEV) {
      unregister.push(
        registerCommand({
          id: 'nav.dev.components',
          title: 'Go to Component showcase',
          group: 'Navigate',
          keywords: ['design system', 'dev', 'storybook'],
          icon: Component,
          run: () => void navigate('/dev/components'),
        }),
      );
    }
    return () => {
      for (const off of unregister) off();
    };
  }, [navigate]);
}

function engineStatus(state: ConnectionState, transport: TransportKind | null): StatusItem {
  const base = { id: 'engine', order: 0 } as const;
  if (state === 'offline') {
    return { ...base, tone: 'crit', label: 'Engine offline', detail: 'The connection was lost. Reconnecting…' };
  }
  if (state === 'connecting') {
    return { ...base, tone: 'idle', label: 'Connecting to engine…' };
  }
  if (transport === 'mock') {
    return {
      ...base,
      tone: 'maint',
      label: 'Mock engine',
      detail: 'No engine is connected: data comes from the mock transport (src/dev/mocks).',
    };
  }
  return {
    ...base,
    tone: 'ok',
    label: 'Engine online',
    detail: transport === 'tauri' ? 'Connected to the built-in engine' : 'Connected to the Nocturne server',
  };
}

/** Keeps the engine and version items of the status bar current. */
export function useShellStatusItems(): void {
  const state = useConnection((connection) => connection.state);
  const transport = useConnection((connection) => connection.transport);
  useStatusItem(engineStatus(state, transport));
  useStatusItem({
    id: 'version',
    side: 'right',
    order: 100,
    label: `Nocturne ${__APP_VERSION__}`,
    detail: 'Version of this interface',
  });
}
