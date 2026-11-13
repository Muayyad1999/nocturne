/**
 * `confirm()`: ask a question from anywhere (a command, an event handler)
 * and await the answer.
 *
 *   if (await confirm({ title: 'Delete this backup?', confirmLabel: 'Delete', tone: 'danger' })) {
 *     remove.mutate({ id });
 *   }
 *
 * The shell mounts one ConfirmHost, which shows the question.
 */

import type { ReactNode } from 'react';
import { create } from 'zustand';
import { ConfirmDialog } from './ConfirmDialog';

export interface ConfirmOptions {
  /** The question, naming the action and its object. */
  title: string;
  /** Exactly what will happen, and on which devices. */
  description?: ReactNode;
  /** The verb of the action: "Delete", "Reboot", "Apply". */
  confirmLabel: string;
  cancelLabel?: string;
  tone?: 'default' | 'danger';
  /** Text the user must type before confirming, for actions that cannot be undone. */
  confirmText?: string;
}

interface Request extends ConfirmOptions {
  resolve: (confirmed: boolean) => void;
}

const useConfirmRequest = create<{ request: Request | null }>(() => ({ request: null }));

function settle(confirmed: boolean): void {
  const { request } = useConfirmRequest.getState();
  useConfirmRequest.setState({ request: null });
  request?.resolve(confirmed);
}

/** Resolves true if the user confirmed, false if they backed out. */
export function confirm(options: ConfirmOptions): Promise<boolean> {
  // A question that is still open is answered "no" by the next one.
  settle(false);
  return new Promise((resolve) => {
    useConfirmRequest.setState({ request: { ...options, resolve } });
  });
}

/** Renders the pending `confirm()` question. Mounted once by the app shell. */
export function ConfirmHost() {
  const request = useConfirmRequest((state) => state.request);
  if (!request) return null;
  return (
    <ConfirmDialog
      open
      title={request.title}
      description={request.description}
      confirmLabel={request.confirmLabel}
      cancelLabel={request.cancelLabel}
      tone={request.tone}
      confirmText={request.confirmText}
      onClose={() => settle(false)}
      onConfirm={() => settle(true)}
    />
  );
}
