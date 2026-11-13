/**
 * Adding a device, and editing one. The same form either way: what Nocturne
 * needs to reach the device, who it logs in as, and where the device belongs.
 *
 * **Test connection** is the moment a user decides whether to trust the
 * product, so it is here rather than two screens away. `devices.test` works on
 * a device that exists, so a new device is saved first; the button says so.
 */

import { KeyRound, Plug, Plus } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  ConnectionTest,
  Credential,
  CredentialInput,
  DeviceInput,
  DeviceRow,
  DeviceUpdateParams,
  ProfileList,
  Transport,
} from '@/api/types/core';
import { Button } from '@/design/Button';
import { Combobox, type ComboboxOption } from '@/design/Combobox';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { toast } from '@/design/Toast';
import { defaultPort, TRANSPORT_OPTIONS } from './device';
import { TestResult, TestResultPending } from './TestResult';
import styles from './DeviceDrawer.module.css';

/** The form's own state: strings, because that is what the controls hold. */
interface FormState {
  name: string;
  address: string;
  transport: Transport;
  port: string;
  profile: string;
  credential_id: string;
  jump_device_id: string;
  site: string;
  role: string;
  tags: string[];
  notes: string;
  monitor_enabled: boolean;
  monitor_icmp: boolean;
  monitor_snmp: boolean;
  protected: boolean;
}

const BLANK: FormState = {
  name: '',
  address: '',
  transport: 'ssh',
  port: '',
  profile: '',
  credential_id: '',
  jump_device_id: '',
  site: '',
  role: '',
  tags: [],
  notes: '',
  monitor_enabled: true,
  monitor_icmp: true,
  monitor_snmp: false,
  protected: false,
};

function fromDevice(device: DeviceRow): FormState {
  return {
    name: device.name,
    address: device.address,
    transport: device.transport === 'sim' ? 'ssh' : device.transport,
    port: device.port == null ? '' : String(device.port),
    profile: device.profile,
    credential_id: device.credential_id ?? '',
    jump_device_id: device.jump_device_id ?? '',
    site: device.site,
    role: device.role,
    tags: device.tags,
    notes: device.notes,
    monitor_enabled: device.monitor.enabled,
    monitor_icmp: device.monitor.icmp,
    monitor_snmp: device.monitor.snmp,
    protected: device.protected,
  };
}

/** Only what the backend asked for: `port: 0` is how a port is cleared. */
function toInput(form: FormState, device: DeviceRow | null): DeviceInput {
  const monitor = device?.monitor;
  return {
    name: form.name.trim(),
    address: form.address.trim(),
    transport: form.transport,
    port: form.port.trim() === '' ? 0 : Number(form.port),
    profile: form.profile,
    credential_id: form.credential_id,
    jump_device_id: form.jump_device_id,
    site: form.site.trim(),
    role: form.role.trim(),
    tags: form.tags,
    notes: form.notes,
    protected: form.protected,
    monitor: {
      enabled: form.monitor_enabled,
      icmp: form.monitor_icmp,
      snmp: form.monitor_snmp,
      snmp_credential_id: monitor?.snmp_credential_id ?? null,
      snmp_port: monitor?.snmp_port ?? null,
      interval_s: monitor?.interval_s ?? 60,
    },
  };
}

export interface DeviceDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The device being edited, or null to add a new one. */
  device: DeviceRow | null;
  /** Every device, for the jump-host list. */
  devices: readonly DeviceRow[];
  /** Called with the id of a device that was just created. */
  onCreated?: (device: DeviceRow) => void;
}

