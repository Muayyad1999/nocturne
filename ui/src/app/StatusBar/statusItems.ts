/**
 * The status bar's slots. Each slice registers what it knows (monitoring,
 * jobs, alerts, lab); the shell registers the engine and the version. An item
 * must always be true: when a slice does not know, it removes its item rather
 * than showing a stale one.
 */

import { useEffect, useRef } from 'react';
import { create } from 'zustand';
import type { LucideIcon } from '@/design/Icon';
import type { StatusTone } from '@/design/tone';

export interface StatusItem {
  id: string;
  /** `left` for live state, `right` for facts that rarely change. Default `left`. */
  side?: 'left' | 'right';
  /** Position within its side; lower comes first. Default 50. */
  order?: number;
  label: string;
  /** Shows the shape-coded status dot of this tone before the label. */
  tone?: StatusTone;
  icon?: LucideIcon;
  /** More detail, shown on hover and read by screen readers. */
  detail?: string;
  /** Makes the item a button, typically to open the screen it summarises. */
  onSelect?: () => void;
}

interface StatusItemsState {
  items: Record<string, StatusItem>;
}

/** The store behind the status bar. Read it with a selector; write through the functions below. */
export const useStatusItems = create<StatusItemsState>(() => ({ items: {} }));

/** Adds or replaces an item. Returns a function that removes it. */
export function registerStatusItem(item: StatusItem): () => void {
  useStatusItems.setState((state) => ({ items: { ...state.items, [item.id]: item } }));
  return () => {
    useStatusItems.setState((state) => {
      if (state.items[item.id] !== item) return state;
      const { [item.id]: _removed, ...items } = state.items;
      return { items };
    });
  };
}

/**
 * Shows an item in the status bar while the component is mounted, kept up to
 * date as its fields change. Pass `null` to show nothing.
 */
export function useStatusItem(item: StatusItem | null): void {
  const onSelect = useRef(item?.onSelect);
  useEffect(() => {
    onSelect.current = item?.onSelect;
  });

  const { id, side, order, label, tone, icon, detail } = item ?? {};
  const selectable = Boolean(item?.onSelect);

  useEffect(() => {
    if (id === undefined || label === undefined) return;
    return registerStatusItem({
      id,
      side,
      order,
      label,
      tone,
      icon,
      detail,
      onSelect: selectable ? () => onSelect.current?.() : undefined,
    });
  }, [id, side, order, label, tone, icon, detail, selectable]);
}

/** Items of one side, in display order. */
export function selectStatusItems(items: Record<string, StatusItem>, side: 'left' | 'right'): StatusItem[] {
  return Object.values(items)
    .filter((item) => (item.side ?? 'left') === side)
    .sort((a, b) => (a.order ?? 50) - (b.order ?? 50) || a.id.localeCompare(b.id));
}
