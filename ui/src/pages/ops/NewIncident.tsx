/**
 * Opening an incident — from an alert, or by hand.
 *
 * From an alert is the common case and the better one: the alert's title, time
 * and severity become the incident's, its device is linked, and the timeline
 * arrives with the alert's own history already on it. The backend does all of
 * that from one `from_alert`, so this dialog sends the id and gets out of the
 * way.
 *
 * By hand is for the outage somebody phoned in. Then the start time matters:
 * it is what the timeline is gathered around, so it is editable and defaults
 * to now rather than being hidden.
 */

import { Plus, X } from 'lucide-react';
import { useState } from 'react';
import type { IncidentSeverity } from '@/api/types/ops';
import { Button } from '@/design/Button';
import { Combobox } from '@/design/Combobox';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { LabPill } from '@/design/StatusPill';
import { Textarea } from '@/design/Textarea';
import { formatTimestamp } from '@/lib/time';
import { useAuthor } from './author';
import { useCreateIncident, useDevices, useOpenAlerts } from './data';
import styles from './Ops.module.css';
import { SEVERITY_LABEL, SEVERITY_MEANING } from './ops';

const SEVERITIES = (['sev1', 'sev2', 'sev3', 'sev4'] as IncidentSeverity[]).map((severity) => ({
  value: severity,
  label: `${SEVERITY_LABEL[severity]} — ${SEVERITY_MEANING[severity]}`,
}));

/** `<input type="datetime-local">` wants local wall-clock text, not an ISO instant. */
function toLocalInput(at: number): string {
  const date = new Date(at - new Date(at).getTimezoneOffset() * 60_000);
  return date.toISOString().slice(0, 16);
}

function fromLocalInput(text: string): number | null {
  const at = new Date(text).getTime();
  return Number.isFinite(at) ? at : null;
}

export interface NewIncidentDialogProps {
  open: boolean;
  /** Whether this build has an alerting feature to open one from. */
  canUseAlerts: boolean;
  onClose: () => void;
  onOpened: (id: string) => void;
}

export function NewIncidentDialog({ open, canUseAlerts, onClose, onOpened }: NewIncidentDialogProps) {
  const [fromAlert, setFromAlert] = useState<string | null>(null);
  const [title, setTitle] = useState('');
  const [severity, setSeverity] = useState<IncidentSeverity>('sev3');
  const [startedAt, setStartedAt] = useState(() => toLocalInput(Date.now()));
  const [deviceIds, setDeviceIds] = useState<string[]>([]);
  const [summary, setSummary] = useState('');
  const [author, setAuthor] = useAuthor();

  const alerts = useOpenAlerts(open && canUseAlerts);
  const devices = useDevices();
  const create = useCreateIncident();

  const chosenAlert = alerts.data?.find((alert) => alert.id === fromAlert);
  // The alert decides the title, the time and the severity; typing them again
  // would only be a chance to disagree with the record.
  const needsTitle = fromAlert === null;
  const started = fromLocalInput(startedAt);
  const ready = (!needsTitle || title.trim() !== '') && (!needsTitle || started !== null);

  const submit = () => {
    create.mutate(
      fromAlert !== null
        ? { from_alert: fromAlert, author, summary: summary.trim(), device_ids: deviceIds }
        : {
            title: title.trim(),
            severity,
            started_at: started ?? Date.now(),
            device_ids: deviceIds,
            summary: summary.trim(),
            author,
          },
      { onSuccess: (incident) => onOpened(incident.id) },
    );
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Open an incident"
      description="An incident is a record with a timeline. Nothing is sent to a device by opening one."
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" icon={Plus} loading={create.isPending} disabled={!ready} onClick={submit}>
            Open it
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {canUseAlerts && (
          <Field
            label="From an open alert"
            hint={
              alerts.isError
                ? `The alerts could not be read: ${alerts.error.message} You can still open one by hand.`
                : 'Takes the alert’s title, time, severity and device, and puts its own history on the timeline.'
            }
          >
            <Combobox
              aria-label="Open alert"
              clearable
              value={fromAlert}
              onChange={setFromAlert}
              placeholder={alerts.isPending ? 'Reading the open alerts…' : 'Nothing, open it by hand'}
              emptyText="No open alert matches"
              options={(alerts.data ?? []).map((alert) => ({
                value: alert.id,
                label: `${alert.severity} · ${alert.title}${alert.device ? ` · ${alert.device}` : ''}`,
              }))}
            />
          </Field>
        )}

        {chosenAlert ? (
          <p className={styles.sentence}>
            It will be called <b>{chosenAlert.title}</b>, start at{' '}
            {formatTimestamp(chosenAlert.started_at)}, and take its severity from the alert.
          </p>
        ) : (
          <>
            <Field label="What is wrong" required>
              <Input
                value={title}
                autoFocus
                data-autofocus
                placeholder="What is wrong, in one line"
                onChange={(event) => setTitle(event.target.value)}
              />
            </Field>
            <div className={styles.formRow}>
              <Field label="How bad">
                <Select
                  value={severity}
                  options={SEVERITIES}
                  onChange={(event) => setSeverity(event.target.value as IncidentSeverity)}
                />
              </Field>
              <Field
                label="When it started"
                hint="The timeline is gathered around this, so put in the time the trouble began — not the time you opened the record."
                error={started === null ? 'That is not a time.' : undefined}
              >
                <Input
                  type="datetime-local"
                  value={startedAt}
                  onChange={(event) => setStartedAt(event.target.value)}
                />
              </Field>
            </div>
          </>
        )}

        <Field
          label="Devices"
          hint={
            devices.isError
              ? `The inventory could not be read: ${devices.error.message}`
              : 'Their alerts, changes and configuration movement are what the timeline gathers.'
          }
        >
          <div className={styles.form}>
            <Combobox
              aria-label="Add a device"
              value={null}
              onChange={(id) => {
                if (id && !deviceIds.includes(id)) setDeviceIds([...deviceIds, id]);
              }}
              placeholder="Add a device…"
              emptyText="No device matches"
              options={(devices.data ?? [])
                .filter((device) => !deviceIds.includes(device.id))
                .map((device) => ({
                  value: device.id,
                  label: device.site ? `${device.name} · ${device.site}` : device.name,
                }))}
            />
            {deviceIds.length > 0 && (
              <div className={styles.wrapRow}>
                {deviceIds.map((id) => {
                  const device = devices.data?.find((entry) => entry.id === id);
                  return (
                    <Button
                      key={id}
                      size="sm"
                      variant="secondary"
                      trailingIcon={X}
                      aria-label={`Remove ${device?.name ?? id}`}
                      onClick={() => setDeviceIds(deviceIds.filter((other) => other !== id))}
                    >
                      {device?.name ?? id}
                      {device?.lab ? <LabPill /> : null}
                    </Button>
                  );
                })}
              </div>
            )}
          </div>
        </Field>

        <Field label="What is happening" hint="Two lines are plenty. It can be filled in later.">
          <Textarea
            rows={3}
            value={summary}
            placeholder="Who cannot do what"
            onChange={(event) => setSummary(event.target.value)}
          />
        </Field>

        <Field
          label="Your name"
          hint="Goes on the timeline with everything you write. Remembered in this browser; Nocturne has no user directory."
        >
          <Input value={author} placeholder="who is opening this" onChange={(event) => setAuthor(event.target.value)} />
        </Field>

        {create.isError && <p className={styles.sentence}>{create.error.message}</p>}
      </div>
    </Dialog>
  );
}
