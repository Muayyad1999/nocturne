/**
 * The left rail: what there is to connect to, and what is already connected.
 *
 * Search filters the devices as you type; `/` puts the cursor here. Open
 * sessions are at the top because that is what the screen is usually used to
 * get back to.
 */

import { Cable, MonitorCog, Plug, Server, TerminalSquare, X } from 'lucide-react';
import { useMemo, useState, type RefObject } from 'react';
import type { RpcError } from '@/api/errors';
import type { Adhoc, SerialPortInfo, SessionInfo } from '@/api/types/terminal';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Spinner } from '@/design/Spinner';
import { StatusDot } from '@/design/StatusDot';
import { Tooltip } from '@/design/Tooltip';
import { fuzzyFilter } from '@/lib/fuzzy';
import { deviceStateLabel, deviceTone, type InventoryDevice } from './inventory';
import { adhocFor, parseQuickConnect, targetLabel } from './quickconnect';
import type { Recent } from './recents';
import styles from './SessionList.module.css';

export interface LocalAccess {
  /** Whether this machine's own shell and serial ports are within reach. */
  allowed: boolean;
  /** The backend's own sentence about why not. */
  reason: string | null;
  ports: SerialPortInfo[];
}

export interface SessionListProps {
  devices: InventoryDevice[] | undefined;
  devicesError: RpcError | null;
  devicesLoading: boolean;
  onRetryDevices: () => void;
  sessions: SessionInfo[];
  activeSessionId: string | null;
  local: LocalAccess;
  recents: Recent[];
  /** The device being dialled right now, for its row's busy state. */
  connectingDeviceId: string | null;
  /** A quick-connect target is being dialled. */
  connectingAdhoc: boolean;
  searchRef: RefObject<HTMLInputElement | null>;
  query: string;
  onQuery: (query: string) => void;
  onOpenDevice: (device: InventoryDevice) => void;
  onOpenAdhoc: (adhoc: Adhoc, label: string) => void;
  onSelectSession: (sessionId: string) => void;
  onCloseSession: (sessionId: string) => void;
}

function sessionTone(session: SessionInfo): 'ok' | 'warn' | 'crit' {
  if (session.state === 'open') return 'ok';
  if (session.state === 'connecting') return 'warn';
  return 'crit';
}

function Section({ title, count, children }: { title: string; count?: number; children: React.ReactNode }) {
  return (
    <section className={styles.section}>
      <header className={styles.sectionHeader}>
        <h2 className={styles.sectionTitle}>{title}</h2>
        {count !== undefined && <span className={styles.sectionCount}>{count}</span>}
      </header>
      {children}
    </section>
  );
}

