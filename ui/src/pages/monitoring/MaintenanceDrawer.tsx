/**
 * The maintenance-window editor. A window does not stop alerts being raised
 * — they are still recorded, and the Alerts tab marks them "Maintenance" —
 * it stops them being sent to a channel. The drawer says so, because the two
 * are easy to confuse and only one of them is true.
 */

import { Save } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type { MaintenanceSaveParams, MaintenanceWindow } from '@/api/types/monitor';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { formatDuration } from '@/lib/format';
import { fromLocalInput, toLocalInput } from './datetime';
import styles from './Monitoring.module.css';

const HOUR = 3_600_000;

export interface MaintenanceDrawerProps {
  window: MaintenanceWindow | null;
  onClose: () => void;
}

export function MaintenanceDrawer({ window: existing, onClose }: MaintenanceDrawerProps) {
  const now = Date.now();
  const [name, setName] = useState(existing?.name ?? '');
  const [starts, setStarts] = useState(toLocalInput(existing?.starts_at ?? now + HOUR));
  const [ends, setEnds] = useState(toLocalInput(existing?.ends_at ?? now + 3 * HOUR));
  const [weekly, setWeekly] = useState(existing?.recurring === 'weekly');
  const [note, setNote] = useState(existing?.note ?? '');
  const [sites, setSites] = useState<string[]>(existing?.scope.sites ?? []);
  const [roles, setRoles] = useState<string[]>(existing?.scope.roles ?? []);
  const [tags, setTags] = useState<string[]>(existing?.scope.tags ?? []);
  const [includeLab, setIncludeLab] = useState(existing?.scope.include_lab ?? false);

  const save = useRpcMutation<MaintenanceWindow, MaintenanceSaveParams>('maintenance.save', {
    invalidates: ['maintenance.'],
    successToast: existing ? 'Window saved' : 'Window created',
    errorToast: false,
  });

  const startsAt = fromLocalInput(starts);
  const endsAt = fromLocalInput(ends);
  const length = startsAt != null && endsAt != null ? endsAt - startsAt : null;
  const problem =
    startsAt == null || endsAt == null
      ? 'Give the window a start and an end.'
      : length != null && length <= 0
        ? 'A window has to end after it starts.'
        : weekly && length != null && length >= 7 * 24 * HOUR
          ? 'A weekly window has to be shorter than a week, or it never closes.'
          : null;

  return (
    <Drawer
      open
      onClose={onClose}
      title={existing ? `Edit ${existing.name}` : 'New maintenance window'}
      description="Alerts raised inside the window are recorded as usual, and marked; they are not sent to a notification channel."
      footer={
        <Button
          variant="primary"
          icon={Save}
          loading={save.isPending}
          disabled={name.trim() === '' || problem !== null}
          onClick={() =>
            save.mutate(
              {
                ...(existing ? { id: existing.id } : {}),
                name: name.trim(),
                starts_at: startsAt ?? 0,
                ends_at: endsAt ?? 0,
                recurring: weekly ? 'weekly' : null,
                note: note.trim(),
                scope: { sites, roles, tags, include_lab: includeLab },
              },
              { onSuccess: onClose },
            )
          }
        >
          Save
        </Button>
      }
    >
      <div className={styles.form}>
        {save.error && <p className={styles.error}>{save.error.message}</p>}

        <Field label="Name" required>
          <Input
            aria-label="Name"
            placeholder="Saturday patching"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>

        <div className={styles.row}>
          <Field label="Starts">
            <Input
              aria-label="Starts"
              type="datetime-local"
              value={starts}
              onChange={(event) => setStarts(event.target.value)}
            />
          </Field>
          <Field
            label="Ends"
            error={problem ?? undefined}
            hint={length ? `Lasts ${formatDuration(length)}.` : undefined}
          >
            <Input
              aria-label="Ends"
              type="datetime-local"
              value={ends}
              onChange={(event) => setEnds(event.target.value)}
            />
          </Field>
        </div>

        <Switch checked={weekly} onChange={(event) => setWeekly(event.target.checked)}>
          Repeat every week
        </Switch>

        <Field label="Only these sites" hint="Empty means every device. The same for roles and tags.">
          <TagInput aria-label="Only these sites" value={sites} onChange={setSites} />
        </Field>
        <div className={styles.row}>
          <Field label="Only these roles">
            <TagInput aria-label="Only these roles" value={roles} onChange={setRoles} />
          </Field>
          <Field label="Only these tags">
            <TagInput aria-label="Only these tags" value={tags} onChange={setTags} />
          </Field>
        </div>

        <Switch checked={includeLab} onChange={(event) => setIncludeLab(event.target.checked)}>
          Include simulated lab devices
        </Switch>

        <Field label="Note" hint="Why the work is happening, for whoever reads the alert afterwards.">
          <Textarea rows={2} aria-label="Note" value={note} onChange={(event) => setNote(event.target.value)} />
        </Field>
      </div>
    </Drawer>
  );
}
