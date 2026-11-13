/**
 * "Explain this" — handing a piece of a session to the assistant.
 *
 * The shell has no shared place for context yet (the Assistant slice owns the
 * dock and is being written in parallel), so the selection is put in a small
 * store of its own and the dock is opened. When the dock learns to read
 * context, it reads `useAssistantContext`; until then the toast is what tells
 * the user their selection went somewhere.
 */

import { create } from 'zustand';
import { useLayout } from '@/app/layout';
import { toast } from '@/design/Toast';

export interface AssistantContext {
  /** Where the text came from: a session title. */
  source: string;
  /** The device behind that session, when there is one. */
  deviceId: string | null;
  text: string;
  at: number;
}

export const useAssistantContext = create<{ context: AssistantContext | null }>(() => ({ context: null }));

/** Opens the dock with this text attached. */
export function explainWithAssistant(context: Omit<AssistantContext, 'at'>): void {
  useAssistantContext.setState({ context: { ...context, at: Date.now() } });
  useLayout.getState().setDockOpen(true);
  const lines = context.text.split('\n').length;
  toast.info('Sent to the assistant', {
    description: `${lines === 1 ? 'One line' : `${lines} lines`} from ${context.source}.`,
  });
}
