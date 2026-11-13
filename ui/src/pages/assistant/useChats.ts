/**
 * The chat list, and getting hold of a chat to talk in.
 *
 * The dock opens the most recent conversation; an empty installation needs one
 * made. That one `ai.chats.create` is also how the UI learns the backend's own
 * sentence about having no model: `create` asks for the default provider
 * first, so with nothing configured it answers `unavailable` with the three
 * ways to add one — and creates nothing. No probe, no invented copy.
 */

import { useEffect, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import { invalidateMethods, useEvent, useRpc } from '@/api/hooks';
import type { AiStatus, Chat } from '@/api/types/ai';
import { useCurrentChat } from './chat';

export function useChats() {
  return useRpc<Chat[]>('ai.chats.list', undefined, { invalidateOn: ['ai.chats.changed'] });
}

export function useAiStatus() {
  return useRpc<AiStatus>('ai.status', undefined, { invalidateOn: ['ai.providers.changed'] });
}

export interface ChatBootstrap {
  chatId: string | null;
  chats: Chat[];
  loading: boolean;
  /** Why there is no chat: the backend's own words when no model is set up. */
  unavailable: RpcError | null;
  /** Any other failure of the list or the create. */
  error: RpcError | null;
  /** Starts a fresh conversation and selects it. */
  newChat: () => Promise<void>;
  /** Makes sure there is a chat to send into, creating one if need be. */
  ensureChat: () => Promise<string>;
  select: (id: string) => void;
}

/** Keeps a chat selected, creating the first one when there is none. */
export function useChatBootstrap(): ChatBootstrap {
  const queryClient = useQueryClient();
  const chats = useChats();
  const chatId = useCurrentChat((state) => state.chatId);
  const setChatId = useCurrentChat((state) => state.setChatId);
  const [unavailable, setUnavailable] = useState<RpcError | null>(null);
  const [error, setError] = useState<RpcError | null>(null);
  // One automatic create per reason to try: a provider appearing is a new one.
  const tried = useRef<string>('');
  const inFlight = useRef<Promise<string> | null>(null);

  /**
   * Single-flight. The first message of a session can ask for a chat from two
   * places at once — the effect below that opens one when the list is empty,
   * and the composer when Enter is pressed before that has answered — and two
   * chats would mean the answer arriving in the one nobody is looking at.
   */
  const create = (fromChatId?: string): Promise<string> => {
    const existing = inFlight.current;
    if (existing) return existing;
    const started = rpc<Chat>('ai.chats.create', fromChatId ? { from_chat_id: fromChatId } : {})
      .then(async (chat) => {
        await invalidateMethods(queryClient, ['ai.chats.']);
        setChatId(chat.id);
        setUnavailable(null);
        setError(null);
        return chat.id;
      })
      .finally(() => {
        inFlight.current = null;
      });
    inFlight.current = started;
    return started;
  };

  const known = chats.data;
  useEffect(() => {
    if (!known) return;
    if (chatId && known.some((chat) => chat.id === chatId)) return;
    const newest = known[0];
    if (newest) {
      setChatId(newest.id);
      return;
    }
    if (tried.current === 'empty') return;
    tried.current = 'empty';
    void create().catch((failure: RpcError) => {
      if (failure.code === 'unavailable') setUnavailable(failure);
      else setError(failure);
    });
    // `create` and `setChatId` are stable enough for this: the effect only
    // needs to run when the list or the selection changes.
  }, [known, chatId]);

  // A provider was added: creating a chat is worth trying again.
  useEvent('ai.providers.changed', () => {
    tried.current = '';
    setUnavailable(null);
  });

  return {
    chatId,
    chats: known ?? [],
    loading: chats.isPending,
    unavailable,
    error: error ?? (chats.error as RpcError | null),
    newChat: async () => {
      try {
        await create();
      } catch (failure) {
        if (failure instanceof RpcError && failure.code === 'unavailable') setUnavailable(failure);
        else if (failure instanceof RpcError) setError(failure);
      }
    },
    // A message belongs in the conversation that is being made, if one is:
    // pressing Enter right after *New chat* must not put it in the old one.
    // Otherwise the store, never the closure, says which chat is current.
    ensureChat: async () =>
      inFlight.current ?? useCurrentChat.getState().chatId ?? (await create()),
    select: (id) => setChatId(id),
  };
}