export function SessionList({
  devices,
  devicesError,
  devicesLoading,
  onRetryDevices,
  sessions,
  activeSessionId,
  local,
  recents,
  connectingDeviceId,
  connectingAdhoc,
  searchRef,
  query,
  onQuery,
  onOpenDevice,
  onOpenAdhoc,
  onSelectSession,
  onCloseSession,
}: SessionListProps) {
  const [transport, setTransport] = useState<'ssh' | 'telnet'>('ssh');
  const [target, setTarget] = useState('');
  const [touched, setTouched] = useState(false);

  const parsed = parseQuickConnect(target, transport);
  const problem = touched && target.trim() !== '' && !parsed.ok ? parsed.problem : null;

  const matches = useMemo(() => {
    const all = devices ?? [];
    if (query.trim() === '') return all;
    return fuzzyFilter(all, query, {
      text: (device) => device.name,
      keywords: (device) => [device.address, device.site, device.role, device.profile, ...device.tags],
    }).map((result) => result.item);
  }, [devices, query]);

  const liveByDevice = new Map<string, SessionInfo>();
  for (const session of sessions) {
    if (session.device_id && session.state !== 'closed') liveByDevice.set(session.device_id, session);
  }

  const connect = () => {
    setTouched(true);
    if (!parsed.ok) return;
    onOpenAdhoc(adhocFor(parsed.target), targetLabel(parsed.target));
    setTarget('');
    setTouched(false);
  };

  return (
    <div className={styles.list}>
      <div className={styles.search}>
        <SearchInput
          ref={searchRef}
          aria-label="Search devices"
          placeholder="Search devices"
          value={query}
          onChange={onQuery}
        />
      </div>

      <div className={styles.scroll}>
        {sessions.length > 0 && (
          <Section title="Open sessions" count={sessions.length}>
            {sessions.map((session) => (
              <div
                key={session.id}
                className={styles.row}
                data-active={session.id === activeSessionId || undefined}
                role="button"
                tabIndex={0}
                onClick={() => onSelectSession(session.id)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    onSelectSession(session.id);
                  }
                }}
              >
                <StatusDot
                  tone={sessionTone(session)}
                  size={8}
                  pulse={session.state === 'connecting'}
                  label={session.state === 'open' ? 'Connected' : session.state === 'connecting' ? 'Connecting' : 'Closed'}
                />
                <span className={styles.rowText}>
                  <span className={styles.rowTitle}>{session.title}</span>
                  <span className={styles.rowDetail}>{session.info.peer}</span>
                </span>
                <span className={styles.rowAside}>
                  {session.lab && <span className={styles.pill}>LAB</span>}
                  <button
                    type="button"
                    className={styles.rowClose}
                    aria-label={`Close session ${session.title}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onCloseSession(session.id);
                    }}
                  >
                    <Icon icon={X} size={12} />
                  </button>
                </span>
              </div>
            ))}
          </Section>
        )}

        <Section title="Devices" count={devices?.length}>
          {devicesError ? (
            <ErrorState
              size="compact"
              title="Devices could not be loaded"
              error={devicesError}
              onRetry={onRetryDevices}
            />
          ) : devicesLoading ? (
            <>
              <Skeleton height={28} />
              <Skeleton height={28} />
              <Skeleton height={28} />
            </>
          ) : matches.length === 0 ? (
            <EmptyState
              size="compact"
              icon={Server}
              title={query ? 'No device matches' : 'No devices yet'}
              description={
                query
                  ? `Nothing in the inventory matches “${query}”. Quick connect reaches anything else.`
                  : 'Add devices on the Devices screen, or reach one straight away with quick connect below.'
              }
            />
          ) : (
            matches.map((device) => {
              const live = liveByDevice.get(device.id);
              return (
                <button
                  key={device.id}
                  type="button"
                  className={styles.row}
                  data-active={live && live.id === activeSessionId ? true : undefined}
                  onClick={() => (live ? onSelectSession(live.id) : onOpenDevice(device))}
                  title={`${device.name} — ${deviceStateLabel(device)}`}
                >
                  <StatusDot tone={deviceTone(device)} size={8} label={deviceStateLabel(device)} />
                  <span className={styles.rowText}>
                    <span className={styles.rowTitle}>{device.name}</span>
                    <span className={styles.rowDetail}>{device.address || device.transport}</span>
                  </span>
                  <span className={styles.rowAside}>
                    {device.lab && <span className={styles.pill}>LAB</span>}
                    {connectingDeviceId === device.id ? (
                      <Spinner size={12} label={`Connecting to ${device.name}`} />
                    ) : (
                      live && <span className={styles.open}>open</span>
                    )}
                  </span>
                </button>
              );
            })
          )}
        </Section>

        {recents.length > 0 && (
          <Section title="Recent">
            {recents.map((recent) => (
              <button
                key={`${recent.kind}:${recent.kind === 'device' ? recent.id : recent.target}`}
                type="button"
                className={styles.row}
                onClick={() => {
                  if (recent.kind === 'device') {
                    const device = devices?.find((entry) => entry.id === recent.id);
                    if (device) onOpenDevice(device);
                    return;
                  }
                  const parsedRecent = parseQuickConnect(recent.target, recent.transport === 'telnet' ? 'telnet' : 'ssh');
                  if (parsedRecent.ok) onOpenAdhoc(adhocFor(parsedRecent.target), recent.label);
                }}
              >
                <Icon icon={recent.kind === 'device' ? Server : Plug} size={14} />
                <span className={styles.rowText}>
                  <span className={styles.rowTitle}>{recent.label}</span>
                </span>
              </button>
            ))}
          </Section>
        )}

        <Section title="Quick connect">
          <div className={styles.quick}>
            <div className={styles.quickRow}>
              <Select
                className={styles.quickTransport}
                size="sm"
                aria-label="Transport"
                value={transport}
                onChange={(event) => setTransport(event.target.value === 'telnet' ? 'telnet' : 'ssh')}
                options={[
                  { value: 'ssh', label: 'SSH' },
                  { value: 'telnet', label: 'Telnet' },
                ]}
              />
              <Input
                className={styles.quickInput}
                size="sm"
                mono
                aria-label="Host to connect to"
                placeholder={transport === 'ssh' ? 'user@host:port' : 'host:port'}
                value={target}
                invalid={problem !== null}
                onChange={(event) => setTarget(event.target.value)}
                onBlur={() => setTouched(true)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    connect();
                  }
                }}
              />
            </div>
            {problem && <p className={styles.problem}>{problem}</p>}
            <Button size="sm" icon={Plug} onClick={connect} loading={connectingAdhoc}>
              Connect
            </Button>
          </div>
        </Section>

        <Section title="This machine">
          {!local.allowed && <p className={styles.note}>{local.reason ?? 'Not available from here.'}</p>}
          <LocalRow
            icon={TerminalSquare}
            title="Local shell"
            detail="The shell of the machine Nocturne runs on"
            disabled={!local.allowed}
            reason={local.reason}
            onOpen={() => onOpenAdhoc({ transport: 'local' }, 'Local shell')}
          />
          {local.ports.map((port) => (
            <LocalRow
              key={port.name}
              icon={Cable}
              title={port.name}
              detail={port.description || port.manufacturer || 'Serial port'}
              disabled={!local.allowed}
              reason={local.reason}
              onOpen={() => onOpenAdhoc({ transport: 'serial', host: port.name }, port.name)}
            />
          ))}
          {local.allowed && local.ports.length === 0 && (
            <p className={styles.note}>No serial ports on this machine.</p>
          )}
        </Section>
      </div>
    </div>
  );
}

interface LocalRowProps {
  icon: typeof MonitorCog;
  title: string;
  detail: string;
  disabled: boolean;
  reason: string | null;
  onOpen: () => void;
}

function LocalRow({ icon, title, detail, disabled, reason, onOpen }: LocalRowProps) {
  const row = (
    <button
      type="button"
      className={styles.row}
      data-disabled={disabled || undefined}
      aria-disabled={disabled || undefined}
      onClick={() => {
        if (!disabled) onOpen();
      }}
    >
      <Icon icon={icon} size={14} />
      <span className={styles.rowText}>
        <span className={styles.rowTitle}>{title}</span>
        <span className={styles.rowDetail}>{detail}</span>
      </span>
    </button>
  );
  if (!disabled) return row;
  return (
    <Tooltip content={reason ?? 'Not available from here.'}>
      <span>{row}</span>
    </Tooltip>
  );
}
