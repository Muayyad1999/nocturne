/**
 * The assistant's settings that this screen holds. Today that is one switch,
 * `isp_access`, and only in a build that has the ISP ledger (`ledger.ts`):
 * the key exists in the free edition too, false and inert, so it is not
 * offered there.
 *
 * Switching it on sends other people's records — names, plans, balances —
 * to whichever model a conversation uses, which may be a service outside
 * the building. So turning it on asks once and names the default model;
 * turning it off does not ask. The switch follows what the engine stored,
 * never what was clicked, and the line under it says which of the ledger's
 * tools the engine offers now, read from `ai.tools`.
 */

import { useId } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { AiToolInfo, AssistantSettings } from '@/api/types/ai';
import type { SettingsSetParams } from '@/api/types/core';
import { confirm } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { Switch } from '@/design/Switch';
import { toast } from '@/design/Toast';
import { LEDGER_WORDS, modelWords, offerLine, savedWords } from './ledger';
import { useAiStatus } from './useChats';
import styles from './Assistant.module.css';

/** The settings section the engine declares (`nocturne-assistant/src/settings.rs`). */
const SECTION = 'assistant';

export function SettingsTab() {
  const settings = useRpc<AssistantSettings>(
    'settings.get',
    { section: SECTION },
    { invalidateOn: ['settings.changed'] },
  );
  // The offer changes with the setting and with the default model, from
  // this window or another one.
  const tools = useRpc<AiToolInfo[]>('ai.tools', undefined, {
    invalidateOn: ['settings.changed', 'ai.providers.changed'],
  });
  const status = useAiStatus();
  const save = useRpcMutation<AssistantSettings, SettingsSetParams>('settings.set', {
    // `ai.tools` and `ai.status` answer from the setting, wherever they are
    // shown; nothing else the assistant answers depends on it.
    invalidates: ['settings.', 'ai.tools', 'ai.status'],
    errorToast: false,
  });
  const explained = useId();

  if (settings.isError) {
    return (
      <ErrorState
        title="The assistant’s settings could not be read"
        error={settings.error}
        onRetry={() => void settings.refetch()}
      />
    );
  }
  if (!settings.data) {
    return (
      <div className={styles.loading} aria-busy="true" aria-label="Loading the assistant’s settings">
        <Skeleton width="30%" />
        <Skeleton height={140} shape="block" />
      </div>
    );
  }

  const on = settings.data.isp_access === true;

  const change = async (next: boolean) => {
    if (next) {
      const sure = await confirm({
        title: 'Let the assistant read the customer ledger?',
        description: (
          <>
            When a question needs them, the model a conversation uses is sent the ledger’s
            totals and each customer’s id, name, state, plan and balance, and their invoices.{' '}
            {modelWords(status.data)} Phone numbers, addresses, e-mail, account codes and notes
            are never sent.
          </>
        ),
        confirmLabel: 'Switch it on',
      });
      if (!sure) return;
    }
    save.mutate(
      { section: SECTION, value: { isp_access: next } },
      {
        // Said once the engine has answered, from what it says it holds.
        onSuccess: (answer) => {
          const words = savedWords(next, answer);
          if (words.saved) toast.success(words.text);
          else toast.error(words.text);
        },
      },
    );
  };

  return (
    <div className={styles.settingsPage}>
      <Panel title="The customer ledger">
        <div className={styles.settingBlock}>
          <p className={styles.settingLead}>
            Whether the assistant may read the ISP ledger to answer questions about customers,
            plans, invoices and arrears. It is off until somebody here switches it on.
          </p>
          <Switch
            checked={on}
            disabled={save.isPending}
            aria-describedby={explained}
            onChange={(event) => void change(event.target.checked)}
          >
            {LEDGER_WORDS.label}
          </Switch>
          <div id={explained} className={styles.settingWords}>
            <p className={styles.settingState} data-on={on || undefined}>
              {on ? LEDGER_WORDS.on : LEDGER_WORDS.off}
            </p>
            <p className={styles.settingNever}>{LEDGER_WORDS.never}</p>
          </div>
          <p className={styles.settingEffect} aria-live="polite">
            {offerLine({
              tools: tools.data,
              reading: tools.isFetching || settings.isFetching,
              error: tools.error?.message,
              on,
              configured: status.data?.configured !== false,
            })}
          </p>
          {save.error && (
            <p className={styles.settingError} role="alert">
              It was not changed: {save.error.message}
            </p>
          )}
        </div>
      </Panel>
    </div>
  );
}
