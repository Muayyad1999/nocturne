/**
 * The filter chips above the device list. Each chip is a menu of the values
 * that actually exist, with the number of devices behind each, so the list
 * can be narrowed without typing and without guessing what is there.
 */

import { Check, ChevronDown, X } from 'lucide-react';
import { Button } from '@/design/Button';
import { DropdownMenu } from '@/design/DropdownMenu';
import type { MenuEntry } from '@/design/Menu';
import { formatCount } from '@/lib/format';
import type { FacetCount, Facets } from '@/api/types/core';
import type { DeviceFilterState } from './device';
import styles from './DeviceList.module.css';

interface ChipProps {
  label: string;
  value: string | null;
  values: readonly FacetCount[];
  onSelect: (value: string | null) => void;
  /** How a value reads in the chip and the menu (a profile id becomes its name). */
  display?: (value: string) => string;
}

function Chip({ label, value, values, onSelect, display }: ChipProps) {
  const show = (name: string) => display?.(name) ?? name;
  const items: MenuEntry[] = values.map(([name, count]) => ({
    label: `${show(name)} · ${formatCount(count)}`,
    checked: value?.toLowerCase() === name.toLowerCase(),
    onSelect: () => onSelect(value?.toLowerCase() === name.toLowerCase() ? null : name),
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
          : [{ type: 'label', label: `No ${label.toLowerCase()} is set on any device` }]
      }
      trigger={
        <Button
          size="sm"
          variant={value === null ? 'ghost' : 'secondary'}
          trailingIcon={ChevronDown}
          icon={value === null ? undefined : Check}
        >
          {value === null ? label : `${label}: ${show(value)}`}
        </Button>
      }
    />
  );
}

/** How many devices on screen are up, down and not yet checked. */
export interface StatusCounts {
  up: number;
  down: number;
  unknown: number;
}

const STATUS_LABEL: Record<string, string> = { up: 'Up', down: 'Down', unknown: 'Not checked' };

export interface DeviceFacetsProps {
  facets: Facets | undefined;
  /** Counted from the devices themselves: the backend has no status facet. */
  statusCounts: StatusCounts;
  filter: DeviceFilterState;
  onChange: (change: Partial<DeviceFilterState>) => void;
  /** Turns a device-type id into the name its profile gives it. */
  profileName: (id: string) => string;
}

/** Site, role, tag, device type and status, in that order. */
export function DeviceFacets({
  facets,
  statusCounts,
  filter,
  onChange,
  profileName,
}: DeviceFacetsProps) {
  const statuses: FacetCount[] = [
    ['up', statusCounts.up],
    ['down', statusCounts.down],
    ['unknown', statusCounts.unknown],
  ];

  return (
    <div className={styles.chips} role="group" aria-label="Filter devices">
      <Chip
        label="Site"
        value={filter.site}
        values={facets?.sites ?? []}
        onSelect={(value) => onChange({ site: value })}
      />
      <Chip
        label="Role"
        value={filter.role}
        values={facets?.roles ?? []}
        onSelect={(value) => onChange({ role: value })}
      />
      <Chip
        label="Tag"
        value={filter.tag}
        values={facets?.tags ?? []}
        onSelect={(value) => onChange({ tag: value })}
      />
      <Chip
        label="Type"
        value={filter.profile}
        values={facets?.profiles ?? []}
        onSelect={(value) => onChange({ profile: value })}
        display={profileName}
      />
      <Chip
        label="Status"
        value={filter.status}
        values={statuses}
        onSelect={(value) => onChange({ status: value as DeviceFilterState['status'] })}
        display={(value) => STATUS_LABEL[value] ?? value}
      />
    </div>
  );
}
