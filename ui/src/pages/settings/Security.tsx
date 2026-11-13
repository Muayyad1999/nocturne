/**
 * Security — the credential vault, and the SSH host keys Nocturne trusts.
 *
 * Both are things a person has to be able to check: whether the secrets on
 * this machine are protected by a password, and exactly which keys have been
 * accepted for which hosts.
 */

import { KeyRound, Lock, LockOpen, ShieldCheck, Trash } from 'lucide-react';
import { useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  ConnectionSettings,
  HostKeyForgetResult,
  HostKeyRef,
  KnownHost,
  VaultSetPasswordParams,
  VaultStatus,
} from '@/api/types/core';
import { Button } from '@/design/Button';
import { confirm, Dialog } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { HOST_KEY_POLICIES } from './policies';
import { Group, Setting, useSettingsSection } from './parts';
import styles from './Settings.module.css';

const VAULT_STATE: Record<string, { tone: 'ok' | 'warn' | 'crit'; label: string; text: string }> = {
  open: {
    tone: 'warn',
    label: 'No master password',
    text: 'Secrets are encrypted with a key file next to the database. Anyone who can read this computer’s disk can read them.',
  },
  unlocked: {
    tone: 'ok',
    label: 'Unlocked',
    text: 'A master password protects the key file. It is unlocked for this session.',
  },
  locked: {
    tone: 'crit',
    label: 'Locked',
    text: 'Nothing that needs a credential can run until the vault is unlocked.',
  },
};

const COLUMNS: TableColumn<KnownHost>[] = [
  {
    id: 'host',
    header: 'Host',
    required: true,
    mono: true,
    sortValue: (host) => host.host,
    cell: (host) => (host.port === 22 ? host.host : `${host.host}:${host.port}`),
  },
  {
    id: 'algorithm',
    header: 'Algorithm',
    width: 150,
    mono: true,
    sortValue: (host) => host.algorithm,
    cell: (host) => host.algorithm,
  },
  {
    id: 'fingerprint',
    header: 'Fingerprint',
    width: 290,
    mono: true,
    sortValue: (host) => host.fingerprint,
    cell: (host) => host.fingerprint,
  },
  {
    id: 'first_seen',
    header: 'Trusted',
    width: 100,
    align: 'right',
    defaultHidden: true,
    sortValue: (host) => host.first_seen,
    cell: (host) => <TimeAgo at={host.first_seen} />,
  },
  {
    id: 'last_seen',
    header: 'Last seen',
    width: 118,
    align: 'right',
    sortValue: (host) => host.last_seen,
    cell: (host) => <TimeAgo at={host.last_seen} />,
  },
];

export function Security() {
  const [changing, setChanging] = useState(false);

  const vault = useRpc<VaultStatus>('vault.state', undefined, { invalidateOn: ['vault.changed'] });
  const hosts = useRpc<KnownHost[]>('hostkeys.list', undefined, {
    invalidateOn: ['connect.host_key_changed', 'connect.host_key_unknown'],
  });
  const connections = useSettingsSection<ConnectionSettings>('connections');

  const lock = useRpcMutation<VaultStatus, undefined>('vault.lock', {
    invalidates: ['vault.'],
    errorToast: 'The vault was not locked',
  });
  const forget = useRpcMutation<HostKeyForgetResult, HostKeyRef>('hostkeys.forget', {
    invalidates: ['hostkeys.'],
    errorToast: 'The host key was not forgotten',
  });

  const state = vault.data?.state;
  const describe = state ? VAULT_STATE[state] : undefined;
  const policy = connections.value?.host_key_policy;

  const onForget = async (host: KnownHost) => {
    const confirmed = await confirm({
      title: `Forget the host key of ${host.host}?`,
      description:
        'The next connection to it will be treated as a first contact: under the “trust on first use” policy the key it presents then is accepted and remembered, so only do this when you know the device was replaced or re-keyed.',
      confirmLabel: 'Forget it',
      tone: 'danger',
    });
    if (!confirmed) return;
    await forget.mutateAsync({ host: host.host, port: host.port });
    toast.success(`Forgot the host key of ${host.host}`);
  };

  return (
    <div className={styles.stack}>
      <Group
        title="Credential vault"
        description="Every password, enable secret and private key is encrypted before it is stored, and is never sent back to this interface."
        actions={
          <>
            <Button icon={KeyRound} onClick={() => setChanging(true)}>
              {state === 'open' ? 'Set a master password' : 'Change master password'}
            </Button>
            {state === 'unlocked' && (
              <Button icon={Lock} loading={lock.isPending} onClick={() => lock.mutate(undefined)}>
                Lock now
              </Button>
            )}
          </>
        }
      >
        {vault.isError ? (
          <ErrorState
            size="compact"
            title="The vault state could not be read"
            error={vault.error}
            onRetry={() => void vault.refetch()}
          />
        ) : !describe ? (
          <Skeleton height={56} shape="block" />
        ) : (
          <div className={styles.row}>
            <StatusPill tone={describe.tone} icon={state === 'locked' ? Lock : LockOpen}>
              {describe.label}
            </StatusPill>
            <span className={styles.muted}>{describe.text}</span>
          </div>
        )}
      </Group>

      <Group
        title="Host keys"
        description="How Nocturne decides whether the device at the other end of an SSH connection is the one it has spoken to before."
      >
        <Setting
          label="When a host key is not known"
          help={HOST_KEY_POLICIES.find((option) => option.value === policy)?.description}
          saved={connections.justSaved === 'host_key_policy'}
          error={
            connections.saveError?.key === 'host_key_policy'
              ? connections.saveError.message
              : undefined
          }
        >
          <Select
            aria-label="When a host key is not known"
            disabled={connections.value === undefined}
            value={policy ?? 'tofu'}
            options={HOST_KEY_POLICIES.map(({ value, label }) => ({ value, label }))}
            onChange={async (event) => {
              const next = event.target.value;
              if (next === 'accept') {
                const confirmed = await confirm({
                  title: 'Turn host key checking off?',
                  description:
                    'Nocturne will connect to anything that answers at the address, and will not notice if a device is impersonated or a connection is intercepted. This is for a lab, not for a network you care about.',
                  confirmLabel: 'Turn checking off',
                  tone: 'danger',
                  confirmText: 'accept',
                });
                if (!confirmed) return;
              }
              connections.set({ host_key_policy: next });
            }}
          />
        </Setting>

        {policy === 'accept' && (
          <p className={styles.note} data-tone="crit">
            Host key checking is off. Nothing verifies that the device answering is the one you
            think it is.
          </p>
        )}
      </Group>

      <Group
        title="Known hosts"
        description="Keys Nocturne has accepted. A device whose key changes is refused until you say the change was expected."
      >
        <Table
          className={styles.table}
          id="known-hosts"
          aria-label="Known SSH host keys"
          columns={COLUMNS}
          rows={hosts.data ?? []}
          rowKey={(host) => `${host.host}:${host.port}`}
          loading={hosts.isPending}
          maxHeight={360}
          defaultSort={{ columnId: 'host', direction: 'asc' }}
          rowActions={(host) => [
            {
              label: 'Forget this key…',
              icon: Trash,
              tone: 'danger',
              onSelect: () => void onForget(host),
            },
          ]}
          empty={
            hosts.isError ? (
              <ErrorState
                size="compact"
                title="The known hosts could not be read"
                error={hosts.error}
                onRetry={() => void hosts.refetch()}
              />
            ) : (
              <EmptyState
                size="compact"
                icon={ShieldCheck}
                title="No host key yet"
                description="The first time Nocturne reaches a device over SSH, the key it presents is recorded here."
              />
            )
          }
        />
      </Group>

      <MasterPassword open={changing} onClose={() => setChanging(false)} state={state} />
    </div>
  );
}

