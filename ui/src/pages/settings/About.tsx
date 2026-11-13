/**
 * About — which Nocturne this is, where it keeps its data, and what it is
 * made of.
 *
 * The data directory is the one thing a person needs when they want to back
 * Nocturne up or move it, so it is here and it is copyable.
 */

import { ExternalLink } from 'lucide-react';
import { useRpc } from '@/api/hooks';
import type { AppInfo } from '@/api/types/core';
import { useConnection } from '@/api/connection';
import { Badge } from '@/design/Badge';
import { CodeBlock } from '@/design/CodeBlock';
import { ErrorState } from '@/design/ErrorState';
import { KeyValue } from '@/design/KeyValue';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount } from '@/lib/format';
import { Group } from './parts';
import styles from './Settings.module.css';

const TRANSPORT_WORD: Record<string, string> = {
  tauri: 'the desktop shell',
  http: 'HTTP and a WebSocket',
  mock: 'the development mock',
};

const CONNECTION_WORD: Record<string, string> = {
  connecting: 'Connecting…',
  online: 'Online',
  offline: 'Offline',
};

const LICENCES =
  'Nocturne is built on open source. The full list of components and their licences ships with the application as THIRD-PARTY-NOTICES.md, next to the binary.';

export function About() {
  const info = useRpc<AppInfo>('app.info');
  const methods = useRpc<string[]>('app.methods');
  const transport = useConnection((connection) => connection.transport);
  const state = useConnection((connection) => connection.state);

  if (info.isError) {
    return (
      <ErrorState
        title="The engine did not say what it is"
        error={info.error}
        onRetry={() => void info.refetch()}
      />
    );
  }
  if (!info.data) return <Skeleton height={220} shape="block" />;

  const data = info.data;
  /** The feature modules this build has, worked out from the methods it answers. */
  const modules = [...new Set((methods.data ?? []).map((method) => method.split('.')[0] ?? ''))]
    .filter(Boolean)
    .sort();

  return (
    <div className={styles.stack}>
      <Group title="This installation">
        <KeyValue
          items={[
            { label: 'Version', value: `${data.name} ${data.version}`, mono: true },
            { label: 'Running on', value: `${data.os} · ${data.arch}`, mono: true },
            { label: 'Started', value: <TimeAgo at={data.started_at} /> },
            {
              label: 'Connected over',
              value: `${TRANSPORT_WORD[transport ?? 'http']} · ${CONNECTION_WORD[state]}`,
            },
            { label: 'RPC methods', value: formatCount(data.methods) },
          ]}
        />
      </Group>

      <Group
        title="Where the data lives"
        description="The database, the vault key file and the logs are all in one directory: copy it and you have copied Nocturne."
      >
        {data.data_dir ? (
          <CodeBlock code={data.data_dir} copyable />
        ) : (
          <p className={styles.note}>
            This engine is running in memory, with no data directory. Nothing is kept when it stops.
          </p>
        )}
      </Group>

      <Group
        title="What this build contains"
        description="Nocturne is assembled from feature modules. Settings only offers the sections whose module is here."
      >
        <div className={styles.kinds}>
          {modules.length === 0 ? (
            <Skeleton width={320} height={14} />
          ) : (
            modules.map((module) => (
              <Badge key={module} variant="outline">
                {module}
              </Badge>
            ))
          )}
        </div>
      </Group>

      <Group title="Licences">
        <p className={styles.note}>
          <ExternalLink size={12} /> {LICENCES}
        </p>
      </Group>
    </div>
  );
}
