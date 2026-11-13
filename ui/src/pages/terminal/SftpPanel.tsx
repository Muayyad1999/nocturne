/**
 * Files on the device, next to its session. SSH only, because that is where
 * SFTP lives.
 *
 * Transfers are jobs: `sftp.download` and `sftp.upload` answer with a job id
 * and the bytes arrive as `jobs.changed` events, which is what the progress
 * rows show.
 */

import { ChevronUp, File, Folder, RefreshCw, Upload, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useEvent, useRpc, useRpcMutation } from '@/api/hooks';
import type {
  JobStarted,
  SftpClosedEvent,
  SftpEntry,
  SftpListing,
  SftpSessionInfo,
  SftpTransferParams,
} from '@/api/types/terminal';
import { Button, IconButton } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { ProgressBar } from '@/design/ProgressBar';
import { Skeleton } from '@/design/Skeleton';
import { formatBytes } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import styles from './SftpPanel.module.css';

interface Transfer {
  id: string;
  title: string;
  done: number;
  total: number;
  state: 'running' | 'succeeded' | 'failed' | 'cancelled';
  error: string | null;
}

/** The job summary `jobs.changed` carries, as much of it as this panel reads. */
interface JobEvent {
  id: string;
  kind: string;
  title: string;
  state: Transfer['state'];
  done: number;
  total: number;
  error?: string | null;
}

export interface SftpPanelProps {
  deviceId: string;
  deviceName: string;
  onClose: () => void;
}

