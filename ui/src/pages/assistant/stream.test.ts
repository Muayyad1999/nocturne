import { describe, expect, it } from 'vitest';
import type { AiChatView, AiEvent, AiEventEnvelope, UiMessage } from '@/api/types/ai';
import { ChatStream, isAnswering } from './stream';

const CHAT = 'chat1';
const TURN = 'turn1';

function event(seq: number, event: AiEvent, turn = TURN): AiEventEnvelope {
  return { chat_id: CHAT, turn_id: turn, seq, event };
}

function stored(turn: string, role: 'user' | 'assistant', text: string, id = `m${turn}${role}`): UiMessage {
  return { id, chat_id: CHAT, turn_id: turn, role, at: 1, blocks: [{ type: 'text', text }] };
}

function view(messages: UiMessage[], seq: number, pendingTurn?: string): AiChatView {
  return {
    chat: {
      id: CHAT,
      title: 'a chat',
      created_at: 1,
      updated_at: 1,
      provider_id: 'p1',
      provider_kind: 'openai',
      model: 'm',
      messages: messages.length,
      seq,
      generation: 1,
      pending_turn: pendingTurn ?? null,
      prefix: 'x',
    },
    messages,
    seq,
  };
}

/** The text of the live turn, as it would read on screen. */
function liveText(stream: ChatStream): string {
  return (stream.snapshot().live?.blocks ?? [])
    .map((block) => (block.type === 'text' ? block.text : ''))
    .join('');
}

