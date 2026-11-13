/**
 * Choosing the one device a view is about.
 *
 * The options come from `config.devices`, so what is on offer is exactly
 * what this feature knows about — and a device with nothing stored is
 * offered too, with "never captured" next to its name. Leaving it out would
 * hide precisely the devices somebody needs to go and back up.
 */

import type { Coverage } from '@/api/types/config';
import { Combobox } from '@/design/Combobox';
import { captureState } from './vocab';

export interface DevicePickerProps {
  coverage: Coverage[] | undefined;
  value: string | null;
  onChange: (deviceId: string | null) => void;
  label?: string;
}

export function DevicePicker({ coverage, value, onChange, label = 'Device' }: DevicePickerProps) {
  const options = (coverage ?? [])
    .map((row) => {
      const state = captureState(row);
      const note =
        state === 'never'
          ? 'never captured'
          : state === 'failed'
            ? 'could not be read'
            : state === 'skipped'
              ? 'not backed up'
              : `${row.versions} stored`;
      return {
        value: row.device_id,
        label: `${row.name}${row.lab ? ' · LAB' : ''} — ${note}`,
      };
    })
    .sort((a, b) => a.label.localeCompare(b.label));

  return (
    <Combobox
      size="sm"
      aria-label={label}
      placeholder="Choose a device"
      emptyText="No device of that name"
      clearable
      options={options}
      value={value}
      onChange={onChange}
    />
  );
}
