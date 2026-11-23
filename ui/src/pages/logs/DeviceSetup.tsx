/**
 * What one device has to be told so that it sends here.
 *
 * `logs.device_setup` writes the configuration lines for the chosen device's
 * platform: the real port this installation listens on and the real address
 * of this machine as that device would reach it. It writes two sets — the
 * syslog lines and the SNMP notification lines — and applies neither: they
 * are text, for the change pipeline or for a person's own session, and the
 * engine's note under each block says so.
 *
 * The trap lines are **not complete**. Where the community goes they say
 * `<snmp-community>`, because a community is a secret and Nocturne puts one
 * into nothing it returns. That is a placeholder and the block says so in
 * so many words, above the engine's own note: a line pasted as it stands
 * would configure a device to send with the literal text.
 */

import { Download } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import type { Device } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Combobox } from '@/design/Combobox';
import { ErrorState } from '@/design/ErrorState';
import { Skeleton } from '@/design/Skeleton';
import { formatCount, plural } from '@/lib/format';
import { useDeviceSetup, usePull } from './data';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';

/** What stands in a trap line where the community goes (the engine's own text). */
export const COMMUNITY_PLACEHOLDER = '<snmp-community>';

export interface DeviceSetupProps {
  devices: readonly Device[];
  /** The device to explain first, if the screen is already about one. */
  deviceId?: string | null;
  /** Which lines to show. */
  show: 'syslog' | 'traps' | 'both';
  capabilities: Capabilities;
  /** Distinguishes two pickers on one page for assistive technology. */
  id: string;
}

export function DeviceSetup({ devices, deviceId = null, show, capabilities, id }: DeviceSetupProps) {
  const pull = usePull();

  // The device to show the lines for: the one being looked at, else the first
  // one whose platform is known, because only then are the lines real.
  const withProfile = devices.filter((device) => device.profile !== '');
  const [chosen, setChosen] = useState<string | null>(null);
  const device = chosen ?? deviceId ?? withProfile[0]?.id ?? devices[0]?.id ?? null;
  const setup = useDeviceSetup(device, capabilities.deviceSetup);
  const name = devices.find((entry) => entry.id === device)?.name ?? 'This device';

  // With neither method there is nothing a device picker could lead to.
  const canPull = show !== 'traps' && capabilities.pull;
  if (!capabilities.deviceSetup && !canPull) return null;

  if (devices.length === 0) {
    return (
      <p className={styles.hintDetail}>
        There are no devices yet, so there is no platform to write the lines for.{' '}
        <Link to="/devices">Add a device</Link> and the exact commands for it appear here.
      </p>
    );
  }

  const syslog = show !== 'traps';
  const traps = show !== 'syslog';

  return (
    <div className={styles.hintBody}>
      <div className={styles.hintRow}>
        <label className={styles.hintLabel} htmlFor={`${id}-device`}>
          Point this device at Nocturne
        </label>
        <Combobox
          id={`${id}-device`}
          className={styles.hintPicker}
          size="sm"
          aria-label="Device to show the commands for"
          value={device}
          onChange={setChosen}
          options={devices.map((entry) => ({
            value: entry.id,
            label: entry.name,
            description: entry.profile === '' ? 'platform not known yet' : entry.profile,
          }))}
          emptyText="No such device"
        />
        {canPull && (
          <Button
            size="sm"
            icon={Download}
            loading={pull.isPending}
            disabled={device === null}
            onClick={() => device && pull.mutate({ device_id: device })}
            title="Reads the device's own log buffer over the read-only CLI. Nothing is configured and nothing is sent."
          >
            Read its buffer instead
          </Button>
        )}
      </div>

      {setup.isError ? (
        <ErrorState size="compact" title="The lines for this device are not known" error={setup.error} />
      ) : setup.data ? (
        <>
          {syslog && (
            <div className={styles.setupBlock} data-testid="setup-syslog">
              <CodeBlock
                code={setup.data.lines.join('\n')}
                title={`${name} · ${setup.data.profile} · syslog to ${setup.data.address}:${setup.data.udp_port}`}
                copyable
                lineNumbers
              />
              <p className={styles.hintDetail}>
                {setup.data.address} is {setup.data.address_note}. {setup.data.note}
              </p>
            </div>
          )}
          {traps && (
            <div className={styles.setupBlock} data-testid="setup-traps">
              <CodeBlock
                code={setup.data.trap_lines.join('\n')}
                title={`${name} · ${setup.data.profile} · SNMP notifications to ${setup.data.address}:${setup.data.trap_port}`}
                copyable
                lineNumbers
              />
              {setup.data.trap_lines.some((line) => line.includes(COMMUNITY_PLACEHOLDER)) && (
                <p className={styles.hintDetail}>
                  <Badge tone="warn" variant="outline">
                    Placeholder
                  </Badge>{' '}
                  <code className={styles.inlineCode}>{COMMUNITY_PLACEHOLDER}</code> is not a community.
                  It stands where this device’s own SNMP community goes: a community is a secret, and
                  Nocturne never puts one into a line it shows. Replace it before the lines are used.
                </p>
              )}
              <p className={styles.hintDetail}>{setup.data.trap_note}</p>
              <p className={styles.muted}>
                Not verified: these lines are written from the vendors’ documentation and have not been
                applied to a device.
              </p>
            </div>
          )}
        </>
      ) : capabilities.deviceSetup ? (
        <Skeleton shape="block" height={96} />
      ) : null}

      {pull.data && (
        <p className={styles.hintDetail} data-testid="pull-result">
          {pull.data.note === ''
            ? `${pull.data.command}: ${plural(pull.data.read, 'line')} read, ${formatCount(pull.data.stored)} stored, ${formatCount(pull.data.duplicates)} already known, ${formatCount(pull.data.dated_by_device)} dated by the device’s own clock.`
            : pull.data.note}
        </p>
      )}
    </div>
  );
}
