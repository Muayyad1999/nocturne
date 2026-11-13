/**
 * Connections — how long Nocturne waits for a device, how it keeps a session
 * alive, and what it is willing to negotiate with old equipment.
 *
 * Numbers save when the field loses focus; choices save at once. Every one of
 * them says what it costs.
 */

import { useEffect, useState } from 'react';
import type { ConnectionSettings } from '@/api/types/core';
import { ErrorState } from '@/design/ErrorState';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { confirm } from '@/design/Dialog';
import { HOST_KEY_POLICIES, LEGACY_CRYPTO_OPTIONS } from './policies';
import { Group, Setting, useSettingsSection } from './parts';
import styles from './Settings.module.css';

/** A number field that saves when it loses focus, and only if it changed. */
function NumberSetting({
  label,
  help,
  unit,
  min,
  max,
  value,
  saved,
  error,
  onCommit,
}: {
  label: string;
  help: string;
  unit: string;
  min: number;
  max: number;
  value: number;
  saved: boolean;
  error?: string;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => setText(String(value)), [value]);

  const commit = () => {
    const next = Number(text);
    if (!Number.isFinite(next) || next < min || next > max) {
      setProblem(`A whole number between ${min} and ${max}.`);
      setText(String(value));
      return;
    }
    setProblem(null);
    if (next !== value) onCommit(next);
  };

  return (
    <Setting narrow label={label} help={help} error={problem ?? error} saved={saved}>
      <Input
        mono
        size="sm"
        inputMode="numeric"
        aria-label={label}
        invalid={problem !== null}
        value={text}
        trailing={<span className={styles.muted}>{unit}</span>}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur();
        }}
      />
    </Setting>
  );
}

export function Connections() {
  const section = useSettingsSection<ConnectionSettings>('connections');
  const value = section.value;

  if (section.error) {
    return (
      <ErrorState
        title="These settings could not be read"
        error={section.error}
        onRetry={section.refetch}
      />
    );
  }
  if (!value) return <Skeleton height={260} shape="block" />;

  const errorFor = (key: string) =>
    section.saveError?.key === key ? section.saveError.message : undefined;

  return (
    <div className={styles.stack}>
      <Group
        title="Timeouts"
        description="A device on the far end of a slow link needs more patience than one in the next rack."
      >
        <NumberSetting
          label="Connect timeout"
          help="How long to wait for a device to answer at all before giving up."
          unit="seconds"
          min={1}
          max={300}
          value={value.connect_timeout_s}
          saved={section.justSaved === 'connect_timeout_s'}
          error={errorFor('connect_timeout_s')}
          onCommit={(connect_timeout_s) => section.set({ connect_timeout_s })}
        />
        <NumberSetting
          label="Login timeout"
          help="How long from “connected” to a prompt Nocturne recognises. A device that boots slowly or prints a long banner needs more."
          unit="seconds"
          min={5}
          max={300}
          value={value.login_timeout_s}
          saved={section.justSaved === 'login_timeout_s'}
          error={errorFor('login_timeout_s')}
          onCommit={(login_timeout_s) => section.set({ login_timeout_s })}
        />
        <NumberSetting
          label="Keepalive"
          help="Seconds between SSH keepalives, so an idle session is not dropped by a firewall. Zero turns them off."
          unit="seconds"
          min={0}
          max={3600}
          value={value.keepalive_s}
          saved={section.justSaved === 'keepalive_s'}
          error={errorFor('keepalive_s')}
          onCommit={(keepalive_s) => section.set({ keepalive_s })}
        />
      </Group>

      <Group title="Sessions">
        <NumberSetting
          label="Sessions per device"
          help="How many automated sessions Nocturne may hold to one device at a time. Most platforms are happier with one."
          unit="at a time"
          min={1}
          max={16}
          value={value.max_sessions_per_device}
          saved={section.justSaved === 'max_sessions_per_device'}
          error={errorFor('max_sessions_per_device')}
          onCommit={(max_sessions_per_device) => section.set({ max_sessions_per_device })}
        />
      </Group>

      <Group
        title="Cryptography"
        description="Equipment that is still in service is often older than the algorithms a modern SSH client will offer."
      >
        <Setting
          label="Legacy SSH algorithms"
          help={
            LEGACY_CRYPTO_OPTIONS.find((option) => option.value === value.legacy_crypto)?.description
          }
          saved={section.justSaved === 'legacy_crypto'}
          error={errorFor('legacy_crypto')}
        >
          <Select
            aria-label="Legacy SSH algorithms"
            value={value.legacy_crypto}
            options={LEGACY_CRYPTO_OPTIONS.map(({ value: option, label }) => ({
              value: option,
              label,
            }))}
            onChange={(event) => section.set({ legacy_crypto: event.target.value })}
          />
        </Setting>

        <Setting
          label="When a host key is not known"
          help={
            HOST_KEY_POLICIES.find((option) => option.value === value.host_key_policy)?.description
          }
          saved={section.justSaved === 'host_key_policy'}
          error={errorFor('host_key_policy')}
        >
          <Select
            aria-label="When a host key is not known"
            value={value.host_key_policy}
            options={HOST_KEY_POLICIES.map(({ value: option, label }) => ({
              value: option,
              label,
            }))}
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
              section.set({ host_key_policy: next });
            }}
          />
        </Setting>

        {value.host_key_policy === 'accept' && (
          <p className={styles.note} data-tone="crit">
            Host key checking is off. The keys devices present are not verified against anything.
          </p>
        )}
      </Group>
    </div>
  );
}
