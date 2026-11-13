/**
 * A twin: a lab built from the real network, so a change can be rehearsed on
 * a copy before it touches anything.
 *
 * The nodes come from what has been collected from each device, the cables
 * from its neighbour tables and the configuration from its latest backup —
 * **as far as the simulator accepts it**. That last part is why this screen
 * shows the accounting rather than a tick: a twin whose fidelity is not
 * stated is a twin nobody should trust. Every line the simulated device
 * refused is listed, and every device that could not be twinned says why in
 * the backend's own words.
 */

import { CircleCheck, Copy, TriangleExclamationPoint } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { DeviceRow, Job } from '@/api/types/core';
import type { TwinResult } from '@/api/types/lab';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { ProgressBar } from '@/design/ProgressBar';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import styles from './TwinDrawer.module.css';

export interface TwinDrawerProps {
  open: boolean;
  onClose: () => void;
}

export function TwinDrawer({ open, onClose }: TwinDrawerProps) {
  const navigate = useNavigate();
  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    enabled: open,
    invalidateOn: ['devices.changed'],
  });
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<ReadonlySet<string>>(new Set());
  const [name, setName] = useState('');
  const [jobId, setJobId] = useState<string | null>(null);

  const twin = useRpcMutation<{ job_id: string }, { device_ids: string[]; name?: string }>(
    'labs.twin',
    { errorToast: 'The twin was not built' },
  );
  const job = useRpc<Job>(
    'jobs.get',
    { id: jobId },
    { enabled: jobId !== null, invalidateOn: ['jobs.changed'] },
  );

  // A lab device is already simulated; the backend refuses to twin one, so it
  // is not offered.
  const real = useMemo(
    () => (devices.data ?? []).filter((device) => device.lab == null),
    [devices.data],
  );
  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === '') return real;
    return real.filter((device) =>
      [device.name, device.address, device.site, device.role].some((field) =>
        field?.toLowerCase().includes(needle),
      ),
    );
  }, [real, query]);

  const result = job.data?.state === 'succeeded' ? (job.data.result as TwinResult | null) : null;
  const running = job.data?.state === 'running';

  const close = () => {
    setJobId(null);
    setChosen(new Set());
    setQuery('');
    onClose();
  };

  return (
    <Drawer
      open={open}
      onClose={close}
      size="lg"
      title="Build a lab from the real network"
      description="A copy of devices you already manage: their interfaces, the cables their neighbour tables imply and their latest backed-up configuration, in the simulator."
      footer={
        result ? (
          <>
            <Button onClick={close}>Close</Button>
            <Button
              variant="primary"
              onClick={() => {
                close();
                void navigate(`/lab/${result.lab_id}`);
              }}
            >
              Open {result.name}
            </Button>
          </>
        ) : (
          <>
            <Button onClick={close}>Cancel</Button>
            <Button
              variant="primary"
              icon={Copy}
              loading={twin.isPending || running}
              disabled={chosen.size === 0}
              onClick={() =>
                twin.mutate(
                  {
                    device_ids: [...chosen],
                    ...(name.trim() === '' ? {} : { name: name.trim() }),
                  },
                  { onSuccess: (started) => setJobId(started.job_id) },
                )
              }
            >
              Build the twin
            </Button>
          </>
        )
      }
    >
      {jobId === null ? (
        <>
          <p className={styles.caveat}>
            A device can only be twinned once Nocturne has collected its interfaces and taken a
            configuration backup. Anything the simulator will not accept is reported line by line
            when the twin is built, so you know exactly how faithful the copy is.
          </p>

          <Field label="Name" hint="A unique name; “Twin” by default.">
            <Input value={name} placeholder="Twin" onChange={(event) => setName(event.target.value)} />
          </Field>

          <div className={styles.pickerHead}>
            <SearchInput value={query} onChange={setQuery} aria-label="Search devices" />
            <span className={styles.count}>
              {chosen.size} of {real.length} selected
            </span>
          </div>

          {devices.isPending && <Skeleton height={200} shape="block" />}
          {devices.error && (
            <ErrorState
              title="The inventory could not be read"
              error={devices.error}
              onRetry={() => void devices.refetch()}
              size="compact"
            />
          )}
          {devices.data && real.length === 0 && (
            <EmptyState
              icon={Copy}
              size="compact"
              title="There is nothing to twin yet"
              description="Every device in this inventory is already a simulated one. Add a real device and collect from it, then its twin can be built here."
            />
          )}
          {shown.length > 0 && (
            <ul className={styles.devices}>
              {shown.map((device) => (
                <li key={device.id} className={styles.device}>
                  <Checkbox
                    checked={chosen.has(device.id)}
                    onChange={(event) => {
                      const next = new Set(chosen);
                      if (event.target.checked) next.add(device.id);
                      else next.delete(device.id);
                      setChosen(next);
                    }}
                  >
                    <span className={styles.deviceName}>{device.name}</span>
                    <span className={styles.deviceMeta}>
                      {device.address || 'no address'} · {device.profile || 'type not set'}
                      {device.site ? ` · ${device.site}` : ''}
                    </span>
                  </Checkbox>
                </li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <TwinProgress job={job.data} result={result} />
      )}
    </Drawer>
  );
}

function TwinProgress({ job, result }: { job: Job | undefined; result: TwinResult | null }) {
  if (!job) return <Skeleton height={120} shape="block" />;

  if (job.state === 'failed') {
    return (
      <>
        <ErrorState title="No twin was built" error={job.error ?? 'the job failed'} size="compact" />
        <h3 className={styles.heading}>What the engine found</h3>
        <ul className={styles.log}>
          {job.log.map((line, index) => (
            <li key={index} data-level={line.level}>
              {line.message}
            </li>
          ))}
        </ul>
      </>
    );
  }

  if (!result) {
    return (
      <div className={styles.progress}>
        <ProgressBar
          value={job.total > 0 ? job.done : undefined}
          max={Math.max(1, job.total)}
          aria-label="Building the twin"
          valueText={job.message}
        />
        <p className={styles.message}>{job.message}</p>
        <ul className={styles.log}>
          {job.log.map((line, index) => (
            <li key={index} data-level={line.level}>
              {line.message}
            </li>
          ))}
        </ul>
      </div>
    );
  }

  const perfect = result.nodes.every((node) => node.rejected.length === 0 && node.interfaces_unmapped.length === 0);

  return (
    <div className={styles.result}>
      <header className={styles.verdict} data-ok={perfect || undefined}>
        <Icon icon={perfect ? CircleCheck : TriangleExclamationPoint} size={16} />
        <strong>
          {result.name}: {result.nodes.length} device{result.nodes.length === 1 ? '' : 's'},{' '}
          {result.links.length} cable{result.links.length === 1 ? '' : 's'}
        </strong>
      </header>

      {result.nodes.map((node) => (
        <section key={node.device_id} className={styles.node}>
          <header className={styles.nodeHead}>
            <span className={styles.nodeName}>
              {node.device} → {node.node}
            </span>
            <Badge tone={node.rejected.length === 0 ? 'ok' : 'warn'}>
              {node.accepted} of {node.config_lines} lines
            </Badge>
            <Badge>{node.model}</Badge>
            <Badge tone="neutral" title="Which backup the configuration came from">
              {node.config_source}-config
            </Badge>
          </header>

          {node.interfaces_unmapped.length > 0 && (
            <p className={styles.unmapped}>
              No simulated model has {node.interfaces_unmapped.join(', ')}; anything configured on
              those interfaces is not in the twin.
            </p>
          )}

          {node.rejected.length > 0 && (
            <details className={styles.rejected}>
              <summary>
                {node.rejected.length} line{node.rejected.length === 1 ? '' : 's'} the simulator
                would not take
              </summary>
              <ul>
                {node.rejected.map((line, index) => (
                  <li key={index}>
                    <code>{line.line}</code>
                    <span>{line.error}</span>
                  </li>
                ))}
              </ul>
            </details>
          )}
        </section>
      ))}

      {result.refused.length > 0 && (
        <section className={styles.node}>
          <h3 className={styles.heading}>Not twinned</h3>
          <ul className={styles.refused}>
            {result.refused.map((refusal) => (
              <li key={refusal.device_id}>
                <strong>{refusal.device}</strong> — {refusal.reason}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
