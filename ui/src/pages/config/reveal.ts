/**
 * Permission to see a configuration with its secrets in place.
 *
 * Nocturne redacts before a human sees a configuration, by default. A reader
 * may still need the real value — to compare it with a password manager, to
 * prove a key rotated — so it can be asked for, once per session, and the
 * backend records every unredacted read in the audit trail. The asking lives
 * here rather than in a component so every view agrees about whether it has
 * been asked.
 */

import { create } from 'zustand';
import { confirm } from '@/design/Dialog';

interface RevealState {
  /** The user has agreed, in this session, that unredacted views are audited. */
  allowed: boolean;
}

export const useReveal = create<RevealState>(() => ({ allowed: false }));

/**
 * Asks for permission if it has not been given in this session. Resolves
 * true when secrets may be shown.
 */
export async function askToReveal(): Promise<boolean> {
  if (useReveal.getState().allowed) return true;
  const ok = await confirm({
    title: 'Show the secrets in this configuration?',
    description:
      'The passwords, keys and community strings will be shown as the device stores them. Nocturne records every unredacted view in the audit trail, with the configuration it was. You will be asked once per session.',
    confirmLabel: 'Show secrets',
  });
  if (ok) useReveal.setState({ allowed: true });
  return ok;
}
