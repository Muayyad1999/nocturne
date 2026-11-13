/**
 * What the transport actually negotiated, on hover (and on focus, for the
 * keyboard). Everything here is read from `SessionInfo`; a field the backend
 * left empty is simply not shown, never guessed.
 */

import { Info } from 'lucide-react';
import type { SessionInfo } from '@/api/types/terminal';
import { IconButton } from '@/design/Button';
import { Tooltip } from '@/design/Tooltip';
import { formatBytes } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import styles from './TransportDetails.module.css';

const KIND_LABEL: Record<SessionInfo['kind'], string> = {
  ssh: 'SSH',
  telnet: 'Telnet',
  serial: 'Serial',
  local: 'Local shell',
  sim: 'Simulated console',
};

function Row({ label, value, mono = true }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className={styles.row}>
      <dt className={styles.key}>{label}</dt>
      <dd className={styles.value} data-mono={mono || undefined}>
        {value}
      </dd>
    </div>
  );
}

export function TransportDetails({ info }: { info: SessionInfo }) {
  const { info: connection } = info;

  const card = (
    <div className={styles.card}>
      <p className={styles.heading}>{KIND_LABEL[info.kind]}</p>
      <dl className={styles.list}>
        <Row label="Peer" value={connection.peer} />
        {connection.via.length > 0 && <Row label="Via" value={connection.via.join(' → ')} />}
        {connection.server_banner && <Row label="Banner" value={connection.server_banner} />}
        {connection.kex && <Row label="Key exchange" value={connection.kex} />}
        {connection.cipher && <Row label="Cipher" value={connection.cipher} />}
        {connection.mac && <Row label="MAC" value={connection.mac} />}
        {connection.host_key_algorithm && <Row label="Host key" value={connection.host_key_algorithm} />}
        {connection.host_key_fingerprint && <Row label="Fingerprint" value={connection.host_key_fingerprint} />}
        <Row label="Opened" value={formatTimestamp(info.opened_at)} mono={false} />
        <Row label="Size" value={`${info.cols} × ${info.rows}`} />
        <Row label="Traffic" value={`${formatBytes(info.bytes_in)} in · ${formatBytes(info.bytes_out)} out`} />
        {info.logging && <Row label="Log file" value={info.logging} />}
      </dl>
      {connection.legacy_crypto && (
        <p className={styles.warning}>
          Negotiated only thanks to the legacy algorithm set. This device speaks old crypto; treat the link as
          weaker than SSH normally is.
        </p>
      )}
      {info.log_error && <p className={styles.warning}>This session is not being logged: {info.log_error}</p>}
    </div>
  );

  return (
    <Tooltip content={card} placement="bottom-end">
      <IconButton
        icon={Info}
        size="sm"
        aria-label={`Connection details for ${info.title}`}
        tooltip={false}
      />
    </Tooltip>
  );
}
