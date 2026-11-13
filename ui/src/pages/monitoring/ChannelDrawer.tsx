/**
 * The notification-channel editor.
 *
 * A channel's secret — the webhook URL (which for Slack, Teams and Discord
 * *is* the secret) or the SMTP password — lives in the vault and is never
 * returned, so the field shows only whether one is set and offers Replace and
 * Clear. Leaving it alone sends nothing at all, which is what keeps a save of
 * the name from wiping the URL.
 */

import { Save, Send } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type {
  Channel,
  ChannelKind,
  ChannelSaveParams,
  ChannelTestResult,
  EmailSettings,
  Severity,
} from '@/api/types/monitor';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { formatCount } from '@/lib/format';
import styles from './Monitoring.module.css';
import { SecretField } from './SecretField';
import { SEVERITIES, severityLabel } from './vocab';

const KINDS: readonly { value: ChannelKind; label: string; help: string }[] = [
  { value: 'webhook', label: 'Webhook', help: 'A JSON POST of the alert itself.' },
  { value: 'slack', label: 'Slack', help: 'An incoming webhook URL from a Slack app.' },
  { value: 'teams', label: 'Microsoft Teams', help: 'An incoming webhook URL from a Teams connector.' },
  { value: 'discord', label: 'Discord', help: 'A channel webhook URL from Discord.' },
  { value: 'email', label: 'E-mail', help: 'SMTP submission, with STARTTLS where the server offers it.' },
];

const EMPTY_EMAIL: EmailSettings = {
  host: '',
  port: 587,
  starttls: 'auto',
  username: '',
  from: '',
  to: [],
};

export interface ChannelDrawerProps {
  channel: Channel | null;
  onClose: () => void;
}