describe('ChatStream', () => {
  it('holds events that arrive before the chat has been read, then applies them in order', () => {
    const stream = new ChatStream();
    stream.receive(event(2, { type: 'text', delta: 'world' }));
    stream.receive(event(1, { type: 'text', delta: 'hello ' }));
    expect(stream.snapshot().live).toBeNull();

    stream.attach(view([stored(TURN, 'user', 'hi')], 0));

    expect(liveText(stream)).toBe('hello world');
    expect(stream.snapshot().gap).toBe(false);
  });

  it('drops the events the snapshot already covers, so a reload cannot say it twice', () => {
    const stream = new ChatStream();
    stream.attach(view([stored(TURN, 'user', 'hi')], 0));
    stream.receive(event(1, { type: 'text', delta: 'the port is down' }));

    // The turn finished; the read brings the stored message and a higher seq.
    const finished = [stored(TURN, 'user', 'hi'), stored(TURN, 'assistant', 'the port is down')];
    stream.attach(view(finished, 2));

    expect(stream.snapshot().live).toBeNull();
    expect(stream.snapshot().messages).toHaveLength(2);

    // A duplicate delivery of an event already covered changes nothing.
    stream.receive(event(1, { type: 'text', delta: 'the port is down' }));
    stream.receive(event(2, { type: 'done', stop: 'end' }));
    expect(stream.snapshot().live).toBeNull();
    expect(stream.snapshot().messages.map((message) => message.blocks[0])).toEqual([
      { type: 'text', text: 'hi' },
      { type: 'text', text: 'the port is down' },
    ]);
  });

  it('raises a gap rather than rendering a hole when a sequence number jumps', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.receive(event(1, { type: 'text', delta: 'first ' }));
    stream.receive(event(4, { type: 'text', delta: 'fourth' }));

    expect(stream.snapshot().gap).toBe(true);
    expect(liveText(stream)).toBe('first ');

    // The read that answers the gap settles it.
    stream.attach(view([stored(TURN, 'assistant', 'first second third fourth')], 4));
    expect(stream.snapshot().gap).toBe(false);
    expect(stream.snapshot().live).toBeNull();
  });

  it('keeps streaming a turn that was already under way, and marks it partial', () => {
    const stream = new ChatStream();
    // A window that reloads mid-answer: the user message is stored, the
    // answer so far is not, and the backend is at seq 3.
    stream.attach(view([stored(TURN, 'user', 'why is Gi0/3 down?')], 3, TURN));
    stream.receive(event(4, { type: 'text', delta: ' and the neighbour agrees.' }));

    const live = stream.snapshot().live;
    expect(live?.partial).toBe(true);
    expect(live?.running).toBe(true);
    expect(liveText(stream)).toBe(' and the neighbour agrees.');

    // When it ends, the stored message replaces the fragment entirely.
    stream.receive(event(5, { type: 'done', stop: 'end' }));
    stream.attach(view(
      [
        stored(TURN, 'user', 'why is Gi0/3 down?'),
        stored(TURN, 'assistant', 'The port is err-disabled and the neighbour agrees.'),
      ],
      5,
    ));
    expect(stream.snapshot().live).toBeNull();
    expect(stream.snapshot().messages[1]?.blocks[0]).toEqual({
      type: 'text',
      text: 'The port is err-disabled and the neighbour agrees.',
    });
  });

  it('a turn that began while this window was listening is not partial', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.sent(TURN, 'hello');
    stream.receive(event(1, { type: 'text', delta: 'hi' }));
    expect(stream.snapshot().live?.partial).toBe(false);

    // Not even when a read lands mid-turn and reports it as running: this
    // window has had every event of it.
    stream.attach(view([stored(TURN, 'user', 'hello')], 1, TURN));
    stream.receive(event(2, { type: 'text', delta: ' there' }));
    expect(stream.snapshot().live?.partial).toBe(false);
    expect(liveText(stream)).toBe('hi there');
  });

  it('a turn that starts while this window is listening is whole, whoever started it', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    // Someone else's window sends; the backend stores the user message and
    // this window reads it back before the first event arrives.
    stream.attach(view([stored('turn9', 'user', 'from another window')], 0, 'turn9'));
    stream.receive(event(1, { type: 'text', delta: 'answering' }, 'turn9'));
    expect(stream.snapshot().live?.partial).toBe(false);
  });

  it('shows the message it sent until the backend reads it back', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.sent(TURN, 'list the switches');
    expect(stream.snapshot().pendingUser?.text).toBe('list the switches');
    expect(isAnswering(stream.snapshot())).toBe(true);

    stream.attach(view([stored(TURN, 'user', 'list the switches')], 1));
    expect(stream.snapshot().pendingUser).toBeNull();
    expect(stream.snapshot().messages).toHaveLength(1);
  });

  it('builds a tool step from its two events and keeps the link', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.receive(
      event(1, {
        type: 'tool_start',
        call_id: 'c1',
        name: 'run_commands',
        title: 'Run commands',
        input_summary: 'devices: SW1 · commands: show version',
      }),
    );
    expect(stream.snapshot().live?.blocks[0]).toMatchObject({ type: 'tool', status: 'running' });

    stream.receive(
      event(2, {
        type: 'tool_end',
        call_id: 'c1',
        status: 'ok',
        result_summary: '1 device, 2 lines',
        link: { type: 'device', id: 'dev1' },
      }),
    );
    expect(stream.snapshot().live?.blocks[0]).toEqual({
      type: 'tool',
      call_id: 'c1',
      name: 'run_commands',
      title: 'Run commands',
      status: 'ok',
      input_summary: 'devices: SW1 · commands: show version',
      result_summary: '1 device, 2 lines',
      link: { type: 'device', id: 'dev1' },
    });
  });

  it('carries an approval request until the call it belongs to ends', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.receive(
      event(1, {
        type: 'tool_start',
        call_id: 'c1',
        name: 'run_commands',
        title: 'Run commands',
        input_summary: 'devices: SW1',
      }),
    );
    stream.receive(
      event(2, {
        type: 'approval_needed',
        call_id: 'c1',
        name: 'run_commands',
        title: 'Run commands',
        detail: 'devices: SW1 · commands: show running-config',
      }),
    );
    expect(stream.snapshot().live?.approvals).toEqual([
      {
        call_id: 'c1',
        name: 'run_commands',
        title: 'Run commands',
        detail: 'devices: SW1 · commands: show running-config',
      },
    ]);

    stream.receive(
      event(3, { type: 'tool_end', call_id: 'c1', status: 'denied', result_summary: 'denied by the user' }),
    );
    expect(stream.snapshot().live?.approvals).toEqual([]);
    expect(stream.snapshot().live?.blocks[0]).toMatchObject({ status: 'denied' });
  });

  it('adds up the usage of every model call in the turn', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.receive(event(1, { type: 'usage', input_tokens: 1200, output_tokens: 30 }));
    stream.receive(event(2, { type: 'usage', input_tokens: 1500, output_tokens: 20 }));
    expect(stream.snapshot().live?.usage).toEqual({ input_tokens: 2700, output_tokens: 50 });
  });

  it('keeps a model endpoint failure with its code, and stops answering', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.sent(TURN, 'hello?');
    stream.receive(event(1, { type: 'error', code: 'unavailable', message: 'the GPU fell over' }));
    stream.receive(event(2, { type: 'done', stop: 'end' }));

    expect(stream.snapshot().live?.blocks[0]).toEqual({
      type: 'error',
      code: 'unavailable',
      message: 'the GPU fell over',
    });
    expect(isAnswering(stream.snapshot())).toBe(false);
  });

  it('starts a new live turn when the next one begins', () => {
    const stream = new ChatStream();
    stream.attach(view([], 0));
    stream.receive(event(1, { type: 'text', delta: 'first answer' }));
    stream.receive(event(2, { type: 'done', stop: 'end' }));
    stream.receive(event(3, { type: 'text', delta: 'second answer' }, 'turn2'));

    expect(stream.snapshot().live?.turn_id).toBe('turn2');
    expect(liveText(stream)).toBe('second answer');
  });

  it('says a turn is still running after a reload, from the chat the backend returned', () => {
    const stream = new ChatStream();
    stream.attach(view([stored(TURN, 'user', 'hi')], 3));
    expect(isAnswering(stream.snapshot(), TURN)).toBe(true);
    expect(isAnswering(stream.snapshot(), null)).toBe(false);
  });
});
