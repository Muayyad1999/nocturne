/**
 * One forwarder: where everything that is stored is handed on to.
 *
 * Two things about this form are not like the others.
 *
 * **The destination of an HTTPS forwarder is write-only.** Its URL and its
 * token go to the vault and are returned by nothing, so the form says that a
 * destination is stored and offers to replace it; it never shows one. A new
 * URL does not carry the old token with it — the engine will not send a
 * token to an address it was not given for — and the form says so before
 * the save, not after the collector starts refusing.
 *
 * **Forwarding the audit trail sends operator activity off this machine.**
 * It is never a default, it has its own sentence here, and the audit entry
 * for saving such a forwarder says that it does.
 *
 * The engine checks the destination and says what is wrong in a sentence
 * that never repeats the host, the URL or the token. That sentence is shown
 * beside the field it is about (`fieldOfForwardError`).
 */

import { Save, Send, Trash } from 'lucide-react';
import { useState } from 'react';
import type { ForwardKind, ForwardStream, Forwarder } from '@/api/types/logs';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { Textarea } from '@/design/Textarea';
import { useSaveForwarder } from './data';
import {
  FORWARD_KINDS,
  FORWARD_STREAMS,
  draftOfForwarder,
  fieldOfForwardError,
  isEncrypted,
  isSyslogKind,
  saveParams,
  sendsAudit,
  tokenFate,
  willHaveDestination,
  type ForwardErrors,
  type ForwarderDraft,
} from './forward';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';
import { SEVERITY_NAMES } from './query';
import { WriteOnlyField } from './WriteOnlyField';

export interface ForwarderDrawerProps {
  /** `null` for a forwarder that does not exist yet. */
  forwarder: Forwarder | null;
  capabilities: Capabilities;
  onClose: () => void;
  /** Send one test event. Offered for a forwarder that is already saved. */
  onTest?: (forwarder: Forwarder) => void;
  onDelete?: (forwarder: Forwarder) => void;
  testing?: boolean;
  /** What the last test of this forwarder came to, in the engine's words. */
  outcome?: { ok: boolean; message: string } | null;
}

const TOKEN_NOTE: Record<string, string> = {
  kept: 'The token is left as it is. Whether one is stored cannot be read back.',
  replaced: 'The token is replaced when you save.',
  removed: 'The token is removed when you save: requests are then sent without an Authorization header.',
  'dropped-with-new-url':
    'A new URL starts without a token: the stored one was given for the old address and is not sent to another. Type the token again, or there will be none.',
  none: '',
};

