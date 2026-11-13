/**
 * Device types — what Nocturne knows about each platform: how to recognise
 * it, how to log in, what to run to read it, and how it is configured.
 *
 * The list says how far each profile has been checked against real hardware,
 * because that is what decides whether to believe what it collects. The
 * editor validates as you type through `profiles.validate`, which never
 * touches the live registry, and shows the profile as it ends up after
 * `extends` has been applied.
 */

import { ArrowLeft, Copy, Cpu, FileCode, Plus, Save, Trash, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  Kind,
  Maturity,
  ProfileDeleted,
  ProfileDetail,
  ProfileList,
  ProfileRow,
  ProfileValidation,
  ProfileYamlParams,
} from '@/api/types/core';
import { KINDS } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { KeyValue } from '@/design/KeyValue';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { StatusDot } from '@/design/StatusDot';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { Tooltip } from '@/design/Tooltip';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { Group } from './parts';
import { plural } from './plural';
import styles from './Settings.module.css';

/** What each maturity level actually promises. */
const MATURITY: Record<Maturity, { tone: 'ok' | 'info' | 'warn'; label: string; help: string }> = {
  verified: {
    tone: 'ok',
    label: 'Verified',
    help: 'Its commands, prompts and parsers are checked against recorded output of the real platform by Nocturne’s own tests.',
  },
  community: {
    tone: 'info',
    label: 'Community',
    help: 'Written from vendor documentation and other tools’ definitions, but not yet checked against a device.',
  },
  experimental: {
    tone: 'warn',
    label: 'Experimental',
    help: 'A draft. Expect it to read some things wrong.',
  },
};

const KIND_SHORT: Record<Kind, string> = {
  facts: 'facts',
  interfaces: 'interfaces',
  neighbors: 'neighbours',
  vlans: 'VLANs',
  mac_table: 'MACs',
  arp: 'ARP',
  routes: 'routes',
  health: 'health',
  inventory: 'hardware',
};

/** The skeleton a new profile starts from. */
const TEMPLATE = `id: my_switch
name: My switch
vendor: Example
os: ExampleOS
description: What this platform is.
# Your own profiles start as a draft; \`verified\` is reserved for the
# built-ins, whose recorded device output is tested with every build.
maturity: experimental

# Only commands matching these patterns may be run to read the device.
policy:
  read_only:
    - '^show '

# How an unidentified session is recognised as this platform.
detect:
  banner:
    - '(?i)exampleos'

cli:
  prompt: '^(?P<host>[\\w.-]+)[>#]\\s*$'
  init:
    - terminal length 0

# What to run, and how to turn its output into Nocturne's model.
collect:
  facts:
    - command: show version
      parser: { regex: 'Version (?P<os_version>\\S+)' }
      map:
        os_version: os_version
`;

