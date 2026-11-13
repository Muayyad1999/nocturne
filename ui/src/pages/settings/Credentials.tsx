/**
 * Credentials — the logins devices are reached with.
 *
 * A stored secret is never sent back to this interface, so the editor cannot
 * show one and does not pretend to: each secret says **set** or **not set**,
 * and offers to replace or clear it. A field left alone is left alone.
 */

import { KeyRound, Pencil, Plus, Trash } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  Credential,
  CredentialInput,
  CredentialUpdateParams,
  DeletedResult,
} from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Group } from './parts';
import { SecretField } from './SecretField';
import styles from './Settings.module.css';

export function Credentials() {
  const [editing, setEditing] = useState<Credential | null>(null);
  const [adding, setAdding] = useState(false);

  const credentials = useRpc<Credential[]>('credentials.list', undefined, {
    invalidateOn: ['credentials.changed'],
  });
  const remove = useRpcMutation<DeletedResult, { id: string }>('credentials.delete', {
    invalidates: ['credentials.'],
    errorToast: false,
  });

  const onDelete = async (credential: Credential) => {
    const confirmed = await confirm({
      title: `Delete “${credential.name}”?`,
      description:
        'Its user name and every secret it holds are removed from the vault. Devices that still use it will have no login until another is assigned.',
      confirmLabel: 'Delete',
      tone: 'danger',
    });
    if (!confirmed) return;
    try {
      await remove.mutateAsync({ id: credential.id });
      toast.success(`Deleted “${credential.name}”`);
    } catch (error) {
      // The backend refuses while devices still use it, and names them.
      toast.error('That credential is still in use', {
        description: error instanceof Error ? error.message : undefined,
      });
    }
  };

  const columns: TableColumn<Credential>[] = [
    {
      id: 'name',
      header: 'Name',
      required: true,
      sortValue: (credential) => credential.name,
      cell: (credential) => credential.name,
    },
    {
      id: 'username',
      header: 'User name',
      width: 170,
      mono: true,
      sortValue: (credential) => credential.username,
      cell: (credential) => credential.username || '—',
    },
    {
      id: 'secrets',
      header: 'Holds',
      sortValue: (credential) => Number(credential.has_password) + Number(credential.has_private_key),
      cell: (credential) => {
        const held = [
          credential.has_password && 'password',
          credential.has_enable_secret && 'enable secret',
          credential.has_private_key && 'private key',
          credential.has_passphrase && 'key passphrase',
        ].filter(Boolean) as string[];
        if (held.length === 0) return <span className={styles.muted}>no secret</span>;
        return (
          <span className={styles.rowBadges}>
            {held.map((what) => (
              <Badge key={what} variant="outline">
                {what}
              </Badge>
            ))}
          </span>
        );
      },
    },
    {
      id: 'snmp',
      header: 'SNMP',
      width: 100,
      sortValue: (credential) => credential.snmp.version,
      cell: (credential) =>
        credential.snmp.version ? (
          <Badge tone="info" variant="outline">
            {credential.snmp.version}
          </Badge>
        ) : (
          <span className={styles.muted}>—</span>
        ),
    },
    {
      id: 'updated',
      header: 'Changed',
      width: 110,
      align: 'right',
      sortValue: (credential) => credential.updated_at,
      cell: (credential) => <TimeAgo at={credential.updated_at} />,
    },
  ];

  return (
    <div className={styles.stack}>
      <Group
        title="Stored logins"
        description="Secrets go into the vault when they are saved and are never read back out to this interface — not even by this editor."
        actions={
          <Button variant="primary" icon={Plus} onClick={() => setAdding(true)}>
            Add credential
          </Button>
        }
      >
        <Table
          className={styles.table}
          id="credentials"
          aria-label="Credentials"
          columns={columns}
          rows={credentials.data ?? []}
          rowKey={(credential) => credential.id}
          loading={credentials.isPending}
          defaultSort={{ columnId: 'name', direction: 'asc' }}
          onRowActivate={(credential) => setEditing(credential)}
          rowActions={(credential) => [
            { label: 'Edit…', icon: Pencil, onSelect: () => setEditing(credential) },
            { type: 'separator' },
            {
              label: 'Delete…',
              icon: Trash,
              tone: 'danger',
              onSelect: () => void onDelete(credential),
            },
          ]}
          empty={
            credentials.isError ? (
              <ErrorState
                size="compact"
                title="The credentials could not be read"
                error={credentials.error}
                onRetry={() => void credentials.refetch()}
              />
            ) : (
              <EmptyState
                size="compact"
                icon={KeyRound}
                title="No credentials yet"
                description="A credential is a name for one login: a user name and whatever a device asks for after it."
                action={
                  <Button variant="primary" size="sm" icon={Plus} onClick={() => setAdding(true)}>
                    Add credential
                  </Button>
                }
              />
            )
          }
        />
      </Group>

      <CredentialDrawer
        open={adding || editing !== null}
        credential={editing}
        onClose={() => {
          setAdding(false);
          setEditing(null);
        }}
      />
    </div>
  );
}

