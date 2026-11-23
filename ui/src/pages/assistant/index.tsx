/**
 * Assistant — the full screen at `/assistant`.
 *
 * The same conversation as the dock, with room for the things a dock cannot
 * hold: the chat history, the models this installation can use, and what the
 * conversations have cost. The dock and this screen share one selected chat
 * (`chat.ts`), so moving between them never loses your place.
 *
 * Everything it reads: `ai.status`, `ai.chats.list/.create/.get/.rename/
 * .delete`, `ai.send`, `ai.cancel`, `ai.approve`, `ai.presets`, `ai.tools`,
 * `ai.usage`, `ai.providers.list/.presets/.save/.delete/.test/.models/
 * .detect_local`, `settings.get` / `settings.set` (section `assistant`, in
 * the ISP edition only), `app.methods`, and the `ai.event`,
 * `ai.chats.changed`, `ai.providers.changed` and `settings.changed` topics.
 */

import { Coins, Cpu, MessageSquare, Plus, SlidersHorizontal } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { LocalEndpoint, ProviderPreset } from '@/api/types/ai';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button } from '@/design/Button';
import { Skeleton } from '@/design/Skeleton';
import { Tabs, type TabItem } from '@/design/Tabs';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { useCurrentChat } from './chat';
import { ChatList } from './ChatList';
import { Conversation } from './Conversation';
import { ledgerSwitchAvailable } from './ledger';
import { Providers } from './Providers';
import { SettingsTab } from './SettingsTab';
import { useChatBootstrap } from './useChats';
import { Usage } from './Usage';
import styles from './Assistant.module.css';

type Tab = 'chat' | 'models' | 'usage' | 'settings';

const TABS: readonly (TabItem & { id: Tab })[] = [
  { id: 'chat', label: 'Conversation', icon: MessageSquare },
  { id: 'models', label: 'Models', icon: Cpu },
  { id: 'usage', label: 'Usage', icon: Coins },
  // Holds only what a build has something to set for: the ledger switch.
  { id: 'settings', label: 'Settings', icon: SlidersHorizontal },
];

/**
 * The tab the address asks for. `settings` is honoured while `app.methods`
 * is still on its way — a link to it must not bounce to the conversation
 * and back — and dropped once the build turns out to have nothing to set.
 */
function tabFor(requested: string | null, settings: boolean | undefined): Tab {
  if (requested === 'settings') return settings === false ? 'chat' : 'settings';
  return requested === 'models' || requested === 'usage' ? requested : 'chat';
}

export default function AssistantPage() {
  const [params, setParams] = useSearchParams();
  const methods = useRpc<string[]>('app.methods');
  // A method list that cannot be read offers nothing that depends on it.
  const settingsAvailable = methods.isError ? false : ledgerSwitchAvailable(methods.data);
  const tab = tabFor(params.get('tab'), settingsAvailable);
  const tabs = TABS.filter((entry) => entry.id !== 'settings' || settingsAvailable === true);
  const bootstrap = useChatBootstrap();
  const setChatId = useCurrentChat((state) => state.setChatId);
  const [opening, setOpening] = useState<{ presetId: string | null; baseUrl?: string } | null>(
    () => {
      // The dock's setup card links here with the door it was asked for.
      const add = params.get('add');
      const at = params.get('at');
      if (!add && !at) return null;
      return { presetId: add, ...(at ? { baseUrl: at } : {}) };
    },
  );

  // A conversation can be linked to: `#/assistant?chat=<id>`.
  const linked = params.get('chat');
  useEffect(() => {
    if (linked) setChatId(linked);
  }, [linked, setChatId]);

  const go = (next: Tab) => {
    const updated = new URLSearchParams(params);
    if (next === 'chat') updated.delete('tab');
    else updated.set('tab', next);
    setParams(updated, { replace: true });
  };

  usePageHeader({ title: 'Assistant' });

  const newChat = bootstrap.newChat;
  useEffect(
    () =>
      registerCommand({
        id: 'assistant.new-chat',
        title: 'New assistant chat',
        group: 'Assistant',
        keywords: ['ai', 'ask', 'conversation'],
        icon: Plus,
        run: () => void newChat(),
      }),
    [newChat],
  );

  const addProvider = (preset: ProviderPreset | null, local?: LocalEndpoint) => {
    setOpening({ presetId: preset?.id ?? null, ...(local ? { baseUrl: local.base_url } : {}) });
    go('models');
  };

  return (
    <Page flush className={styles.page}>
      <Toolbar className={styles.toolbar}>
        <Tabs
          id="assistant-tabs"
          aria-label="Assistant"
          tabs={tabs.map(({ id, label, icon }) => ({ id, label, icon }))}
          value={tab}
          onChange={(next) => go(next as Tab)}
        />
        <ToolbarSpacer />
        {tab === 'models' ? (
          <Button variant="primary" icon={Plus} onClick={() => setOpening({ presetId: null })}>
            Add a model
          </Button>
        ) : tab === 'settings' ? null : (
          <Button variant="primary" icon={Plus} onClick={() => void newChat()}>
            New chat
          </Button>
        )}
      </Toolbar>

      {tab === 'chat' && (
        <div className={styles.split}>
          <ChatList
            chats={bootstrap.chats}
            loading={bootstrap.loading}
            currentId={bootstrap.chatId}
            onSelect={bootstrap.select}
          />
          <section className={styles.main} aria-label="Conversation">
            <Conversation bootstrap={bootstrap} onAddProvider={addProvider} variant="page" />
          </section>
        </div>
      )}
      {tab === 'models' && (
        <div className={styles.scroller}>
          <Providers opening={opening} onOpened={() => setOpening(null)} />
        </div>
      )}
      {tab === 'usage' && (
        <div className={styles.scroller}>
          <Usage />
        </div>
      )}
      {tab === 'settings' && (
        <div className={styles.scroller}>
          {settingsAvailable ? (
            <SettingsTab />
          ) : (
            <div className={styles.loading} aria-busy="true" aria-label="Loading the assistant’s settings">
              <Skeleton width="30%" />
              <Skeleton height={140} shape="block" />
            </div>
          )}
        </div>
      )}
    </Page>
  );
}
