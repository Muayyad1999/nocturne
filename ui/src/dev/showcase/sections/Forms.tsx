import { Globe, KeyRound } from 'lucide-react';
import { useState } from 'react';
import { Checkbox } from '@/design/Checkbox';
import { Combobox } from '@/design/Combobox';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { RadioGroup } from '@/design/RadioGroup';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Slider } from '@/design/Slider';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { formatDuration } from '@/lib/format';
import { parseIp } from '@/lib/net';
import { Example, Section } from '../parts';
import { sampleDevices } from '../sample';
import styles from '../Showcase.module.css';

const DEVICE_OPTIONS = sampleDevices(40).map((device) => ({
  value: device.id,
  label: device.name,
  description: device.site,
}));

const PLATFORMS = [
  { value: 'ios-xe', label: 'Cisco IOS-XE' },
  { value: 'routeros', label: 'MikroTik RouterOS' },
  { value: 'eos', label: 'Arista EOS' },
  { value: 'junos', label: 'Juniper Junos', disabled: true },
];

export function Forms() {
  const [name, setName] = useState('core-sw-01');
  const [address, setAddress] = useState('192.0.2.300');
  const [platform, setPlatform] = useState('ios-xe');
  const [device, setDevice] = useState<string | null>('dev-3');
  const [query, setQuery] = useState('core');
  const [tags, setTags] = useState(['192.0.2.10', '192.0.2.11']);
  const [monitored, setMonitored] = useState(true);
  const [notify, setNotify] = useState(false);
  const [checks, setChecks] = useState({ ping: true, snmp: true, ssh: false });
  const [protocol, setProtocol] = useState<string | null>('ssh');
  const [interval, setInterval] = useState(60);
  const allChecks = Object.values(checks).every(Boolean);
  const someChecks = Object.values(checks).some(Boolean);

  return (
    <>
      <Section
        title="Field"
        description="Label, hint and error around one control, wired together for assistive technology. An error replaces the hint and says how to fix the value."
      >
        <Example title="With a hint" layout="column">
          <Field label="Hostname" hint="Shown in tables and on the map." required>
            <Input value={name} onChange={(event) => setName(event.target.value)} />
          </Field>
        </Example>
        <Example title="With an error" layout="column">
          <Field
            label="Management address"
            hint="IPv4 or IPv6."
            error={parseIp(address) ? undefined : 'That is not a valid address. Example: 192.0.2.10'}
          >
            <Input mono value={address} onChange={(event) => setAddress(event.target.value)} />
          </Field>
        </Example>
        <Example title="Disabled" layout="column">
          <Field label="Serial number" hint="Read from the device.">
            <Input mono value="FOC2231X0AB" disabled readOnly />
          </Field>
        </Example>
      </Section>

      <Section title="Input, Textarea, SearchInput">
        <Example title="Sizes and adornments" layout="column">
          <Input aria-label="Default" placeholder="Default (28px)" />
          <Input aria-label="Small" size="sm" placeholder="Small (24px)" />
          <Input aria-label="URL" leading={<Icon icon={Globe} size={14} />} placeholder="status.example.net" />
          <Input aria-label="Timeout" type="number" defaultValue={30} trailing="seconds" />
          <Input aria-label="Secret" type="password" leading={<Icon icon={KeyRound} size={14} />} defaultValue="hunter2-not-real" />
        </Example>
        <Example title="Textarea" layout="column">
          <Textarea aria-label="Notes" placeholder="Notes for the next shift" />
          <Textarea aria-label="Commands" mono rows={4} defaultValue={'interface Gi1/0/3\n description camera lobby\n no shutdown'} />
        </Example>
        <Example title="SearchInput: Escape clears" layout="column">
          <SearchInput aria-label="Search devices" value={query} onChange={setQuery} />
          <SearchInput aria-label="Search interfaces" size="sm" value="" onChange={() => undefined} />
        </Example>
      </Section>

      <Section title="Select, Combobox, TagInput">
        <Example title="Select: native list" layout="column">
          <Select aria-label="Platform" value={platform} onChange={(event) => setPlatform(event.target.value)} options={PLATFORMS} />
          <Select aria-label="Platform, small" size="sm" options={PLATFORMS} defaultValue="eos" />
          <Select aria-label="Platform, disabled" options={PLATFORMS} disabled />
        </Example>
        <Example title="Combobox: type to filter" layout="column">
          <Field label="Device">
            <Combobox options={DEVICE_OPTIONS} value={device} onChange={setDevice} placeholder="Choose a device" clearable />
          </Field>
          <Combobox aria-label="Device, disabled" options={DEVICE_OPTIONS} value="dev-1" onChange={() => undefined} disabled />
        </Example>
        <Example title="TagInput: Enter or comma adds, paste a list" layout="column">
          <Field label="Syslog servers" hint="Try adding something that is not an address.">
            <TagInput
              mono
              value={tags}
              onChange={setTags}
              placeholder="Add an address"
              validate={(tag) => (parseIp(tag) ? null : `“${tag}” is not an IP address.`)}
            />
          </Field>
        </Example>
      </Section>

      <Section title="Checkbox, Switch, RadioGroup, Slider">
        <Example title="Checkbox" layout="column">
          <Checkbox
            checked={allChecks}
            indeterminate={someChecks && !allChecks}
            onChange={() => setChecks({ ping: !allChecks, snmp: !allChecks, ssh: !allChecks })}
          >
            All checks
          </Checkbox>
          <div className={styles.stack} style={{ paddingLeft: 24 }}>
            {(['ping', 'snmp', 'ssh'] as const).map((key) => (
              <Checkbox key={key} checked={checks[key]} onChange={() => setChecks({ ...checks, [key]: !checks[key] })}>
                {key.toUpperCase()}
              </Checkbox>
            ))}
          </div>
          <Checkbox disabled>Disabled</Checkbox>
          <Checkbox disabled defaultChecked>
            Disabled, checked
          </Checkbox>
        </Example>
        <Example title="Switch: takes effect at once" layout="column">
          <Switch checked={monitored} onChange={(event) => setMonitored(event.target.checked)}>
            Monitor this device
          </Switch>
          <Switch checked={notify} onChange={(event) => setNotify(event.target.checked)}>
            Notify on change
          </Switch>
          <Switch disabled>Disabled</Switch>
          <Switch disabled defaultChecked>
            Disabled, on
          </Switch>
        </Example>
        <Example title="RadioGroup and Slider" layout="column">
          <Field label="Connect with">
            <RadioGroup
              value={protocol}
              onChange={setProtocol}
              options={[
                { value: 'ssh', label: 'SSH', description: 'Port 22, key or password' },
                { value: 'telnet', label: 'Telnet', description: 'Unencrypted: lab use only' },
                { value: 'serial', label: 'Serial console', disabled: true },
              ]}
            />
          </Field>
          <Field label="Polling interval">
            <Slider
              min={10}
              max={300}
              step={10}
              value={interval}
              onChange={setInterval}
              formatValue={(seconds) => formatDuration(seconds * 1000)}
            />
          </Field>
        </Example>
      </Section>
    </>
  );
}