export function DeviceDrawer({ open, onClose, device, devices, onCreated }: DeviceDrawerProps) {
  const [form, setForm] = useState<FormState>(BLANK);
  /** The device this form is bound to: set once a new device has been saved. */
  const [saved, setSaved] = useState<DeviceRow | null>(null);
  const [test, setTest] = useState<ConnectionTest | null>(null);
  const [newCredential, setNewCredential] = useState<CredentialInput | null>(null);
  /** The test result is below the fold on a long form: bring it into view. */
  const resultRef = useRef<HTMLDivElement>(null);
  /** And a refusal is usually at the top: bring that into view instead. */
  const formRef = useRef<HTMLDivElement>(null);

  const target = device ?? saved;
  const editing = target !== null;

  // The form is filled when the drawer opens, and cleared when it closes, so
  // a second "Add device" never shows the previous device's details.
  useEffect(() => {
    if (!open) return;
    setForm(device ? fromDevice(device) : BLANK);
    setSaved(null);
    setTest(null);
    setNewCredential(null);
  }, [open, device]);

  const credentials = useRpc<Credential[]>('credentials.list', undefined, {
    enabled: open,
    invalidateOn: ['credentials.changed'],
  });
  const profiles = useRpc<ProfileList>('profiles.list', undefined, {
    enabled: open,
    invalidateOn: ['profiles.changed'],
  });

  const create = useRpcMutation<DeviceRow, DeviceInput>('devices.create', {
    invalidates: ['devices.'],
    errorToast: false,
  });
  const update = useRpcMutation<DeviceRow, DeviceUpdateParams>('devices.update', {
    invalidates: ['devices.'],
    errorToast: false,
  });
  const createCredential = useRpcMutation<Credential, CredentialInput>('credentials.create', {
    invalidates: ['credentials.'],
    errorToast: false,
  });
  const runTest = useRpcMutation<ConnectionTest, { id: string }>('devices.test', {
    errorToast: 'The test could not be run',
  });

  const saving = create.isPending || update.isPending;
  const error = create.error ?? update.error;

  const profileOptions: ComboboxOption[] = [
    { value: '', label: 'Detect automatically', description: 'On the first connection' },
    ...(profiles.data?.profiles ?? []).map((profile) => ({
      value: profile.id,
      label: profile.name,
      description: profile.id,
    })),
  ];
  const profileName = (id: string) =>
    profiles.data?.profiles.find((profile) => profile.id === id)?.name ?? id;

  const credentialOptions: ComboboxOption[] = (credentials.data ?? []).map((credential) => ({
    value: credential.id,
    label: credential.name,
    description: credential.username || 'no user name',
  }));

  const jumpOptions: ComboboxOption[] = devices
    .filter((candidate) => candidate.id !== target?.id && candidate.transport === 'ssh')
    .map((candidate) => ({
      value: candidate.id,
      label: candidate.name,
      description: candidate.address,
    }));

  /** Saves the device and returns it, or null when the backend refused. */
  const save = async (): Promise<DeviceRow | null> => {
    const input = toInput(form, target);
    try {
      if (target) return await update.mutateAsync({ id: target.id, ...input });
      const created = await create.mutateAsync(input);
      setSaved(created);
      onCreated?.(created);
      return created;
    } catch {
      // The message is shown under the field it belongs to, which may be
      // above the fold on a long form.
      formRef.current?.scrollIntoView({ block: 'start', behavior: 'smooth' });
      return null;
    }
  };

  const onSave = async () => {
    const result = await save();
    if (!result) return;
    toast.success(target ? `Saved ${result.name}` : `Added ${result.name}`);
    onClose();
  };

  const onTest = async () => {
    const subject = target ?? (await save());
    if (!subject) return;
    setTest(null);
    const result = await runTest.mutateAsync({ id: subject.id });
    setTest(result);
    requestAnimationFrame(() =>
      resultRef.current?.scrollIntoView({ block: 'center', behavior: 'smooth' }),
    );
  };

  const onCreateCredential = async () => {
    if (!newCredential?.name?.trim()) return;
    try {
      const created = await createCredential.mutateAsync(newCredential);
      setForm((state) => ({ ...state, credential_id: created.id }));
      setNewCredential(null);
      toast.success(`Credential “${created.name}” saved`);
    } catch {
      // Shown under the credential name field.
    }
  };

  const monitorOnly = form.transport === 'none';
  const serial = form.transport === 'serial';

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="lg"
      title={device ? `Edit ${device.name}` : saved ? `Edit ${saved.name}` : 'Add device'}
      description={
        editing
          ? 'Changes take effect on the next connection.'
          : 'Nocturne connects to it as soon as it is saved — nothing is changed on the device.'
      }
      footer={
        <>
          <Button onClick={onClose} disabled={saving}>
            {editing ? 'Close' : 'Cancel'}
          </Button>
          <Button variant="primary" loading={saving} onClick={() => void onSave()}>
            {editing ? 'Save changes' : 'Add device'}
          </Button>
        </>
      }
    >
      <div className={styles.form} ref={formRef}>
        <section className={styles.section}>
          <h4 className={styles.sectionTitle}>How to reach it</h4>
          <Field
            label="Name"
            hint="What you will call it here. Left empty, the address is used."
            error={error?.code === 'conflict' ? error.message : undefined}
          >
            <Input
              data-autofocus
              value={form.name}
              placeholder="core-sw1"
              onChange={(event) => setForm({ ...form, name: event.target.value })}
            />
          </Field>

          <div className={styles.row}>
            <Field
              label={serial ? 'Serial port' : 'Address'}
              required={!monitorOnly}
              hint={serial ? 'COM3 or /dev/ttyUSB0' : 'Hostname or IP address'}
              error={
                error && error.code === 'invalid' && /address|port/i.test(error.message)
                  ? error.message
                  : undefined
              }
            >
              <Input
                mono
                value={form.address}
                placeholder={serial ? '/dev/ttyUSB0' : '10.0.0.1'}
                onChange={(event) => setForm({ ...form, address: event.target.value })}
              />
            </Field>
            <Field label={serial ? 'Baud rate' : 'Port'} hint="Default">
              <Input
                mono
                inputMode="numeric"
                value={form.port}
                placeholder={defaultPort(form.transport)}
                onChange={(event) => setForm({ ...form, port: event.target.value })}
              />
            </Field>
          </div>

          <Field
            label="Connection"
            hint={TRANSPORT_OPTIONS.find((option) => option.value === form.transport)?.hint}
          >
            <Select
              value={form.transport}
              options={TRANSPORT_OPTIONS.map(({ value, label }) => ({ value, label }))}
              onChange={(event) =>
                setForm({ ...form, transport: event.target.value as FormState['transport'] })
              }
            />
          </Field>

          <Field
            label="Device type"
            hint="Left on “Detect automatically”, Nocturne works it out on the first connection and remembers it."
          >
            <Combobox
              options={profileOptions}
              value={form.profile}
              onChange={(value) => setForm({ ...form, profile: value ?? '' })}
              placeholder="Detect automatically"
            />
          </Field>
        </section>

        {!monitorOnly && (
          <section className={styles.section}>
            <h4 className={styles.sectionTitle}>How to log in</h4>
            {newCredential === null ? (
              <Field
                label="Credential"
                hint={
                  credentials.data?.length === 0
                    ? 'No credential exists yet. Create one here, or under Settings → Credentials.'
                    : 'The stored login Nocturne uses for this device.'
                }
              >
                <div className={styles.testBar}>
                  <div className={styles.grow}>
                    <Combobox
                      clearable
                      options={credentialOptions}
                      value={form.credential_id === '' ? null : form.credential_id}
                      onChange={(value) => setForm({ ...form, credential_id: value ?? '' })}
                      placeholder="No credential"
                      emptyText="No credential matches"
                    />
                  </div>
                  <Button
                    size="sm"
                    icon={Plus}
                    onClick={() => setNewCredential({ name: '', username: '', password: '' })}
                  >
                    New credential
                  </Button>
                </div>
              </Field>
            ) : (
              <div className={styles.inline}>
                <div className={styles.inlineHead}>
                  <h5 className={styles.inlineTitle}>New credential</h5>
                  <span className={styles.testHint}>
                    Secrets go straight into the vault and are never shown again.
                  </span>
                </div>
                <Field label="Name" required error={createCredential.error?.message}>
                  <Input
                    value={newCredential.name ?? ''}
                    placeholder="Campus switches"
                    onChange={(event) =>
                      setNewCredential({ ...newCredential, name: event.target.value })
                    }
                  />
                </Field>
                <div className={styles.pair}>
                  <Field label="User name">
                    <Input
                      mono
                      autoComplete="off"
                      value={newCredential.username ?? ''}
                      onChange={(event) =>
                        setNewCredential({ ...newCredential, username: event.target.value })
                      }
                    />
                  </Field>
                  <Field label="Password">
                    <Input
                      type="password"
                      autoComplete="new-password"
                      value={newCredential.password ?? ''}
                      onChange={(event) =>
                        setNewCredential({ ...newCredential, password: event.target.value })
                      }
                    />
                  </Field>
                </div>
                <div className={styles.inlineActions}>
                  <Button size="sm" onClick={() => setNewCredential(null)}>
                    Cancel
                  </Button>
                  <Button
                    size="sm"
                    variant="primary"
                    icon={KeyRound}
                    loading={createCredential.isPending}
                    onClick={() => void onCreateCredential()}
                  >
                    Save credential
                  </Button>
                </div>
              </div>
            )}

            <Field
              label="Jump host"
              hint="Reach this device through another one over SSH. Up to five hops."
            >
              <Combobox
                clearable
                options={jumpOptions}
                value={form.jump_device_id === '' ? null : form.jump_device_id}
                onChange={(value) => setForm({ ...form, jump_device_id: value ?? '' })}
                placeholder="Connect directly"
                emptyText="No other SSH device"
              />
            </Field>
          </section>
        )}

        <section className={styles.section}>
          <h4 className={styles.sectionTitle}>Where it belongs</h4>
          <div className={styles.pair}>
            <Field label="Site" hint="Building, campus or data centre.">
              <Input
                value={form.site}
                placeholder="HQ"
                onChange={(event) => setForm({ ...form, site: event.target.value })}
              />
            </Field>
            <Field label="Role" hint="core, distribution, access, edge, firewall…">
              <Input
                value={form.role}
                placeholder="access"
                onChange={(event) => setForm({ ...form, role: event.target.value })}
              />
            </Field>
          </div>
          <Field label="Tags" hint="Enter or a comma ends a tag.">
            <TagInput
              value={form.tags}
              onChange={(tags) => setForm({ ...form, tags })}
              placeholder="critical, l3"
            />
          </Field>
        </section>

        <section className={styles.section}>
          <h4 className={styles.sectionTitle}>Monitoring and safety</h4>
          <div className={styles.switches}>
            <div className={styles.switchRow}>
              <Switch
                checked={form.monitor_enabled}
                onChange={(event) => setForm({ ...form, monitor_enabled: event.target.checked })}
              >
                Monitor this device
              </Switch>
              <p className={styles.switchHint}>
                Check regularly that it answers, and keep its history.
              </p>
            </div>
            <div className={styles.switchRow}>
              <Switch
                checked={form.monitor_icmp}
                disabled={!form.monitor_enabled}
                onChange={(event) => setForm({ ...form, monitor_icmp: event.target.checked })}
              >
                Reachability by ping
              </Switch>
            </div>
            <div className={styles.switchRow}>
              <Switch
                checked={form.monitor_snmp}
                disabled={!form.monitor_enabled}
                onChange={(event) => setForm({ ...form, monitor_snmp: event.target.checked })}
              >
                Interface counters and health over SNMP
              </Switch>
              <p className={styles.switchHint}>
                Needs a credential with an SNMP community or a v3 user.
              </p>
            </div>
            <div className={styles.switchRow}>
              <Switch
                checked={form.protected}
                onChange={(event) => setForm({ ...form, protected: event.target.checked })}
              >
                Protected
              </Switch>
              <p className={styles.switchHint}>
                Nothing automated and nothing the assistant proposes is applied to it without a
                person approving that change.
              </p>
            </div>
          </div>
        </section>

        {!monitorOnly && (
          <section className={styles.section}>
            <h4 className={styles.sectionTitle}>Check it works</h4>
            <div className={styles.testBar}>
              <Button
                icon={Plug}
                loading={runTest.isPending || saving}
                onClick={() => void onTest()}
                disabled={form.address.trim() === ''}
              >
                {editing ? 'Test connection' : 'Save and test connection'}
              </Button>
              <p className={styles.testHint}>
                {editing
                  ? 'Logs in, reads the prompt, and disconnects. Nothing is changed on the device.'
                  : 'The device is added first, then Nocturne logs in, reads the prompt and disconnects.'}
              </p>
            </div>
            <div ref={resultRef}>
              {runTest.isPending && <TestResultPending />}
              {!runTest.isPending && test && <TestResult result={test} profileName={profileName} />}
            </div>
          </section>
        )}

        {error && error.code !== 'conflict' && !/address|port/i.test(error.message) && (
          <p className={styles.error}>{error.message}</p>
        )}
      </div>
    </Drawer>
  );
}
