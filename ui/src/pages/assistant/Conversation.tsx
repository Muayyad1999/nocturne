import { Sparkles, X, Zap } from 'lucide-react';
import { useEffect, useState } from 'react';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import { useRpc } from '@/api/hooks';
import type { AiCancelled, AiPreset, AiTurnStarted, ProviderEntry } from '@/api/types/ai';
import { DockComposer, DockTranscript } from '@/app/AssistantDock';
import { Badge } from '@/design/Badge';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { ApprovalCard } from './ApprovalCard';
import { useLiveChat } from './chat';
import { clearContext, dismissContext, relevantPresets, useScreenContext } from './context';
import { SetupCard, type SetupCardProps } from './SetupCard';
import { isAnswering } from './stream';
import { Transcript } from './Transcript';
import type { ChatBootstrap } from './useChats';
import { useAiStatus } from './useChats';
import styles from './Assistant.module.css';

export interface ConversationProps {
  bootstrap: ChatBootstrap;
  /** Opens the provider editor. The dock sends the user to `/assistant`. */
  onAddProvider: SetupCardProps['onAdd'];
  /** `dock` is 320–640px wide and shows less around the conversation. */
  variant: 'dock' | 'page';
}

/**
 * The conversation itself: the transcript, the approvals it is waiting on and
 * the composer. The dock and the full screen render the same component, so
 * what a person learns in one is true in the other.
 */
export function Conversation({ bootstrap, onAddProvider, variant }: ConversationProps) {
  const { chatId, unavailable } = bootstrap;
  const status = useAiStatus();
  const chat = useLiveChat(chatId);
  const presets = useRpc<AiPreset[]>('ai.presets');
  const providers = useRpc<ProviderEntry[]>('ai.providers.list', undefined, {
    invalidateOn: ['ai.providers.changed'],
  });
  const { context, chips } = useScreenContext();
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);

  // The composer is deliberately not remounted when the chat changes — a
  // half-typed message must survive the list refreshing underneath it — so
  // what belonged to the old conversation is cleared by hand.
  useEffect(() => {
    setRefused(null);
    setSending(false);
  }, [chatId]);

  const configured = status.data?.configured ?? false;
  const answering = isAnswering(chat.snapshot, chat.view?.chat.pending_turn);
  const provider = providers.data?.find((entry) => entry.id === chat.view?.chat.provider_id);

  const send = (text: string): boolean | void => {
    if (!configured) {
      // The composer explains itself rather than failing: the words are the
      // backend's own, and what was typed stays in the field.
      setRefused(unavailable?.message ?? null);
      void bootstrap
        .ensureChat()
        .then((id) => startTurn(id, text))
        .catch((failure: unknown) => {
          if (failure instanceof RpcError) setRefused(failure.message);
        });
      return false;
    }
    setSending(true);
    void bootstrap
      .ensureChat()
      .then((id) => startTurn(id, text))
      .catch((failure: unknown) => {
        if (failure instanceof RpcError) {
          if (failure.code === 'unavailable') setRefused(failure.message);
          else toast.error('That message was not sent', { description: failure.message });
        }
      })
      .finally(() => setSending(false));
  };

  const startTurn = async (id: string, text: string) => {
    const started = await rpc<AiTurnStarted>('ai.send', { chat_id: id, text, context });
    chat.sent(started.turn_id, text);
    clearContext();
    setRefused(null);
  };

  const stop = () => {
    if (!chatId) return;
    void rpc<AiCancelled>('ai.cancel', { chat_id: chatId }).catch((failure: RpcError) => {
      toast.error('The assistant could not be stopped', { description: failure.message });
    });
  };

  const quick = relevantPresets(presets.data ?? [], context);

  const body = () => {
    if (!configured && !chatId) {
      return <SetupCard sentence={unavailable?.message} onAdd={onAddProvider} />;
    }
    if (bootstrap.error) {
      return (
        <ErrorState
          size="compact"
          title="The assistant could not be reached"
          error={bootstrap.error}
          onRetry={() => void bootstrap.newChat()}
        />
      );
    }
    if (chat.error) {
      return (
        <ErrorState
          size="compact"
          title="This conversation could not be read"
          error={chat.error}
          onRetry={chat.reload}
        />
      );
    }
    if (chat.loading || bootstrap.loading) {
      return (
        <div className={styles.loading} aria-busy="true" aria-label="Loading the conversation">
          <Skeleton width="40%" />
          <Skeleton width="85%" height={48} shape="block" />
          <Skeleton width="60%" />
        </div>
      );
    }
    const { messages, pendingUser, live } = chat.snapshot;
    if (messages.length === 0 && !pendingUser && !live) {
      return (
        <div className={styles.welcome}>
          <span className={styles.setupIcon}>
            <Icon icon={Sparkles} size={18} />
          </span>
          <h2 className={styles.welcomeTitle}>Ask about your network</h2>
          <p className={styles.setupText}>
            It reads the inventory, the devices, the configurations, monitoring and the logs, and
            shows you every step it took. It can draft a change; applying one is still your
            decision.
          </p>
          {refused && <p className={styles.refused}>{refused}</p>}
        </div>
      );
    }
    return (
      <Transcript
        messages={messages}
        pendingUser={pendingUser}
        live={live}
        footer={
          <>
            {chatId &&
              (live?.approvals ?? []).map((approval) => (
                <ApprovalCard key={approval.call_id} chatId={chatId} approval={approval} />
              ))}
            {refused && <p className={styles.refused}>{refused}</p>}
          </>
        }
      />
    );
  };

  return (
    <>
      <DockTranscript>{body()}</DockTranscript>
      <DockComposer
        onSubmit={send}
        pending={sending || answering}
        onStop={answering ? stop : undefined}
        placeholder={
          configured ? 'Ask about your network' : 'Ask anything — you will need a model first'
        }
        above={
          <div className={styles.above}>
            {chips.length > 0 && (
              <div className={styles.chips}>
                {chips.map((chip) => (
                  <span key={chip.id} className={styles.chip}>
                    {chip.label}
                    {chip.detail && <span className={styles.chipDetail}>{chip.detail}</span>}
                    <button
                      type="button"
                      aria-label={`Remove ${chip.label}`}
                      onClick={() => dismissContext(chip.id)}
                    >
                      <Icon icon={X} size={11} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            {quick.length > 0 && chat.snapshot.messages.length === 0 && !answering && (
              <div className={styles.quick}>
                {quick.slice(0, variant === 'dock' ? 3 : 6).map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    className={styles.quickAction}
                    title={preset.description}
                    onClick={() => send(preset.prompt)}
                  >
                    <Icon icon={Zap} size={11} />
                    {preset.title}
                  </button>
                ))}
              </div>
            )}
          </div>
        }
        hint={
          provider ? (
            <>
              <Badge tone="ai">{provider.model}</Badge>
              <span>
                {variant === 'dock'
                  ? provider.name
                  : `${provider.name} · Enter sends, Shift+Enter for a new line${
                      status.data?.redaction
                        ? ' · secrets are removed before the model sees anything'
                        : ''
                    }`}
              </span>
            </>
          ) : undefined
        }
      />
    </>
  );
}
