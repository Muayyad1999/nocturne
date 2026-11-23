/**
 * Sending a snippet: the one way a filled-in snippet reaches a device.
 *
 * The rule this file exists to keep is the paste funnel's own — nothing is
 * typed that the person has not seen and, where it is worth a question, said
 * yes to — for text that the engine types instead of this window
 * (`terminal.snippets.send`, which leaves the record of what was sent
 * where). The steps, in order:
 *
 * 1. The text is the engine's (`terminal.snippets.render`): what was put on
 *    screen, with the hash of exactly that.
 * 2. The policy of every device it would reach is asked about every line.
 * 3. The person is asked — on the same sheet a paste uses, queued behind
 *    any paste that is waiting — when the text has several lines, when any
 *    line is not read-only on any of the devices, or when it goes to more
 *    than one session. One read-only line into one session is not asked
 *    about, exactly as when it is pasted.
 * 4. Only then `terminal.snippets.send`, held to the hash from step 1: if
 *    the snippet or a value changed in between, the engine refuses and
 *    nothing is typed.
 *
 * No React and no engine in here: the four things it needs are handed in,
 * so the order of events can be tested without either.
 */

import type {
  SnippetRendered,
  SnippetSendParams,
  SnippetSendResult,
  TerminalLineClass,
} from '@/api/types/terminal';
import { needsConfirmation } from './paste';
import type { PasteQuestion } from './pasteFunnel';
import { worstClasses, type SendTarget } from './snippets';

export interface SnippetSendDeps {
  /** `terminal.classify` for one device (the generic policy for null). A rejection means it could not be asked. */
  classify(deviceId: string | null, text: string): Promise<TerminalLineClass[]>;
  /** Puts the question on the paste sheet; true sends. */
  confirm(question: PasteQuestion): Promise<boolean>;
  /** `terminal.snippets.send`. */
  send(params: SnippetSendParams): Promise<SnippetSendResult>;
}

export interface SnippetSendRequest {
  id: string;
  /** The values the render on screen was made with. */
  values: Record<string, string>;
  /** The render the person is looking at: its text is what is asked about, its hash what is sent. */
  shown: Pick<SnippetRendered, 'text' | 'sha256'>;
  /** Where it goes: one session or several, in the order they will be typed into. */
  targets: readonly SendTarget[];
}

export type SnippetSendOutcome =
  /** The engine typed it; `result` says who took it and who did not. */
  | { kind: 'sent'; result: SnippetSendResult }
  /** The person said no on the sheet. Nothing was sent. */
  | { kind: 'cancelled' }
  /** There was nothing to send, or nowhere to send it. Nothing was sent. */
  | { kind: 'nothing' };

/**
 * Sends what is shown to the targets, asking first when it is worth a
 * question. Rejects with the engine's own error when the send is refused —
 * nothing was typed then, and the caller shows why.
 */
export async function sendSnippet(deps: SnippetSendDeps, request: SnippetSendRequest): Promise<SnippetSendOutcome> {
  const { shown, targets } = request;
  const first = targets[0];
  if (!first || shown.text === '') return { kind: 'nothing' };

  // One question per distinct device: two sessions to the same switch share a policy.
  const devices = [...new Set(targets.map((target) => target.deviceId))];
  let classes: TerminalLineClass[] | null;
  try {
    classes = worstClasses(await Promise.all(devices.map((deviceId) => deps.classify(deviceId, shown.text))));
  } catch {
    // A policy that cannot be asked must not stop a send; the sheet says so.
    classes = null;
  }

  if (targets.length > 1 || needsConfirmation(shown.text, classes)) {
    const confirmed = await deps.confirm({
      sessionId: first.sessionId,
      title: first.title,
      text: shown.text,
      classes,
      source: 'snippet',
      targets: targets.length,
    });
    if (!confirmed) return { kind: 'cancelled' };
  }

  const result = await deps.send({
    id: request.id,
    values: request.values,
    session_ids: targets.map((target) => target.sessionId),
    expect_sha256: shown.sha256,
  });
  return { kind: 'sent', result };
}