export function ChannelDrawer({ channel, onClose }: ChannelDrawerProps) {
  const [name, setName] = useState(channel?.name ?? '');
  const [kind, setKind] = useState<ChannelKind>(channel?.kind ?? 'slack');
  const [enabled, setEnabled] = useState(channel?.enabled ?? true);
  const [minSeverity, setMinSeverity] = useState<Severity>(channel?.min_severity ?? 'info');
  const [email, setEmail] = useState<EmailSettings>(channel?.email ?? EMPTY_EMAIL);
  /** `undefined` leaves the stored secret alone; `''` clears it. */
  const [secret, setSecret] = useState<string | undefined>(undefined);

  const save = useRpcMutation<Channel, ChannelSaveParams>('notify.channels.save', {
    invalidates: ['notify.'],
    successToast: channel ? 'Channel saved' : 'Channel created',
    errorToast: false,
  });
  const test = useRpcMutation<ChannelTestResult, { id: string }>('notify.channels.test', {
    invalidates: ['notify.'],
    errorToast: false,
  });

  const isEmail = kind === 'email';
  const hasSecret = channel?.has_secret ?? false;
  const willHaveSecret = secret === undefined ? hasSecret : secret !== '';

  return (
    <Drawer
      open
      onClose={onClose}
      title={channel ? `Edit ${channel.name}` : 'New notification channel'}
      description={KINDS.find((entry) => entry.value === kind)?.help}
      footer={
        <>
          {channel && (
            <Button
              icon={Send}
              loading={test.isPending}
              disabled={!channel.has_secret && channel.kind !== 'email'}
              onClick={() => test.mutate({ id: channel.id })}
            >
              Send a test message
            </Button>
          )}
          <Button
            variant="primary"
            icon={Save}
            loading={save.isPending}
            disabled={name.trim() === '' || (!isEmail && !willHaveSecret)}
            onClick={() =>
              save.mutate(
                {
                  ...(channel ? { id: channel.id } : {}),
                  name: name.trim(),
                  kind,
                  enabled,
                  min_severity: minSeverity,
                  ...(isEmail ? { email } : {}),
                  ...(secret === undefined ? {} : isEmail ? { password: secret } : { url: secret }),
                },
                { onSuccess: onClose },
              )
            }
          >
            Save
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {save.error && <p className={styles.error}>{save.error.message}</p>}
        {test.error && <p className={styles.error}>{test.error.message}</p>}
        {test.data?.sent && (
          <p className={styles.testResult} role="status">
            Test message delivered
            {test.data.attempts > 1 ? ` after ${formatCount(test.data.attempts)} attempts.` : '.'}
          </p>
        )}

        <Field label="Name" required>
          <Input
            aria-label="Name"
            placeholder="Night shift"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>

        <div className={styles.row}>
          <Field
            label="Kind"
            hint={
              channel
                ? 'Fixed once the channel exists: the vault holds either a URL or a password for it, not both.'
                : undefined
            }
          >
            <Select
              aria-label="Kind"
              value={kind}
              disabled={Boolean(channel)}
              options={KINDS.map((entry) => ({ value: entry.value, label: entry.label }))}
              onChange={(event) => setKind(event.target.value as ChannelKind)}
            />
          </Field>
          <Field label="Only from" hint="Alerts below this severity are not sent to this channel.">
            <Select
              aria-label="Only from"
              value={minSeverity}
              options={SEVERITIES.map((severity) => ({
                value: severity,
                label: `${severityLabel(severity)} and above`,
              }))}
              onChange={(event) => setMinSeverity(event.target.value as Severity)}
            />
          </Field>
        </div>

        {isEmail ? (
          <>
            <div className={styles.row}>
              <Field label="SMTP host" required>
                <Input
                  aria-label="SMTP host"
                  mono
                  value={email.host}
                  onChange={(event) => setEmail({ ...email, host: event.target.value })}
                />
              </Field>
              <Field label="Port">
                <Input
                  aria-label="Port"
                  type="number"
                  value={String(email.port)}
                  onChange={(event) => setEmail({ ...email, port: Number(event.target.value) || 587 })}
                />
              </Field>
              <Field label="STARTTLS">
                <Select
                  aria-label="STARTTLS"
                  value={email.starttls}
                  options={[
                    { value: 'auto', label: 'When the server offers it' },
                    { value: 'required', label: 'Required' },
                    { value: 'never', label: 'Never' },
                  ]}
                  onChange={(event) => setEmail({ ...email, starttls: event.target.value })}
                />
              </Field>
            </div>
            <div className={styles.row}>
              <Field label="Username">
                <Input
                  aria-label="Username"
                  value={email.username}
                  onChange={(event) => setEmail({ ...email, username: event.target.value })}
                />
              </Field>
              <Field label="From" required>
                <Input
                  aria-label="From"
                  placeholder="nocturne@example.com"
                  value={email.from}
                  onChange={(event) => setEmail({ ...email, from: event.target.value })}
                />
              </Field>
            </div>
            <Field label="To" required hint="One or more addresses; press Enter after each.">
              <TagInput
                aria-label="To"
                value={email.to}
                onChange={(to) => setEmail({ ...email, to })}
                placeholder="oncall@example.com"
              />
            </Field>
            <SecretField
              label="Password"
              help="The SMTP password, kept in the vault."
              isSet={hasSecret}
              value={secret}
              onChange={setSecret}
              onReset={() => setSecret(undefined)}
            />
          </>
        ) : (
          <SecretField
            label="Webhook URL"
            help={
              kind === 'webhook'
                ? 'Where the JSON is posted. It is a secret: it is kept in the vault and never shown again.'
                : 'The incoming-webhook URL. It is the secret itself: anyone holding it can post to that channel.'
            }
            placeholder="https://…"
            isSet={hasSecret}
            value={secret}
            onChange={setSecret}
            onReset={() => setSecret(undefined)}
          />
        )}

        <Switch checked={enabled} onChange={(event) => setEnabled(event.target.checked)}>
          Channel is on
        </Switch>

        {!isEmail && !willHaveSecret && (
          <p className={styles.hint}>A webhook channel cannot be saved without the URL it posts to.</p>
        )}
        {channel && !channel.has_secret && (
          <p className={styles.hint}>
            This channel has no secret stored, so it cannot send anything until one is set.
          </p>
        )}
      </div>
    </Drawer>
  );
}
