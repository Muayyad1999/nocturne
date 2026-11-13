import { useRef } from 'react';
import { Outlet } from 'react-router';
import { ConfirmHost } from '@/design/Dialog';
import { Toaster } from '@/design/Toast';
import styles from './AppShell.module.css';
import { AssistantDock } from './AssistantDock/AssistantDock';
import { useBuiltinCommands, useShellStatusItems } from './builtins';
import { CommandPalette } from './CommandPalette';
import { Header } from './Header';
import { useLayout } from './layout';
import { Nav } from './Nav';
import { useGlobalShortcuts } from './shortcuts';
import { StatusBar } from './StatusBar';

/**
 * The frame every screen lives in: nav rail, header, the page, the assistant
 * dock and the status bar, plus the layers that float above them.
 */
export function AppShell() {
  const navCollapsed = useLayout((state) => state.navCollapsed);
  const dockOpen = useLayout((state) => state.dockOpen);
  const mainRef = useRef<HTMLElement>(null);

  useGlobalShortcuts();
  useBuiltinCommands();
  useShellStatusItems();

  return (
    <div className={styles.shell} data-nav={navCollapsed ? 'collapsed' : 'expanded'}>
      <button type="button" className={styles.skip} onClick={() => mainRef.current?.focus()}>
        Skip to content
      </button>
      <Nav />
      <Header />
      <main ref={mainRef} className={styles.main} tabIndex={-1}>
        <Outlet />
      </main>
      {dockOpen && <AssistantDock />}
      <StatusBar />
      <CommandPalette />
      <ConfirmHost />
      <Toaster />
    </div>
  );
}
