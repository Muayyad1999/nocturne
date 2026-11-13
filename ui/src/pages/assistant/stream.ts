/**
 * The attach protocol for a chat, on its own so it can be reasoned about and
 * tested.
 *
 * A turn's output reaches the UI twice: live, as `ai.event` increments, and
 * afterwards as the message the backend stored. `seq` rises by one per chat
 * and is persisted, so a window that comes back can tell whether it missed
 * anything. The rules this implements:
 *
 *   * events that arrive before the `ai.chats.get` answer are held, not
 *     dropped;
 *   * an event the snapshot already covers (`seq <= applied`) is ignored, so a
 *     reload can never show a sentence twice;
 *   * a jump in `seq` means the window was not listening: `gap` is raised and
 *     the caller reads the chat again rather than rendering a hole;
 *   * the live turn is thrown away the moment the stored history contains the
 *     message for it — which the backend appends *before* it sends `done`, so
 *     the full answer always lands, exactly once.
 *
 * What cannot be recovered is the part of a running turn that streamed before
 * this window attached: the backend keeps it in memory until the turn ends.
 * That turn is marked `partial`, said so on screen, and replaced in full when
 * `done` arrives.
 */

import type {
  AiChatView,
  AiEventEnvelope,
  AiStop,
  TokenUsage,
  UiBlock,
  UiMessage,
} from '@/api/types/ai';

/** A tool call waiting for `ai.approve`. */
export interface PendingApproval {
  call_id: string;
  name: string;
  title: string;
  /** The arguments, as the backend summarised them for a human to read. */
  detail: string;
}

/**
 * A model endpoint failure. The backend stores it as a notice, but while the
 * turn is live it is worth more than a notice: it carries the code.
 */
export interface ErrorBlock {
  type: 'error';
  code: string;
  message: string;
}

export type LiveBlock = UiBlock | ErrorBlock;

/** The turn that is being answered right now. */
export interface LiveTurn {
  turn_id: string;
  blocks: LiveBlock[];
  usage: TokenUsage | null;
  running: boolean;
  stop: AiStop | null;
  approvals: PendingApproval[];
  /**
   * The turn was already under way when this window attached, so its first
   * part is in the backend's store and not here.
   */
  partial: boolean;
}

/** The message this window sent, before the backend's copy of it arrives. */
export interface PendingUserMessage {
  turn_id: string;
  text: string;
  at: number;
}

export interface ChatSnapshot {
  /** The stored history, oldest first. */
  messages: UiMessage[];
  /** What this window sent and has not read back yet. */
  pendingUser: PendingUserMessage | null;
  live: LiveTurn | null;
  /** Events were missed: read the chat again. */
  gap: boolean;
  /** The highest `seq` applied to this snapshot. */
  seq: number;
}

function lastBlock(blocks: LiveBlock[]): LiveBlock | undefined {
  return blocks[blocks.length - 1];
}

/** Keeps one chat's transcript in step with the backend's numbering. */
export class ChatStream {
  private messages: UiMessage[] = [];
  private turnsInHistory = new Set<string>();
  private pendingUser: PendingUserMessage | null = null;
  private live: LiveTurn | null = null;
  /** The last event number applied. */
  private applied = 0;
  private held: AiEventEnvelope[] = [];
  private attached = false;
  private gap = false;
  /**
   * A turn that was already running when this window last read the chat: its
   * beginning was published before anyone here was listening.
   */
  private joinedTurn: string | null = null;

  snapshot(): ChatSnapshot {
    return {
      messages: this.messages,
      pendingUser: this.pendingUser,
      live: this.live,
      gap: this.gap,
      seq: this.applied,
    };
  }

  get isAttached(): boolean {
    return this.attached;
  }

  /**
   * Record that `ai.send` was accepted. The user's message is stored by the
   * backend but never announced on the event stream, so it is shown from here
   * until the next read brings it back.
   */
  sent(turn_id: string, text: string, at: number = Date.now()): void {
    this.pendingUser = { turn_id, text, at };
  }

  /**
   * The answer to `ai.chats.get`. Safe to call again at any time — that is
   * what `done`, a raised `gap` and a reconnect each do.
   */
  attach(view: AiChatView): void {
    this.messages = view.messages;
    this.turnsInHistory = new Set(
      view.messages.filter((message) => message.role === 'assistant').map((message) => message.turn_id),
    );
    if (this.pendingUser) {
      const read = view.messages.some(
        (message) => message.role === 'user' && message.turn_id === this.pendingUser?.turn_id,
      );
      if (read) this.pendingUser = null;
    }
    // A live turn whose stored message has arrived is finished with; one that
    // is still running keeps streaming on top of this snapshot.
    if (this.live && this.turnsInHistory.has(this.live.turn_id)) this.live = null;

    // A turn is only missing its beginning if events could have been missed:
    // the first read of this chat, or a read answering a gap. A read that
    // lands while the stream is healthy teaches nothing new about what was
    // published, so a turn that starts around it is still whole here.
    const couldHaveMissed = !this.attached || this.gap;
    const pending = view.chat.pending_turn ?? null;
    this.joinedTurn =
      couldHaveMissed &&
      pending !== null &&
      this.live?.turn_id !== pending &&
      this.pendingUser?.turn_id !== pending
        ? pending
        : this.joinedTurn;

    // The snapshot's own number only ever moves forward: a read that answers
    // after an event has already been applied must not re-open that event.
    if (view.seq > this.applied) this.applied = view.seq;
    this.gap = false;
    this.attached = true;

    const held = [...this.held].sort((a, b) => a.seq - b.seq);
    this.held = [];
    for (const event of held) this.receive(event);
  }

