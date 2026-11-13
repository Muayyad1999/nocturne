/**
 * Which chat the dock and the full screen are both looking at, and the hook
 * that keeps one chat's transcript live.
 *
 * The dock and `/assistant` are the same conversation seen through two
 * windows, so the chosen chat is one piece of state shared between them. It is
 * deliberately not persisted: a new session opens the most recent chat, which
 * is what a person coming back to the application expects.
 */

import { useEffect, useRef, useState } from 'react';
import { create } from 'zustand';
import { rpc, subscribe, useConnection } from '@/api/client';
import type { RpcError } from '@/api/errors';
import type { AiChatView, AiEventEnvelope } from '@/api/types/ai';
import { ChatStream, type ChatSnapshot } from './stream';

interface CurrentChatState {
  chatId: string | null;
  setChatId: (id: string | null) => void;
}

export const useCurrentChat = create<CurrentChatState>((set) => ({
  chatId: null,
  setChatId: (chatId) => set({ chatId }),
}));

const EMPTY: ChatSnapshot = {
  messages: [],
  pendingUser: null,
  live: null,
  gap: false,
  seq: 0,
};

export interface LiveChat {
  /** The chat as the backend last described it, with its running turn. */
  view: AiChatView | null;
  snapshot: ChatSnapshot;
  /** The first read has not answered yet. */
  loading: boolean;
  error: RpcError | null;
  /** Read the chat again (what Retry does). */
  reload: () => void;
  /** Tell the transcript that `ai.send` was accepted. */
  sent: (turnId: string, text: string) => void;
}

/**
 * Follows one chat: subscribes **first**, then reads it, then drops whatever
 * the read already covered (`stream.ts`). A gap in `seq`, the end of a turn
 * and the event stream coming back each cause another read.
 */
export function useLiveChat(chatId: string | null): LiveChat {
  const [snapshot, setSnapshot] = useState<ChatSnapshot>(EMPTY);
  const [view, setView] = useState<AiChatView | null>(null);
  const [error, setError] = useState<RpcError | null>(null);
  const [loading, setLoading] = useState(Boolean(chatId));
  const streamRef = useRef<ChatStream | null>(null);
  const reloadRef = useRef<() => void>(() => {});
  const connection = useConnection((state) => state.state);
  const wasOnline = useRef(connection === 'online');

  useEffect(() => {
    if (!chatId) {
      streamRef.current = null;
      setSnapshot(EMPTY);
      setView(null);
      setError(null);
      setLoading(false);
      return;
    }

    const stream = new ChatStream();
    streamRef.current = stream;
    let live = true;
    setSnapshot(EMPTY);
    setView(null);
    setError(null);
    setLoading(true);

    let reading = false;
    let readAgain = false;
    let lastHeard = Date.now();
    let pendingTurn: string | null = null;
    const read = () => {
      // A read asked for while one is in flight is not dropped: the answer in
      // flight was computed before the event that asked, so it cannot settle
      // it.
      if (reading) {
        readAgain = true;
        return;
      }
      reading = true;
      lastHeard = Date.now();
      stream.clearGap();
      void rpc<AiChatView>('ai.chats.get', { id: chatId })
        .then((answer) => {
          if (!live) return;
          pendingTurn = answer.chat.pending_turn ?? null;
          stream.attach(answer);
          setView(answer);
          setError(null);
          setLoading(false);
          setSnapshot(stream.snapshot());
        })
        .catch((failure: RpcError) => {
          if (!live) return;
          setError(failure);
          setLoading(false);
        })
        .finally(() => {
          reading = false;
          if (live && readAgain) {
            readAgain = false;
            read();
          }
        });
    };
    reloadRef.current = read;

    const publish = () => {
      if (!live) return;
      lastHeard = Date.now();
      const current = stream.snapshot();
      setSnapshot(current);
      // A turn that has just ended, or an event we were not there for, is only
      // settled by reading the chat back.
      if (current.gap || current.live?.running === false) read();
    };

    /*
     * A watchdog for the one case the sequence numbers cannot cover.
     * `ai.chats.get` reads the stored messages and the sequence number one
     * after the other, so a turn that finishes in between is answered with a
     * `seq` ahead of the messages: every event of that turn then counts as
     * already covered and the transcript stays empty for ever. While the
     * chat says a turn is running and nothing is arriving, read it again.
     */
    const QUIET_MS = 2_000;
    const watchdog = setInterval(() => {
      if (!live || reading) return;
      if (Date.now() - lastHeard < QUIET_MS) return;
      // Only while the backend says something should be happening.
      if (!pendingTurn) return;
      read();
    }, 1_000);

    // Subscribe before the read, so nothing that happens while it is in
    // flight is lost: those events are held and applied on top of the answer.
    const unsubscribeEvents = subscribe<AiEventEnvelope>('ai.event', (message) => {
      if (message.payload.chat_id !== chatId) return;
      stream.receive(message.payload);
      publish();
    });
    // A rename or a delete elsewhere changes the header of this chat.
    const unsubscribeChats = subscribe<{ id: string }>('ai.chats.changed', (message) => {
      if (message.payload.id === chatId) read();
    });
    read();

    return () => {
      live = false;
      clearInterval(watchdog);
      unsubscribeEvents();
      unsubscribeChats();
    };
  }, [chatId]);

  // The stream can drop and come back without a single event arriving (a turn
  // that ended while the socket was down). Coming back online is the signal.
  useEffect(() => {
    const online = connection === 'online';
    if (online && !wasOnline.current) {
      streamRef.current?.detach();
      reloadRef.current();
    }
    wasOnline.current = online;
  }, [connection]);

  return {
    view,
    snapshot,
    loading,
    error,
    reload: () => reloadRef.current(),
    sent: (turnId, text) => {
      const stream = streamRef.current;
      if (!stream) return;
      stream.sent(turnId, text);
      setSnapshot(stream.snapshot());
    },
  };
}