export function SftpPanel({ deviceId, deviceName, onClose }: SftpPanelProps) {
  const [session, setSession] = useState<SftpSessionInfo | null>(null);
  const [path, setPath] = useState('');
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [download, setDownload] = useState<SftpEntry | null>(null);
  const [uploading, setUploading] = useState(false);

  const open = useRpcMutation<SftpSessionInfo, { device_id: string }>('sftp.open', { errorToast: false });
  const close = useRpcMutation<unknown, { sftp_id: string }>('sftp.close', { errorToast: false });
  const start = useRpcMutation<JobStarted, SftpTransferParams>('sftp.download');
  const send = useRpcMutation<JobStarted, SftpTransferParams>('sftp.upload');

  // One SFTP session per device while the panel is open.
  useEffect(() => {
    let cancelled = false;
    let id: string | null = null;
    open.mutate(
      { device_id: deviceId },
      {
        onSuccess: (info) => {
          if (cancelled) {
            close.mutate({ sftp_id: info.sftp_id });
            return;
          }
          id = info.sftp_id;
          setSession(info);
          setPath(info.home);
        },
      },
    );
    return () => {
      cancelled = true;
      setSession(null);
      if (id) close.mutate({ sftp_id: id });
    };
    // Re-opened only when the device changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceId]);

  useEvent<SftpClosedEvent>('sftp.closed', (payload) => {
    if (payload.sftp_id === session?.sftp_id) setSession(null);
  });

  useEvent<JobEvent>('jobs.changed', (job) => {
    if (job.kind !== 'sftp.download' && job.kind !== 'sftp.upload') return;
    setTransfers((current) => {
      const next: Transfer = {
        id: job.id,
        title: job.title,
        done: job.done,
        total: job.total,
        state: job.state,
        error: job.error ?? null,
      };
      const index = current.findIndex((transfer) => transfer.id === job.id);
      if (index === -1) return [next, ...current].slice(0, 6);
      const copy = [...current];
      copy[index] = next;
      return copy;
    });
  });

  const listing = useRpc<SftpListing>(
    'sftp.list',
    { sftp_id: session?.sftp_id ?? '', path },
    { enabled: Boolean(session) },
  );

  const parent = path.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';

  return (
    <aside className={styles.panel} aria-label={`Files on ${deviceName}`}>
      <header className={styles.header}>
        <div className={styles.heading}>
          <h2 className={styles.title}>Files on {deviceName}</h2>
          <p className={styles.peer}>{session?.peer ?? 'Opening…'}</p>
        </div>
        <IconButton
          icon={Upload}
          size="sm"
          aria-label="Upload a file to this directory"
          tooltip="Upload…"
          disabled={!session}
          onClick={() => setUploading(true)}
        />
        <IconButton
          icon={RefreshCw}
          size="sm"
          aria-label="Refresh this directory"
          disabled={!session}
          onClick={() => void listing.refetch()}
        />
        <IconButton icon={X} size="sm" aria-label="Close the file panel" onClick={onClose} />
      </header>

      {open.isError ? (
        <div className={styles.state}>
          <ErrorState
            size="compact"
            title="Files could not be opened"
            error={open.error}
            onRetry={() => open.mutate({ device_id: deviceId })}
          />
        </div>
      ) : (
        <>
          <div className={styles.path} title={path}>
            <IconButton
              icon={ChevronUp}
              size="sm"
              aria-label="Up one directory"
              disabled={!session || path === '/' || path === ''}
              onClick={() => setPath(parent)}
            />
            <code className={styles.pathText}>{path || '…'}</code>
          </div>

          <div className={styles.entries}>
            {listing.isError ? (
              <ErrorState
                size="compact"
                title="This directory could not be read"
                error={listing.error}
                onRetry={() => void listing.refetch()}
              />
            ) : listing.isPending ? (
              <>
                <Skeleton height={24} />
                <Skeleton height={24} />
                <Skeleton height={24} />
              </>
            ) : listing.data.entries.length === 0 ? (
              <p className={styles.note}>This directory is empty.</p>
            ) : (
              listing.data.entries.map((entry) => (
                <button
                  key={entry.path}
                  type="button"
                  className={styles.entry}
                  onClick={() => (entry.is_dir ? setPath(entry.path) : setDownload(entry))}
                  title={entry.path}
                >
                  <Icon icon={entry.is_dir ? Folder : File} size={14} />
                  <span className={styles.entryName}>{entry.name}</span>
                  <span className={styles.entrySize}>{entry.is_dir ? '' : formatBytes(entry.size)}</span>
                  <span className={styles.entryTime}>
                    {entry.modified ? formatDateTime(entry.modified * 1000) : ''}
                  </span>
                </button>
              ))
            )}
          </div>
        </>
      )}

      {transfers.length > 0 && (
        <div className={styles.transfers}>
          {transfers.map((transfer) => (
            <div key={transfer.id} className={styles.transfer}>
              <span className={styles.transferTitle}>{transfer.title}</span>
              <ProgressBar
                aria-label={transfer.title}
                value={transfer.total > 0 ? transfer.done : undefined}
                max={transfer.total || 1}
                tone={transfer.state === 'failed' ? 'crit' : transfer.state === 'succeeded' ? 'ok' : 'accent'}
                size="sm"
                valueText={`${formatBytes(transfer.done)} of ${transfer.total > 0 ? formatBytes(transfer.total) : 'unknown'}`}
              />
              <span className={styles.transferDetail}>
                {transfer.state === 'failed'
                  ? (transfer.error ?? 'failed')
                  : transfer.state === 'succeeded'
                    ? `${formatBytes(transfer.done)} copied`
                    : `${formatBytes(transfer.done)}${transfer.total > 0 ? ` of ${formatBytes(transfer.total)}` : ''}`}
              </span>
            </div>
          ))}
        </div>
      )}

      <TransferDialog
        open={download !== null}
        title={`Download ${download?.name ?? ''}`}
        description="Where to put it on the machine Nocturne runs on. An absolute path."
        label="Local path"
        initial={download?.name ?? ''}
        remote={download?.path ?? ''}
        confirmLabel="Download"
        pending={start.isPending}
        error={start.error?.message ?? null}
        onCancel={() => setDownload(null)}
        onConfirm={(local) => {
          if (!session || !download) return;
          start.mutate(
            { sftp_id: session.sftp_id, remote: download.path, local },
            { onSuccess: () => setDownload(null) },
          );
        }}
      />

      <TransferDialog
        open={uploading}
        title="Upload a file"
        description={`It is copied into ${path} on ${deviceName}.`}
        label="Local path"
        initial=""
        remote={path}
        confirmLabel="Upload"
        pending={send.isPending}
        error={send.error?.message ?? null}
        onCancel={() => setUploading(false)}
        onConfirm={(local) => {
          if (!session) return;
          const name = local.split('/').pop() ?? local;
          send.mutate(
            { sftp_id: session.sftp_id, remote: `${path.replace(/\/$/, '')}/${name}`, local },
            { onSuccess: () => setUploading(false) },
          );
        }}
      />
    </aside>
  );
}

interface TransferDialogProps {
  open: boolean;
  title: string;
  description: string;
  label: string;
  initial: string;
  remote: string;
  confirmLabel: string;
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: (local: string) => void;
}

function TransferDialog({
  open,
  title,
  description,
  label,
  initial,
  remote,
  confirmLabel,
  pending,
  error,
  onCancel,
  onConfirm,
}: TransferDialogProps) {
  const [local, setLocal] = useState(initial);
  useEffect(() => {
    if (open) setLocal(initial);
  }, [open, initial]);

  if (!open) return null;
  return (
    <Dialog
      open
      onClose={onCancel}
      title={title}
      description={description}
      dismissible={!pending}
      footer={
        <>
          <Button onClick={onCancel} disabled={pending}>
            Cancel
          </Button>
          <Button variant="primary" loading={pending} onClick={() => onConfirm(local)}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <Field label={label} hint={`Remote: ${remote}`} error={error ?? undefined}>
        <Input data-autofocus mono value={local} onChange={(event) => setLocal(event.target.value)} />
      </Field>
    </Dialog>
  );
}