  /** Back to "nothing on screen is known", for the next attach. */
  detach(): void {
    this.attached = false;
    this.held = [];
  }

  /** One `ai.event` for this chat. */
  receive(event: AiEventEnvelope): void {
    if (!this.attached) {
      this.held.push(event);
      return;
    }
    // Already on screen, or covered by the snapshot we read.
    if (event.seq <= this.applied) return;
    if (event.seq > this.applied + 1) {
      // Something happened while nobody was listening. Only a fresh read can
      // say what, so nothing is rendered from here until it lands.
      this.gap = true;
      return;
    }
    this.applied = event.seq;
    this.apply(event);
  }

  /** Forget a `gap` that the caller has answered with a read. */
  clearGap(): void {
    this.gap = false;
  }

  private turn(envelope: AiEventEnvelope): LiveTurn {
    if (this.live?.turn_id === envelope.turn_id) return this.live;
    this.live = {
      turn_id: envelope.turn_id,
      blocks: [],
      usage: null,
      running: true,
      stop: null,
      approvals: [],
      // Whole unless the last read found this very turn already running.
      partial: this.joinedTurn === envelope.turn_id,
    };
    return this.live;
  }

  private apply(envelope: AiEventEnvelope): void {
    const event = envelope.event;
    const turn = this.turn(envelope);
    // A new object every time, so React sees the change.
    const blocks = [...turn.blocks];

    switch (event.type) {
      case 'text': {
        const last = lastBlock(blocks);
        if (last?.type === 'text') blocks[blocks.length - 1] = { type: 'text', text: last.text + event.delta };
        else blocks.push({ type: 'text', text: event.delta });
        break;
      }
      case 'thinking': {
        const last = lastBlock(blocks);
        if (last?.type === 'thinking') {
          blocks[blocks.length - 1] = { type: 'thinking', summary: last.summary + event.delta };
        } else {
          blocks.push({ type: 'thinking', summary: event.delta });
        }
        break;
      }
      case 'tool_start':
        blocks.push({
          type: 'tool',
          call_id: event.call_id,
          name: event.name,
          title: event.title,
          status: 'running',
          input_summary: event.input_summary,
          result_summary: '',
        });
        break;
      case 'tool_end': {
        const index = blocks.findLastIndex(
          (block) => block.type === 'tool' && block.call_id === event.call_id,
        );
        const existing = blocks[index];
        if (existing?.type === 'tool') {
          blocks[index] = {
            ...existing,
            status: event.status,
            result_summary: event.result_summary,
            ...(event.link ? { link: event.link } : {}),
          };
        }
        this.live = {
          ...turn,
          blocks,
          approvals: turn.approvals.filter((approval) => approval.call_id !== event.call_id),
        };
        return;
      }
      case 'approval_needed':
        this.live = {
          ...turn,
          approvals: [
            ...turn.approvals.filter((approval) => approval.call_id !== event.call_id),
            {
              call_id: event.call_id,
              name: event.name,
              title: event.title,
              detail: event.detail,
            },
          ],
        };
        return;
      case 'notice':
        blocks.push({ type: 'notice', text: event.text });
        break;
      case 'usage':
        this.live = {
          ...turn,
          usage: {
            input_tokens: (turn.usage?.input_tokens ?? 0) + event.input_tokens,
            output_tokens: (turn.usage?.output_tokens ?? 0) + event.output_tokens,
          },
        };
        return;
      case 'error':
        blocks.push({ type: 'error', code: event.code, message: event.message });
        break;
      case 'done':
        this.live = { ...turn, running: false, stop: event.stop, approvals: [] };
        return;
    }

    this.live = { ...turn, blocks };
  }
}

/** Every answer the window is still waiting for, across the whole snapshot. */
export function approvalsOf(snapshot: ChatSnapshot): PendingApproval[] {
  return snapshot.live?.approvals ?? [];
}

/** True while a turn of this chat is being answered. */
export function isAnswering(snapshot: ChatSnapshot, chatPendingTurn?: string | null): boolean {
  if (snapshot.live) return snapshot.live.running;
  // Between `ai.send` answering and the first event, and after a reload while
  // a turn of someone else's window is still running.
  if (snapshot.pendingUser) return true;
  return Boolean(chatPendingTurn);
}
