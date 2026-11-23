/**
 * Terminal — the screen a network engineer keeps open all day.
 *
 * Sessions live in the backend, so this screen can come and go: `terminal.list`
 * finds what is running and `terminal.attach` hands over the scrollback and
 * the place in the event stream to carry on from. The xterm instances are
 * owned by `sessions.ts`, outside React, so output never costs a render and
 * leaving the screen does not disturb a session.
 */

import {
  CircleDot,
  ClipboardPaste,
  Copy,
  Eraser,
  FileText,
  FolderTree,
  Radio,
  ScrollText,
  Search,
  SquareSplitHorizontal,
  SquareTerminal,
  TerminalSquare,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import { useRpc, useRpcMutation } from '@/api/hooks';
import { inDesktopShell, readClipboardText } from '@/api/transports/shell';
import type {
  Adhoc,
  SerialPortInfo,
  SessionInfo,
  Snippet,
  TerminalInputManyResult,
  TerminalOpenParams,
} from '@/api/types/terminal';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Button, IconButton } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Skeleton } from '@/design/Skeleton';
import { Splitter } from '@/design/Splitter';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { copyText } from '@/lib/clipboard';
import { isMacPlatform } from '@/lib/keyboard';
import { useTerminalAppearance } from './appearance';
import { explainWithAssistant } from './assistant';
import type { InventoryDevice } from './inventory';
import { activeSessionId, broadcastTargets, MAX_PANES, useTerminalLayout } from './layout';
import { Pane, type PaneActions } from './Pane';
import type { PasteQuestion } from './pasteFunnel';
import { PasteSheet } from './PasteSheet';
import { adhocFromSession } from './quickconnect';
import { readRecents, rememberRecent, type Recent } from './recents';
import styles from './Screen.module.css';
import { SessionList } from './SessionList';
import { terminals } from './sessions';
import { SftpPanel } from './SftpPanel';
import { SnippetsDrawer } from './SnippetsDrawer';
import './tokens.css';

const SESSION_TOPICS = ['terminal.opened', 'terminal.changed', 'terminal.closed', 'terminal.removed'];

