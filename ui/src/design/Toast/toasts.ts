/**
 * Toasts: brief confirmation or bad news about something the user just did or
 * that just happened. Call `toast.success('Backup saved')` from anywhere.
 *
 * Everything shown is also kept for the session as a notification, so the
 * header bell can show what was missed.
 */

import { create } from 'zustand';

export type ToastTone = 'info' | 'success' | 'warning' | 'error';

export interface ToastAction {
  label: string;
  onSelect: () => void;
}

export interface ToastOptions {
  /** One more sentence of detail. */
  description?: string;
  /** A single follow-up, such as "Undo" or "View". */
  action?: ToastAction;
  /**
   * Milliseconds before the toast dismisses itself. Defaults to five seconds;
   * errors stay until dismissed. Pass `null` to keep any toast open.
   */
  duration?: number | null;
}

export interface ToastItem {
  id: number;
  tone: ToastTone;
  title: string;
  description?: string;
  action?: ToastAction;
  /** Null: stays until dismissed. */
  duration: number | null;
  /** When it was raised, in milliseconds since the epoch. */
  at: number;
}

interface ToastState {
  /** Toasts on screen, oldest first. */
  toasts: ToastItem[];
  /** Everything raised this session, newest first. */
  history: ToastItem[];
  /** How many of them have not been looked at in the notification list. */
  unread: number;
}

const DEFAULT_DURATION_MS = 5000;
const HISTORY_LIMIT = 50;

export const useToasts = create<ToastState>(() => ({ toasts: [], history: [], unread: 0 }));

let nextId = 1;

function show(tone: ToastTone, title: string, options: ToastOptions = {}): number {
  const item: ToastItem = {
    id: nextId++,
    tone,
    title,
    description: options.description,
    action: options.action,
    duration:
      options.duration !== undefined
        ? options.duration
        : tone === 'error'
          ? null
          : DEFAULT_DURATION_MS,
    at: Date.now(),
  };
  useToasts.setState((state) => ({
    toasts: [...state.toasts, item],
    history: [item, ...state.history].slice(0, HISTORY_LIMIT),
    unread: state.unread + 1,
  }));
  return item.id;
}

export const toast = {
  info: (title: string, options?: ToastOptions) => show('info', title, options),
  success: (title: string, options?: ToastOptions) => show('success', title, options),
  warning: (title: string, options?: ToastOptions) => show('warning', title, options),
  /** Stays on screen until dismissed. Say what happened and what to do. */
  error: (title: string, options?: ToastOptions) => show('error', title, options),
  /** Removes a toast from the screen. It stays in the notification list. */
  dismiss(id: number): void {
    useToasts.setState((state) => ({ toasts: state.toasts.filter((item) => item.id !== id) }));
  },
};

export const notifications = {
  markAllRead(): void {
    useToasts.setState({ unread: 0 });
  },
  clear(): void {
    useToasts.setState({ history: [], unread: 0 });
  },
};