export function DeviceTypes() {
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState<{ id: string | null; yaml: string } | null>(null);

  const list = useRpc<ProfileList>('profiles.list', undefined, {
    invalidateOn: ['profiles.changed'],
  });

  const rows = useMemo(() => {
    const text = query.trim().toLowerCase();
    const profiles = list.data?.profiles ?? [];
    if (text === '') return profiles;
    return profiles.filter((profile) =>
      `${profile.id} ${profile.name} ${profile.vendor} ${profile.os} ${profile.description}`
        .toLowerCase()
        .includes(text),
    );
  }, [list.data, query]);

  const columns: TableColumn<ProfileRow>[] = [
    {
      id: 'name',
      header: 'Device type',
      required: true,
      sortValue: (profile) => profile.name,
      cell: (profile) => (
        <span className={styles.row}>
          {profile.name}
          {profile.origin === 'user' && (
            <Badge tone="accent" variant="outline">
              {profile.overrides_builtin ? 'Customised' : 'Yours'}
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'id',
      header: 'Id',
      width: 150,
      mono: true,
      sortValue: (profile) => profile.id,
      cell: (profile) => profile.id,
    },
    {
      id: 'maturity',
      header: 'Checked',
      width: 120,
      sortValue: (profile) => profile.maturity,
      cell: (profile) => {
        const level = MATURITY[profile.maturity];
        return (
          <Tooltip content={level.help}>
            <span className={styles.secretState}>
              <StatusDot tone={level.tone} label={level.label} />
              {level.label}
            </span>
          </Tooltip>
        );
      },
    },
    {
      id: 'vendor',
      header: 'Vendor',
      width: 110,
      sortValue: (profile) => profile.vendor,
      cell: (profile) => profile.vendor || '—',
    },
    {
      id: 'collects',
      header: 'Can read',
      sortValue: (profile) => profile.collects.length,
      cell: (profile) =>
        profile.collects.length === 0 ? (
          <span className={styles.muted}>nothing yet</span>
        ) : (
          <Tooltip content={profile.collects.map((kind) => KIND_SHORT[kind]).join(', ')}>
            <span className={styles.muted}>
              {formatCount(profile.collects.length)} of {KINDS.length} kinds
            </span>
          </Tooltip>
        ),
    },
    {
      id: 'configure',
      header: 'Configures',
      width: 118,
      sortValue: (profile) => Number(profile.can_configure),
      cell: (profile) =>
        profile.can_configure ? 'Yes' : <span className={styles.muted}>Read only</span>,
    },
    {
      id: 'devices',
      header: 'In use',
      width: 96,
      align: 'right',
      sortValue: (profile) => profile.devices,
      cell: (profile) => (profile.devices === 0 ? '—' : formatCount(profile.devices)),
    },
  ];

  if (open) {
    return (
      <ProfileEditor
        key={`${open.id}:${open.yaml}`}
        id={open.id}
        start={open.yaml}
        onClose={() => setOpen(null)}
      />
    );
  }

  return (
    <div className={styles.stack}>
      {(list.data?.problems.length ?? 0) > 0 && (
        <div className={styles.note} data-tone="warn">
          <strong>
            {plural(list.data?.problems.length ?? 0, 'device type')} could not be loaded.
          </strong>
          <ul className={styles.kinds}>
            {list.data?.problems.map((problem) => (
              <li key={problem.id}>
                <code className="mono">{problem.id}</code>: {problem.problems.join('; ')}
              </li>
            ))}
          </ul>
        </div>
      )}

      <Group
        title="Device types"
        description="A device type says how to recognise a platform, how to log in to it, what to run to read it, and how its configuration is written."
        actions={
          <Button
            variant="primary"
            icon={Plus}
            onClick={() => setOpen({ id: null, yaml: TEMPLATE })}
          >
            New device type
          </Button>
        }
      >
        <Toolbar>
          <SearchInput
            aria-label="Search device types"
            placeholder="Search by name, vendor or id"
            value={query}
            onChange={setQuery}
          />
          <ToolbarSpacer />
          <span className={styles.muted}>
            {formatCount(rows.length)} of {formatCount(list.data?.profiles.length ?? 0)}
          </span>
        </Toolbar>

        <Table
          className={styles.table}
          id="profiles"
          aria-label="Device types"
          columns={columns}
          rows={rows}
          rowKey={(profile) => profile.id}
          loading={list.isPending}
          maxHeight={420}
          defaultSort={{ columnId: 'name', direction: 'asc' }}
          onRowActivate={(profile) => setOpen({ id: profile.id, yaml: '' })}
          rowActions={(profile) => [
            {
              label: profile.origin === 'user' ? 'Edit…' : 'Open…',
              icon: FileCode,
              onSelect: () => setOpen({ id: profile.id, yaml: '' }),
            },
            {
              label: 'Duplicate…',
              icon: Copy,
              onSelect: () => setOpen({ id: profile.id, yaml: 'duplicate' }),
            },
          ]}
          empty={
            list.isError ? (
              <ErrorState
                size="compact"
                title="The device types could not be read"
                error={list.error}
                onRetry={() => void list.refetch()}
              />
            ) : (
              <EmptyState
                size="compact"
                icon={Cpu}
                title={query ? 'No device type matches' : 'No device types'}
                description={
                  query
                    ? `Nothing matches “${query}”.`
                    : 'The built-in pack could not be loaded at all, which should not happen.'
                }
              />
            )
          }
        />
      </Group>

    </div>
  );
}

/** `kv`, or the single key of `{ntc: …}` / `{regex: …}` / `{json: …}`. */
function parserName(parser: string | Record<string, string>): string {
  return typeof parser === 'string' ? parser : (Object.keys(parser)[0] ?? '');
}

/** How long the YAML must be still before it is validated. */
const DEBOUNCE_MS = 350;

function ProfileEditor({
  id,
  start,
  onClose,
}: {
  id: string | null;
  /** The YAML to start from, `duplicate` to copy the loaded one, or empty to load it. */
  start: string;
  onClose: () => void;
}) {
  const [yaml, setYaml] = useState(id === null ? start : '');
  const [debounced, setDebounced] = useState(yaml);
  const [touched, setTouched] = useState(id === null);

  const detail = useRpc<ProfileDetail>('profiles.get', { id }, { enabled: id !== null });

  // The editor opens with the profile as it is written today.
  useEffect(() => {
    if (!detail.data || touched) return;
    setYaml(
      start === 'duplicate'
        ? detail.data.yaml.replace(/^id:\s*(\S+)/m, (_line, old: string) => `id: ${old}_custom`)
        : detail.data.yaml,
    );
  }, [detail.data, start, touched]);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(yaml), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [yaml]);

  const touchedYaml = touched || id === null;
  const check = useRpc<ProfileValidation>(
    'profiles.validate',
    { yaml: debounced },
    { enabled: touchedYaml && debounced.trim() !== '' },
  );
  const save = useRpcMutation<ProfileDetail, ProfileYamlParams>('profiles.save', {
    invalidates: ['profiles.', 'devices.'],
    errorToast: false,
  });
  const remove = useRpcMutation<ProfileDeleted, { id: string }>('profiles.delete', {
    invalidates: ['profiles.', 'devices.'],
    errorToast: 'The device type was not deleted',
  });

  const duplicating = start === 'duplicate';
  const builtin = detail.data?.origin === 'builtin' && !duplicating;
  /**
   * Until the YAML is touched, the right-hand pane shows the profile the
   * engine actually has. Validating an untouched built-in would report its
   * own `maturity: verified` as a problem, which is true of a *copy* and
   * confusing about the original.
   */
  const resolved = touchedYaml ? check.data?.resolved : detail.data?.resolved;
  const valid = touchedYaml && check.data?.ok === true;

  const onSave = () => {
    save.mutate(
      { yaml },
      {
        onSuccess: (saved) => {
          toast.success(`Saved “${saved.resolved.name}”`);
          onClose();
        },
      },
    );
  };

  const onDelete = async () => {
    if (!id) return;
    const confirmed = await confirm({
      title: `Delete the device type “${id}”?`,
      description:
        'If it replaces a built-in of the same id, the built-in becomes active again. Otherwise the devices using it lose their type and are detected again on their next connection.',
      confirmLabel: 'Delete',
      tone: 'danger',
    });
    if (!confirmed) return;
    const result = await remove.mutateAsync({ id });
    toast.success(
      result.restored_builtin
        ? `The built-in “${id}” is active again`
        : `Deleted “${id}”${result.devices_affected > 0 ? ` — ${plural(result.devices_affected, 'device')} now have no type` : ''}`,
    );
    onClose();
  };

  return (
    <div className={styles.full}>
      <Toolbar>
        <Button size="sm" icon={ArrowLeft} onClick={onClose} disabled={save.isPending}>
          All device types
        </Button>
        <h3 className={styles.editorTitle}>
          {id === null ? 'New device type' : duplicating ? `Duplicate ${id}` : id}
        </h3>
        <span className={styles.muted}>
          {builtin
            ? 'Built in. Saving keeps your own version, which replaces it until you delete that again.'
            : 'Checked as you type. Nothing is saved until you press Save.'}
        </span>
        <ToolbarSpacer />
        {id !== null && detail.data?.origin === 'user' && !duplicating && (
          <Button
            variant="danger"
            icon={Trash}
            loading={remove.isPending}
            onClick={() => void onDelete()}
          >
            Delete
          </Button>
        )}
        <Button
          variant="primary"
          icon={Save}
          loading={save.isPending}
          disabled={!valid}
          onClick={onSave}
        >
          {builtin ? 'Save my version' : 'Save'}
        </Button>
      </Toolbar>

      <div className={styles.editor}>
        <div className={styles.editorPane}>
          <div className={styles.row}>
            <span className={styles.muted}>The profile, as written</span>
            <ToolbarSpacer />
            {!touchedYaml ? (
              <span className={styles.muted}>Unchanged</span>
            ) : check.isFetching ? (
              <span className={styles.muted}>Checking…</span>
            ) : valid ? (
              <span className={styles.secretState}>
                <StatusDot tone="ok" label="Valid" />
                Valid
              </span>
            ) : check.data ? (
              <span className={styles.secretState}>
                <StatusDot tone="crit" label="Has problems" />
                {plural(check.data.problems.length, 'problem')}
              </span>
            ) : null}
          </div>

          {detail.isPending && id !== null ? (
            <Skeleton height={320} shape="block" />
          ) : (
            <Textarea
              mono
              className={styles.yaml}
              aria-label="Device type YAML"
              spellCheck={false}
              value={yaml}
              onChange={(event) => {
                setTouched(true);
                setYaml(event.target.value);
              }}
            />
          )}

          {check.data && !check.data.ok && (
            <ul className={styles.problems}>
              {check.data.problems.map((problem) => (
                <li key={problem} className={styles.problem}>
                  {problem}
                </li>
              ))}
            </ul>
          )}

          {save.error && (
            <p className={styles.note} data-tone="crit">
              {save.error.message}
            </p>
          )}
        </div>

        <div className={styles.editorPane}>
          <span className={styles.muted}>As it ends up, with everything it extends applied</span>
          {resolved ? (
            <>
              <KeyValue
                items={[
                  { label: 'Id', value: resolved.id, mono: true },
                  { label: 'Name', value: resolved.name || null },
                  { label: 'Vendor', value: resolved.vendor || null },
                  { label: 'Operating system', value: resolved.os || null },
                  { label: 'Extends', value: resolved.extends ?? null, mono: true },
                  { label: 'Checked', value: MATURITY[resolved.maturity].label },
                  {
                    label: 'Prompt',
                    value: resolved.cli.prompt || null,
                    mono: true,
                  },
                  {
                    label: 'Reads',
                    value:
                      Object.keys(resolved.collect ?? {}).length === 0 ? null : (
                        <span className={styles.kinds}>
                          {(Object.keys(resolved.collect ?? {}) as Kind[]).map((kind) => (
                            <Badge key={kind} variant="outline">
                              {KIND_SHORT[kind] ?? kind}
                            </Badge>
                          ))}
                        </span>
                      ),
                  },
                  {
                    label: 'Configuration',
                    value: resolved.config.running
                      ? `${resolved.config.syntax} · reads with “${resolved.config.running}”`
                      : null,
                  },
                  {
                    label: 'Dead-man switch',
                    value: resolved.config.guard
                      ? `${resolved.config.guard.name} — ${resolved.config.guard.effect}`
                      : null,
                  },
                ]}
                layout="stacked"
              />
              <CodeBlock
                title="Commands it will run"
                maxHeight={240}
                code={
                  Object.entries(resolved.collect ?? {})
                    .flatMap(([kind, steps]) =>
                      // Several steps may share a command and differ only in
                      // how its output is parsed, so the parser is named too.
                      (steps ?? []).map(
                        (step) =>
                          `${kind.padEnd(12)} ${step.command.padEnd(34)} ${parserName(step.parser)}`,
                      ),
                    )
                    .join('\n') || 'This device type runs no collection commands.'
                }
              />
            </>
          ) : (
            <EmptyState
              size="compact"
              icon={TriangleAlert}
              title="Nothing to show yet"
              description="The resolved profile appears here as soon as the YAML is valid."
            />
          )}
        </div>
      </div>
    </div>
  );
}