export default function TerminalPage() {
  usePageHeader({ title: 'Terminal' });

  const layout = useTerminalLayout();
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [recents, setRecents] = useState<Recent[]>(() => readRecents());
  const [snippetsOpen, setSnippetsOpen] = useState(false);
  const [findPanes, setFindPanes] = useState<string[]>([]);
  const [dragging, setDragging] = useState<string | null>(null);
  /** The paste question on screen, and how the funnel is told the answer. */
  const [pasteQuestion, setPasteQuestion] = useState<{
    question: PasteQuestion;
    answer: (confirmed: boolean) => void;
  } | null>(null);

  // -------------------------------------------------------------- the data

  const sessions = useRpc<SessionInfo[]>('terminal.list', undefined, { invalidateOn: SESSION_TOPICS });
  const devices = useRpc<InventoryDevice[]>('devices.list', undefined, { invalidateOn: ['devices.*'] });
  // The one honest way to ask whether this machine is within reach: the
  // backend refuses with `forbidden` when the caller is not sitting at it.
  const serialPorts = useRpc<SerialPortInfo[]>('terminal.serial_ports');

  const sessionList = useMemo(() => sessions.data ?? [], [sessions.data]);
  const byId = useMemo(() => new Map(sessionList.map((session) => [session.id, session])), [sessionList]);
  const tabIds = useMemo(() => layout.panes.flatMap((pane) => pane.tabs), [layout.panes]);
  const focusedId = activeSessionId(layout);
  const focusedSession = focusedId ? byId.get(focusedId) : undefined;
  const targets = broadcastTargets(layout);
  const targetKey = targets.join(' ');

  const local = useMemo(() => {
    const error = serialPorts.error;
    if (error) {
      return {
        allowed: false,
        reason:
          error.code === 'unknown_method'
            ? 'This build has no local shell and no serial ports.'
            : error.message,
        ports: [],
      };
    }
    return { allowed: !serialPorts.isPending, reason: null, ports: serialPorts.data ?? [] };
  }, [serialPorts.data, serialPorts.error, serialPorts.isPending]);

  // ------------------------------------------------------------- the looks

  const appearance = useTerminalAppearance();
  useEffect(() => {
    terminals.setAppearance(appearance);
  }, [appearance]);

  // -------------------------------------------------- sessions ⇄ the panes

  // What the backend holds is the truth: tabs for sessions that are gone go
  // away, and a session this window does not know about gets a tab.
  useEffect(() => {
    if (!sessions.data) return;
    useTerminalLayout.getState().reconcile(sessions.data.map((session) => session.id));
  }, [sessions.data]);

  useEffect(() => {
    const wanted = new Set(tabIds);
    for (const id of tabIds) terminals.adopt(id);
    for (const id of terminals.ids()) if (!wanted.has(id)) terminals.forget(id);
  }, [tabIds]);

  // Broadcast typing: one call carrying every target, so a device that is
  // slower than the others cannot reorder what the rest receive.
  useEffect(() => {
    if (!layout.broadcast || targets.length < 2) {
      terminals.setBroadcast(null);
      return;
    }
    const to = targetKey.split(' ');
    terminals.setBroadcast({
      targets: to,
      send: (data) => {
        void rpc<TerminalInputManyResult>('terminal.input_many', { session_ids: to, data })
          .then((result) => {
            for (const failure of result.failed) {
              toast.error('A session did not take that', { description: failure.message });
            }
          })
          .catch((error: unknown) => {
            if (error instanceof RpcError) toast.error(error.message);
          });
      },
    });
    return () => terminals.setBroadcast(null);
  }, [layout.broadcast, targets.length, targetKey]);

  // The seat belt. Every paste, by whatever door, goes through the registry's
  // funnel (`terminals.paste`); when one is worth a question this screen
  // shows it as the sheet with every line on it.
  useEffect(() => {
    let open: ((confirmed: boolean) => void) | null = null;
    terminals.setPasteAsker(
      (question) =>
        new Promise<boolean>((resolve) => {
          open = (confirmed) => {
            open = null;
            setPasteQuestion(null);
            resolve(confirmed);
          };
          setPasteQuestion({ question, answer: open });
        }),
    );
    return () => {
      terminals.setPasteAsker(null);
      // Leaving the screen with the sheet open is a "no": an answer that
      // never came would hold up every paste after it, on every screen.
      open?.(false);
    };
  }, []);

  // ----------------------------------------------------------- the actions

  const open = useRpcMutation<SessionInfo, TerminalOpenParams>('terminal.open', {
    invalidates: ['terminal.list'],
    errorToast: 'Could not connect',
  });
  const close = useRpcMutation<unknown, { session_id: string }>('terminal.close', {
    invalidates: ['terminal.list'],
    errorToast: false,
  });
  const logging = useRpcMutation<SessionInfo, { session_id: string; enabled: boolean }>('terminal.logging', {
    invalidates: ['terminal.list'],
  });

  const landSession = (info: SessionInfo) => {
    terminals.adopt(info.id);
    useTerminalLayout.getState().open(info.id);
  };

  const openDevice = (device: InventoryDevice) => {
    open.mutate(
      { device_id: device.id },
      {
        onSuccess: (info) => {
          landSession(info);
          setRecents(rememberRecent({ kind: 'device', id: device.id, label: device.name }));
        },
      },
    );
  };

  const openAdhoc = (adhoc: Adhoc, label: string) => {
    open.mutate(
      { adhoc },
      {
        onSuccess: (info) => {
          landSession(info);
          const transport = adhoc.transport ?? 'ssh';
          if (transport !== 'local') {
            setRecents(rememberRecent({ kind: 'adhoc', transport, target: label, label }));
          }
        },
      },
    );
  };

  const closeSession = async (id: string) => {
    const info = byId.get(id);
    if (info && info.state !== 'closed') {
      const confirmed = await confirm({
        title: `Close ${info.title}?`,
        description: `The session to ${info.info.peer} ends now. Anything unsaved on the device stays unsaved.`,
        confirmLabel: 'Close session',
        tone: 'danger',
      });
      if (!confirmed) return;
    }
    useTerminalLayout.getState().close(id);
    terminals.forget(id);
    close.mutate({ session_id: id });
  };

  const reconnect = async (id: string) => {
    const info = byId.get(id);
    if (!info) return;
    if (info.state !== 'closed') {
      const confirmed = await confirm({
        title: `Connect to ${info.title} again?`,
        description: `The session to ${info.info.peer} is closed first and then opened again. Anything unsaved on the device stays unsaved.`,
        confirmLabel: 'Reconnect',
      });
      if (!confirmed) return;
    }
    useTerminalLayout.getState().close(id);
    terminals.forget(id);
    close.mutate({ session_id: id });

    if (info.device_id) {
      open.mutate({ device_id: info.device_id }, { onSuccess: landSession });
      return;
    }
    const adhoc = adhocFromSession(info);
    if (!adhoc) {
      toast.info('Type the target again', {
        description: 'This session was opened by hand, and its password was never stored.',
      });
      searchRef.current?.focus();
      return;
    }
    open.mutate({ adhoc }, { onSuccess: landSession });
  };

  const toggleLogging = (id: string) => {
    const info = byId.get(id);
    if (!info) return;
    logging.mutate(
      { session_id: id, enabled: !info.logging },
      {
        onSuccess: (next) => {
          if (next.logging) toast.success('Logging this session', { description: next.logging });
          else toast.success('Logging stopped');
        },
      },
    );
  };

  const copySelection = async (sessionId: string) => {
    const selection = terminals.terminal(sessionId)?.getSelection() ?? '';
    if (selection === '') {
      toast.info('Nothing is selected');
      return;
    }
    const copied = await copyText(selection);
    if (copied) toast.success(`Copied ${selection.split('\n').length} lines`);
    else toast.error('The clipboard refused the copy');
  };

  const explain = (sessionId: string) => {
    const info = byId.get(sessionId);
    const selection = terminals.terminal(sessionId)?.getSelection() ?? '';
    if (selection === '') return;
    explainWithAssistant({
      source: info?.title ?? 'a terminal session',
      deviceId: info?.device_id ?? null,
      text: selection,
    });
  };

  /**
   * A paste with no `paste` event to carry its text — the toolbar button, the
   * context menu — reads the clipboard: through the shell on the desktop, and
   * with the browser's permission elsewhere. When that is refused the person
   * is told, and told what works without it.
   */
  const pasteFromClipboard = async (sessionId: string) => {
    let text: string;
    try {
      text = await readClipboardText();
    } catch (error) {
      // The shell rejects with its reason as a sentence (an empty clipboard,
      // most often); a browser with an exception that says nothing useful.
      const why =
        inDesktopShell() && typeof error === 'string'
          ? error.replace(/[.\s]*$/, '.')
          : inDesktopShell()
            ? 'The clipboard could not be read.'
            : 'The browser did not hand over the clipboard.';
      // The keyboard's paste carries its own text, so it works regardless.
      const chord = isMacPlatform() ? '⌘V' : 'Ctrl+Shift+V or Shift+Insert';
      toast.info('Nothing was pasted', {
        description: `${why} ${chord} in the terminal pastes without reading it.`,
      });
      return;
    }
    if (text === '') {
      toast.info('Nothing was pasted', { description: 'There is no text on the clipboard.' });
      return;
    }
    terminals.paste(sessionId, text);
  };

  const focusSearch = () => {
    searchRef.current?.focus();
    searchRef.current?.select();
  };

  const setFinding = (paneId: string, finding: boolean) =>
    setFindPanes((current) =>
      finding ? [...new Set([...current, paneId])] : current.filter((id) => id !== paneId),
    );

  const actions: PaneActions = {
    select: (paneId, sessionId) => {
      useTerminalLayout.getState().select(paneId, sessionId);
      terminals.focus(sessionId);
    },
    focusPane: (paneId) => useTerminalLayout.getState().focus(paneId),
    closeTab: (sessionId) => void closeSession(sessionId),
    reconnect: (sessionId) => void reconnect(sessionId),
    toggleLogging,
    split: (sessionId) => useTerminalLayout.getState().split(sessionId),
    moveTo: (sessionId, paneId) => useTerminalLayout.getState().moveTo(sessionId, paneId),
    closePane: (paneId) => useTerminalLayout.getState().closePane(paneId),
    pasteFromClipboard: (sessionId) => void pasteFromClipboard(sessionId),
    copySelection: (sessionId) => void copySelection(sessionId),
    explain,
    clear: (sessionId) => terminals.clear(sessionId),
    connect: focusSearch,
  };

  // --------------------------------------------------- keyboard and status

  useEffect(() => {
    const registered = [
      registerCommand({
        id: 'terminal.connect',
        title: 'Connect to a device',
        group: 'Terminal',
        keywords: ['ssh', 'telnet', 'serial', 'session', 'open'],
        icon: TerminalSquare,
        run: () => searchRef.current?.focus(),
      }),
      registerCommand({
        id: 'terminal.find',
        title: 'Find in scrollback',
        group: 'Terminal',
        shortcut: 'mod+shift+f',
        icon: Search,
        run: () => setFinding(useTerminalLayout.getState().focused, true),
      }),
      registerCommand({
        id: 'terminal.snippets',
        title: 'Open snippets',
        group: 'Terminal',
        shortcut: 'mod+shift+s',
        icon: ScrollText,
        run: () => setSnippetsOpen(true),
      }),
      registerCommand({
        id: 'terminal.broadcast',
        title: 'Type into every pane',
        group: 'Terminal',
        shortcut: 'mod+shift+b',
        icon: Radio,
        run: () => {
          const state = useTerminalLayout.getState();
          state.setBroadcast(!state.broadcast);
        },
      }),
      registerCommand({
        id: 'terminal.split',
        title: 'Move this session to a new pane',
        group: 'Terminal',
        shortcut: 'mod+shift+d',
        icon: SquareSplitHorizontal,
        run: () => {
          const state = useTerminalLayout.getState();
          const id = activeSessionId(state);
          if (id) state.split(id);
        },
      }),
      registerCommand({
        id: 'terminal.clear',
        title: 'Clear the screen',
        group: 'Terminal',
        shortcut: 'mod+shift+k',
        icon: Eraser,
        run: () => {
          const id = activeSessionId(useTerminalLayout.getState());
          if (id) terminals.clear(id);
        },
      }),
    ];
    return () => {
      for (const remove of registered) remove();
    };
  }, []);

  // `/` focuses the screen's search, as on every other screen. Inside a
  // terminal it is a character like any other.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        (target.closest('[data-global-shortcuts="off"]') || target.closest('input, textarea'))
      ) {
        return;
      }
      event.preventDefault();
      searchRef.current?.focus();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const live = sessionList.filter((session) => session.state === 'open').length;
  useStatusItem(
    sessions.data
      ? {
          id: 'terminal',
          tone: live > 0 ? 'ok' : 'idle',
          label: `${live} session${live === 1 ? '' : 's'}`,
          detail:
            sessionList.length === live
              ? 'Terminal sessions running in the engine'
              : `${sessionList.length - live} of ${sessionList.length} have closed`,
        }
      : null,
  );

  // An empty screen puts the cursor where the next thing happens.
  const nothingOpen = tabIds.length === 0;
  useEffect(() => {
    if (nothingOpen) searchRef.current?.focus();
  }, [nothingOpen]);

  // ------------------------------------------------------------- the parts

  const sftpDevice =
    focusedSession && focusedSession.kind === 'ssh' && focusedSession.device_id
      ? { id: focusedSession.device_id, name: focusedSession.title }
      : null;

  const draggingPane = dragging ? layout.panes.find((pane) => pane.tabs.includes(dragging)) : undefined;
  const draggingCanSplit = Boolean(
    dragging && layout.panes.length < MAX_PANES && (draggingPane?.tabs.length ?? 0) > 1,
  );

  const content = (() => {
    // A method this build does not have is not a failure to shout about.
    if (sessions.error?.code === 'unknown_method') {
      return (
        <div className={styles.empty}>
          <EmptyState
            icon={SquareTerminal}
            title="Terminal sessions are not available in this build"
            description="This engine does not answer terminal.list, so there is nothing for this screen to show or open."
          />
        </div>
      );
    }
    if (sessions.isError) {
      return (
        <div className={styles.empty}>
          <ErrorState
            title="Sessions could not be listed"
            error={sessions.error}
            onRetry={() => void sessions.refetch()}
          />
        </div>
      );
    }
    if (sessions.isPending) {
      return (
        <div className={styles.panes} data-count={1} aria-busy="true">
          <Skeleton shape="block" height="100%" />
        </div>
      );
    }
    if (nothingOpen) {
      return (
        <div className={styles.empty}>
          <EmptyState
            icon={SquareTerminal}
            title="Connect to a device"
            description="Pick one from the list, or type a target into quick connect. Sessions keep running in the engine: leaving this screen, or reloading the window, finds them again."
            action={
              <Button variant="primary" icon={TerminalSquare} onClick={() => searchRef.current?.focus()}>
                Search devices
              </Button>
            }
          />
        </div>
      );
    }
    return (
      <div className={styles.panes} data-count={layout.panes.length}>
        {layout.panes.map((pane) => (
          <Pane
            key={pane.id}
            pane={pane}
            sessions={byId}
            focused={pane.id === layout.focused}
            paneCount={layout.panes.length}
            broadcasting={layout.broadcast && targets.length > 1 && !layout.excluded.includes(pane.id)}
            actions={actions}
            dragging={dragging}
            draggingCanSplit={draggingCanSplit}
            onDragging={setDragging}
            finding={findPanes.includes(pane.id)}
            onFinding={(finding) => setFinding(pane.id, finding)}
          />
        ))}
      </div>
    );
  })();

  return (
    <Page flush>
      <div className={styles.screen}>
        <Splitter
          storageId="terminal.sidebar"
          defaultSize={264}
          minSize={220}
          maxSize={420}
          handleLabel="Resize the session list"
          className={styles.sidebar}
        >
          <SessionList
            devices={devices.data}
            devicesError={devices.error}
            devicesLoading={devices.isPending}
            onRetryDevices={() => void devices.refetch()}
            sessions={sessionList}
            activeSessionId={focusedId}
            local={local}
            recents={recents}
            connectingDeviceId={open.isPending ? (open.variables.device_id ?? null) : null}
            connectingAdhoc={open.isPending && open.variables.adhoc !== undefined}
            searchRef={searchRef}
            query={query}
            onQuery={setQuery}
            onOpenDevice={openDevice}
            onOpenAdhoc={openAdhoc}
            onSelectSession={(id) => {
              const state = useTerminalLayout.getState();
              const pane = state.panes.find((entry) => entry.tabs.includes(id));
              if (pane) state.select(pane.id, id);
              else state.open(id);
              terminals.focus(id);
            }}
            onCloseSession={(id) => void closeSession(id)}
          />

          <div className={styles.main}>
            <Toolbar className={styles.toolbar} aria-label="Terminal actions">
              <div className={styles.group}>
                <IconButton
                  icon={Search}
                  size="sm"
                  aria-label="Find in scrollback"
                  tooltip="Find in scrollback (Ctrl+Shift+F)"
                  disabled={!focusedId}
                  onClick={() => setFinding(layout.focused, true)}
                />
                <IconButton
                  icon={Copy}
                  size="sm"
                  aria-label="Copy the selection"
                  tooltip="Copy the selection (Ctrl+Shift+C)"
                  disabled={!focusedId}
                  onClick={() => focusedId && void copySelection(focusedId)}
                />
                <IconButton
                  icon={ClipboardPaste}
                  size="sm"
                  aria-label="Paste into this session"
                  tooltip="Paste (Ctrl+Shift+V)"
                  disabled={!focusedId}
                  onClick={() => focusedId && void pasteFromClipboard(focusedId)}
                />
                <IconButton
                  icon={Eraser}
                  size="sm"
                  aria-label="Clear the screen"
                  tooltip="Clear the screen (Ctrl+Shift+K)"
                  disabled={!focusedId}
                  onClick={() => focusedId && terminals.clear(focusedId)}
                />
              </div>

              <span className={styles.divider} aria-hidden="true" />

              <Button
                size="sm"
                variant="ghost"
                icon={ScrollText}
                aria-label="Snippets"
                title="Snippets (Ctrl+Shift+S)"
                onClick={() => setSnippetsOpen(true)}
              >
                <span className={styles.label}>Snippets</span>
              </Button>
              <Button
                size="sm"
                variant="ghost"
                icon={focusedSession?.logging ? CircleDot : FileText}
                disabled={!focusedSession}
                aria-pressed={Boolean(focusedSession?.logging)}
                aria-label={focusedSession?.logging ? 'Stop logging this session' : 'Log this session to disk'}
                title={focusedSession?.logging ? 'Stop logging' : 'Write this session to a log file'}
                onClick={() => focusedId && toggleLogging(focusedId)}
              >
                <span className={styles.label}>
                  {focusedSession?.logging ? 'Stop logging' : 'Log session'}
                </span>
              </Button>
              <Button
                size="sm"
                variant="ghost"
                icon={FolderTree}
                disabled={!sftpDevice}
                aria-pressed={layout.sftpOpen}
                aria-label="Files on this device"
                title={sftpDevice ? `Files on ${sftpDevice.name} over SFTP` : 'SFTP needs an SSH session to a device'}
                onClick={() => layout.setSftpOpen(!layout.sftpOpen)}
              >
                <span className={styles.label}>Files</span>
              </Button>
              <Button
                size="sm"
                variant={layout.broadcast ? 'danger' : 'ghost'}
                icon={Radio}
                aria-pressed={layout.broadcast}
                disabled={layout.panes.length < 2}
                aria-label="Broadcast"
                title="Type into every pane at once (Ctrl+Shift+B)"
                onClick={() => layout.setBroadcast(!layout.broadcast)}
              >
                <span className={styles.label}>Broadcast</span>
              </Button>

              <ToolbarSpacer />
              <Button
                variant="primary"
                size="sm"
                icon={TerminalSquare}
                aria-label="Connect to a device"
                title="Connect to a device"
                onClick={() => searchRef.current?.focus()}
              >
                <span className={styles.label}>Connect…</span>
              </Button>
            </Toolbar>

            {layout.broadcast && (
              <div className={styles.broadcast} role="status">
                <span>
                  Typing goes to {targets.length} session{targets.length === 1 ? '' : 's'}
                </span>
                <span className={styles.broadcastTargets}>
                  {layout.panes.map((pane) => {
                    const info = pane.active ? byId.get(pane.active) : undefined;
                    if (!info) return null;
                    return (
                      <Checkbox
                        key={pane.id}
                        checked={!layout.excluded.includes(pane.id)}
                        onChange={() => layout.toggleExcluded(pane.id)}
                      >
                        {info.title}
                      </Checkbox>
                    );
                  })}
                </span>
                <Button size="sm" onClick={() => layout.setBroadcast(false)}>
                  Stop
                </Button>
              </div>
            )}

            <div className={styles.content}>
              {content}
              {layout.sftpOpen && sftpDevice && (
                <div className={styles.files}>
                  <SftpPanel
                    deviceId={sftpDevice.id}
                    deviceName={sftpDevice.name}
                    onClose={() => layout.setSftpOpen(false)}
                  />
                </div>
              )}
            </div>
          </div>
        </Splitter>
      </div>

      <PasteSheet
        request={pasteQuestion?.question ?? null}
        onCancel={() => pasteQuestion?.answer(false)}
        onConfirm={(question) => {
          // The funnel sends it; this only says yes.
          pasteQuestion?.answer(true);
          terminals.focus(question.sessionId);
        }}
      />

      <SnippetsDrawer
        open={snippetsOpen}
        onClose={() => setSnippetsOpen(false)}
        profile={focusedSession?.device_id ? deviceProfile(devices.data, focusedSession.device_id) : null}
        destination={focusedSession?.title ?? null}
        onSend={(snippet: Snippet) => {
          if (!focusedId) return;
          setSnippetsOpen(false);
          // As a snippet, so that its last newline is not sent: the Enter is
          // never ours.
          terminals.paste(focusedId, snippet.text, 'snippet');
        }}
      />
    </Page>
  );
}

function deviceProfile(devices: InventoryDevice[] | undefined, deviceId: string): string | null {
  const profile = devices?.find((entry) => entry.id === deviceId)?.profile;
  return profile ? profile : null;
}
