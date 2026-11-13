/** How far back the charts under it look. */

import { Select } from '@/design/Select';
import styles from './Monitoring.module.css';
import { RANGES } from './series';

export interface RangePickerProps {
  value: string;
  onChange: (id: string) => void;
  label?: string;
}

export function RangePicker({ value, onChange, label = 'Time range' }: RangePickerProps) {
  return (
    <Select
      size="sm"
      className={styles.control}
      aria-label={label}
      options={RANGES.map((entry) => ({ value: entry.id, label: entry.label }))}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}
