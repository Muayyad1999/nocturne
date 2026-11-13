/**
 * Changing many devices at once. Every field is "leave as it is" until its
 * checkbox is ticked, and the dialog says, before anything happens, exactly
 * what will change and on how many devices.
 */

import { useEffect, useState, type ReactNode } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  CountResult,
  Credential,
  DeviceBulkUpdateParams,
  ProfileList,
} from '@/api/types/core';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Combobox } from '@/design/Combobox';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { TagInput } from '@/design/TagInput';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import styles from './BulkEditDialog.module.css';

/** `null` means "leave this field as it is on every device". */
interface Edits {
  site: string | null;
  role: string | null;
  profile: string | null;
  credential_id: string | null;
  monitor_enabled: boolean | null;
  protected: boolean | null;
  add_tags: string[];
  remove_tags: string[];
}

const NOTHING: Edits = {
  site: null,
  role: null,
  profile: null,
  credential_id: null,
  monitor_enabled: null,
  protected: null,
  add_tags: [],
  remove_tags: [],
};

type TextKey = 'site' | 'role' | 'profile' | 'credential_id';
type FlagKey = 'monitor_enabled' | 'protected';

interface ArmedRowProps {
  label: string;
  armed: boolean;
  onArm: (armed: boolean) => void;
  children: ReactNode;
}

function ArmedRow({ label, armed, onArm, children }: ArmedRowProps) {
  return (
    <div className={styles.row}>
      <Checkbox checked={armed} onChange={(event) => onArm(event.target.checked)}>
        {label}
      </Checkbox>
      <div className={styles.control} data-off={!armed}>
        {children}
      </div>
    </div>
  );
}

/** One phrase per change, so the user reads what they are about to do. */
function describe(
  edits: Edits,
  profileName: (id: string) => string,
  credentialName: (id: string) => string,
): string[] {
  const lines: string[] = [];
  if (edits.site !== null) lines.push(edits.site === '' ? 'clear the site' : `set the site to “${edits.site}”`);
  if (edits.role !== null) lines.push(edits.role === '' ? 'clear the role' : `set the role to “${edits.role}”`);
  if (edits.profile !== null) {
    lines.push(
      edits.profile === ''
        ? 'detect the device type again on the next connection'
        : `set the device type to ${profileName(edits.profile)}`,
    );
  }
  if (edits.credential_id !== null) {
    lines.push(
      edits.credential_id === ''
        ? 'remove the credential'
        : `use the credential “${credentialName(edits.credential_id)}”`,
    );
  }
  if (edits.monitor_enabled !== null) {
    lines.push(edits.monitor_enabled ? 'turn monitoring on' : 'turn monitoring off');
  }
  if (edits.protected !== null) {
    lines.push(edits.protected ? 'mark them protected' : 'remove their protection');
  }
  if (edits.add_tags.length > 0) lines.push(`add the tags ${edits.add_tags.join(', ')}`);
  if (edits.remove_tags.length > 0) lines.push(`remove the tags ${edits.remove_tags.join(', ')}`);
  return lines;
}

export interface BulkEditDialogProps {
  open: boolean;
  onClose: () => void;
  ids: readonly string[];
}

