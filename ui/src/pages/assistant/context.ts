/**
 * What the user is looking at when they ask, as the backend's `Context`.
 *
 * A screen hands something over ("Explain this" on a terminal selection) and
 * the dock shows it as a chip above the composer, which the user can take off
 * again. Nothing is sent that is not on a chip: the context is what the user
 * can see it is.
 *
 * The handover itself lives in the Terminal screen's little store
 * (`pages/terminal/assistant.ts`) because the shell has no shared place for
 * it yet. This module is the one that reads it; when the shell grows an
 * "Ask the assistant" button of its own, that store moves to `app/` and only
 * the import below changes.
 */

import { useEffect } from 'react';
import { useLocation } from 'react-router';
import { create } from 'zustand';
import type { AiContext, AiPreset } from '@/api/types/ai';
import { useAssistantContext as useHandover } from '@/pages/terminal/assistant';

/** One removable piece of context, as the composer shows it. */
export interface ContextChip {
  id: string;
  /** What it is: "Selection from SW1". */
  label: string;
  /** How much of it: "12 lines". */
  detail?: string;
}

interface ContextState {
  /** Text a screen handed over, with where it came from. */
  selection: { source: string; deviceId: string | null; text: string; at: number } | null;
  /** Handovers the user has taken off again, by their timestamp. */
  dismissed: number[];
  /** Device ids the user has not taken off. */
  devices: string[];
}

const useContextStore = create<ContextState>(() => ({
  selection: null,
  dismissed: [],
  devices: [],
}));

/** Takes a chip off: nothing of it is sent with the next message. */
export function dismissContext(id: string): void {
  useContextStore.setState((state) => {
    if (id === 'selection' && state.selection) {
      return { dismissed: [...state.dismissed, state.selection.at], selection: null, devices: [] };
    }
    return { devices: state.devices.filter((device) => device !== id) };
  });
}

/** Forgets everything on offer, which is what sending a message does. */
export function clearContext(): void {
  useContextStore.setState((state) => ({
    selection: null,
    devices: [],
    dismissed: state.selection ? [...state.dismissed, state.selection.at] : state.dismissed,
  }));
}

function lineCount(text: string): number {
  return text.split('\n').length;
}

export interface ScreenContext {
  /** What `ai.send` is given. Empty when nothing is attached. */
  context: AiContext;
  chips: ContextChip[];
}

/**
 * The context the composer offers: the screen the user is on, plus whatever a
 * screen handed over and they have not removed.
 */
export function useScreenContext(): ScreenContext {
  const { pathname } = useLocation();
  const handover = useHandover((state) => state.context);
  const { selection, devices } = useContextStore();

  // A new handover replaces the last one; one the user already took off does
  // not come back.
  useEffect(() => {
    if (!handover) return;
    const state = useContextStore.getState();
    if (state.dismissed.includes(handover.at)) return;
    if (state.selection?.at === handover.at) return;
    useContextStore.setState({
      selection: handover,
      devices: handover.deviceId ? [handover.deviceId] : [],
    });
  }, [handover]);

  const chips: ContextChip[] = [];
  const context: AiContext = { page: pathname };
  if (selection) {
    chips.push({
      id: 'selection',
      label: `Selection from ${selection.source}`,
      detail: `${lineCount(selection.text)} lines`,
    });
    context.selection = selection.text;
  }
  for (const device of devices) {
    chips.push({ id: device, label: 'Device', detail: device });
  }
  if (devices.length > 0) context.device_ids = devices;
  return { context, chips };
}

/**
 * The quick actions worth offering right now. A preset says what it needs; one
 * that needs a selection is no use when there is nothing selected, and an
 * engineer should not have to find that out by pressing it.
 */
export function relevantPresets(presets: readonly AiPreset[], context: AiContext): AiPreset[] {
  const has = {
    device: (context.device_ids?.length ?? 0) > 0,
    selection: Boolean(context.selection),
    alert: Boolean(context.alert_id),
    change: Boolean(context.change_id),
    none: true,
  } as const;
  return presets.filter((preset) =>
    preset.needs.length === 0 ? true : preset.needs.every((need) => has[need]),
  );
}
