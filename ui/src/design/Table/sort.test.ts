import { describe, expect, it } from 'vitest';
import { compareIp } from '@/lib/net';
import { nextSort, sortRows, type SortableColumn } from './sort';

interface Device {
  name: string;
  address: string;
  uptime: number | null;
}

const devices: Device[] = [
  { name: 'access-10', address: '10.0.0.10', uptime: 500 },
  { name: 'access-2', address: '10.0.0.9', uptime: null },
  { name: 'core-1', address: '9.0.0.1', uptime: 90_000 },
  { name: 'Access-1', address: '2001:db8::1', uptime: 12 },
];

const columns: SortableColumn<Device>[] = [
  { id: 'name', sortValue: (device) => device.name },
  { id: 'address', compare: (a, b) => compareIp(a.address, b.address) },
  { id: 'uptime', sortValue: (device) => device.uptime },
  { id: 'notes' },
];

const names = (rows: readonly Device[]) => rows.map((device) => device.name);

describe('sortRows', () => {
  it('returns the rows untouched without a sort', () => {
    expect(sortRows(devices, columns, null)).toBe(devices);
  });

  it('ignores a column that cannot be sorted or does not exist', () => {
    expect(sortRows(devices, columns, { columnId: 'notes', direction: 'asc' })).toBe(devices);
    expect(sortRows(devices, columns, { columnId: 'gone', direction: 'asc' })).toBe(devices);
  });

  it('sorts text naturally and case-insensitively', () => {
    expect(names(sortRows(devices, columns, { columnId: 'name', direction: 'asc' }))).toEqual([
      'Access-1',
      'access-2',
      'access-10',
      'core-1',
    ]);
  });

  it('sorts numbers numerically and reverses on descending', () => {
    expect(names(sortRows(devices, columns, { columnId: 'uptime', direction: 'desc' }))).toEqual([
      'core-1',
      'access-10',
      'Access-1',
      'access-2',
    ]);
  });

  it('keeps missing values last in both directions', () => {
    expect(names(sortRows(devices, columns, { columnId: 'uptime', direction: 'asc' })).at(-1)).toBe('access-2');
    expect(names(sortRows(devices, columns, { columnId: 'uptime', direction: 'desc' })).at(-1)).toBe('access-2');
  });

  it('uses a custom comparator', () => {
    expect(names(sortRows(devices, columns, { columnId: 'address', direction: 'asc' }))).toEqual([
      'core-1',
      'access-2',
      'access-10',
      'Access-1',
    ]);
  });

  it('does not mutate its input', () => {
    const before = [...devices];
    sortRows(devices, columns, { columnId: 'name', direction: 'desc' });
    expect(devices).toEqual(before);
  });
});

describe('nextSort', () => {
  it('cycles ascending, descending, unsorted', () => {
    const first = nextSort(null, 'name');
    expect(first).toEqual({ columnId: 'name', direction: 'asc' });
    const second = nextSort(first, 'name');
    expect(second).toEqual({ columnId: 'name', direction: 'desc' });
    expect(nextSort(second, 'name')).toBeNull();
  });

  it('starts ascending on a different column', () => {
    expect(nextSort({ columnId: 'name', direction: 'desc' }, 'uptime')).toEqual({
      columnId: 'uptime',
      direction: 'asc',
    });
  });
});