export function ForwarderDrawer({
  forwarder,
  capabilities,
  onClose,
  onTest,
  onDelete,
  testing,
  outcome,
}: ForwarderDrawerProps) {
  const existing = forwarder ?? undefined;
  const [draft, setDraft] = useState<ForwarderDraft>(() => draftOfForwarder(existing));
  const [local, setLocal] = useState<ForwardErrors>({});
  const save = useSaveForwarder();

  const set = <K extends keyof ForwarderDraft>(key: K, value: ForwarderDraft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  const errors: ForwardErrors = { ...local };
  if (save.error) {
    const field = fieldOfForwardError(save.error.message);
    errors[field] ??= save.error.message;
  }

  const kind = FORWARD_KINDS.find((entry) => entry.value === draft.kind);
  const syslog = isSyslogKind(draft.kind);
  // The stored secret belongs to the HTTPS kind; a forwarder that was a
  // syslog one has none.
  const storedSecret = existing?.kind === 'https_json' && existing.has_secret;
  const fate = tokenFate(draft, existing);
  const audit = sendsAudit(draft.streams);

  const toggleStream = (stream: ForwardStream, on: boolean) =>
    set(
      'streams',
      on ? [...draft.streams.filter((entry) => entry !== stream), stream] : draft.streams.filter((entry) => entry !== stream),
    );

  const submit = () => {
    const { params, errors: found } = saveParams(draft);
    setLocal(found);
    if (Object.keys(found).length > 0) return;
    save.mutate(params, { onSuccess: onClose });
  };

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={forwarder ? `Edit ${forwarder.name}` : 'New forwarder'}
      description={kind?.help}
      footer={
        <>
          {forwarder && onDelete && capabilities.forwardDelete && (
            <Button icon={Trash} variant="ghost" onClick={() => onDelete(forwarder)}>
              Delete…
            </Button>
          )}
          {forwarder && onTest && capabilities.forwardTest && (
            <Button
              icon={Send}
              loading={testing}
              onClick={() => onTest(forwarder)}
              title="Sends one event that says it is a test, on a connection of its own, as the forwarder is saved now — whether it is switched on or not."
            >
              Send a test event
            </Button>
          )}
          {capabilities.forwardSave && (
            <Button
              variant="primary"
              icon={Save}
              loading={save.isPending}
              disabled={draft.name.trim() === '' || !willHaveDestination(draft, existing)}
              onClick={submit}
            >
              Save
            </Button>
          )}
        </>
      }
    >
      <div className={styles.form}>
        {errors.form && (
          <p className={styles.fieldError} role="alert">
            {errors.form}
          </p>
        )}
        {outcome && (
          <p className={outcome.ok ? styles.testOk : styles.fieldError} role={outcome.ok ? 'status' : 'alert'}>
            {outcome.ok ? outcome.message : `The test event was not sent: ${outcome.message}`}
          </p>
        )}

        <Field label="Name" required error={errors.name}>
          <Input value={draft.name} onChange={(event) => set('name', event.target.value)} placeholder="SIEM" />
        </Field>

        <div className={styles.formRow}>
          <Field label="Kind" error={errors.kind}>
            <Select
              value={draft.kind}
              options={FORWARD_KINDS.map(({ value, label }) => ({ value, label }))}
              onChange={(event) => set('kind', event.target.value as ForwardKind)}
            />
          </Field>
          {syslog && (
            <Field
              label="Format"
              error={errors.format}
              hint="RFC 5424: one message per entry. JSON: the stored row, one document to a line."
            >
              <Select
                value={draft.format}
                options={[
                  { value: 'rfc5424', label: 'RFC 5424' },
                  { value: 'json', label: 'JSON' },
                ]}
                onChange={(event) => set('format', event.target.value as ForwarderDraft['format'])}
              />
            </Field>
          )}
        </div>

        {syslog ? (
          <div className={styles.formRow}>
            <Field
              label="Collector"
              required
              error={errors.host}
              hint="A host name or an address. Collectors live on internal networks, so no address is refused for being private."
            >
              <Input
                mono
                value={draft.host}
                onChange={(event) => set('host', event.target.value)}
                placeholder="siem.example.net"
              />
            </Field>
            <Field
              label="Port"
              error={errors.port}
              hint={`Empty: ${kind?.defaultPort ?? 514}, this kind’s own port.`}
            >
              <Input
                mono
                inputMode="numeric"
                value={draft.port}
                onChange={(event) => set('port', event.target.value)}
                placeholder={String(kind?.defaultPort ?? 514)}
              />
            </Field>
          </div>
        ) : (
          <>
            <p className={styles.note} role="note">
              The URL and the token are write-only. They are stored in the vault and are in no
              document, event, log line or error message — so once saved they are never shown again,
              here or anywhere else.
            </p>
            <WriteOnlyField
              label="URL"
              noun="URL"
              required={!storedSecret}
              stored={storedSecret}
              value={draft.url}
              onChange={(value) => set('url', value)}
              error={errors.url}
              hint="https://, or http:// for a collector on your own network — over http:// a token would cross that network as text. A redirect is not followed."
              placeholder="https://collector.example.net:8088/services/collector/event"
            />
            <WriteOnlyField
              label="Token"
              // Whether a token is stored with the URL cannot be read back.
              stored={storedSecret ? 'unknown' : false}
              removable={storedSecret}
              value={draft.token}
              onChange={(value) => set('token', value)}
              error={errors.token}
              hint="Sent as Authorization: Splunk <token>, or as written when it carries its own scheme (Bearer …)."
            />
            {TOKEN_NOTE[fate] !== '' && (
              <p className={styles.hintDetail} data-fate={fate} data-testid="token-fate">
                {TOKEN_NOTE[fate]}
              </p>
            )}
          </>
        )}

        {isEncrypted(draft.kind) && (
          <Field
            label="Certificate authority (PEM)"
            error={errors.ca_pem}
            hint="Only for a collector whose certificate is signed by a private authority. Certificates only: this text is public, is stored with the forwarder and is shown again. The certificate is always checked; there is no way to switch that off."
          >
            <Textarea
              mono
              rows={4}
              spellCheck={false}
              value={draft.caPem}
              onChange={(event) => set('caPem', event.target.value)}
              placeholder="-----BEGIN CERTIFICATE-----"
            />
          </Field>
        )}

        <fieldset className={styles.group}>
          <legend className={styles.groupLegend}>What it sends</legend>
          {FORWARD_STREAMS.map((stream) => (
            <Checkbox
              key={stream.value}
              checked={draft.streams.includes(stream.value)}
              onChange={(event) => toggleStream(stream.value, event.target.checked)}
            >
              <span className={styles.checkLabel}>
                {stream.label}
                <span className={styles.muted}>{stream.help}</span>
              </span>
            </Checkbox>
          ))}
          {errors.streams && (
            <p className={styles.fieldError} role="alert">
              {errors.streams}
            </p>
          )}
          {audit && (
            <p className={styles.caution} data-testid="audit-warning">
              Forwarding the audit trail sends what people did here — every change approved, every
              unredacted view, every setting saved — off this machine. The audit entry for saving this
              forwarder says that it does.
            </p>
          )}
          <Field
            label="Hold back what is less severe than"
            error={errors.min_severity}
            hint="An entry with no severity — every trap, a line that came without a priority — is never held back, and neither is the audit trail."
          >
            <Select value={draft.minSeverity} onChange={(event) => set('minSeverity', event.target.value)}>
              <option value="">Nothing: send everything</option>
              {SEVERITY_NAMES.map((name, index) => (
                <option key={name} value={String(index)}>
                  {name} ({index})
                </option>
              ))}
            </Select>
          </Field>
          <Switch checked={draft.lab} onChange={(event) => set('lab', event.target.checked)}>
            Also what lab devices write
          </Switch>
          <p className={styles.muted}>
            Off unless asked for: a log platform would take a lab’s port-security violation for a real
            one. With it on, a lab is the way to try a forwarder out.
          </p>
        </fieldset>

        <Switch checked={draft.enabled} onChange={(event) => set('enabled', event.target.checked)}>
          Switched on
        </Switch>
        <p className={styles.muted}>
          A forwarder sends what is stored from the moment it is saved; nothing older. What waits in
          its queue when it is saved again, or when Nocturne stops, is dropped and counted.
        </p>
      </div>
    </Drawer>
  );
}