interface FormState {
  name: string;
  username: string;
  notes: string;
  snmp_version: string;
  snmp_user: string;
  snmp_auth_protocol: string;
  snmp_priv_protocol: string;
}

const BLANK: FormState = {
  name: '',
  username: '',
  notes: '',
  snmp_version: '',
  snmp_user: '',
  snmp_auth_protocol: '',
  snmp_priv_protocol: '',
};

function CredentialDrawer({
  open,
  credential,
  onClose,
}: {
  open: boolean;
  credential: Credential | null;
  onClose: () => void;
}) {
  const [form, setForm] = useState<FormState>(BLANK);
  /** Secret id → what to send: a new value, or `''` to clear it. */
  const [secrets, setSecrets] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!open) return;
    setSecrets({});
    setForm(
      credential
        ? {
            name: credential.name,
            username: credential.username,
            notes: credential.notes,
            snmp_version: credential.snmp.version,
            snmp_user: credential.snmp.user,
            snmp_auth_protocol: credential.snmp.auth_protocol,
            snmp_priv_protocol: credential.snmp.priv_protocol,
          }
        : BLANK,
    );
  }, [open, credential]);

  const create = useRpcMutation<Credential, CredentialInput>('credentials.create', {
    invalidates: ['credentials.'],
    errorToast: false,
  });
  const update = useRpcMutation<Credential, CredentialUpdateParams>('credentials.update', {
    invalidates: ['credentials.'],
    errorToast: false,
  });

  const saving = create.isPending || update.isPending;
  const error = create.error ?? update.error;
  const snmp = form.snmp_version;

  const save = () => {
    const input: CredentialInput = { ...form, ...secrets };
    const done = () => {
      toast.success(credential ? `Saved “${form.name}”` : `Added “${form.name}”`);
      onClose();
    };
    if (credential) update.mutate({ id: credential.id, ...input }, { onSuccess: done });
    else create.mutate(input, { onSuccess: done });
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="lg"
      title={credential ? `Edit “${credential.name}”` : 'Add credential'}
      description="A secret you do not touch is left exactly as it is."
      footer={
        <>
          <Button onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button variant="primary" loading={saving} onClick={save} disabled={form.name.trim() === ''}>
            {credential ? 'Save changes' : 'Add credential'}
          </Button>
        </>
      }
    >
      <div className={styles.stack}>
        <Field
          label="Name"
          required
          hint="What you will pick it by when adding a device."
          error={error?.code === 'conflict' || error?.message.includes('name') ? error.message : undefined}
        >
          <Input
            data-autofocus
            value={form.name}
            placeholder="Campus switches"
            onChange={(event) => setForm({ ...form, name: event.target.value })}
          />
        </Field>

        <Field label="User name">
          <Input
            mono
            autoComplete="off"
            value={form.username}
            onChange={(event) => setForm({ ...form, username: event.target.value })}
          />
        </Field>

        <SecretField
          label="Password"
          help="What the device asks for after the user name."
          isSet={credential?.has_password ?? false}
          value={secrets.password}
          onChange={(value) => setSecrets({ ...secrets, password: value })}
          onReset={() => {
            const { password: _password, ...rest } = secrets;
            setSecrets(rest);
          }}
        />

        <SecretField
          label="Enable secret"
          help="For platforms with a second, privileged mode."
          isSet={credential?.has_enable_secret ?? false}
          value={secrets.enable_secret}
          onChange={(value) => setSecrets({ ...secrets, enable_secret: value })}
          onReset={() => {
            const { enable_secret: _secret, ...rest } = secrets;
            setSecrets(rest);
          }}
        />

        <SecretField
          label="SSH private key"
          help="Paste the key itself, in OpenSSH or PEM form."
          multiline
          isSet={credential?.has_private_key ?? false}
          value={secrets.private_key}
          onChange={(value) => setSecrets({ ...secrets, private_key: value })}
          onReset={() => {
            const { private_key: _key, ...rest } = secrets;
            setSecrets(rest);
          }}
        />

        <SecretField
          label="Key passphrase"
          help="Only if the private key is encrypted."
          isSet={credential?.has_passphrase ?? false}
          value={secrets.passphrase}
          onChange={(value) => setSecrets({ ...secrets, passphrase: value })}
          onReset={() => {
            const { passphrase: _passphrase, ...rest } = secrets;
            setSecrets(rest);
          }}
        />

        <Field label="SNMP" hint="For reading interface counters and health without a login.">
          <Select
            value={snmp}
            options={[
              { value: '', label: 'Not used' },
              { value: 'v1', label: 'Version 1' },
              { value: 'v2c', label: 'Version 2c' },
              { value: 'v3', label: 'Version 3' },
            ]}
            onChange={(event) => setForm({ ...form, snmp_version: event.target.value })}
          />
        </Field>

        {(snmp === 'v1' || snmp === 'v2c') && (
          <SecretField
            label="Community"
            help="The read community string."
            isSet={credential?.snmp.has_community ?? false}
            value={secrets.snmp_community}
            onChange={(value) => setSecrets({ ...secrets, snmp_community: value })}
            onReset={() => {
              const { snmp_community: _community, ...rest } = secrets;
              setSecrets(rest);
            }}
          />
        )}

        {snmp === 'v3' && (
          <>
            <Field label="SNMPv3 user" required>
              <Input
                mono
                value={form.snmp_user}
                onChange={(event) => setForm({ ...form, snmp_user: event.target.value })}
              />
            </Field>
            <div className={styles.fields}>
              <Field label="Authentication">
                <Select
                  value={form.snmp_auth_protocol}
                  options={[
                    { value: '', label: 'None' },
                    { value: 'md5', label: 'MD5' },
                    { value: 'sha1', label: 'SHA-1' },
                    { value: 'sha224', label: 'SHA-224' },
                    { value: 'sha256', label: 'SHA-256' },
                    { value: 'sha384', label: 'SHA-384' },
                    { value: 'sha512', label: 'SHA-512' },
                  ]}
                  onChange={(event) =>
                    setForm({ ...form, snmp_auth_protocol: event.target.value })
                  }
                />
              </Field>
              <Field label="Privacy" hint="Privacy needs authentication as well.">
                <Select
                  value={form.snmp_priv_protocol}
                  options={[
                    { value: '', label: 'None' },
                    { value: 'des', label: 'DES' },
                    { value: 'aes128', label: 'AES-128' },
                    { value: 'aes192', label: 'AES-192' },
                    { value: 'aes256', label: 'AES-256' },
                  ]}
                  onChange={(event) =>
                    setForm({ ...form, snmp_priv_protocol: event.target.value })
                  }
                />
              </Field>
            </div>
            <SecretField
              label="Authentication password"
              isSet={credential?.snmp.has_auth_password ?? false}
              value={secrets.snmp_auth_password}
              onChange={(value) => setSecrets({ ...secrets, snmp_auth_password: value })}
              onReset={() => {
                const { snmp_auth_password: _auth, ...rest } = secrets;
                setSecrets(rest);
              }}
            />
            <SecretField
              label="Privacy password"
              isSet={credential?.snmp.has_priv_password ?? false}
              value={secrets.snmp_priv_password}
              onChange={(value) => setSecrets({ ...secrets, snmp_priv_password: value })}
              onReset={() => {
                const { snmp_priv_password: _priv, ...rest } = secrets;
                setSecrets(rest);
              }}
            />
          </>
        )}

        <Field label="Notes">
          <Textarea
            rows={3}
            value={form.notes}
            placeholder="Where this login comes from, who owns it, when it expires."
            onChange={(event) => setForm({ ...form, notes: event.target.value })}
          />
        </Field>

        {error && (
          <p className={styles.note} data-tone="crit">
            {error.message}
          </p>
        )}

        {credential && (
          <p className={styles.muted}>
            Added <TimeAgo at={credential.created_at} />, last changed{' '}
            <TimeAgo at={credential.updated_at} />.
          </p>
        )}
      </div>
    </Drawer>
  );
}
