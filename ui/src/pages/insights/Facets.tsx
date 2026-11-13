/**
 * The filter chips above the findings: severity, category, device, site and
 * check. Each chip is a menu of the values that actually have findings right
 * now, with the number behind each, so the list can be narrowed without
 * typing and without guessing what is there.
 *
 * Severity and category counts come from `insights.list`'s own `counts`, which
 * count everything that matches the filter rather than what fits on the page.
 * Device, site and check are counted from the findings on screen, and the
 * footer of the list says so when there are more than fit.
 */

import { Check, ChevronDown, X } from 'lucide-react';
import { Button } from '@/design/Button';
import { DropdownMenu } from '@/design/DropdownMenu';
import type { MenuEntry } from '@/design/Menu';
import { formatCount } from '@/lib/format';
import styles from './Insights.module.css';

export interface FacetValue {
  value: string;
  label: string;
  count: number;
}

export interface ChipProps {
  label: string;
  /** The chosen value, or null for "any". */
  value: string | null;
  values: readonly FacetValue[];
  onSelect: (value: string | null) => void;
}

export function Chip({ label, value, values, onSelect }: ChipProps) {
  const chosen = values.find((entry) => entry.value === value);
  const items: MenuEntry[] = values.map((entry) => ({
    label: `${entry.label} · ${formatCount(entry.count)}`,
    checked: entry.value === value,
    disabled: entry.count === 0 && entry.value !== value,
    onSelect: () => onSelect(entry.value === value ? null : entry.value),
  }));
  if (value !== null) {
    items.unshift(
      { label: `Clear ${label.toLowerCase()}`, icon: X, onSelect: () => onSelect(null) },
      { type: 'separator' },
    );
  }

  return (
    <DropdownMenu
      aria-label={`Filter by ${label.toLowerCase()}`}
      items={
        items.length > 0
          ? items
          : [{ type: 'label', label: `Nothing on screen has a ${label.toLowerCase()}` }]
      }
      trigger={
        <Button
          size="sm"
          variant={value === null ? 'ghost' : 'secondary'}
          trailingIcon={ChevronDown}
          icon={value === null ? undefined : Check}
          className={styles.chip}
        >
          {value === null ? label : `${label}: ${chosen?.label ?? value}`}
        </Button>
      }
    />
  );
}