/** Setting, changing and removing the master password. */
function MasterPassword({
  open,
  onClose,
  state,
}: {
  open: boolean;
  onClose: () => void;
  state: string | undefined;
}) {
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [again, setAgain] = useState('');
  const [mode, setMode] = useState<'set' | 'remove'>('set');

  const save = useRpcMutation<VaultStatus, VaultSetPasswordParams>('vault.set_password', {
    invalidates: ['vault.'],
    errorToast: false,
  });

  const existing = state !== 'open';
  const mismatch = mode === 'set' && again !== '' && next !== again;
  const tooShort = mode === 'set' && next !== '' && [...next].length < 8;

  const close = () => {
    setCurrent('');
    setNext('');
    setAgain('');
    setMode('set');
    save.reset();
    onClose();
  };

  const submit = () => {
    save.mutate(
      {
        ...(existing ? { current } : {}),
        ...(mode === 'remove' ? { new: '' } : { new: next }),
      },
      {
        onSuccess: () => {
          toast.success(
            mode === 'remove' ? 'The master password was removed' : 'The master password was saved',
          );
          close();
        },
      },
    );
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      title={existing ? 'Change the master password' : 'Set a master password'}
      description={
        existing
          ? 'Nocturne asks for it once per session before any credential can be used.'
          : 'It encrypts the key that protects every stored secret. Without it, anyone who can read this computer’s disk can read them.'
      }
      dismissible={!save.isPending}
      footer={
        <>
          <Button onClick={close} disabled={save.isPending}>
            Cancel
          </Button>
          {existing && mode === 'set' && (
            <Button
              variant="danger"
              disabled={save.isPending}
              onClick={() => {
                setMode('remove');
                setNext('');
                setAgain('');
              }}
            >
              Remove it instead
            </Button>
          )}
          <Button
            variant={mode === 'remove' ? 'danger' : 'primary'}
            loading={save.isPending}
            disabled={
              (existing && current === '') ||
              (mode === 'set' && (next === '' || mismatch || tooShort))
            }
            onClick={submit}
          >
            {mode === 'remove' ? 'Remove the password' : 'Save'}
          </Button>
        </>
      }
    >
      <div className={styles.stack}>
        {existing && (
          <Field label="Current master password" required error={save.error?.message}>
            <Input
              data-autofocus
              type="password"
              autoComplete="current-password"
              value={current}
              onChange={(event) => setCurrent(event.target.value)}
            />
          </Field>
        )}

        {mode === 'remove' ? (
          <p className={styles.note} data-tone="warn">
            The vault stays usable, but its key file will no longer be protected by a password.
          </p>
        ) : (
          <>
            <Field
              label="New master password"
              required
              hint="At least eight characters. There is no way to recover it."
              error={tooShort ? 'At least eight characters.' : (!existing ? save.error?.message : undefined)}
            >
              <Input
                data-autofocus={!existing || undefined}
                type="password"
                autoComplete="new-password"
                value={next}
                onChange={(event) => setNext(event.target.value)}
              />
            </Field>
            <Field
              label="Repeat it"
              required
              error={mismatch ? 'The two do not match.' : undefined}
            >
              <Input
                type="password"
                autoComplete="new-password"
                value={again}
                onChange={(event) => setAgain(event.target.value)}
              />
            </Field>
          </>
        )}
      </div>
    </Dialog>
  );
}
