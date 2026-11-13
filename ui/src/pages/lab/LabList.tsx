/**
 * The labs: every simulated network this installation holds, running or not.
 *
 * `labs.list` is the only source — a lab survives a restart as a stored
 * specification, and whether it is *running* is a fact about the backend's
 * memory, so both are read from there and never remembered here.
 */

import { CircleStop, Copy, FlaskConical, Play, Plus, Trash } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { LabSummary, LabView } from '@/api/types/lab';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { NewLabDialog } from './NewLabDialog';
import { TwinDrawer } from './TwinDrawer';
import styles from './LabList.module.css';

export default function LabList() {
  const navigate = useNavigate();
  const labs = useRpc<LabSummary[]>('labs.list', undefined, { invalidateOn: ['labs.changed'] });
  const [creating, setCreating] = useState(false);
  const [twinning, setTwinning] = useState(false);
  const [acting, setActing] = useState<string | null>(null);

  const create = useRpcMutation<LabView, { name: string; example?: string }>('labs.create', {
    invalidates: ['labs.'],
    errorToast: 'The lab was not created',
  });
  const start = useRpcMutation<LabView, { id: string }>('labs.start', {
    invalidates: ['labs.'],
    errorToast: 'The lab did not start',
  });
  const stop = useRpcMutation<unknown, { id: string }>('labs.stop', {
    invalidates: ['labs.'],
    errorToast: 'The lab did not stop',
  });
  const remove = useRpcMutation<unknown, { id: string }>('labs.delete', {
    invalidates: ['labs.'],
    errorToast: 'The lab was not deleted',
  });

  usePageHeader({ title: 'Lab' });

  useEffect(
    () =>
      registerCommand({
        id: 'lab.new',
        title: 'New lab',
        group: 'Lab',
        keywords: ['simulate', 'topology', 'packet'],
        run: () => setCreating(true),
      }),
    [],
  );
  useEffect(
    () =>
      registerCommand({
        id: 'lab.twin',
        title: 'Build a lab from the real network',
        group: 'Lab',
        keywords: ['twin', 'rehearse', 'copy', 'digital twin'],
        run: () => setTwinning(true),
      }),
    [],
  );

  const deleteLab = async (lab: LabSummary) => {
    const ok = await confirm({
      title: `Delete ${lab.name}?`,
      description: `The lab, its ${lab.nodes} simulated devices and everything collected from them are removed. This cannot be undone.`,
      confirmLabel: 'Delete the lab',
      tone: 'danger',
      confirmText: lab.name,
    });
    if (!ok) return;
    setActing(lab.id);
    remove.mutate({ id: lab.id }, { onSettled: () => setActing(null) });
  };

  const rows = labs.data ?? [];

  return (
    <Page>
      <Toolbar>
        <p className={styles.lead}>
          Networks simulated inside Nocturne. Every device in one is also an inventory device, so
          the terminal, the collectors and the change pipeline work on it unchanged.
        </p>
        <ToolbarSpacer />
        <Button icon={Copy} onClick={() => setTwinning(true)}>
          Twin the real network…
        </Button>
        <Button variant="primary" icon={Plus} onClick={() => setCreating(true)}>
          New lab
        </Button>
      </Toolbar>

      {labs.isPending && (
        <div className={styles.grid} aria-busy="true" aria-label="Loading labs">
          {Array.from({ length: 3 }, (_value, index) => (
            <Skeleton key={index} height={116} shape="block" />
          ))}
        </div>
      )}

      {labs.error && (
        <ErrorState
          title="The labs could not be read"
          error={labs.error}
          onRetry={() => void labs.refetch()}
        />
      )}

      {labs.data && rows.length === 0 && (
        <EmptyState
          icon={FlaskConical}
          title="No labs yet"
          description="A lab is a network you can build, break and rehearse a change on — simulated in this process, never deployed anywhere. Start from the demo campus, or from an empty canvas."
          action={
            <Button variant="primary" icon={Plus} onClick={() => setCreating(true)}>
              New lab
            </Button>
          }
        />
      )}

      {rows.length > 0 && (
        <ul className={styles.grid}>
          {rows.map((lab) => (
            <li key={lab.id}>
              <Card className={styles.card} padding="none">
                <Link to={`/lab/${lab.id}`} className={styles.body}>
                  <span className={styles.head}>
                    <span className={styles.name}>{lab.name}</span>
                    <StatusPill tone={lab.running ? 'ok' : 'idle'} icon={lab.running ? Play : CircleStop}>
                      {lab.running ? 'Running' : 'Stopped'}
                    </StatusPill>
                  </span>
                  {lab.description && <span className={styles.description}>{lab.description}</span>}
                  <span className={styles.meta}>
                    {lab.nodes} device{lab.nodes === 1 ? '' : 's'} · {lab.links} cable
                    {lab.links === 1 ? '' : 's'} · changed <TimeAgo at={lab.updated_at} />
                  </span>
                </Link>
                <div className={styles.actions}>
                  {lab.running ? (
                    <Button
                      size="sm"
                      icon={CircleStop}
                      loading={stop.isPending && acting === lab.id}
                      onClick={() => {
                        setActing(lab.id);
                        stop.mutate({ id: lab.id }, { onSettled: () => setActing(null) });
                      }}
                    >
                      Stop
                    </Button>
                  ) : (
                    <Button
                      size="sm"
                      icon={Play}
                      loading={start.isPending && acting === lab.id}
                      onClick={() => {
                        setActing(lab.id);
                        start.mutate({ id: lab.id }, { onSettled: () => setActing(null) });
                      }}
                    >
                      Start
                    </Button>
                  )}
                  <Button size="sm" onClick={() => void navigate(`/lab/${lab.id}`)}>
                    Open
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Trash}
                    aria-label={`Delete ${lab.name}`}
                    onClick={() => void deleteLab(lab)}
                  />
                </div>
              </Card>
            </li>
          ))}
        </ul>
      )}

      <NewLabDialog
        open={creating}
        onClose={() => setCreating(false)}
        busy={create.isPending}
        onCreate={(params) =>
          create.mutate(params, {
            onSuccess: (lab) => {
              setCreating(false);
              void navigate(`/lab/${lab.id}`);
            },
          })
        }
      />
      <TwinDrawer open={twinning} onClose={() => setTwinning(false)} />
    </Page>
  );
}