export function BulkEditDialog({ open, onClose, ids }: BulkEditDialogProps) {
  const [edits, setEdits] = useState<Edits>(NOTHING);
  useEffect(() => {
    if (open) setEdits(NOTHING);
  }, [open]);

  const credentials = useRpc<Credential[]>('credentials.list', undefined, { enabled: open });
  const profiles = useRpc<ProfileList>('profiles.list', undefined, { enabled: open });
  const bulk = useRpcMutation<CountResult, DeviceBulkUpdateParams>('devices.bulk_update', {
    invalidates: ['devices.'],
    errorToast: 'The devices were not changed',
  });

  const profileName = (id: string) =>
    profiles.data?.profiles.find((profile) => profile.id === id)?.name ?? id;
  const credentialName = (id: string) =>
    credentials.data?.find((credential) => credential.id === id)?.name ?? id;

  const armText = (key: TextKey, armed: boolean) =>
    setEdits((state) => ({ ...state, [key]: armed ? '' : null }));
  const armFlag = (key: FlagKey, armed: boolean) =>
    setEdits((state) => ({ ...state, [key]: armed ? true : null }));

  const changes = describe(edits, profileName, credentialName);
  const count = formatCount(ids.length);
  const plural = ids.length === 1 ? '' : 's';

  const apply = async () => {
    const params: DeviceBulkUpdateParams = { ids: [...ids] };
    if (edits.site !== null) params.site = edits.site;
    if (edits.role !== null) params.role = edits.role;
    if (edits.profile !== null) params.profile = edits.profile;
    if (edits.credential_id !== null) params.credential_id = edits.credential_id;
    if (edits.monitor_enabled !== null) params.monitor_enabled = edits.monitor_enabled;
    if (edits.protected !== null) params.protected = edits.protected;
    if (edits.add_tags.length > 0) params.add_tags = edits.add_tags;
    if (edits.remove_tags.length > 0) params.remove_tags = edits.remove_tags;
    const result = await bulk.mutateAsync(params);
    toast.success(`Changed ${formatCount(result.count)} device${result.count === 1 ? '' : 's'}`);
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title={`Edit ${count} device${plural}`}
      description="Only the fields you tick are changed; everything else is left exactly as it is."
      dismissible={!bulk.isPending}
      footer={
        <>
          <Button onClick={onClose} disabled={bulk.isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            loading={bulk.isPending}
            disabled={changes.length === 0}
            onClick={() => void apply()}
          >
            Apply to {count} device{plural}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <ArmedRow label="Site" armed={edits.site !== null} onArm={(armed) => armText('site', armed)}>
          <Input
            aria-label="Site"
            disabled={edits.site === null}
            value={edits.site ?? ''}
            placeholder="Leave empty to clear it"
            onChange={(event) => setEdits({ ...edits, site: event.target.value })}
          />
        </ArmedRow>

        <ArmedRow label="Role" armed={edits.role !== null} onArm={(armed) => armText('role', armed)}>
          <Input
            aria-label="Role"
            disabled={edits.role === null}
            value={edits.role ?? ''}
            placeholder="Leave empty to clear it"
            onChange={(event) => setEdits({ ...edits, role: event.target.value })}
          />
        </ArmedRow>

        <ArmedRow
          label="Device type"
          armed={edits.profile !== null}
          onArm={(armed) => armText('profile', armed)}
        >
          <Combobox
            aria-label="Device type"
            disabled={edits.profile === null}
            options={[
              { value: '', label: 'Detect automatically' },
              ...(profiles.data?.profiles ?? []).map((profile) => ({
                value: profile.id,
                label: profile.name,
                description: profile.id,
              })),
            ]}
            value={edits.profile}
            onChange={(value) => setEdits({ ...edits, profile: value ?? '' })}
          />
        </ArmedRow>

        <ArmedRow
          label="Credential"
          armed={edits.credential_id !== null}
          onArm={(armed) => armText('credential_id', armed)}
        >
          <Combobox
            aria-label="Credential"
            disabled={edits.credential_id === null}
            options={[
              { value: '', label: 'No credential' },
              ...(credentials.data ?? []).map((credential) => ({
                value: credential.id,
                label: credential.name,
                description: credential.username || 'no user name',
              })),
            ]}
            value={edits.credential_id}
            onChange={(value) => setEdits({ ...edits, credential_id: value ?? '' })}
          />
        </ArmedRow>

        <ArmedRow
          label="Monitoring"
          armed={edits.monitor_enabled !== null}
          onArm={(armed) => armFlag('monitor_enabled', armed)}
        >
          <Select
            aria-label="Monitoring"
            disabled={edits.monitor_enabled === null}
            value={edits.monitor_enabled === false ? 'off' : 'on'}
            options={[
              { value: 'on', label: 'Monitored' },
              { value: 'off', label: 'Not monitored' },
            ]}
            onChange={(event) =>
              setEdits({ ...edits, monitor_enabled: event.target.value === 'on' })
            }
          />
        </ArmedRow>

        <ArmedRow
          label="Protected"
          armed={edits.protected !== null}
          onArm={(armed) => armFlag('protected', armed)}
        >
          <Select
            aria-label="Protected"
            disabled={edits.protected === null}
            value={edits.protected === false ? 'no' : 'yes'}
            options={[
              { value: 'yes', label: 'Protected from automation' },
              { value: 'no', label: 'Not protected' },
            ]}
            onChange={(event) => setEdits({ ...edits, protected: event.target.value === 'yes' })}
          />
        </ArmedRow>

        <div className={styles.tags}>
          <Field label="Add tags" hint="Added to whatever each device already has.">
            <TagInput
              value={edits.add_tags}
              onChange={(tags) => setEdits({ ...edits, add_tags: tags })}
              placeholder="critical"
            />
          </Field>
          <Field label="Remove tags" hint="Ignored on devices that do not have them.">
            <TagInput
              value={edits.remove_tags}
              onChange={(tags) => setEdits({ ...edits, remove_tags: tags })}
              placeholder="legacy"
            />
          </Field>
        </div>

        {changes.length > 0 ? (
          <p className={styles.summary}>
            This will {changes.join(', ')} on {count} device{plural}.
          </p>
        ) : (
          <p className={styles.nothing}>Tick a field to change it. Nothing is changed yet.</p>
        )}
      </div>
    </Dialog>
  );
}
